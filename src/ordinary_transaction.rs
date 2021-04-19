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
    Grams, CurrencyCollection, Serializable,
    Account, AccStatusChange, CommonMsgInfo, Message,
    Transaction, TransactionDescrOrdinary, TransactionDescr, TrComputePhase,
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
        let msg_cell = in_msg.serialize()?; // TODO: get from outside
        log::debug!(target: "executor", "Ordinary transaction executing, in message id: {:x}", msg_cell.repr_hash());
        let (credit_first, is_ext_msg) = match in_msg.header() {
            CommonMsgInfo::ExtOutMsgInfo(_) => fail!(ExecutorError::InvalidExtMessage),
            CommonMsgInfo::IntMsgInfo(ref hdr) => (!hdr.bounce, false),
            CommonMsgInfo::ExtInMsgInfo(_) => (true, true)
        };

        let account_address = in_msg.dst_ref().ok_or_else(|| ExecutorError::TrExecutorError(
            format!("Input message {:x} has no dst address", msg_cell.repr_hash())
        ))?;
        match account.get_id() {
            Some(account_id) => log::debug!(target: "executor", "Account = {:x}", account_id),
            None => log::debug!(target: "executor", "Account = None, address = {}", account_address)
        }

        let acc_balance = account.balance().cloned().unwrap_or_default();
        log::debug!(target: "executor", "acc_balance: {}, credit_first: {}", acc_balance.grams, credit_first);

        let is_special = self.config.is_special_account(account_address)?;
        let lt = last_tr_lt.load(Ordering::Relaxed);
        let mut tr = Transaction::with_account_and_message(&account, &in_msg, lt)?;
        tr.set_now(block_unixtime);
        let mut description = TransactionDescrOrdinary::default();
        description.credit_first = credit_first;

        // TODO: add and process ihr_delivered parameter (if ihr_delivered ihr_fee is added to total fees)
        let mut msg_remaining_balance = in_msg.get_value().cloned().unwrap_or_default();

        // first check if contract can pay for importing external message
        if is_ext_msg && !is_special {
            let in_fwd_fee = self.config.get_fwd_prices(in_msg.is_masterchain()).fwd_fee(&in_msg.serialize()?);
            let in_fwd_fee = CurrencyCollection::from_grams(in_fwd_fee);
            log::debug!(target: "executor", "import message fee: {}, acc_balance: {}", in_fwd_fee.grams, acc_balance.grams);
            if !account.sub_funds(&in_fwd_fee)? {
                fail!(ExecutorError::NoFundsToImportMsg)
            }
            tr.set_total_fees(in_fwd_fee);
        }

        if description.credit_first && !is_ext_msg {
            description.credit_ph = self.credit_phase(&in_msg, account);
        }
        description.storage_ph = self.storage_phase(account, &mut tr, is_special);
        log::debug!(target: "executor",
            "storage_phase: {}", if description.storage_ph.is_some() {"present"} else {"none"});

        if !description.credit_first && !is_ext_msg {
            description.credit_ph = self.credit_phase(&in_msg, account);
        }
        log::debug!(target: "executor", 
            "credit_phase: {}", if description.credit_ph.is_some() {"present"} else {"none"});

        #[cfg(feature="timings")] {
            self.timings[0].fetch_add(now.elapsed().as_micros() as u64, Ordering::SeqCst);
            now = Instant::now();
        }

        let smci = self.build_contract_info(self.config.raw_config(), &account, &account_address, block_unixtime, block_lt, lt); 
        log::debug!(target: "executor", "compute_phase");
        let mut stack = Stack::new();
        stack
            .push(int!(acc_balance.grams.0))
            .push(int!(msg_remaining_balance.grams.0))
            .push(StackItem::Cell(msg_cell))
            .push(StackItem::Slice(in_msg.body().unwrap_or_default()))
            .push(boolean!(is_ext_msg));
        
        let (compute_ph, actions) = self.compute_phase(
            Some(&in_msg), 
            account,
            state_libs,
            &smci,
            stack,
            is_special,
            debug
        )?;
        let old_account = account.clone();
        let gas_fees;
        let mut out_msgs = vec![];
        description.compute_ph = compute_ph;
        description.action = match description.compute_ph {
            TrComputePhase::Vm(ref phase) => {
                tr.add_fee_grams(&phase.gas_fees)?;
                if phase.success {
                    log::debug!(target: "executor", "compute_phase: success");
                    log::debug!(target: "executor", "action_phase: lt={}", lt);
                    gas_fees = None;
                    match self.action_phase(&mut tr, account, &mut msg_remaining_balance, actions.unwrap_or_default(), is_special) {
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
            TrComputePhase::Skipped(ref skipped) => {
                log::debug!(target: "executor", 
                    "compute_phase: skipped reason {:?}", skipped.reason);
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
        if description.aborted {
            if let Some(gas_fees) = gas_fees {
                log::debug!(target: "executor", "bounce_phase");
                description.bounce = match self.bounce_phase(&in_msg, account, &mut tr, gas_fees) {
                    Some((bounce_ph, Some(bounce_msg))) => {
                        out_msgs.push(bounce_msg);
                        Some(bounce_ph)
                    }
                    Some((bounce_ph, None)) => Some(bounce_ph),
                    None => None
                };
            }
            if description.bounce.is_none() {
                *account = old_account
            }
        }
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
        let account_balance = int!(account.balance().map(|value| value.grams.0.clone()).unwrap_or_default());
        let msg_balance = int!(in_msg.get_value().map(|val| val.grams.0).unwrap_or_default());
        let function_selector = boolean!(in_msg.is_inbound_external());
        let body_slice = in_msg.body().unwrap_or_default();
        let msg_cell = in_msg.serialize().unwrap_or_default();
        stack
            .push(account_balance)
            .push(msg_balance)
            .push(StackItem::Cell(msg_cell))
            .push(StackItem::Slice(body_slice))
            .push(function_selector);
        
        stack
    }
}
