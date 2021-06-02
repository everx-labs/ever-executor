/*
* Copyright 2018-2020 TON DEV SOLUTIONS LTD.
*
* Licensed under the SOFTWARE EVALUATION License (the "License"); you may not use
* this file except in compliance with the License.
*
* Unless required by applicable law or agreed to in writing, software
* distributed under the License is distributed on an "AS IS" BASIS,
* WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
* See the License for the specific TON DEV software governing permissions and
* limitations under the License.
*/

use crate::{
    blockchain_config::{BlockchainConfig, CalcMsgFwdFees}, error::ExecutorError,
    TransactionExecutor,
};

use std::sync::{atomic::{AtomicU64, Ordering}, Arc};
#[cfg(feature="timings")]
use std::time::Instant;
use ton_block::{
    AddSub, Grams, Serializable,
    Account, AccStatusChange, CommonMsgInfo, Message,
    Transaction, TransactionDescrOrdinary, TransactionDescr,
    TrComputePhase, TrBouncePhase,
};
use ton_types::{error, fail, Result, HashmapE};
use ton_vm::{
    boolean, int, stack::{Stack, StackItem, integer::IntegerData}
};


pub struct OrdinaryTransactionExecutor {
    config: BlockchainConfig,

    #[cfg(feature="timings")]
    timings: [AtomicU64; 3], // 0 - preparation, 1 - compute, 2 - after compute
}

impl OrdinaryTransactionExecutor {
    pub fn new(config: BlockchainConfig) -> Self {
        Self {
            config,
            
            #[cfg(feature="timings")]
            timings: [AtomicU64::new(0), AtomicU64::new(0), AtomicU64::new(0)],
        }
    }

    #[cfg(feature="timings")]
    pub fn timing(&self, kind: usize) -> u64 {
        self.timings[kind].load(Ordering::Relaxed)
    }
}

impl TransactionExecutor for OrdinaryTransactionExecutor {
    ///
    /// Create end execute transaction from message for account
    fn execute_for_account(
        &self,
        in_msg: Option<&Message>,
        account: &mut Account,
        state_libs: HashmapE, // masterchain libraries
        block_unixtime: u32,
        block_lt: u64,
        last_tr_lt: Arc<AtomicU64>,
        debug: bool
    ) -> Result<Transaction> {
        #[cfg(feature="timings")]
        let mut now = Instant::now();

        let in_msg = in_msg.ok_or_else(|| error!("Ordinary transaction must have input message"))?;
        let in_msg_cell = in_msg.serialize()?; // TODO: get from outside
        let is_masterchain = in_msg.is_masterchain();
        log::debug!(target: "executor", "Ordinary transaction executing, in message id: {:x}", in_msg_cell.repr_hash());
        let (bounce, is_ext_msg) = match in_msg.header() {
            CommonMsgInfo::ExtOutMsgInfo(_) => fail!(ExecutorError::InvalidExtMessage),
            CommonMsgInfo::IntMsgInfo(ref hdr) => (hdr.bounce, false),
            CommonMsgInfo::ExtInMsgInfo(_) => (false, true)
        };

        let account_address = in_msg.dst_ref().ok_or_else(|| ExecutorError::TrExecutorError(
            format!("Input message {:x} has no dst address", in_msg_cell.repr_hash())
        ))?;
        let account_id = match account.get_id() {
            Some(account_id) => {
                log::debug!(target: "executor", "Account = {:x}", account_id);
                account_id
            }
            None => {
                log::debug!(target: "executor", "Account = None, address = {}", account_address.address());
                account_address.address()
            }
        };

        // TODO: add and process ihr_delivered parameter (if ihr_delivered ihr_fee is added to total fees)
        let mut acc_balance = account.balance().cloned().unwrap_or_default();
        let mut msg_balance = in_msg.get_value().cloned().unwrap_or_default();
        log::debug!(target: "executor", "acc_balance: {}, msg_balance: {}, credit_first: {}",
            acc_balance.grams, msg_balance.grams, !bounce);

        let is_special = self.config.is_special_account(account_address)?;
        let lt = last_tr_lt.load(Ordering::Relaxed);
        let mut tr = Transaction::with_address_and_status(account_id, account.status());
        tr.set_logical_time(lt);
        tr.set_now(block_unixtime);
        tr.set_in_msg_cell(in_msg_cell.clone());

        let mut description = TransactionDescrOrdinary::default();
        description.credit_first = !bounce;

        // first check if contract can pay for importing external message
        if is_ext_msg && !is_special {
            // extranal message is come serailized
            let in_fwd_fee = self.config.get_fwd_prices(is_masterchain).fwd_fee(&in_msg_cell);
            log::debug!(target: "executor", "import message fee: {}, acc_balance: {}", in_fwd_fee, acc_balance.grams);
            if !acc_balance.grams.sub(&in_fwd_fee)? {
                fail!(ExecutorError::NoFundsToImportMsg)
            }
            tr.add_fee_grams(&in_fwd_fee)?;
        }

        if description.credit_first && !is_ext_msg {
            description.credit_ph = self.credit_phase(&msg_balance, &mut acc_balance);
        }
        description.storage_ph = self.storage_phase(
            account,
            &mut acc_balance,
            &mut tr,
            is_masterchain,
            is_special
        );
        log::debug!(target: "executor",
            "storage_phase: {}", if description.storage_ph.is_some() {"present"} else {"none"});
        let old_acc_balance = acc_balance.clone();

        if !description.credit_first && !is_ext_msg {
            description.credit_ph = self.credit_phase(&msg_balance, &mut acc_balance);
        }
        log::debug!(target: "executor", 
            "credit_phase: {}", if description.credit_ph.is_some() {"present"} else {"none"});

        if !is_special {
            account.set_last_paid(block_unixtime);
        }
        // TODO: check here
        // if bounce && (msg_balance.grams > acc_balance.grams) {
        //     msg_balance.grams = acc_balance.grams.clone();
        // }
        #[cfg(feature="timings")] {
            self.timings[0].fetch_add(now.elapsed().as_micros() as u64, Ordering::SeqCst);
            now = Instant::now();
        }

