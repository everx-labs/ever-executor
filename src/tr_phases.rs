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
    blockchain_config::{BlockchainConfig, GasConfigFull, CalcMsgFwdFees}, 
    error::ExecutorError, vmsetup::VMSetup,
    transaction_executor::TransactionExecutor,
};

use num_traits::ToPrimitive;
use std::sync::{atomic::{AtomicU64, Ordering}, Arc};
use ton_block::{
    Deserializable, GetRepresentationHash, Serializable,
    Account, AccountState, MsgForwardPrices,
    AddSub, CommonMsgInfo, CurrencyCollection, InternalMessageHeader, 
    Message, MsgAddressInt, MsgAddressIntOrNone,
    OutAction, OutActions, RESERVE_ALL_BUT, RESERVE_IGNORE_ERROR, RESERVE_VALID_MODES,
    SENDMSG_ALL_BALANCE, SENDMSG_IGNORE_ERROR, SENDMSG_PAY_FEE_SEPARATELY,
    SENDMSG_REMAINING_MSG_BALANCE, SENDMSG_VALID_FLAGS,
    AccStatusChange, ComputeSkipReason, Transaction, TrActionPhase, 
    TrBouncePhase, TrBouncePhaseOk, TrBouncePhaseNofunds, TrComputePhase,
    TrComputePhaseSkipped, TrComputePhaseVm, TrCreditPhase,
    TrStoragePhase,
    Grams, VarUInteger7,
};
use ton_types::{Cell, error, fail, Result};
use ton_vm::{
    error::TvmError, executor::gas::gas_state::Gas,
    smart_contract_info::SmartContractInfo, stack::StackItem
};


const RESULT_CODE_ACTIONLIST_INVALID: i32 = 32;
const RESULT_CODE_TOO_MANY_ACTIONS:   i32 = 33;
const RESULT_CODE_UNKNOWN_ACTION:     i32 = 34;
const RESULT_CODE_NOT_ENOUGH_GRAMS:   i32 = 37;
const RESULT_CODE_INVALID_BALANCE:    i32 = 40;
const RESULT_CODE_BAD_ACCOUNT_STATE:  i32 = 41;
const RESULT_CODE_UNSUPPORTED:        i32 = -1;
const MAX_ACTIONS: usize = 255;

// #[allow(dead_code)]
// const MAX_MSG_CELLS: usize = 1 << 13;
// #[allow(dead_code)]
// const MAX_MSG_BITS: usize = 1 << 21;

pub const MINIMAL_FEE: u64 = 1; //1 nanogram

/// Implementation of transaction's storage phase.
/// If account does not exist - phase skipped.
/// Calculates storage fees and substracts them from account balance.
/// If account balance becomes negative after that, then account is frozen.
pub fn storage_phase(
    acc: &mut Account,
    tr: &mut Transaction,
    config: &BlockchainConfig,
    is_special: bool
) -> Option<TrStoragePhase> {
    log::debug!(target: "executor", "storage_phase");
    let balance = acc.get_balance()?.clone();

    let mut fee;
    if is_special {
        fee = Grams::zero()
    } else {
        fee = acc
            .storage_info()
            .and_then(|info| info.due_payment.clone())
            .unwrap_or(Grams::zero());
        fee.add(&config.calc_storage_fee(
            acc.storage_info()?,
            acc.get_addr()?,
            tr.now(),
        ).into()).unwrap();
    }

    if balance.grams >= fee {
        assert_eq!(acc.sub_funds(&CurrencyCollection::from_grams(fee.clone())).unwrap(), true, "all checks have been done");
        tr.total_fees_mut().add(&CurrencyCollection::from_grams(fee.clone())).unwrap();
        log::debug!(target: "executor", "AccStatusChange::Unchanged");
        Some(TrStoragePhase::with_params(fee, None, AccStatusChange::Unchanged))
    } else {
        fee.sub(&balance.grams).unwrap();
        let collected = balance.grams.clone();
        assert_eq!(acc.sub_funds(&CurrencyCollection::from_grams(balance.grams)).unwrap(), true, "all checks have been done");
        acc.freeze_account();
        tr.total_fees_mut().add(&CurrencyCollection::from_grams(collected.clone())).unwrap();
        log::debug!(target: "executor", "AccStatusChange::Frozen");
        Some(TrStoragePhase::with_params(collected, Some(fee), AccStatusChange::Frozen))
    }
}

