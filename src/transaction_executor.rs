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
    blockchain_config::{BlockchainConfig, CalcMsgFwdFees},
    error::ExecutorError, vmsetup::VMSetup,
};

use std::sync::{atomic::{AtomicU64, Ordering}, Arc};
use ton_block::{
    Deserializable, GetRepresentationHash, Serializable,
    Account, AccountState, ConfigParams, GasLimitsPrices, GlobalCapabilities,
    AddSub, CurrencyCollection, Grams,
    Message, MsgAddressInt, MsgAddressIntOrNone,
    OutAction, OutActions, RESERVE_ALL_BUT, RESERVE_IGNORE_ERROR, RESERVE_VALID_MODES,
    SENDMSG_ALL_BALANCE, SENDMSG_IGNORE_ERROR, SENDMSG_PAY_FEE_SEPARATELY, SENDMSG_DELETE_IF_EMPTY,
    SENDMSG_REMAINING_MSG_BALANCE, SENDMSG_VALID_FLAGS,
    AccStatusChange, ComputeSkipReason, Transaction,
    TrActionPhase, TrBouncePhase, TrComputePhase, TrComputePhaseVm, TrCreditPhase, TrStoragePhase,
};
use ton_types::{error, fail, ExceptionCode, Cell, Result, HashmapE, HashmapType, IBitstring, UInt256};
use ton_vm::{
    error::tvm_exception, executor::gas::gas_state::Gas,
    smart_contract_info::SmartContractInfo,
    stack::{Stack, StackItem},
};

const RESULT_CODE_ACTIONLIST_INVALID: i32 = 32;
const RESULT_CODE_TOO_MANY_ACTIONS:   i32 = 33;
const RESULT_CODE_UNKNOWN_ACTION:     i32 = 34;
const RESULT_CODE_NOT_ENOUGH_GRAMS:   i32 = 37;
const RESULT_CODE_NOT_ENOUGH_EXTRA:   i32 = 38;
const RESULT_CODE_INVALID_BALANCE:    i32 = 40;
const RESULT_CODE_BAD_ACCOUNT_STATE:  i32 = 41;
const RESULT_CODE_UNSUPPORTED:        i32 = -1;
const MAX_ACTIONS: usize = 255;



pub trait TransactionExecutor {
    fn execute_with_libs(
        &self,
        in_msg: Option<&Message>,
        account_root: &mut Cell,
        state_libs: HashmapE, // masterchain libraries
        block_unixtime: u32,
        block_lt: u64,
        last_tr_lt: Arc<AtomicU64>,
        debug: bool
    ) -> Result<Transaction>;

    fn execute(
        &self,
        in_msg: Option<&Message>,
        account_root: &mut Cell,
        block_unixtime: u32,
        block_lt: u64,
        last_tr_lt: Arc<AtomicU64>,
        debug: bool
    ) -> Result<Transaction> {
        self.execute_with_libs(in_msg, account_root, HashmapE::default(), block_unixtime, block_lt, last_tr_lt, debug)
    }
    fn build_contract_info(&self, config_params: &ConfigParams, acc: &Account, acc_address: &MsgAddressInt, block_unixtime: u32, block_lt: u64, tr_lt: u64) -> SmartContractInfo {
        let mut info = SmartContractInfo::with_myself(acc_address.serialize().unwrap_or_default().into());
        *info.block_lt_mut() = block_lt;
        *info.trans_lt_mut() = tr_lt;
        *info.unix_time_mut() = block_unixtime;
        if let Some(balance) = acc.get_balance() {
            // info.set_remaining_balance(balance.grams.0, balance.other.clone());
            *info.balance_remaining_grams_mut() = balance.grams.0;
            *info.balance_remaining_other_mut() = balance.other_as_hashmap();
        }
        if let Some(data) = config_params.config_params.data() {
            info.set_config_params(data.clone());
        }
        info
    }

    fn ordinary_transaction(&self) -> bool;
    fn config(&self) -> &BlockchainConfig;

    fn build_stack(&self, in_msg: Option<&Message>, account: &Account) -> Stack;

