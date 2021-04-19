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
    blockchain_config::BlockchainConfig,
    TransactionExecutor,
};

use std::sync::{atomic::{AtomicU64, Ordering}, Arc};
use ton_block::{
    CurrencyCollection, TransactionTickTock,
    Account, Message,
    Transaction, TrComputePhase, TransactionDescrTickTock, TransactionDescr
};
use ton_types::{fail, HashmapE, Result};
use ton_vm::{
    int, boolean, stack::{Stack, StackItem, integer::IntegerData}
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
        if in_msg.is_some() {
            fail!("Tick Tock transaction must not have input message")
        }
        let account_addr = match account.get_id() {
            Some(addr) => addr,
            None => fail!("Tick Tock contract should have Standard address")
        };
        match account.get_tick_tock() {
            Some(tt) => if tt.tock != self.tt.is_tock() && tt.tick != self.tt.is_tick() {
                fail!("wrong type of account's tick tock flag")
            }
            None => fail!("Account {:x} is not special account for tick tock", account_addr)
        }
        let account_address = account.get_addr().cloned().unwrap_or_default();
        log::debug!(target: "executor", "tick tock transation account {:x}", account_addr);
        let is_special = true;
        let lt = last_tr_lt.load(Ordering::Relaxed);
        let mut tr = Transaction::with_address_and_status(account_addr.clone(), account.status());
        tr.set_now(block_unixtime);
        let mut description = TransactionDescrTickTock::default();
        description.tt = self.tt.clone();

        description.storage = match self.storage_phase(account, &mut tr, is_special) {
            Some(storage_ph) => storage_ph,
            None => fail!("Problem with storage phase")
        };
        let old_account = account.clone();

        log::debug!(target: "executor", "compute_phase {}", lt);
        let smci = self.build_contract_info(self.config().raw_config(), &account, &account_address, block_unixtime, block_lt, lt); 
        let mut stack = Stack::new();
        stack
            .push(int!(account.balance().map(|value| value.grams.0).unwrap_or_default()))
            .push(int!(account_addr.get_bigint(256)))
            .push(boolean!(self.tt.is_tock()))
            .push(int!(-2));
        let (compute_ph, actions) = self.compute_phase(
            None, 
            account,
            state_libs,
            &smci, 
            stack,
            is_special,
            debug
        )?;
        let mut out_msgs = vec![];
        description.compute_ph = compute_ph;
        description.action = match description.compute_ph {
            TrComputePhase::Vm(ref phase) => {
                tr.add_fee_grams(&phase.gas_fees)?;
                if phase.success {
                    log::debug!(target: "executor", "compute_phase: TrComputePhase::Vm success");
                    log::debug!(target: "executor", "action_phase {}", lt);
                    match self.action_phase(&mut tr, account, &mut CurrencyCollection::default(), actions.unwrap_or_default(), is_special) {
                        Some((action_ph, msgs)) => {
                            out_msgs = msgs;
                            Some(action_ph)
                        }
                        None => None
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
        if description.aborted {
            *account = old_account;
        }
        let lt = self.add_messages(&mut tr, out_msgs, last_tr_lt)?;
        account.set_last_tr_time(lt);
        tr.write_description(&TransactionDescr::TickTock(description))?;
        Ok(tr)
    }
    fn ordinary_transaction(&self) -> bool { false }
    fn config(&self) -> &BlockchainConfig { &self.config }
    fn build_stack(&self, _in_msg: Option<&Message>, account: &Account) -> Stack {
        let account_balance = account.balance().map(|balance| balance.grams.clone()).unwrap_or_default();
        let account_id = account.get_id().unwrap_or_default();
        let mut stack = Stack::new();
        stack
            .push(int!(account_balance.0.clone()))
            .push(int!(account_id.clone().get_bigint(256)))
            .push(boolean!(self.tt.is_tock()))
            .push(int!(-2));
        stack
    }
}