/// Implementation of transaction's credit phase.
/// Increases account balance by the amount that appears in the internal message header.
/// If account does not exist - phase skipped.
/// If message is not internal - phase skipped.
pub fn credit_phase(msg: &Message, acc: &mut Account) -> Option<TrCreditPhase> {
    log::debug!(target: "executor", "credit_phase");
    if Account::AccountNone == *acc {
        log::debug!(target: "executor", " Account::AccountNone");
        return None;
    }
    msg.get_value().and_then(|value| {
        acc.add_funds(value).unwrap();
        Some(TrCreditPhase::with_params(None, value.clone()))
    })
    //TODO: Is it need to credit with ihr_fee value in internal messages?
}

pub fn init_gas(acc_balance: u128, msg_balance: u128, is_external: bool, is_special: bool, gas_info: &GasConfigFull) -> Gas {
    let gas_max = if is_special {
        gas_info.special_gas_limit
    } else {
        std::cmp::min(gas_info.gas_limit, (acc_balance / gas_info.get_real_gas_price() as u128) as u64)
    };
    let (gas_limit, gas_credit) =
        if is_external {
            (0, std::cmp::min(gas_info.gas_credit, gas_max))
        } else {
            let gasl = std::cmp::min(gas_max, (msg_balance / gas_info.get_real_gas_price() as u128) as u64);
            (gasl, 0)
        };
    log::debug!(
        target: "executor", 
        "gas before: gm: {}, gl: {}, gc: {}, price: {}", 
        gas_max, gas_limit, gas_credit, gas_info.get_real_gas_price()
    );
    Gas::new(gas_limit as i64, gas_credit as i64, gas_max as i64, gas_info.get_real_gas_price() as i64)
}