    /// Implementation of transaction's storage phase.
    /// If account does not exist - phase skipped.
    /// Calculates storage fees and substracts them from account balance.
    /// If account balance becomes negative after that, then account is frozen.
    fn storage_phase(
        &self,
        acc: &mut Account,
        tr: &mut Transaction,
        is_special: bool
    ) -> Option<TrStoragePhase> {
        log::debug!(target: "executor", "storage_phase");
        if is_special {
            log::debug!(target: "executor", "Spceial account: AccStatusChange::Unchanged");
            return Some(TrStoragePhase::with_params(Grams::zero(), None, AccStatusChange::Unchanged))
        }
        if acc == &Account::AccountNone {
            log::debug!(target: "executor", "Account::None");
            return Some(TrStoragePhase::with_params(Grams::default(), None, AccStatusChange::Unchanged))
        }

        let mut fee = Grams::from(self.config().calc_storage_fee(
            acc.storage_info()?,
            acc.get_addr()?.is_masterchain(),
            tr.now(),
        ));
        if let Some(ref due_payment) = acc.storage_info()?.due_payment {
            fee.add(due_payment).ok()?;
        }

        let balance = acc.get_balance()?.grams.clone();
        if balance >= fee {
            let fee = CurrencyCollection::from_grams(fee);
            log::debug!(target: "executor", "storage fee: {}", fee.grams);
            acc_sub_funds(acc, &fee)?;
            tr.total_fees_mut().add(&fee).ok()?;
            log::debug!(target: "executor", "AccStatusChange::Unchanged");
            acc.set_last_paid(tr.now());
            Some(TrStoragePhase::with_params(fee.grams, None, AccStatusChange::Unchanged))
        } else {
            fee.sub(&balance).ok()?;
            let balance = CurrencyCollection::from_grams(balance);
            log::debug!(target: "executor", "storage fee: {}", balance.grams);
            acc_sub_funds(acc, &balance)?;
            acc.try_freeze().ok()?;
            tr.total_fees_mut().add(&balance).ok()?;
            log::debug!(target: "executor", "AccStatusChange::Frozen");
            acc.set_last_paid(tr.now());
            Some(TrStoragePhase::with_params(balance.grams, Some(fee), AccStatusChange::Frozen))
        }
    }

    /// Implementation of transaction's credit phase.
    /// Increases account balance by the amount that appears in the internal message header.
    /// If account does not exist - phase skipped.
    /// If message is not internal - phase skipped.
    fn credit_phase(&self, msg: &Message, acc: &mut Account) -> Option<TrCreditPhase> {
        log::debug!(target: "executor", "credit_phase");
        let header = msg.int_header()?;
        if acc == &Account::AccountNone {
            log::debug!(target: "executor", "Account::AccountNone");
            if !header.value.grams.is_zero() {
                return Some(TrCreditPhase::with_params(None, header.value.clone()))
            }
            return None
        }
        log::debug!(target: "executor", "add funds {}", header.value.grams);
        acc.add_funds(&header.value).ok()?;
        Some(TrCreditPhase::with_params(None, header.value.clone()))
        //TODO: Is it need to credit with ihr_fee value in internal messages?
    }

