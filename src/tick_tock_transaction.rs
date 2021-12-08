/*
* Copyright (C) 2019-2021 TON Labs. All Rights Reserved.
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

use crate::{blockchain_config::BlockchainConfig, ExecuteParams, TransactionExecutor, error::ExecutorError};

use std::sync::{atomic::Ordering, Arc};
use ton_block::{
    Account, CurrencyCollection, Grams, Message, TrComputePhase, Transaction, TransactionDescr,
    TransactionDescrTickTock, TransactionTickTock,
};
use ton_types::{error, fail, Result};
use ton_vm::{
    boolean, int,
    stack::{integer::IntegerData, Stack, StackItem},
};

pub struct TickTockTransactionExecutor {
    pub config: BlockchainConfig,
    pub tt: TransactionTickTock,
}

impl TickTockTransactionExecutor {
    pub fn new(config: BlockchainConfig, tt: TransactionTickTock) -> Self {
        Self {
            config,
            tt,
        }
    }
}

impl TransactionExecutor for TickTockTransactionExecutor {
    ///
    /// Create end execute tick or tock transaction for special account
    fn execute_with_params(
        &self,
        in_msg: Option<&Message>,
        account: &mut Account,
        params: ExecuteParams,
    ) -> Result<Transaction> {
        if in_msg.is_some() {
            fail!("Tick Tock transaction must not have input message")
        }
        let account_id = match account.get_id() {
            Some(addr) => addr,
            None => fail!("Tick Tock contract should have Standard address")
        };
        match account.get_tick_tock() {
            Some(tt) => if tt.tock != self.tt.is_tock() && tt.tick != self.tt.is_tick() {
                fail!("wrong type of account's tick tock flag")
            }
            None => fail!("Account {:x} is not special account for tick tock", account_id)
        }
        let account_address = account.get_addr().cloned().unwrap_or_default();
        log::debug!(target: "executor", "tick tock transation account {:x}", account_id);
        let mut acc_balance = account.balance().cloned().unwrap_or_default();

        let is_masterchain = true;
        let is_special = true;
        let lt = std::cmp::max(account.last_tr_time().unwrap_or(0), params.last_tr_lt.load(Ordering::Relaxed));
        let mut tr = Transaction::with_address_and_status(account_id.clone(), account.status());
        tr.set_logical_time(lt);
        tr.set_now(params.block_unixtime);
        account.set_last_paid(0);
        let storage = match self.storage_phase(
            account,
            &mut acc_balance,
            &mut tr,
            is_masterchain,
            is_special,
        ) {
            Ok(storage_ph) => storage_ph,
            Err(e) => fail!(ExecutorError::TrExecutorError(format!("cannot create storage phase of a new transaction for smart contract for reason {}", e)))
        };
        let mut description = TransactionDescrTickTock {
            tt: self.tt.clone(),
            storage,
            ..TransactionDescrTickTock::default()
        };

        let old_account = account.clone();
        let original_acc_balance = acc_balance.clone();

        log::debug!(target: "executor", "compute_phase {}", lt);
        let smci = self.build_contract_info(&acc_balance, &account_address, params.block_unixtime, params.block_lt, lt, params.seed_block);
        let mut stack = Stack::new();
        stack
            .push(int!(account.balance().map(|value| value.grams.0).unwrap_or_default()))
            .push(StackItem::integer(IntegerData::from_unsigned_bytes_be(&account_id.get_bytestring(0))))
            .push(boolean!(self.tt.is_tock()))
            .push(int!(-2));
        let (compute_ph, actions, new_data) = match self.compute_phase(
            None, 
            account,
            &mut acc_balance,
            &CurrencyCollection::default(),
            params.state_libs,
            smci,
            stack,
            is_masterchain,
            is_special,
            params.debug
        ) {
            Ok((compute_ph, actions, new_data)) => (compute_ph, actions, new_data),
            Err(e) =>
                if let Some(e) = e.downcast_ref::<ExecutorError>() {
                    match e {
                        ExecutorError::NoAcceptError(num, stack) => fail!(ExecutorError::NoAcceptError(*num, stack.clone())),
                        _ => fail!("Unknown error")
                    }
                } else {
                    fail!(ExecutorError::TrExecutorError(e.to_string()))
                }
        };
        let mut out_msgs = vec![];
        description.compute_ph = compute_ph;
        description.action = match description.compute_ph {
            TrComputePhase::Vm(ref phase) => {
                tr.add_fee_grams(&phase.gas_fees)?;
                if phase.success {
                    log::debug!(target: "executor", "compute_phase: TrComputePhase::Vm success");
                    log::debug!(target: "executor", "action_phase {}", lt);
                    match self.action_phase(&mut tr, account, &original_acc_balance, &mut acc_balance, &mut CurrencyCollection::default(), &Grams(0), actions.unwrap_or_default(), new_data, is_special) {
                        Ok((action_ph, msgs)) => {
                            out_msgs = msgs;
                            Some(action_ph)
                        }
                        Err(e) => fail!(ExecutorError::TrExecutorError(format!("cannot create action phase of a new transaction for smart contract for reason {}", e)))
                    }
                } else {
                    log::debug!(target: "executor", "compute_phase: TrComputePhase::Vm failed");
                    None
                }
            }
            TrComputePhase::Skipped(ref skipped) => {
                log::debug!(target: "executor", 
                    "compute_phase: skipped: reason {:?}", skipped.reason);
                None
            }
        };

        description.aborted = match description.action {
            Some(ref phase) => {
                log::debug!(target: "executor", 
                    "action_phase: present: success={}, err_code={}", phase.success, phase.result_code);
                !phase.success
            }
            None => {
                log::debug!(target: "executor", "action_phase: none");
                true
            }
        };
        
        log::debug!(target: "executor", "Desciption.aborted {}", description.aborted);
        tr.set_end_status(account.status());
        account.set_balance(acc_balance);
        if description.aborted {
            *account = old_account;
        }
        params.last_tr_lt.store(lt, Ordering::Relaxed);
        let lt = self.add_messages(&mut tr, out_msgs, params.last_tr_lt)?;
        account.set_last_tr_time(lt);
        tr.write_description(&TransactionDescr::TickTock(description))?;
        Ok(tr)
    }
    fn ordinary_transaction(&self) -> bool { false }
    fn config(&self) -> &BlockchainConfig { &self.config }
    fn build_stack(&self, _in_msg: Option<&Message>, account: &Account) -> Stack {
        let account_balance = account.balance().map(|balance| balance.grams.clone()).unwrap();
        let account_id = account.get_id().unwrap();
        let mut stack = Stack::new();
        stack
            .push(int!(account_balance.0))
            .push(StackItem::integer(IntegerData::from_unsigned_bytes_be(&account_id.get_bytestring(0))))
            .push(boolean!(self.tt.is_tock()))
            .push(int!(-2));
        stack
    }
}