/// Implementation of transaction's computing phase.
/// Evaluates new accout state and invokes TVM if account has contract code.
pub fn compute_phase(
    msg: Option<&Message>,
    acc: &mut Account, 
    smc_info: &SmartContractInfo, 
    stack_builder: &dyn TransactionExecutor,
    config: &GasConfigFull,
    is_special: bool,
    debug: bool,                                                                  
) -> Result<(TrComputePhase, Option<Cell>)> {
    let mut msg_balance = 0;
    let mut is_external = false;
    let (mut new_acc, mut phase) = match msg {
        Some(ref msg) => {
            msg_balance = msg.get_value().map(|value| value.grams.value())
                .cloned().unwrap_or_default().to_u128()
                .ok_or(ExecutorError::TrExecutorError("Failed to convert msg balance to u128".to_string()))?;
            is_external = msg.is_inbound_external();
            compute_new_state(acc.clone(), msg)
        }
        None => (acc.clone(), TrComputePhase::Vm(TrComputePhaseVm::default()))
    };

    if let TrComputePhase::Skipped(_) = phase {
        return Ok((phase, None));
    }

    let acc_balance = new_acc.get_balance().cloned().unwrap_or_default()
        .grams.value().to_u128()
        .ok_or(ExecutorError::TrExecutorError(
            "Failed to convert account balance to u128".to_string()))?;
    log::debug!(target: "executor", "acc balance: {}", acc_balance);
    log::debug!(target: "executor", "msg balance: {}", msg_balance);
    //code must present but can be empty (i.g. for uninitialized account)
    let code = new_acc.get_code().unwrap_or_default();

    let gas = init_gas(acc_balance, msg_balance, is_external, is_special, config);
    let vm_phase = phase.get_vmphase_mut().unwrap();
    vm_phase.gas_credit = match gas.get_gas_credit() as u32 {
        0 => None,
        value => Some(value.into())
    };
    vm_phase.gas_limit = (gas.get_gas_limit() as u64).into();

    let mut vm = VMSetup::new(code.into())
        .set_contract_info(&smc_info)
        .set_stack(stack_builder.build_stack(msg, &new_acc))
        .set_data(new_acc.get_data().unwrap_or(Cell::default()))
        .set_gas(gas)
        .set_debug(debug)
        .create();
    
    //TODO: set vm_init_state_hash

    match vm.execute() {
        Err(e) => {
            log::debug!(target: "executor", "VM terminated with exception: {}", e);
            vm_phase.exit_code = if let Some(TvmError::TvmExceptionFull(e)) = e.downcast_ref() {
                e.number as i32
            } else if let Some(TvmError::TvmException(e)) = e.downcast_ref() {
                *e as i32
            } else if let Some(e) = e.downcast_ref::<ton_types::types::ExceptionCode>() {
                *e as i32
            } else {
                -1
            };
            vm_phase.success = vm.get_committed_state().is_committed();
        },
        Ok(exit_code) => {
            //TODO: implement VM exit_code() method 
            vm_phase.exit_code = exit_code;
            vm_phase.success = vm.get_committed_state().is_committed();
        },
    };
    log::debug!(target: "executor", "VM terminated with exit code {}", vm_phase.exit_code);

    // calc gas fees
    let gas = vm.get_gas();
    let credit = gas.get_gas_credit() as u32;
    //for external messages gas will not be exacted if VM throws the exception and gas_credit != 0 
    let used = gas.get_gas_used() as u64;
    vm_phase.gas_used = VarUInteger7(used.into());
    if credit != 0 {
        if is_external {
            fail!(
                ExecutorError::TrExecutorError( 
                    "Contract did not accepted on external message processing".to_string()
                )
            )
        }
        vm_phase.gas_fees = Grams::zero();
    } else { // credit == 0 means contract accepted
        let gas_fees = if is_special { 0 } else { config.calc_gas_fee(used) };
        vm_phase.gas_fees = Grams(gas_fees.into());
    };
    //TODO: get vm steps

    log::debug!(
        target: "executor", 
        "gas after: gl: {}, gc: {}, gu: {}, fees:{}", 
        gas.get_gas_limit() as u64, credit, used, vm_phase.gas_fees.0
    );
   
    //set mode
    vm_phase.mode = 0;

    //TODO: set vm_steps
    //TODO: vm_final_state_hash
    let gas_fees = vm_phase.gas_fees.clone();
    //exact gass from account balance, balance cannot be less than gas_fees.
    new_acc.sub_funds(&CurrencyCollection::from_grams(gas_fees)).unwrap();
    
    match vm.get_committed_state().get_root() {
        StackItem::Cell(cell) => { new_acc.set_data(cell); },
        _ => {
            log::debug!(target: "executor", "invalid contract, it must be cell in c4 register");
            vm_phase.success = false;
        },
    }
    *acc = new_acc;

    let out_actions = match vm.get_committed_state().get_actions() {
        StackItem::Cell(root_cell) => Some(root_cell),
        _ => {
            log::debug!(target: "executor", "invalid contract, it must be cell in c5 register");
            vm_phase.success = false;
            None
        },
    };
    
    Ok((phase, out_actions))
}