    /// Implementation of transaction's computing phase.
    /// Evaluates new accout state and invokes TVM if account has contract code.
    fn compute_phase(
        &self,
        msg: Option<&Message>,
        acc: &mut Account, 
        state_libs: HashmapE, // masgerchain libraries
        smc_info: &SmartContractInfo, 
        stack_builder: &dyn TransactionExecutor,
        is_special: bool,
        debug: bool,
    ) -> Result<(TrComputePhase, Option<Cell>)> {
        let mut vm_phase = TrComputePhaseVm::default();
        let is_masterchain;
        let (msg_balance, is_external) = if let Some(ref msg) = msg {
            is_masterchain = msg.dst().map(|addr| addr.is_masterchain()).unwrap_or_default();
            if let Some(header) = msg.int_header() {
                log::debug!(target: "executor", "msg internal");
                if acc == &Account::AccountNone && create_account_state(acc, msg, header.bounce, smc_info.unix_time()) {
                }
                (header.value.grams.0, false)
            } else {
                log::debug!(target: "executor", "msg external");
                (0, true)
            }
        } else {
            debug_assert!(acc != &Account::AccountNone);
            is_masterchain = acc.get_addr().map(|addr| addr.is_masterchain()).unwrap_or_default();
            (0, false)
        };
        let acc_balance = acc.get_balance().map(|value| value.grams.0).unwrap_or_default();
        log::debug!(target: "executor", "acc balance: {}", acc_balance);
        log::debug!(target: "executor", "msg balance: {}", msg_balance);
        let is_ordinary = self.ordinary_transaction();
        let gas_config = self.config().get_gas_config(is_masterchain);
        let gas = init_gas(acc_balance, msg_balance, is_external, is_special, is_ordinary, gas_config);
        if gas.get_gas_limit() == 0 && gas.get_gas_credit() == 0 {
            log::debug!(target: "executor", "skip computing phase no gas");
            return Ok((TrComputePhase::skipped(ComputeSkipReason::NoGas), None))
        }

        let mut libs = vec![];
        if let Some(ref msg) = msg {
            if let Some(state_init) = msg.state_init() {
                libs.push(state_init.libraries().inner());
            }
            if let Some(reason) = compute_new_state(acc, msg) {
                return Ok((TrComputePhase::skipped(reason), None))
            }
        };
        //code must present but can be empty (i.g. for uninitialized account)
        let code = acc.get_code().unwrap_or_default();
        let data = acc.get_data().unwrap_or_default();
        libs.push(acc.libraries().inner());
        libs.push(state_libs);

        vm_phase.gas_credit = match gas.get_gas_credit() as u32 {
            0 => None,
            value => Some(value.into())
        };
        vm_phase.gas_limit = (gas.get_gas_limit() as u64).into();

        let mut vm = VMSetup::new(code.into())
            .set_contract_info(&smc_info)
            .set_stack(stack_builder.build_stack(msg, acc))
            .set_data(data)
            .set_libraries(libs)
            .set_gas(gas)
            .set_debug(debug)
            .create();
        
        //TODO: set vm_init_state_hash

        let result = vm.execute();
        log::trace!(target: "executor", "execute result: {:?}", result);
        match result {
            Err(err) => {
                log::debug!(target: "executor", "VM terminated with exception: {}", err);
                let exception = tvm_exception(err)?;
                vm_phase.exit_code = if let Some(code) = exception.custom_code() {
                    code
                } else {
                    !(exception.exception_code().unwrap_or(ExceptionCode::UnknownError) as i32)
                };
                vm_phase.exit_arg = match exception.value.as_integer().and_then(|value| value.into(std::i32::MIN..=std::i32::MAX)) {
                    Err(_) | Ok(0) => None,
                    Ok(exit_arg) => Some(exit_arg)
                };
            }
            Ok(exit_code) => vm_phase.exit_code = exit_code
        };
        vm_phase.success = vm.get_committed_state().is_committed();
        log::debug!(target: "executor", "VM terminated with exit code {}", vm_phase.exit_code);

        // calc gas fees
        let gas = vm.get_gas();
        let credit = gas.get_gas_credit() as u32;
        //for external messages gas will not be exacted if VM throws the exception and gas_credit != 0 
        let used = gas.get_gas_used() as u64;
        vm_phase.gas_used = used.into();
        if credit != 0 {
            if is_external {
                fail!(ExecutorError::NoAcceptError(vm_phase.exit_code))
            }
            vm_phase.gas_fees = Grams::zero();
        } else { // credit == 0 means contract accepted
            let gas_fees = if is_special { 0 } else { gas_config.calc_gas_fee(used) };
            vm_phase.gas_fees = Grams(gas_fees.into());
        };

        log::debug!(
            target: "executor", 
            "gas after: gl: {}, gc: {}, gu: {}, fees:{}", 
            gas.get_gas_limit() as u64, credit, used, vm_phase.gas_fees
        );
    
        //set mode
        vm_phase.mode = 0;
        vm_phase.vm_steps = vm.steps();
        //TODO: vm_final_state_hash
        let gas_fees = vm_phase.gas_fees.clone();
        //exact gass from account balance, balance cannot be less than gas_fees.
        acc_sub_funds(acc, &CurrencyCollection::from_grams(gas_fees));
        
        if let StackItem::Cell(cell) = vm.get_committed_state().get_root() {
            acc.set_data(cell);
        } else {
            log::debug!(target: "executor", "invalid contract, it must be cell in c4 register");
            vm_phase.success = false;
        }

        let out_actions = if let StackItem::Cell(root_cell) = vm.get_committed_state().get_actions() {
            Some(root_cell)
        } else {
            log::debug!(target: "executor", "invalid contract, it must be cell in c5 register");
            vm_phase.success = false;
            None
        };

        Ok((TrComputePhase::Vm(vm_phase), out_actions))
    }

