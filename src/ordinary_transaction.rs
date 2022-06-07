/*
* Copyright (C) 2019-2022 TON Labs. All Rights Reserved.
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
    ActionPhaseResult, blockchain_config::{BlockchainConfig, CalcMsgFwdFees}, error::ExecutorError,
    ExecuteParams, TransactionExecutor, VERSION_BLOCK_REVERT_MESSAGES_WITH_ANYCAST_ADDRESSES
};
#[cfg(feature = "timings")]
use std::sync::atomic::AtomicU64;
use std::sync::{atomic::Ordering, Arc};
#[cfg(feature = "timings")]
use std::time::Instant;
use ton_block::{
    AccStatusChange, Account, AccountStatus, AddSub, CommonMsgInfo, Grams, Message, Serializable,
    TrBouncePhase, TrComputePhase, Transaction, TransactionDescr, TransactionDescrOrdinary, MASTERCHAIN_ID
};
use ton_types::{error, fail, Result, HashmapType};
use ton_vm::{
    boolean, int,
    stack::{integer::IntegerData, Stack, StackItem}, SmartContractInfo,
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
    fn execute_with_params(
        &self,
        in_msg: Option<&Message>,
        account: &mut Account,
        params: ExecuteParams,
    ) -> Result<Transaction> {
        #[cfg(feature="timings")]
        let mut now = Instant::now();

        let revert_anycast = 
            self.config.global_version() >= VERSION_BLOCK_REVERT_MESSAGES_WITH_ANYCAST_ADDRESSES;

        let in_msg = in_msg.ok_or_else(|| error!("Ordinary transaction must have input message"))?;
        let in_msg_cell = in_msg.serialize()?; // TODO: get from outside
        let is_masterchain = in_msg.dst_workchain_id() == Some(MASTERCHAIN_ID);
        log::debug!(
            target: "executor", 
            "Ordinary transaction executing, in message id: {:x}", 
            in_msg_cell.repr_hash()
        );
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
                log::debug!(target: "executor", "Account = None, address = {:x}", account_address.address());
                account_address.address()
            }
        };

        let mut acc_balance = account.balance().cloned().unwrap_or_default();
        let mut msg_balance = in_msg.get_value().cloned().unwrap_or_default();
        let ihr_delivered = false;  // ihr is disabled because it does not work
        if !ihr_delivered {
            if let Some(h) = in_msg.int_header() {
                msg_balance.grams += h.ihr_fee;
            }
        }
        log::debug!(target: "executor", "acc_balance: {}, msg_balance: {}, credit_first: {}",
            acc_balance.grams, msg_balance.grams, !bounce);

        let is_special = self.config.is_special_account(account_address)?;
        let lt = std::cmp::max(
            account.last_tr_time().unwrap_or(0), 
            std::cmp::max(params.last_tr_lt.load(Ordering::Relaxed), in_msg.lt().unwrap_or(0) + 1)
        );
        let mut tr = Transaction::with_address_and_status(account_id, account.status());
        tr.set_logical_time(lt);
        tr.set_now(params.block_unixtime);
        tr.set_in_msg_cell(in_msg_cell.clone());

        let mut description = TransactionDescrOrdinary {
            credit_first: !bounce,
            ..TransactionDescrOrdinary::default()
        };

        if revert_anycast && account_address.rewrite_pfx().is_some() {
            description.aborted = true;
            tr.set_end_status(account.status());
            params.last_tr_lt.store(lt, Ordering::Relaxed);
            account.set_last_tr_time(lt);
            tr.write_description(&TransactionDescr::Ordinary(description))?;
            return Ok(tr);
        }

        // first check if contract can pay for importing external message
        if is_ext_msg && !is_special {
            // extranal message comes serialized
            let in_fwd_fee = self.config.get_fwd_prices(is_masterchain).fwd_fee_checked(&in_msg_cell)?;
            log::debug!(target: "executor", "import message fee: {}, acc_balance: {}", in_fwd_fee, acc_balance.grams);
            if !acc_balance.grams.sub(&in_fwd_fee)? {
                fail!(ExecutorError::NoFundsToImportMsg)
            }
            tr.add_fee_grams(&in_fwd_fee)?;
        }

        if description.credit_first && !is_ext_msg {
            description.credit_ph = match self.credit_phase(account, &mut tr, &mut msg_balance, &mut acc_balance) {
                Ok(credit_ph) => Some(credit_ph),
                Err(e) => fail!(
                    ExecutorError::TrExecutorError(
                        format!("cannot create credit phase of a new transaction for smart contract for reason {}", e)
                    )
                )
            };
        }
        let due_before_storage = account.due_payment().map(|due| due.as_u128());
        let mut storage_fee;
        description.storage_ph = match self.storage_phase(
            account,
            &mut acc_balance,
            &mut tr,
            is_masterchain,
            is_special
        ) {
            Ok(storage_ph) => {
                storage_fee = storage_ph.storage_fees_collected.as_u128();
                if let Some(due) = &storage_ph.storage_fees_due {
                    storage_fee += due.as_u128()
                }
                if let Some(due) = due_before_storage {
                    storage_fee -= due;
                }
                Some(storage_ph)
            },
            Err(e) => fail!(
                ExecutorError::TrExecutorError(
                    format!("cannot create storage phase of a new transaction for smart contract for reason {}", e)
                )
            )
        };

        if description.credit_first && msg_balance.grams > acc_balance.grams {
            msg_balance.grams = acc_balance.grams;
        }

        log::debug!(target: "executor",
            "storage_phase: {}", if description.storage_ph.is_some() {"present"} else {"none"});
        let mut original_acc_balance = account.balance().cloned().unwrap_or_default();
        original_acc_balance.sub(tr.total_fees())?;

        if !description.credit_first && !is_ext_msg {
            description.credit_ph = match self.credit_phase(account, &mut tr, &mut msg_balance, &mut acc_balance) {
                Ok(credit_ph) => Some(credit_ph),
                Err(e) => fail!(
                    ExecutorError::TrExecutorError(
                        format!("cannot create credit phase of a new transaction for smart contract for reason {}", e)
                    )
                )
            };
        }
        log::debug!(target: "executor",
            "credit_phase: {}", if description.credit_ph.is_some() {"present"} else {"none"});

        let last_paid = if !is_special {params.block_unixtime} else {0};
        account.set_last_paid(last_paid);
        #[cfg(feature="timings")] {
            self.timings[0].fetch_add(now.elapsed().as_micros() as u64, Ordering::SeqCst);
            now = Instant::now();
        }

        let config_params = self.config().raw_config().config_params.data().cloned();
        let mut smc_info = SmartContractInfo {
            capabilities: self.config().raw_config().capabilities(),
            myself: account_address.serialize().unwrap_or_default().into(),
            block_lt: params.block_lt,
            trans_lt: lt,
            unix_time: params.block_unixtime,
            seq_no: params.seq_no,
            balance: acc_balance.clone(),
            config_params,
            ..Default::default()
        };
        smc_info.calc_rand_seed(params.seed_block, &account_address.address().get_bytestring(0));
        let mut stack = Stack::new();
        stack
            .push(int!(acc_balance.grams.as_u128()))
            .push(int!(msg_balance.grams.as_u128()))
            .push(StackItem::Cell(in_msg_cell))
            .push(StackItem::Slice(in_msg.body().unwrap_or_default()))
            .push(boolean!(is_ext_msg));
        log::debug!(target: "executor", "compute_phase");
        let (compute_ph, actions, new_data) = match self.compute_phase(
            Some(in_msg),
            account,
            &mut acc_balance,
            &msg_balance,
            params.state_libs,
            smc_info,
            stack,
            storage_fee,
            is_masterchain,
            is_special,
            params.debug,
            params.trace_callback
        ) {
            Ok((compute_ph, actions, new_data)) => (compute_ph, actions, new_data),
            Err(e) =>
                if let Some(e) = e.downcast_ref::<ExecutorError>() {
                    match e {
                        ExecutorError::NoAcceptError(num, stack) => fail!(
                            ExecutorError::NoAcceptError(*num, stack.clone())
                        ),
                        _ => fail!("Unknown error")
                    }
                } else {
                    fail!(ExecutorError::TrExecutorError(e.to_string()))
                }
        };
        let mut out_msgs = vec![];
        let mut action_phase_processed = false;
        let mut compute_phase_gas_fees = Grams::zero();
        let mut copyleft = None;
        description.compute_ph = compute_ph;
        description.action = match &description.compute_ph {
            TrComputePhase::Vm(phase) => {
                compute_phase_gas_fees = phase.gas_fees;
                tr.add_fee_grams(&phase.gas_fees)?;
                if phase.success {
                    log::debug!(target: "executor", "compute_phase: success");
                    log::debug!(target: "executor", "action_phase: lt={}", lt);
                    action_phase_processed = true;
                    // since the balance is not used anywhere else if we have reached this point, 
                    // then we can change it here
                    match self.action_phase_with_copyleft(
                        &mut tr,
                        account,
                        &original_acc_balance,
                        &mut acc_balance,
                        &mut msg_balance,
                        &compute_phase_gas_fees,
                        actions.unwrap_or_default(),
                        new_data,
                        is_special
                    ) {
                        Ok(ActionPhaseResult{phase, messages, copyleft_reward}) => {
                            out_msgs = messages;
                            if let Some(copyleft_reward) = &copyleft_reward {
                                tr.total_fees_mut().grams.sub(&copyleft_reward.reward)?;
                            }
                            copyleft = copyleft_reward;
                            Some(phase)
                        }
                        Err(e) => fail!(
                            ExecutorError::TrExecutorError(
                                format!("cannot create action phase of a new transaction for smart contract for reason {}", e)
                            )
                        )
                    }
                } else {
                    log::debug!(target: "executor", "compute_phase: failed");
                    None
                }
            }
            TrComputePhase::Skipped(skipped) => {
                log::debug!(target: "executor", "compute_phase: skipped reason {:?}", skipped.reason);
                if is_ext_msg {
                    fail!(ExecutorError::ExtMsgComputeSkipped(skipped.reason.clone()))
                }
                None
            }
        };

        #[cfg(feature="timings")] {
            self.timings[1].fetch_add(now.elapsed().as_micros() as u64, Ordering::SeqCst);
            now = Instant::now();
        }

        description.aborted = match description.action.as_ref() {
            Some(phase) => {
                log::debug!(
                    target: "executor",
                    "action_phase: present: success={}, err_code={}", phase.success, phase.result_code
                );
                if AccStatusChange::Deleted == phase.status_change {
                    *account = Account::default();
                    description.destroyed = true;
                }
                !phase.success
            }
            None => {
                log::debug!(target: "executor", "action_phase: none");
                true
            }
        };

        log::debug!(target: "executor", "Desciption.aborted {}", description.aborted);
        if description.aborted && !is_ext_msg && bounce {
            if !action_phase_processed {
                log::debug!(target: "executor", "bounce_phase");
                let my_addr = account.get_addr().unwrap_or(account_address);
                description.bounce = match self.bounce_phase(
                    msg_balance.clone(),
                    &mut acc_balance, 
                    &compute_phase_gas_fees, 
                    in_msg, 
                    &mut tr,
                    my_addr
                ) {
                    Ok((bounce_ph, Some(bounce_msg))) => {
                        out_msgs.push(bounce_msg);
                        Some(bounce_ph)
                    }
                    Ok((bounce_ph, None)) => Some(bounce_ph),
                    Err(e) => fail!(
                        ExecutorError::TrExecutorError(
                            format!("cannot create bounce phase of a new transaction for smart contract for reason {}", e)
                        )
                    )
                };
            }
            // if money can be returned to sender
            // restore account balance - storage fee
            if let Some(TrBouncePhase::Ok(_)) = description.bounce {
                log::debug!(target: "executor", "restore balance {} => {}", acc_balance.grams, original_acc_balance.grams);
                acc_balance = original_acc_balance;
            } else if account.is_none() && !acc_balance.is_zero()? {
                *account = Account::uninit(
                    account_address.clone(),
                    0, 
                    last_paid, 
                    acc_balance.clone()
                );
            }
        }
        if (account.status() == AccountStatus::AccStateUninit) && acc_balance.is_zero()? {
            *account = Account::default();
        }
        tr.set_end_status(account.status());
        log::debug!(target: "executor", "set balance {}", acc_balance.grams);
        account.set_balance(acc_balance);
        log::debug!(target: "executor", "add messages");
        params.last_tr_lt.store(lt, Ordering::Relaxed);
        let lt = self.add_messages(&mut tr, out_msgs, params.last_tr_lt)?;
        account.set_last_tr_time(lt);
        tr.write_description(&TransactionDescr::Ordinary(description))?;
        #[cfg(feature="timings")]
        self.timings[2].fetch_add(now.elapsed().as_micros() as u64, Ordering::SeqCst);
        tr.set_copyleft_reward(copyleft);
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
        let acc_balance = int!(account.balance().map_or(0, |value| value.grams.as_u128()));
        let msg_balance = int!(in_msg.get_value().map_or(0, |value| value.grams.as_u128()));
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