fn create_account_state(in_msg: &Message, bounce: bool) -> (Account, TrComputePhase) {
    log::debug!(target: "executor", "create_account_state");
    let skipped_phase_no_state = TrComputePhase::Skipped(
        TrComputePhaseSkipped { reason: ComputeSkipReason::NoState }
    );
    //try to create account with constructor message
    let new_acc = Account::with_message(in_msg);
    if new_acc.is_err() {
        //message has no code and data,
        //check bounce flag
        if bounce {
            //let skip computing phase, because account not exist and bounce flag is setted. 
            //Account will not be created, return AccountNone
            (Account::default(), skipped_phase_no_state)
        } else if in_msg.get_value().is_some() {
            //value-bearing message with no bounce: create uninitialized account
            log::debug!(target: "executor", "new uninitialized acc is created");
            let address = in_msg.dst().unwrap();
            let balance = in_msg.get_value();
            (
                Account::with_address_and_ballance(&address, balance.unwrap_or(&CurrencyCollection::default())),
                TrComputePhase::Vm(TrComputePhaseVm::default())
            )
        } else {
            //external message: skip computing phase and account will not be created
            //return undefined account
            (Account::default(), skipped_phase_no_state)
        }
    } else {
        //account created from constructor message
        //but check that inbound message bear some value,
        //otherwise it will be frozen
        if in_msg.get_value().is_some() {

            log::debug!(target: "executor", "new acc is created");

            //it's ok - active account will be created.
            //set apropriate flags in phase.
            let mut phase = TrComputePhase::Vm(TrComputePhaseVm::default());
            phase.activated(true);
            //return activated account
            (new_acc.unwrap(), phase)
        } else {
            log::debug!(target: "executor", "new acc is created and frozen");
            (
                new_acc.and_then(|mut acc| { acc.freeze_account(); Ok(acc) }).unwrap(),
                skipped_phase_no_state,
            )
        }
    }
}

fn compute_account_state(mut acc: Account, in_msg: &Message, bounce: bool) -> (Account, TrComputePhase) {
    log::debug!(target: "executor", "compute_account_state");    
    let mut phase = TrComputePhase::Vm(TrComputePhaseVm::default());
    let skipped_phase_no_state = TrComputePhase::Skipped(
        TrComputePhaseSkipped { reason: ComputeSkipReason::NoState }
    );
    let state = acc.state().expect("account must exist");
    
    //Account exists, but can be in different states.
    match state {
        AccountState::AccountActive(_) => {
            //account is active, just return it
            log::debug!(target: "executor", "account state: AccountActive");
            (acc, phase)
        }
        AccountState::AccountUninit => {
            log::debug!(target: "executor", "AccountUninit");
            if let Some(state_init) = in_msg.state_init() {
                // if msg is a constructor message then
                // borrow code and data from it and switch account state to 'active'.
                acc.activate(state_init.clone());
                phase.activated(true);
                log::debug!(target: "executor", "external message for uninitialized: activated");
                (acc, phase)
            } else {
                if bounce {
                    //skip computing phase, because account is uninitialized
                    //and msg doesn't contain StateInit.  );
                    log::debug!(target: "executor", "skipped_phase_no_state");
                    (acc, skipped_phase_no_state)
                } else if in_msg.get_value().is_some() {
                    //account is uninitialized, but we can send grams to it,
                    //we will not skip computing phase, but invoke TVM as if
                    //the code of the smart contract was empty 
                    //(i.e., consisting of an implicit RET)
                    log::debug!(target: "executor", "account is uninitialized, but we can send grams to it");
                    (acc, phase)
                } else {
                    //external message for uninitialized account,
                    //skip computing phase.
                    log::debug!(
                        target: "executor", 
                        "external message for uninitialized: skip computing phase"
                    );
                    (acc, skipped_phase_no_state)
                }
            }
        }
        AccountState::AccountFrozen(_) => {
            log::debug!(target: "executor", "AccountFrozen");
            //account balance was credited and if it positive after that
            //and inbound message bear code and data then make some check and unfreeze account
            if !acc.get_balance().unwrap().grams.is_zero() {
                if let Some(state_init) = in_msg.state_init() {
                    log::debug!(target: "executor", "external message for frozen: activated");
                    acc.activate(state_init.clone());
                    phase.activated(true);
                    return (acc, phase)
                }
            }
            //skip computing phase, because account is frozen (bad state)
            log::debug!(target: "executor", "account is frozen (bad state): skip computing phase");
            phase = TrComputePhase::Skipped(
                TrComputePhaseSkipped { reason: ComputeSkipReason::BadState }
            );
            (acc, phase)
        }
    }
}

/// Calculate new account state according to inbound message and current account state.
/// If account does not exist - it can be created with uninitialized state.
/// If account is uninitialized - it can be created with active state.
/// If account exists - it can be frozen.
/// Returns computed initial phase.
fn compute_new_state(acc: Account, in_msg: &Message) -> (Account, TrComputePhase) {
    let mut bounce = false;
    if let CommonMsgInfo::IntMsgInfo(ref header) = in_msg.header() {
        bounce = header.bounce;
    }
    match acc {
        Account::AccountNone => create_account_state(in_msg, bounce),
        _ => compute_account_state(acc, in_msg, bounce),
    }
}