    /// Implementation of transaction's action phase.
    /// If computing phase is successful then action phase is started.
    /// If TVM invoked in computing phase returned some output actions, 
    /// then they will be added to transaction's output message list.
    /// Total value from all outbound internal messages will be collected and
    /// substracted from account balance. If account has enough funds this 
    /// will be succeded, otherwise action phase is failed, transaction will be
    /// marked as aborted, account changes will be rollbacked.
    fn action_phase(
        &self,
        tr: &mut Transaction,
        acc: &mut Account,
        msg_remaining_balance: &mut CurrencyCollection,
        actions_cell: Cell,
        lt: Arc<AtomicU64>,
        is_special: bool,
    ) -> Option<TrActionPhase> {
        let mut phase = TrActionPhase::default();
        let mut total_reserved_value = CurrencyCollection::default();
        let mut out_msgs = vec![];
        let mut acc_remaining_balance = acc.get_balance()?.clone();
        let mut actions = match OutActions::construct_from_cell(actions_cell) {
            Err(err) => {
                log::debug!(
                    target: "executor", 
                    "cannot parse action list: format is invalid, err: {}", 
                    err
                );
                phase.result_code = RESULT_CODE_ACTIONLIST_INVALID;
                return Some(phase)
            }
            Ok(actions) => actions
        };

        if actions.len() > MAX_ACTIONS {
            log::debug!(target: "executor", "too many actions: {}", actions.len());
            phase.result_code = RESULT_CODE_TOO_MANY_ACTIONS;
            return Some(phase);
        }
        phase.action_list_hash = actions.hash().ok()?;
        phase.tot_actions = actions.len() as i16;

        let my_addr = acc.get_addr()?.clone();
        for (i, action) in actions.iter_mut().enumerate() {
            let err_code = match std::mem::replace(action, OutAction::None) {
                OutAction::SendMsg{ mode, mut out_msg } => {
                    out_msg.set_src(MsgAddressIntOrNone::Some(my_addr.clone()));
                    let result = outmsg_action_handler(
                        &mut phase, 
                        mode,
                        &mut out_msg,
                        &mut acc_remaining_balance,
                        msg_remaining_balance,
                        self.config(),
                        is_special
                    );
                    match result {
                        Ok(_) => {
                            phase.msgs_created += 1;
                            out_msgs.push(out_msg);
                            0
                        }
                        Err(code) => code
                    }
                }
                OutAction::ReserveCurrency{ mode, value } => {
                    match reserve_action_handler(mode, &value, &mut acc_remaining_balance) {
                        Ok(reserved_value) => {
                            phase.spec_actions += 1;
                            match total_reserved_value.add(&reserved_value) {
                                Ok(_) => 0,
                                Err(_) => RESULT_CODE_INVALID_BALANCE
                            }
                        }
                        Err(code) => code
                    }
                }
                OutAction::SetCode{ new_code: code } => {
                    match setcode_action_handler(acc, code) {
                        None => {
                            phase.spec_actions += 1;
                            0
                        }
                        Some(code) => code
                    }
                }
                OutAction::ChangeLibrary{ mode, code, hash} => {
                    match change_library_action_handler(acc, mode, code, hash) {
                        None => {
                            phase.spec_actions += 1;
                            0
                        }
                        Some(code) => code
                    }
                }
                OutAction::None => RESULT_CODE_UNKNOWN_ACTION
            };
            if err_code != 0 {
                log::debug!(target: "executor", "action failed: error_code={}", err_code);
                phase.valid = true;
                phase.result_code = err_code;
                if i != 0 {
                    phase.result_arg = Some(i as i32);
                }
                if err_code == RESULT_CODE_NOT_ENOUGH_GRAMS || err_code == RESULT_CODE_NOT_ENOUGH_EXTRA {
                    phase.no_funds = true;
                }
                return Some(phase);
            }
        }

        //calc new account balance
        acc_remaining_balance.add(&total_reserved_value).map_err(|err|
            log::debug!(target: "executor", "failed to add account \
                balance with reserved value {}", err)).ok()?;
        //calc difference of new balance from old balance
        let mut balance_diff = acc.get_balance()?.clone();
        //must be succeded, because check is already done
        balance_diff.sub(&acc_remaining_balance).ok()?;

        //TODO: substract difference from account balance
        if acc_sub_funds(acc, &balance_diff).is_none() {
            log::debug!(target: "executor", "account balance doesn't have enought grams");
            phase.no_funds = true;
            phase.result_code = RESULT_CODE_INVALID_BALANCE;
        } else {
            //TODO: some kind of check to freeze account.
        }

        for msg in out_msgs.iter_mut() {
            if self.ordinary_transaction() {
                msg.set_at_and_lt(tr.now(), lt.fetch_add(1, Ordering::SeqCst));
            } else {
                msg.set_at_and_lt(tr.now(), lt.load(Ordering::SeqCst));
            }
            tr.add_out_message(&msg).ok()?;
        }
        if let Some(ref fee) = phase.total_action_fees {
            tr.total_fees_mut().grams.add(fee).ok()?;
        }

        phase.valid = true;
        phase.success = true;
        Some(phase)
    }

