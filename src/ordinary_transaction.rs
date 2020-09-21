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

use std::{sync::{atomic::{AtomicU64, Ordering}, Arc}};
#[cfg(feature="timings")]
use std::time::Instant;
use ton_block::{
    AddSub, CurrencyCollection, Serializable, Deserializable,
    Account, AccStatusChange, CommonMsgInfo, Message, HashUpdate,
    Transaction, TransactionDescrOrdinary, TransactionDescr,
    TrComputePhase,
};
use ton_types::{Cell, error, fail, Result, HashmapE};
use ton_vm::{
    int, stack::{Stack, StackItem, integer::IntegerData}
};


pub struct OrdinaryTransactionExecutor {
    config: BlockchainConfig,

    #[cfg(feature="timings")]
    timings: [AtomicU64; 3],
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
    fn execute_with_libs(
        &self,
        in_msg: Option<&Message>,
        account_root: &mut Cell,
        state_libs: HashmapE, // masterchain libraries
        block_unixtime: u32,
        block_lt: u64,
        last_tr_lt: Arc<AtomicU64>,
        debug: bool
    ) -> Result<Transaction> {
        #[cfg(feature="timings")]
        let mut now = Instant::now();

        let in_msg = match in_msg {
            Some(in_msg) => in_msg,
            None => fail!("Ordinary transaction must have input message")
        };
        log::debug!(target: "executor", "Ordinary transaction executing, in message id: {:x}",
            in_msg.get_int_src_account_id().unwrap_or_default());
        
        let (credit_first, is_ext_msg) = match in_msg.header() {
            CommonMsgInfo::ExtOutMsgInfo(_) => fail!(ExecutorError::InvalidExtMessage),
            CommonMsgInfo::IntMsgInfo(ref hdr) => (!hdr.bounce, false),
            CommonMsgInfo::ExtInMsgInfo(_) => (true, true)
        };

        let mut account = Account::construct_from(&mut account_root.clone().into())?;

        let old_hash = account_root.repr_hash();
        let account_address = &in_msg.dst()
            .ok_or_else(|| ExecutorError::TrExecutorError("Input message has no dst address".to_string()))?;
        match account.get_id() {
            Some(account_id) => log::debug!(target: "executor", "Account = {:x}", account_id),
            None => log::debug!(target: "executor",
                "Account = None, msg address = {:x}", in_msg.int_dst_account_id().unwrap_or_default())
        }

        let acc_balance = account.get_balance().map(|value| value.grams.0).unwrap_or_default();
        log::debug!(target: "executor", "acc_balance: {}, credit_first: {}", acc_balance, credit_first);

        let is_special = self.config.is_special_account(account_address)?;
        let lt = last_tr_lt.fetch_add(1, Ordering::SeqCst);
        let mut tr = Transaction::with_account_and_message(&account, &in_msg, lt)?;
        tr.set_now(block_unixtime);
        let mut description = TransactionDescrOrdinary::default();
        description.credit_first = credit_first;

        // TODO: add and process ihr_delivered parameter (if ihr_delivered ihr_fee is added to total fees)
        let mut msg_remaining_balance = in_msg.get_value().cloned().unwrap_or_default();

        // first check if contract can pay for importing external message
        if is_ext_msg && !is_special {
            let (_, in_fwd_fee) = self.config.get_fwd_prices(in_msg.is_masterchain()).fwd_fee(&in_msg.serialize()?);
            let in_fwd_fee = CurrencyCollection::from_grams(in_fwd_fee);
            log::debug!(target: "executor", "import message fee: {}, acc_balance: {}", in_fwd_fee.grams, acc_balance);
            if !account.sub_funds(&in_fwd_fee)? {
                fail!(ExecutorError::NoFundsToImportMsg)
            }
            tr.set_total_fees(in_fwd_fee);
        }

        if description.credit_first && !is_ext_msg {
            description.credit_ph = self.credit_phase(&in_msg, &mut account);
        }
        description.storage_ph = self.storage_phase(&mut account, &mut tr, is_special);
        log::debug!(target: "executor",
            "storage_phase: {}", if description.storage_ph.is_some() {"present"} else {"none"});

        if !description.credit_first && !is_ext_msg {
            description.credit_ph = self.credit_phase(&in_msg, &mut account);
        }
        log::debug!(target: "executor", 
            "credit_phase: {}", if description.credit_ph.is_some() {"present"} else {"none"});

        #[cfg(feature="timings")]
        {
            self.timings[0].fetch_add(now.elapsed().as_micros() as u64, Ordering::SeqCst);
            now = Instant::now();
        }

        let smci = self.build_contract_info(self.config.raw_config(), &account, &account_address, block_unixtime, block_lt, lt); 
        log::debug!(target: "executor", "compute_phase");
        let (compute_ph, actions) = self.compute_phase(
            Some(&in_msg), 
            &mut account,
            state_libs,
            &smci,
            self,
            is_special,
            debug
        )?;
        let old_account = account.clone();
        let gas_fees;
        description.compute_ph = compute_ph;
        description.action = match description.compute_ph {
            TrComputePhase::Vm(ref phase) => {
                tr.total_fees_mut().add(&CurrencyCollection::from_grams(phase.gas_fees.clone()))?;
                if phase.success {
                    log::debug!(target: "executor", "compute_phase: success");
                    log::debug!(target: "executor", "action_phase: lt={}", last_tr_lt.load(Ordering::SeqCst));
                    gas_fees = None;
                    self.action_phase(&mut tr, &mut account, &mut msg_remaining_balance, actions.unwrap_or_default(), last_tr_lt.clone(), is_special)
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
                    fail!("inbound external message rejected by transaction")
                }
                gas_fees = Some(Default::default());
                None
            }
        };

        #[cfg(feature="timings")]
        self.timings[1].fetch_add(now.elapsed().as_micros() as u64, Ordering::SeqCst);

        description.aborted = match description.action {
            Some(ref phase) => {
                log::debug!(target: "executor", 
                    "action_phase: present: success={}, err_code={}", phase.success, phase.result_code);
                match phase.status_change {
                    AccStatusChange::Deleted => account = Account::default(),
                    AccStatusChange::Frozen => account.try_freeze()?,
                    _ => ()
                }
                !phase.success
            },
            None => {
                log::debug!(target: "executor", "action_phase: none");
                true
            },
        };
        
        log::debug!(target: "executor", "Desciption.aborted {}", description.aborted);
        tr.set_end_status(account.status());
        if description.aborted {
            if let Some(gas_fees) = gas_fees {
                log::debug!(target: "executor", "bounce_phase");
                description.bounce = self.bounce_phase(&in_msg, &mut account, &mut tr, last_tr_lt.clone(), gas_fees);
            }
            if description.bounce.is_none() {
                account = old_account
            }
        }
        log::debug!(target: "executor", "calculate Hash update");
        account.set_last_tr_time(last_tr_lt.load(Ordering::SeqCst));
        *account_root = account.write_to_new_cell()?.into();
        let new_hash = account_root.repr_hash();
        tr.write_state_update(&HashUpdate::with_hashes(old_hash, new_hash))?;
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
        let account_balance = int!(account.get_balance().map(|value| value.grams.0.clone()).unwrap_or_default());
        let msg_balance = int!(in_msg.get_value().map(|val| val.grams.0).unwrap_or_default());
        let function_selector = match in_msg.header() {
            CommonMsgInfo::IntMsgInfo(_) => int!(0),
            _ => int!(-1),
        };

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