/// Implementation of transaction's action phase.
/// If computing phase is successful then action phase is started.
/// If TVM invoked in computing phase returned some output actions, 
/// then they will be added to transaction's output message list.
/// Total value from all outbound internal messages will be collected and
/// substracted from account balance. If account has enough funds this 
/// will be succeded, otherwise action phase is failed, transaction will be
/// marked as aborted, account changes will be rollbacked.
pub fn action_phase(
    tr: &mut Transaction,
    acc: &mut Account,
    actions_cell: Option<Cell>,
    config: &BlockchainConfig,
    lt: Arc<AtomicU64>,
    is_special: bool,
) -> Option<TrActionPhase> {
    let mut phase = TrActionPhase::default();
    let mut total_spend_value = CurrencyCollection::default();
    let mut total_reserved_value = CurrencyCollection::default();
    let mut out_msgs = vec![];
    let mut msg_action_count = 0i16;
    let mut special_action_count = 0i16;
    let skipped_action_count = 0i16;
    let mut actions = OutActions::default();
    let mut remaining_balance = acc.get_balance().unwrap().clone();
    phase.tot_actions = 0;
    phase.spec_actions = 0;
    phase.msgs_created = 0;
    phase.result_code = 0;
    phase.result_arg = None;
    phase.valid = true;
    phase.success = true;

    let actions_cell = actions_cell.unwrap_or_default();
    if let Err(err) = actions.read_from(&mut actions_cell.into()) {
        log::debug!(
            target: "executor", 
            "cannot parse action list: format is invalid, err: {}", 
            err
        );
        phase.success = false;
        phase.valid = false;
        phase.result_code = RESULT_CODE_ACTIONLIST_INVALID;
        return Some(phase);
    }

    if actions.len() > MAX_ACTIONS {
        log::debug!(target: "executor", "too many actions: {}", actions.len());
        phase.success = false;
        phase.valid = false;
        phase.result_code = RESULT_CODE_TOO_MANY_ACTIONS;
        return Some(phase);
    }
    phase.action_list_hash = actions.hash().unwrap();
    phase.tot_actions = actions.len() as i16;

    for (i, action) in actions.iter_mut().enumerate() {
        let act = std::mem::replace(action, OutAction::None);
        //debug!(target: "executor", "OutAction: {}", serde_json::to_string_pretty(&act).unwrap_or_default());
        let err_code = match act {
            OutAction::SendMsg{ mode, mut out_msg } => {
                let result = outmsg_action_handler(
                    &mut phase, 
                    acc.get_addr().unwrap().clone(), 
                    mode, 
                    Arc::make_mut(&mut out_msg),
                    lt.fetch_add(1, Ordering::SeqCst),
                    tr.now(),
                    &mut remaining_balance,
                    config,
                    is_special
                );
                match result {
                    Ok(msg_balance) => {
                        msg_action_count += 1;
                        out_msgs.push(out_msg);
                        total_spend_value.add(&msg_balance).ok()
                            .map_or(RESULT_CODE_INVALID_BALANCE, |_| 0)
                    },
                    Err(code) => code,
                }
            },
            OutAction::ReserveCurrency{ mode, value} => {
                match reserve_action_handler(mode, &value, &mut remaining_balance) {
                    Ok(reserved_value) => {
                        special_action_count += 1;
                        total_reserved_value.add(&reserved_value).ok()
                            .map_or(RESULT_CODE_INVALID_BALANCE, |_| 0)
                    },
                    Err(code) => code,
                }
            },
            OutAction::SetCode{ new_code: code } => {
                log::debug!(target: "executor", "OutAction::SetCode {}", code);
                setcode_action_handler(acc, code).unwrap_or({
                    special_action_count += 1;
                    0
                })
            },
            OutAction::None => {
                RESULT_CODE_UNKNOWN_ACTION
            },
            _ => {
                unimplemented!()
            }
        };
        if err_code != 0 {
            log::debug!(target: "executor", "action failed: error_code={}", err_code);
            phase.result_code = err_code;
            phase.result_arg = Some(i as i32);
            phase.valid = true;
            phase.success = false;
            phase.msgs_created = msg_action_count;
            if err_code == RESULT_CODE_NOT_ENOUGH_GRAMS {
                phase.no_funds = true;
            }
            return Some(phase);
        }        
    }

    //calc new account balance
    let mut new_balance = remaining_balance.clone();
    new_balance.add(&total_reserved_value)
        .map_err(
            |e| { 
                log::debug!(target: "executor", "failed to add account balance with reserved value"); 
                e 
            }
        )
        .ok()?;
    //calc difference of new balance from old balance
    let mut balance_diff = acc.get_balance().unwrap().clone();
    //must be succeded, because check is already done
    assert_eq!(balance_diff.sub(&new_balance).ok()?, true);

    //TODO: substract difference from account balance
    if !acc.sub_funds(&balance_diff).unwrap_or(false) {
        log::debug!(target: "executor", "account balance doesn't have enought grams");
        phase.success = false;
        phase.no_funds = true;
        phase.result_code = RESULT_CODE_INVALID_BALANCE;
    } else {
        //TODO: some kind of check to freeze account.
    }

    for msg in out_msgs {
        tr.add_out_message(&msg).ok()?;
    }
    tr.total_fees_mut()
        .add(&CurrencyCollection::from_grams(
            phase.total_action_fees.clone().unwrap_or(Grams::zero()
    ))).unwrap();

    //for now we support only this change status
    phase.status_change = AccStatusChange::Unchanged;
    phase.spec_actions = special_action_count;
    phase.msgs_created = msg_action_count;
    phase.skipped_actions = skipped_action_count;
    Some(phase)
}