    /// Implementation of transaction's bounce phase.
    /// Bounce phase occurs only if transaction 'aborted' flag is set and
    /// if inbound message is internal message with field 'bounce=true'.
    /// Generates outbound internal message for original message sender, with value equal
    /// to value of original message minus gas payments and forwarding fees 
    /// and empty body. Generated message is added to transaction's output message list.
    fn bounce_phase(
        &self,
        msg: &Message,
        acc: &mut Account,
        tr: &mut Transaction,
        lt: Arc<AtomicU64>,
        gas_fee: Grams
    ) -> Option<TrBouncePhase> {
        let header = msg.int_header()?;
        if !header.bounce {
            return None
        }
        let msg_src = match msg.src() {
            None => {
                log::warn!(target: "executor", "invalid source address");
                return None
            }
            Some(addr) => addr
        };
        let is_masterchain = msg_src.is_masterchain() || header.dst.is_masterchain();
        let fwd_prices = self.config().raw_config().fwd_prices(is_masterchain)
            .map_err(|err| log::error!(target: "executor", "cannot load fwd_prices from config : {}", err)).ok()?;

        let mut header = header.clone();
        let msg_dst = std::mem::replace(&mut header.dst, msg_src);
        header.src = MsgAddressIntOrNone::Some(msg_dst);

        let (storage, fwd_full_fees) = fwd_prices.fwd_fee(&Cell::default());
        let fwd_mine_fees = fwd_prices.mine_fee(&fwd_full_fees);
        let fwd_fees = Grams::from(fwd_full_fees.0 - fwd_mine_fees.0);

        if !header.value.grams.sub(&gas_fee).ok()? {
            return Some(TrBouncePhase::no_funds(storage, fwd_full_fees))
        }
        if header.value.grams.0 < fwd_full_fees.0 {
            return Some(TrBouncePhase::no_funds(storage, fwd_full_fees))
        }
        log::debug!(target: "executor", "get fee {} from bounce msg {}", fwd_full_fees, header.value.grams);
        acc_sub_funds(acc, &header.value)?;
        header.value.grams.sub(&fwd_full_fees).ok()?;
        header.ihr_disabled = true;
        header.bounce = false;
        header.bounced = true;
        header.ihr_fee = Grams::zero();
        header.fwd_fee = fwd_fees.clone();
        header.created_lt = lt.fetch_add(1, Ordering::SeqCst);
        header.created_at = tr.now().into();

        let mut bounce_msg = Message::with_int_header(header);
        if self.config().raw_config().has_capability(GlobalCapabilities::CapBounceMsgBody) {
            let mut builder = (-1i32).write_to_new_cell().ok()?;
            if let Some(mut body) = msg.body() {
                body.shrink_data(0..256);
                builder.append_bytestring(&body).ok()?;
            }
            bounce_msg.set_body(builder.into());
        }
        tr.add_out_message(&bounce_msg)
            .map_err(|err| log::error!(target: "executor", "cannot add bounce \
                message to new transaction : {}", err)).ok()?;
        tr.total_fees_mut().grams.add(&fwd_mine_fees).ok()?;
        Some(TrBouncePhase::ok(storage, fwd_mine_fees, fwd_fees))
    }
}

