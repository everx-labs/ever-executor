/*
* Copyright 2018-2020 TON DEV SOLUTIONS LTD.
*
* Licensed under the SOFTWARE EVALUATION License (the "License"); you may not use
* this file except in compliance with the License.  You may obtain a copy of the
* License at: https://ton.dev/licenses
*
* Unless required by applicable law or agreed to in writing, software
* distributed under the License is distributed on an "AS IS" BASIS,
* WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
* See the License for the specific TON DEV software governing permissions and
* limitations under the License.
*/


use crate::{
    blockchain_config::{BlockchainConfig, CalcMsgFwdFees}, error::ExecutorError,
    tr_phases::{compute_phase, bounce_phase, credit_phase, storage_phase, action_phase}
};

use std::{sync::Arc, time::Instant};
use ton_block::{
    AddSub, CurrencyCollection, GetRepresentationHash, Serializable, 
    accounts::{Account, ShardAccount}, logical_time_generator::LogicalTimeGenerator,
    messages::{CommonMsgInfo, Message}, 
    transactions::{
        HashUpdate, Transaction, TrComputePhase, TransactionDescrOrdinary, 
        TransactionDescr
    }
};
use ton_types::{Cell, error, fail, Result};
use ton_vm::{
    int, smart_contract_info::SmartContractInfo,
    stack::{Stack, StackItem, integer::{IntegerData, conversion::FromInt}}
};

#[cfg(test)]
#[path = "tests/test_transaction_executor.rs"]
mod tests;

#[cfg(test)]
#[path = "tests/test_transaction_executor_with_real_data.rs"]
mod tests_with_real_data;

pub trait TransactionExecutor {
    fn new() -> Self;
    fn execute(
        &mut self,
        config: &BlockchainConfig,
        msg: Message, 
        acc: &mut Option<ShardAccount>, 
        at: u32, 
        lt: u64,
        debug: bool
    ) -> Result<Transaction>;
    fn timing(&self, kind: usize) -> u128;
}

pub struct OrdinaryTransactionExecutor {
    last_tr_lt: u64,
    timings: [u128; 3]
}

impl TransactionExecutor for OrdinaryTransactionExecutor {
    
    fn new() -> Self {
        Self {
            last_tr_lt: 0,
            timings: [0; 3]
        }
    }

    fn timing(&self, kind: usize) -> u128 {
        self.timings[kind]
    }