fn outmsg_action_handler(
    phase: &mut TrActionPhase, 
    myself: MsgAddressInt,
    mut mode: u8, 
    msg: &mut Message,
    lt: u64,
    ut: u32,
    remaining: &mut CurrencyCollection,
    config: &BlockchainConfig,
    is_special: bool,
) -> std::result::Result<CurrencyCollection, i32> {
    let invalid_flags = SENDMSG_REMAINING_MSG_BALANCE | SENDMSG_ALL_BALANCE;
    if  (mode & !SENDMSG_VALID_FLAGS) != 0 ||
        (mode & invalid_flags) == invalid_flags
    {
        log::error!(target: "executor", "outmsg mode has unsupported flags");
        return Err(RESULT_CODE_UNSUPPORTED);
    }
    let skip = (mode & SENDMSG_IGNORE_ERROR) != 0;
    let value = msg.get_value().map(|v| v.clone()).unwrap_or(CurrencyCollection::default());
    let mut fwd_fee = Grams::default();
    let mut ihr_fee = Grams::default();
    let mut is_internal_msg = false;
    let ihr_disabled;

    match msg.header_mut() {
        CommonMsgInfo::IntMsgInfo(ref mut int_header) => {
            int_header.created_at = ut.into();
            int_header.created_lt = lt;
            int_header.src = MsgAddressIntOrNone::Some(myself);
            fwd_fee = int_header.fwd_fee.clone();
            ihr_fee = int_header.ihr_fee.clone();
            is_internal_msg = true;
            ihr_disabled = int_header.ihr_disabled;
        },
        CommonMsgInfo::ExtOutMsgInfo(ref mut ext_header) => {
            ext_header.created_at = ut.into();
            ext_header.created_lt = lt;
            ext_header.src = MsgAddressIntOrNone::Some(myself);
            ihr_disabled = true;
        },
        CommonMsgInfo::ExtInMsgInfo(_) => return Err(-1),
    };

    // TODO: check and rewrite src and dst adresses (see check_replace_src_addr and 
    // check_rewrite_dest_addr functions)

    let config = config.get_fwd_prices(msg);

    let compute_fwd_fee = if is_special {
        Grams::zero()
    } else {
        config.calc_fwd_fee(msg)
            .map_err(|_| RESULT_CODE_ACTIONLIST_INVALID)?
            .1
            .into()
    };

    if fwd_fee < compute_fwd_fee {
        fwd_fee = compute_fwd_fee.clone();
    }

    if !ihr_disabled {
        let compute_ihr_fee = config.calc_ihr_fee(compute_fwd_fee.value().to_u128().unwrap_or(0)).into();
        if ihr_fee < compute_ihr_fee {
            ihr_fee = compute_ihr_fee;
        }
    }

    let fwd_mine_fee = if is_internal_msg {
        config.calc_mine_fee(fwd_fee.value().to_u128().unwrap_or(0)).into()
    } else {
        fwd_fee.clone()
    };
    let fwd_remain_fee = fwd_fee.0.clone() - fwd_mine_fee.0.clone();

    let mut result_grams = value.grams.clone();
    let mut new_msg_value = value.clone();
    if is_internal_msg {
        if (mode & SENDMSG_ALL_BALANCE) != 0 {
            //send all remaining account balance
            result_grams = remaining.grams.clone();
            new_msg_value = remaining.clone();

            mode &= !SENDMSG_PAY_FEE_SEPARATELY;
        }
        // TODO: process SENDMSG_REMAINING_MSG_BALANCE flag
        
        if (mode & SENDMSG_PAY_FEE_SEPARATELY) != 0 {
            //we must pay the fees, sum them with msg value
            result_grams.0 += &ihr_fee.0;
            result_grams.0 += &fwd_fee.0;
        } else if new_msg_value.grams.0 < (&fwd_fee.0 + &ihr_fee.0) {
            //msg value is too small, reciever cannot pay the fees 
            return if skip { Err(0) } else { 
                log::error!(
                    target: "executor", 
                    "msg balance is too small, cannot pay fwd+ihr fees: need = {}, have = {}", 
                    &fwd_fee.0 + &ihr_fee.0, new_msg_value.grams.0
                );
                Err(RESULT_CODE_NOT_ENOUGH_GRAMS) 
            };
        } else {
            //reciever will pay the fees
            new_msg_value.grams.0 -= &ihr_fee.0;
            new_msg_value.grams.0 -= &fwd_fee.0;
        }

        if remaining.grams.0 < result_grams.0 {
            return if skip { Err(0) } else {
                log::error!(
                    target: "executor", 
                    "account balance is too small, cannot send so many grams"
                ); 
                Err(RESULT_CODE_NOT_ENOUGH_GRAMS) 
            };
        }

        //set evaluated fees and value back to msg
        if let CommonMsgInfo::IntMsgInfo(ref mut int_header) = msg.header_mut() {
            int_header.fwd_fee = fwd_remain_fee.into();
            int_header.ihr_fee = ihr_fee.clone();
            int_header.value = new_msg_value;
        }

    } else {
        // TODO: check if account can pay fee
        result_grams = fwd_fee.clone();
    }

    // TODO: process SENDMSG_DELETE_IF_EMPTY flag

    // total fwd fees is sum of messages full fwd and ihr fees
    let total_fwd_fees = phase.total_fwd_fees.take().unwrap_or(Grams::default());
    phase.total_fwd_fees = Some(Grams(total_fwd_fees.0 + fwd_fee.0 + ihr_fee.0));

    // total action fees is sum of messages fwd mine fees
    let total_action_fees = phase.total_action_fees.take().unwrap_or(Grams::default());
    phase.total_action_fees = Some(Grams(total_action_fees.0 + fwd_mine_fee.0));

    phase.tot_msg_size.append(
        &msg
            .write_to_new_cell()
            .map_err(|_| RESULT_CODE_ACTIONLIST_INVALID)?
            .into());

    // TODO: subtract all currencies from account balance
    let value = CurrencyCollection::from_grams(result_grams);
    remaining.sub(&value).or(Err(RESULT_CODE_INVALID_BALANCE))?;
    Ok(value)
}