/// Calculate new account state according to inbound message and current account state.
/// If account does not exist - it can be created with uninitialized state.
/// If account is uninitialized - it can be created with active state.
/// If account exists - it can be frozen.
/// Returns computed initial phase.
fn compute_new_state(acc: &mut Account, in_msg: &Message) -> Option<ComputeSkipReason> {
    log::debug!(target: "executor", "compute_account_state");
    match acc.state() {
        None => {
            log::error!(target: "executor", "account must exist");
            Some(ComputeSkipReason::BadState)
        }
        //Account exists, but can be in different states.
        Some(AccountState::AccountActive(_)) => {
            //account is active, just return it
            log::debug!(target: "executor", "account state: AccountActive");
            None
        }
        Some(AccountState::AccountUninit) => {
            log::debug!(target: "executor", "AccountUninit");
            if let Some(state_init) = in_msg.state_init() {
                // if msg is a constructor message then
                // borrow code and data from it and switch account state to 'active'.
                log::debug!(target: "executor", "external message for uninitialized: activated");
                match acc.try_activate(&state_init) {
                    Err(err) => {
                        log::debug!(target: "executor", "reason: {}", err);
                        Some(ComputeSkipReason::NoState)
                    }
                    Ok(_) => None
                }
            } else {
                log::debug!(target: "executor", "message for uninitialized: skip computing phase");
                Some(ComputeSkipReason::NoState)
            }
        }
        Some(AccountState::AccountFrozen(_)) => {
            log::debug!(target: "executor", "AccountFrozen");
            //account balance was credited and if it positive after that
            //and inbound message bear code and data then make some check and unfreeze account
            if !acc.get_balance().map(|balance| balance.grams.is_zero()).unwrap_or_default() {
                if let Some(state_init) = in_msg.state_init() {
                    log::debug!(target: "executor", "external message for frozen: activated");
                    return match acc.try_activate(&state_init) {
                        Err(err) => {
                            log::debug!(target: "executor", "reason: {}", err);
                            Some(ComputeSkipReason::NoState)
                        }
                        Ok(_) => None
                    }
                }
            }
            //skip computing phase, because account is frozen (bad state)
            log::debug!(target: "executor", "account is frozen (bad state): skip computing phase");
            Some(ComputeSkipReason::NoState)
        }
    }
}

fn create_account_state(acc: &mut Account, in_msg: &Message, bounce: bool, now: u32) -> bool {
    log::debug!(target: "executor", "create_account_state");
    //try to create account with constructor message
    if let Ok(new_acc) = Account::with_message(in_msg) {
        //account created from constructor message
        //but check that inbound message bear some value,
        //otherwise it will be frozen
        *acc = new_acc;
        if in_msg.get_value().is_some() {
            log::debug!(target: "executor", "new acc is created");
            acc.set_last_paid(now);
            //it's ok - active account will be created.
            //set apropriate flags in phase.
            //return activated account
            return true
        } else {
            log::debug!(target: "executor", "new acc is created and frozen");
            acc.try_freeze().ok();
        }
    //message has no code and data,
    //check bounce flag
    } else if bounce {
        log::debug!(target: "executor", "bounce message to non-existing account");
        //let skip computing phase, because account not exist and bounce flag is setted. 
        //Account will not be created, return AccountNone
    } else if let Some(balance) = in_msg.get_value() {
        //value-bearing message with no bounce: create uninitialized account
        log::debug!(target: "executor", "new uninitialized acc is created");
        let address = in_msg.dst().unwrap_or_default();
        *acc = Account::with_address_and_ballance(&address, balance);
        acc.set_last_paid(now);
    }
    false
}