    ///
    /// Create end execute transaction from message for account
    fn execute(
            &mut self,
            config: &BlockchainConfig,
            msg: Message, 
            acc: &mut Option<ShardAccount>, 
            block_unixtime: u32, 
            block_lt: u64,
            debug: bool
    ) -> Result<Transaction> {        

        let now = Instant::now();
        log::debug!(
            target: "executor", 
            "Ordinary transaction executing, in message id: {:x}", 
            msg.get_int_src_account_id().unwrap_or_default()
        );
        
        let (credit_first, is_ext_msg) = match msg.header() {
            CommonMsgInfo::ExtOutMsgInfo(_) => {
                log::warn!(target: "executor", "InvalidExtMessage");
                fail!(ExecutorError::InvalidExtMessage);
            },
            CommonMsgInfo::IntMsgInfo(ref hdr) => (!hdr.bounce, false),
            CommonMsgInfo::ExtInMsgInfo(_) => (true, true)
        };

        // init Logical time generator
        let mut log_time_gen = LogicalTimeGenerator::with_init_value(
            if self.last_tr_lt < block_lt { block_lt } else { self.last_tr_lt }
        );
        self.last_tr_lt = log_time_gen.get_next_time();
        
        let mut new_acc = match acc {
            Some(ref a) => {
                log::debug!(
                    target: "executor", 
                    "Account = {:x}", 
                    a.read_account()?.get_id().unwrap()
                );
                a.clone()
            }
            _ => {
                log::debug!(
                    target: "executor", 
                    "Account = None, msg address = {:x}",
                    msg.int_dst_account_id().unwrap()
                );
                ShardAccount::default()
            }
        };
        let mut account = new_acc.read_account()?;
        let account_address = &msg.dst()
            .ok_or(ExecutorError::TrExecutorError(
                "Input message has no dst address".to_string())
            )?;

        let is_special = config.is_special_account(account_address)?;

        let mut tr = Transaction::with_account_and_message(&account, &msg, self.last_tr_lt)?;
        tr.set_now(block_unixtime);
        let mut description = TransactionDescrOrdinary::default();
        description.credit_first = credit_first;

        // TODO: add and process ihr_delivered parameter (if ihr_delivered ihr_fee is added to total fees)
        // TODO: add msg_balance_remaining variable and use it in phases 

        if is_ext_msg && !is_special {
            let (_, in_fwd_fee) = config.get_fwd_prices(&msg).calc_fwd_fee(&msg)?;
            let in_fwd_fee = CurrencyCollection::with_grams(in_fwd_fee as u64);

            if !account.sub_funds(&in_fwd_fee)? {
                fail!(
                    ExecutorError::TrExecutorError(
                        "Cannot pay for importing this external message".to_string()
                    )
                )
            }

            tr.set_total_fees(in_fwd_fee);
        }

        if credit_first {
            description.credit_ph = credit_phase(&msg, &mut account);
        }
        description.storage_ph = storage_phase(&mut account, &mut tr, config, is_special);
        log::debug!(
            target: "executor", 
            "storage_phase: {}", 
            if description.storage_ph.is_some() {"present"} else {"none"}
        );

        if !credit_first {
            description.credit_ph = credit_phase(&msg, &mut account);
        }
        log::debug!(
            target: "executor", 
            "credit_phase: {}", 
            if description.credit_ph.is_some() {"present"} else {"none"}
        );
        self.timings[0] = now.elapsed().as_micros();

        let now = Instant::now();
        let smci = self.build_contract_info(&account, &msg, block_unixtime, block_lt); 
        log::debug!(target: "executor", "compute_phase");
        let (compute_ph, actions) = compute_phase(
            &msg, 
            &mut account, 
            &smci, 
            build_ordinary_stack,
            config.get_gas_config(account_address),
            is_special,
            debug
        )?;        
        description.compute_ph = compute_ph;
        description.action = match description.compute_ph {
            TrComputePhase::Vm(ref phase) => {
                tr.total_fees_mut().add(&CurrencyCollection::from_grams(phase.gas_fees.clone()))?;
                if phase.success {
                    log::debug!(target: "executor", "compute_phase: TrComputePhase::Vm success");
                    log::debug!(target: "executor", "action_phase");
                    action_phase(&mut tr, &mut account, actions, config, is_special)                    
                } else {
                    log::debug!(target: "executor", "compute_phase: TrComputePhase::Vm failed");
                    None
                }
            },
            TrComputePhase::Skipped(ref skipped) => {
                log::debug!(
                    target: "executor", 
                    "compute_phase: skipped: reason {:?}", 
                    skipped.reason
                );
                None
            },
        };
        self.timings[1] = now.elapsed().as_micros();

        let now = Instant::now();
        description.aborted = match description.action {
            Some(ref phase) => {
                log::debug!(
                    target: "executor", 
                    "action_phase: present: success={}, err_code={}", 
                    phase.success, phase.result_code
                );
                !phase.success
            },
            None => {
                log::debug!(target: "executor", "action_phase: none");
                true
            },
        };
        
        log::debug!(target: "executor", "Desciption.aborted {}", description.aborted);
        account.set_last_tr_time(self.last_tr_lt);
        *new_acc.last_trans_lt_mut() = self.last_tr_lt;

        if description.aborted {
            log::debug!(target: "executor", "bounce_phase");
            let fwd_prices = config.get_fwd_prices(&msg);
            description.bounce = bounce_phase(msg, &mut account, &mut tr, 0, fwd_prices);
            new_acc.write_account(&account)?;
        } else {
            tr.set_end_status(account.status());
            new_acc.write_account(&account)?;

            // calculate Hash update
            log::debug!(target: "executor", "calculate Hash update");
            let old_root = match acc {
                Some(a) =>  Cell::from(a.read_account()?.write_to_new_cell()?),
                _ => Cell::default()
            };
            
            let new_root = Cell::from(account.write_to_new_cell()?);
            tr.write_state_update(&HashUpdate::with_hashes(old_root.repr_hash(), new_root.repr_hash())).unwrap();

            //return modified account (maybe with new state)
            *acc = Some(new_acc);
        }
        tr.write_description(&TransactionDescr::Ordinary(description)).unwrap();
        self.calc_next_tr_time(&tr);
        if let Some(ref mut acc) = *acc {
            *acc.last_trans_hash_mut() = tr.hash()?;
        }
        self.timings[2] = now.elapsed().as_micros();

        Ok(tr)
    }
}

impl OrdinaryTransactionExecutor {
    fn calc_next_tr_time(&mut self, tr: &Transaction) {
        self.last_tr_lt += tr.msg_count() as u64 + 1;
    }

    fn build_contract_info(&self, acc: &Account, msg: &Message, block_unixtime: u32, block_lt: u64) -> SmartContractInfo {
        let mut info = SmartContractInfo::with_myself(msg.dst().unwrap().write_to_new_cell().unwrap().into());
        *info.block_lt_mut() = block_lt;
        *info.trans_lt_mut() = self.last_tr_lt;
        *info.unix_time_mut() = block_unixtime;
        if let Some(balance) = acc.get_balance() {
            *info.balance_remaining_grams_mut() = u128::from_int(balance.grams.value()).unwrap();
            *info.balance_remaining_other_mut() = balance.other.clone();
        }
        info
    }
}

fn build_ordinary_stack(msg: &Message, acc: &Account) -> Stack {
    let account_balance = int!(acc.get_balance().unwrap().grams.0.clone());
    let msg_balance = int!(
        msg.get_value().map(|val| val.grams.value().clone()).unwrap_or_default()
    );
    let function_selector = match msg.header() {
        CommonMsgInfo::IntMsgInfo(_) => int!(0),
        _ => int!(-1),
    };

    let body_slice = msg.body().unwrap_or_default();

    let msg_cell = Cell::from(msg.write_to_new_cell().unwrap());
    let mut stack = Stack::new();
    stack
        .push(account_balance)
        .push(msg_balance)
        .push(StackItem::Cell(msg_cell))
        .push(StackItem::Slice(body_slice))
        .push(function_selector);
    
    stack
}