/// Reserves some grams from accout balance. 
/// Returns calculated reserved value. its calculation depends on mode.
/// Reduces balance by the amount of the reserved value.
fn reserve_action_handler(
    mode: u8, 
    val: &CurrencyCollection,
    remaining: &mut CurrencyCollection,
) -> std::result::Result<CurrencyCollection, i32>  {
    if (mode & !RESERVE_VALID_MODES) != 0 {
        return Err(RESULT_CODE_UNSUPPORTED);
    }

    let mut reserved = val.clone();
    //TODO: need to compare extra currencies too.
    if reserved.grams.0 > remaining.grams.0 {
        //reserving more than remaining balance has,
        //this is error, but check the flag
        if (mode & RESERVE_IGNORE_ERROR) != 0 {
            //reserve all remaining balance
            reserved = remaining.clone();
        } else {
            return Err(RESULT_CODE_NOT_ENOUGH_GRAMS);
        }
    } else {
        // check the mode
        if (mode & RESERVE_ALL_BUT) != 0 {
            // need to reserve all but 'val' grams
            reserved = remaining.clone();
            reserved.sub(&val).or(Err(RESULT_CODE_INVALID_BALANCE))?;
        } 
    }

    remaining.sub(&reserved).or(Err(RESULT_CODE_INVALID_BALANCE))?;
    Ok(reserved)
}