fn outmsg_action_handler(
    phase: &mut TrActionPhase, 
    mut mode: u8, 
    msg: &mut Message,
    acc_balance: &mut CurrencyCollection,
    msg_balance: &mut CurrencyCollection,
    config: &BlockchainConfig,
    is_special: bool,
) -> std::result::Result<CurrencyCollection, i32> {
    // we cannot send all balance from account and from message simultaneously ?
    let invalid_flags = SENDMSG_REMAINING_MSG_BALANCE | SENDMSG_ALL_BALANCE;
    if  (mode & !SENDMSG_VALID_FLAGS) != 0 ||
        (mode & invalid_flags) == invalid_flags
    {
        log::error!(target: "executor", "outmsg mode has unsupported flags");
        return Err(RESULT_CODE_UNSUPPORTED);
    }
    let skip = if (mode & SENDMSG_IGNORE_ERROR) != 0 {
        None
    } else {
        Some(())
    };
    let (fwd_mine_fee, total_fwd_fees);
    let mut result_value; // to sub from acc_balance

    // TODO: check and rewrite src and dst adresses (see check_replace_src_addr and 
    // check_rewrite_dest_addr functions)

    let fwd_prices = config.get_fwd_prices(msg.is_masterchain());
    let compute_fwd_fee = if is_special {
        Grams::default()
    } else {
        msg.serialize()
        .map(|cell| fwd_prices.fwd_fee(&cell).1)
        .map_err(|err| {
            log::error!(target: "executor", "cannot serialize message in action phase : {}", err);
            RESULT_CODE_ACTIONLIST_INVALID
        })?
    };

    if let Some(int_header) = msg.int_header_mut() {
        result_value = int_header.value.clone();
        if int_header.ihr_disabled {
            int_header.ihr_fee = Default::default();
        } else {
            let compute_ihr_fee = fwd_prices.ihr_fee(&compute_fwd_fee);
            if int_header.ihr_fee < compute_ihr_fee {
                int_header.ihr_fee = compute_ihr_fee
            }
        }
        let fwd_fee = std::cmp::max(&int_header.fwd_fee, &compute_fwd_fee).clone();
        fwd_mine_fee = fwd_prices.mine_fee(&fwd_fee);
        total_fwd_fees = Grams::from(fwd_fee.0 + int_header.ihr_fee.0);

        let fwd_remain_fee = fwd_fee.0 - fwd_mine_fee.0;
        if (mode & SENDMSG_ALL_BALANCE) != 0 {
            //send all remaining account balance
            result_value = acc_balance.clone();
            int_header.value = acc_balance.clone();

            mode &= !SENDMSG_PAY_FEE_SEPARATELY;
        }
        if (mode & SENDMSG_REMAINING_MSG_BALANCE) != 0 {
            //send all remainig balance of inbound message
            result_value.add(msg_balance).ok();
            int_header.value.add(msg_balance).ok();
            *msg_balance = Default::default();
        }
        if (mode & SENDMSG_PAY_FEE_SEPARATELY) != 0 {
            //we must pay the fees, sum them with msg value
            result_value.grams.0 += total_fwd_fees.0;
        } else if int_header.value.grams.0 < total_fwd_fees.0 {
            //msg value is too small, reciever cannot pay the fees 
            log::warn!(
                target: "executor", 
                "msg balance {} is too small, cannot pay fwd+ihr fees: {}", 
                int_header.value.grams, total_fwd_fees
            );
            return Err(skip.map(|_| RESULT_CODE_NOT_ENOUGH_GRAMS).unwrap_or(0))
        } else {
            //reciever will pay the fees
            int_header.value.grams.0 -= total_fwd_fees.0;
        }

        //set evaluated fees and value back to msg
        int_header.fwd_fee = fwd_remain_fee.into();
    } else if msg.ext_out_header().is_some() {
        fwd_mine_fee = compute_fwd_fee.clone();
        total_fwd_fees = compute_fwd_fee.clone();
        result_value = CurrencyCollection::from_grams(compute_fwd_fee);
    } else {
        return Err(-1)
    }

    if acc_balance.grams.0 < result_value.grams.0 {
        log::warn!(
            target: "executor", 
            "account balance {} is too small, cannot send {}", acc_balance.grams, result_value.grams
        ); 
        return Err(skip.map(|_| RESULT_CODE_NOT_ENOUGH_GRAMS).unwrap_or(0))
    }
    match acc_balance.sub(&result_value) {
        Ok(false) | Err(_) => {
            log::warn!(
                target: "executor", 
                "account balance {} is too small, cannot send {}", acc_balance, result_value
            ); 
            return Err(skip.map(|_| RESULT_CODE_NOT_ENOUGH_EXTRA).unwrap_or(0))
        }
        _ => ()
    }

    // TODO: check if was reserved something
    if (mode & SENDMSG_DELETE_IF_EMPTY) != 0 {
        if acc_balance.grams.0 == 0 {
            phase.status_change = AccStatusChange::Deleted;
        }
    }

    // total fwd fees is sum of messages full fwd and ihr fees
    if total_fwd_fees.0 != 0 {
        phase.total_fwd_fees.get_or_insert(Grams::default()).0 += total_fwd_fees.0;
    }

    // total action fees is sum of messages fwd mine fees
    if fwd_mine_fee.0 != 0 {
        phase.total_action_fees.get_or_insert(Grams::default()).0 += fwd_mine_fee.0;
    }

    let msg_cell = msg.serialize().map_err(|err| {
        log::error!(target: "executor", "cannot serialize message in action phase : {}", err);
        RESULT_CODE_ACTIONLIST_INVALID
    })?;
    phase.tot_msg_size.append(&msg_cell);

    log::info!(target: "executor", "msg exports value {}", result_value.grams.0);
    Ok(result_value)
}

