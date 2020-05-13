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
    tr_phases::{compute_phase, storage_phase, action_phase}
};

use std::{sync::{atomic::{AtomicU64, Ordering}, Arc}};
use ton_block::{
    AddSub, CurrencyCollection, TransactionTickTock,
    Account, Serializable, Deserializable, Message,
    HashUpdate, Transaction, TrComputePhase, TransactionDescrTickTock, TransactionDescr
};
use ton_types::{fail, Cell, Result, UInt256};
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
    /// Create end execute transaction from message for account
    fn execute(
        &self,
        in_msg: Option<&Message>,
        account_root: &mut Cell,
        block_unixtime: u32,
        block_lt: u64,
        last_tr_lt: Arc<AtomicU64>,
        debug: bool
    ) -> Result<Transaction> {
        if in_msg.is_some() {
            fail!("Tick Tock transaction must not have input message")
        }
        let old_hash = account_root.repr_hash();
        let mut account = Account::construct_from(&mut account_root.clone().into())?;
        let account_address = account.get_addr().cloned().unwrap_or_default();
        match account.get_tick_tock() {
            Some(tt) => if tt.tock != self.tt.is_tock() && tt.tick != self.tt.is_tick() {
                fail!("wrong type of account's tick tock flag")
            }
            None => fail!("It is not special account for tick tock")
        }
        let account_id = match account.get_id() {
            Some(addr) => addr,
            None => fail!("Tick Tock contract should have Standard address")
        };
        let is_special = true;
        let mut tr = Transaction::with_address_and_status(account_id.clone(), account.status());
        let lt = last_tr_lt.fetch_add(1, Ordering::SeqCst);
        tr.prev_trans_hash = UInt256::from([0;32]); // TODO: prev trans hash
        tr.set_now(block_unixtime);

        let mut description = TransactionDescrTickTock::default();
        description.tt = self.tt.clone();

        // TODO: add and process ihr_delivered parameter (if ihr_delivered ihr_fee is added to total fees)
        // TODO: add msg_balance_remaining variable and use it in phases 

        description.storage = match storage_phase(&mut account, &mut tr, &self.config, is_special) {
            Some(storage_ph) => storage_ph,
            None => fail!("Problem with storage phase")
        };

        let smci = self.build_contract_info(&account, &account_address, block_unixtime, block_lt, lt); 
        log::debug!(target: "executor", "compute_phase");
        let (compute_ph, actions) = compute_phase(
            None, 
            &mut account, 
            &smci, 
            self,
            self.config.get_gas_config(&account_address),
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
                    action_phase(&mut tr, &mut account, actions, &self.config, last_tr_lt.clone(), is_special)
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
            },
            None => {
                log::debug!(target: "executor", "action_phase: none");
                true
            },
        };
        
        log::debug!(target: "executor", "Desciption.aborted {}", description.aborted);
        account.set_last_tr_time(lt);

        if !description.aborted {
            tr.set_end_status(account.status());
            *account_root = account.write_to_new_cell()?.into();

            // calculate Hash update
            log::debug!(target: "executor", "calculate Hash update");
            let new_hash = account_root.repr_hash();
            tr.write_state_update(&HashUpdate::with_hashes(old_hash, new_hash))?;
        }
        tr.write_description(&TransactionDescr::TickTock(description))?;

        Ok(tr)
    }
    fn build_stack(&self, _in_msg: Option<&Message>, account: &Account) -> Stack {
        let account_balance = account.get_balance().map(|balance| balance.grams.clone()).unwrap_or_default();
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