fn setcode_action_handler(acc: &mut Account, code: Cell) -> Option<i32> {
    match acc.set_code(code) {
        true => None,
        false => Some(RESULT_CODE_BAD_ACCOUNT_STATE)
    }
}

/// Implementation of transaction's bounce phase.
/// Bounce phase occurs only if transaction 'aborted' flag is set and
/// if inbound message is internal message with field 'bounce=true'.
/// Generates outbound internal message for original message sender, with value equal
/// to value of original message minus gas payments and forwarding fees 
/// and empty body. Generated message is added to transaction's output message list.
pub fn bounce_phase(
    msg: Message,
    _acc: &mut Account,
    tr: &mut Transaction,
    gas_fee: u64,
    config: &MsgForwardPrices
) -> Option<TrBouncePhase> {
    if let CommonMsgInfo::IntMsgInfo(msg) = msg.withdraw_header() {
        if msg.bounce {
            // TODO: msg value must be initialized with msg_balance_remaining
            let mut value = msg.value;
            
            let msg_src = match msg.src {
                MsgAddressIntOrNone::None => {
                    log::warn!(target: "executor", "invalid source address");
                    return None
                }
                MsgAddressIntOrNone::Some(addr) => addr
            };

            let mut header = InternalMessageHeader::with_addresses(
                    msg.dst,
                    msg_src,
                    value.clone());
            header.ihr_disabled = true;
            header.bounce = false;
            header.bounced = true;
            let mut bounce_msg = Message::with_int_header(header.clone());

            let (storage, fwd_full_fees) = config.calc_fwd_fee(&bounce_msg).unwrap();
            let fwd_mine_fees = config.calc_mine_fee(fwd_full_fees);
            let fwd_full_fees = fwd_full_fees;

            // TODO: subtract msg balance from account balance (because it was added during
            // credit phase)
            let mut fee = Grams::from(gas_fee);
            fee.0 += fwd_full_fees;
            let phase_ok = value.grams.sub(&fee).unwrap();

            if phase_ok {
                if let CommonMsgInfo::IntMsgInfo(header) = bounce_msg.header_mut() {
                    header.fwd_fee = (fwd_full_fees - fwd_mine_fees).into();
                    header.value = value;
                }
                tr.total_fees_mut().add(&CurrencyCollection::from_grams(fwd_mine_fees.into())).unwrap();
                tr.add_out_message(&bounce_msg).unwrap();
                Some(TrBouncePhase::Ok(
                    TrBouncePhaseOk::with_params(storage, fwd_mine_fees.into(), fwd_full_fees.into())
                ))
            } else {
                Some(TrBouncePhase::Nofunds(
                    TrBouncePhaseNofunds::with_params(storage, fwd_full_fees.into())
                ))
            }
        } else {
            None
        }
    } else {
        None
    }
}