/// Reserves some grams from accout balance. 
/// Returns calculated reserved value. its calculation depends on mode.
/// Reduces balance by the amount of the reserved value.
fn reserve_action_handler(
    mode: u8, 
    val: &CurrencyCollection,
    acc_remaining_balance: &mut CurrencyCollection,
) -> std::result::Result<CurrencyCollection, i32> {
    if (mode & !RESERVE_VALID_MODES) != 0 {
        return Err(RESULT_CODE_UNSUPPORTED);
    }

    let mut reserved = val.clone();
    //TODO: need to compare extra currencies too.
    if reserved.grams.0 > acc_remaining_balance.grams.0 {
        //reserving more than remaining balance has,
        //this is error, but check the flag
        if (mode & RESERVE_IGNORE_ERROR) != 0 {
            //reserve all remaining balance
            reserved = acc_remaining_balance.clone();
        } else {
            return Err(RESULT_CODE_NOT_ENOUGH_GRAMS);
        }
    } else {
        // check the mode
        if (mode & RESERVE_ALL_BUT) != 0 {
            // need to reserve all but 'val' grams
            reserved = acc_remaining_balance.clone();
            reserved.sub(&val).or(Err(RESULT_CODE_INVALID_BALANCE))?;
        } 
    }

    acc_remaining_balance.sub(&reserved).or(Err(RESULT_CODE_INVALID_BALANCE))?;
    Ok(reserved)
}

fn setcode_action_handler(acc: &mut Account, code: Cell) -> Option<i32> {
    log::debug!(target: "executor", "OutAction::SetCode {}", code);
    match acc.set_code(code) {
        true => None,
        false => Some(RESULT_CODE_BAD_ACCOUNT_STATE)
    }
}

fn change_library_action_handler(acc: &mut Account, mode: u8, code: Option<Cell>, hash: Option<UInt256>) -> Option<i32> {
    let result = match (code, hash) {
        (Some(code), None) => {
            log::debug!(target: "executor", "OutAction::ChangeLibrary mode: {}, code: {}", mode, code);
            if mode == 0 {
                acc.delete_library(&code.repr_hash())
            } else {
                acc.set_library(code, (mode & 2) == 2)
            }
        }
        (None, Some(hash)) => {
            log::debug!(target: "executor", "OutAction::ChangeLibrary mode: {}, hash: {}", mode, hash.to_hex_string());
            if mode == 0 {
                acc.delete_library(&hash)
            } else {
                acc.set_library_flag(&hash, (mode & 2) == 2)
            }
        }
        _ => false
    };
    match result {
        true => None,
        false => Some(RESULT_CODE_BAD_ACCOUNT_STATE)
    }
}

fn init_gas(acc_balance: u128, msg_balance: u128, is_external: bool, is_special: bool, is_ordinary: bool, gas_info: &GasLimitsPrices) -> Gas {
    let gas_max = if is_special {
        gas_info.special_gas_limit
    } else {
        std::cmp::min(gas_info.gas_limit, gas_info.calc_gas(acc_balance))
    };
    let mut gas_credit = 0;
    let gas_limit = if !is_ordinary {
        gas_max
    } else {
        if is_external {
            gas_credit = std::cmp::min(gas_info.gas_credit, gas_max);
        }
        std::cmp::min(gas_max, gas_info.calc_gas(msg_balance))
    };
    log::debug!(
        target: "executor", 
        "gas before: gm: {}, gl: {}, gc: {}, price: {}", 
        gas_max, gas_limit, gas_credit, gas_info.get_real_gas_price()
    );
    Gas::new(gas_limit as i64, gas_credit as i64, gas_max as i64, gas_info.get_real_gas_price() as i64)
}

fn acc_sub_funds(acc: &mut Account, funds_to_sub: &CurrencyCollection) -> Option<()> {
    let acc_balance = acc.get_balance().map(|value| value.grams.0).unwrap_or_default();
    log::debug!(target: "executor", "acc_balance: {}, sub funds: {}", acc_balance, funds_to_sub.grams);
    acc.sub_funds(funds_to_sub).map_err(|err|
        log::error!(target: "executor", "cannot sub funds {:?} from account balance {:?} : {}",
            funds_to_sub, acc_balance, err)).ok().map(|_|())
}