        let smci = self.build_contract_info(&acc_balance, &account_address, block_unixtime, block_lt, lt); 
        let mut stack = Stack::new();
        stack
            .push(int!(acc_balance.grams.0))
            .push(int!(msg_balance.grams.0))
            .push(StackItem::Cell(in_msg_cell))
            .push(StackItem::Slice(in_msg.body().unwrap_or_default()))
            .push(boolean!(is_ext_msg));
        log::debug!(target: "executor", "compute_phase");
        let (compute_ph, actions) = self.compute_phase(
            Some(&in_msg), 
            account,
            &mut acc_balance,
            &msg_balance,
            state_libs,
            &smci,
            stack,
            is_masterchain,
            is_special,
            debug
        )?;
        let gas_fees;
        let mut out_msgs = vec![];
        description.compute_ph = compute_ph;
        description.action = match &description.compute_ph {
            TrComputePhase::Vm(phase) => {
                msg_balance.grams.sub(&phase.gas_fees)?;
                tr.add_fee_grams(&phase.gas_fees)?;
                if phase.success {
                    log::debug!(target: "executor", "compute_phase: success");
                    log::debug!(target: "executor", "action_phase: lt={}", lt);
                    gas_fees = None;
                    match self.action_phase(&mut tr, account, &mut acc_balance, &mut msg_balance, actions.unwrap_or_default(), is_special) {
                        Some((action_ph, msgs)) => {
                            out_msgs = msgs;
                            Some(action_ph)
                        }
                        None => None
                    }
                } else {
                    log::debug!(target: "executor", "compute_phase: failed");
                    gas_fees = Some(phase.gas_fees.clone());
                    None
                }
            }
            TrComputePhase::Skipped(skipped) => {
                log::debug!(target: "executor", "compute_phase: skipped reason {:?}", skipped.reason);
                if is_ext_msg {
                    fail!(ExecutorError::ExtMsgComputeSkipped(skipped.reason.clone()))
                }
                gas_fees = Some(Grams::default());
                None
            }
        };

        #[cfg(feature="timings")] {
            self.timings[1].fetch_add(now.elapsed().as_micros() as u64, Ordering::SeqCst);
            now = Instant::now();
        }

        description.aborted = match description.action.as_ref() {
            Some(phase) => {
                log::debug!(target: "executor", 
                    "action_phase: present: success={}, err_code={}", phase.success, phase.result_code);
                match phase.status_change {
                    AccStatusChange::Deleted => *account = Account::default(),
                    AccStatusChange::Frozen => account.try_freeze()?,
                    _ => ()
                }
                !phase.success
            }
            None => {
                log::debug!(target: "executor", "action_phase: none");
                true
            }
        };

        log::debug!(target: "executor", "Desciption.aborted {}", description.aborted);
        tr.set_end_status(account.status());
        if description.aborted && !is_ext_msg && bounce {
            if let Some(gas_fees) = gas_fees {
                log::debug!(target: "executor", "bounce_phase");
                description.bounce = match self.bounce_phase(&in_msg, &mut tr, gas_fees) {
                    Some((bounce_ph, Some(bounce_msg))) => {
                        out_msgs.push(bounce_msg);
                        Some(bounce_ph)
                    }
                    Some((bounce_ph, None)) => Some(bounce_ph),
                    None => None
                };
            }
            // if money can be returned to sender
            // restore account balance - storage fee
            if let Some(TrBouncePhase::Ok(_)) = description.bounce {
                log::debug!(target: "executor", "restore balance {} => {}", acc_balance.grams, old_acc_balance.grams);
                acc_balance = old_acc_balance;
            }
        }
        log::debug!(target: "executor", "set balance {}", acc_balance.grams);
        account.set_balance(acc_balance);
        log::debug!(target: "executor", "add messages");
        let lt = self.add_messages(&mut tr, out_msgs, last_tr_lt)?;
        account.set_last_tr_time(lt);
        tr.write_description(&TransactionDescr::Ordinary(description))?;
        #[cfg(feature="timings")]
        self.timings[2].fetch_add(now.elapsed().as_micros() as u64, Ordering::SeqCst);
        Ok(tr)
    }
    fn ordinary_transaction(&self) -> bool { true }
    fn config(&self) -> &BlockchainConfig { &self.config }
    fn build_stack(&self, in_msg: Option<&Message>, account: &Account) -> Stack {
        let mut stack = Stack::new();
        let in_msg = match in_msg {
            Some(in_msg) => in_msg,
            None => return stack
        };
        let acc_balance = int!(account.balance().map(|value| value.grams.0.clone()).unwrap_or_default());
        let msg_balance = int!(in_msg.get_value().map(|value| value.grams.0).unwrap_or_default());
        let function_selector = boolean!(in_msg.is_inbound_external());
        let body_slice = in_msg.body().unwrap_or_default();
        let in_msg_cell = in_msg.serialize().unwrap_or_default();
        stack
            .push(acc_balance)
            .push(msg_balance)
            .push(StackItem::Cell(in_msg_cell))
            .push(StackItem::Slice(body_slice))
            .push(function_selector);
        
        stack
    }
}
