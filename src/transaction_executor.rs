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

use num_traits::ToPrimitive;
use std::sync::{atomic::{AtomicU64, Ordering}, Arc};
use ton_block::{
    Deserializable, GetRepresentationHash, Serializable,
    Account, AccountState, ConfigParams, GasLimitsPrices, GlobalCapabilities,
    AddSub, CommonMsgInfo, CurrencyCollection, Grams,
    Message, MsgAddressInt, MsgAddressIntOrNone,
    OutAction, OutActions, RESERVE_ALL_BUT, RESERVE_IGNORE_ERROR, RESERVE_VALID_MODES,
    SENDMSG_ALL_BALANCE, SENDMSG_IGNORE_ERROR, SENDMSG_PAY_FEE_SEPARATELY,
    SENDMSG_REMAINING_MSG_BALANCE, SENDMSG_VALID_FLAGS,
    AccStatusChange, ComputeSkipReason, Transaction,
    TrActionPhase, TrBouncePhase, TrComputePhase, TrComputePhaseVm, TrCreditPhase, TrStoragePhase,
};
use ton_types::{error, fail, Cell, Result, HashmapE, HashmapType, IBitstring, UInt256};
use ton_vm::{
    error::tvm_exception_code_and_value, executor::gas::gas_state::Gas,
    smart_contract_info::SmartContractInfo,
    stack::{Stack, StackItem},
};

const RESULT_CODE_ACTIONLIST_INVALID: i32 = 32;
const RESULT_CODE_TOO_MANY_ACTIONS:   i32 = 33;
const RESULT_CODE_UNKNOWN_ACTION:     i32 = 34;
const RESULT_CODE_NOT_ENOUGH_GRAMS:   i32 = 37;
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
            // info.set_remaining_balance(balance.grams.value().to_u128().unwrap_or_default(), balance.other.clone());
            *info.balance_remaining_grams_mut() = balance.grams.value().to_u128().unwrap_or_default();
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
            acc_sub_funds(acc, &fee)?;
            tr.total_fees_mut().add(&fee).ok()?;
            log::debug!(target: "executor", "AccStatusChange::Unchanged");
            acc.set_last_paid(tr.now());
            Some(TrStoragePhase::with_params(fee.grams, None, AccStatusChange::Unchanged))
        } else {
            fee.sub(&balance).ok()?;
            let balance = CurrencyCollection::from_grams(balance);
            acc_sub_funds(acc, &balance)?;
            acc.freeze_account();
            tr.total_fees_mut().add(&balance).ok()?;
            log::debug!(target: "executor", "AccStatusChange::Frozen");
            Some(TrStoragePhase::with_params(balance.grams, Some(fee), AccStatusChange::Frozen))
        }
    }

    /// Implementation of transaction's credit phase.
    /// Increases account balance by the amount that appears in the internal message header.
    /// If account does not exist - phase skipped.
    /// If message is not internal - phase skipped.
    fn credit_phase(&self, msg: &Message, acc: &mut Account) -> Option<TrCreditPhase> {
        log::debug!(target: "executor", "credit_phase");
        if acc == &Account::AccountNone {
            log::debug!(target: "executor", "Account::AccountNone");
            if let CommonMsgInfo::IntMsgInfo(ref header) = msg.header() {
                if !header.bounce {
                    return Some(TrCreditPhase::with_params(None, header.value.clone()))
                }
            }
            return None;
        }
        msg.get_value().and_then(|value| {
            acc.add_funds(value).ok()?;
            Some(TrCreditPhase::with_params(None, value.clone()))
        })
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
        config: &GasLimitsPrices,
        is_special: bool,
        debug: bool,
    ) -> Result<(TrComputePhase, Option<Cell>)> {
        let (msg_balance, is_external);
        let mut libs = vec![];
        let mut vm_phase = if let Some(ref msg) = msg {
            if let Some(state_init) = msg.state_init() {
                libs.push(state_init.libraries().inner());
            }
            msg_balance = match msg.get_value() {
                Some(value) => value.grams.value().to_u128()
                    .ok_or_else(|| ExecutorError::TrExecutorError("Failed to \
                        convert msg balance to u128".to_string()))?,
                None => 0
            };
            is_external = msg.is_inbound_external();
            match compute_new_state(acc, msg, smc_info.unix_time()) {
                TrComputePhase::Vm(vm_phase) => vm_phase,
                skip => return Ok((skip, None))
            }
        } else {
            msg_balance = 0;
            is_external = false;
            TrComputePhaseVm::default()
        };
        let acc_balance = match acc.get_balance() {
            Some(value) => value.grams.value().to_u128()
                .ok_or_else(|| ExecutorError::TrExecutorError("Failed to \
                    convert account balance to u128".to_string()))?,
            None => 0
        };
        log::debug!(target: "executor", "acc balance: {}", acc_balance);
        log::debug!(target: "executor", "msg balance: {}", msg_balance);
        //code must present but can be empty (i.g. for uninitialized account)
        let code = acc.get_code().unwrap_or_default();
        let data = acc.get_data().unwrap_or_default();
        libs.push(acc.libraries().inner());
        libs.push(state_libs);

        let is_ordinary = self.ordinary_transaction();
        let gas = init_gas(acc_balance, msg_balance, is_external, is_special, is_ordinary, config);
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
                vm_phase.exit_code = tvm_exception_code_and_value(&err).0;
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
            let gas_fees = if is_special { 0 } else { config.calc_gas_fee(used) };
            vm_phase.gas_fees = Grams(gas_fees.into());
        };

        log::debug!(
            target: "executor", 
            "gas after: gl: {}, gc: {}, gu: {}, fees:{}", 
            gas.get_gas_limit() as u64, credit, used, vm_phase.gas_fees.0
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
        actions_cell: Cell,
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
        let mut remaining_balance = acc.get_balance()?.clone();
        phase.tot_actions = 0;
        phase.spec_actions = 0;
        phase.msgs_created = 0;
        phase.result_code = 0;
        phase.result_arg = None;
        phase.valid = true;
        phase.success = true;

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
        phase.action_list_hash = actions.hash().ok()?;
        phase.tot_actions = actions.len() as i16;

        let my_addr = acc.get_addr()?.clone();
        for (i, action) in actions.iter_mut().enumerate() {
            let act = std::mem::replace(action, OutAction::None);
            //debug!(target: "executor", "OutAction: {}", serde_json::to_string_pretty(&act).unwrap_or_default());
            let err_code = match act {
                OutAction::SendMsg{ mode, mut out_msg } => {
                    out_msg.set_src(MsgAddressIntOrNone::Some(my_addr.clone()));
                    if self.ordinary_transaction() {
                        out_msg.set_at_and_lt(tr.now(), lt.fetch_add(1, Ordering::SeqCst));
                    } else {
                        out_msg.set_at_and_lt(tr.now(), lt.load(Ordering::SeqCst));
                    }
                    let result = outmsg_action_handler(
                        &mut phase, 
                        mode,
                        &mut out_msg,
                        &mut remaining_balance,
                        self.config(),
                        is_special
                    );
                    match result {
                        Ok(msg_balance) => {
                            msg_action_count += 1;
                            out_msgs.push(out_msg);
                            match total_spend_value.add(&msg_balance) {
                                Ok(_) => 0,
                                Err(_) => RESULT_CODE_INVALID_BALANCE
                            }
                        }
                        Err(code) => code
                    }
                }
                OutAction::ReserveCurrency{ mode, value } => {
                    match reserve_action_handler(mode, &value, &mut remaining_balance) {
                        Ok(reserved_value) => {
                            special_action_count += 1;
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
                            special_action_count += 1;
                            0
                        }
                        Some(code) => code
                    }
                }
                OutAction::ChangeLibrary{ mode, code, hash} => {
                    match change_library_action_handler(acc, mode, code, hash) {
                        None => {
                            special_action_count += 1;
                            0
                        }
                        Some(code) => code
                    }
                }
                OutAction::None => RESULT_CODE_UNKNOWN_ACTION
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
        new_balance.add(&total_reserved_value).map_err(|err|
            log::debug!(target: "executor", "failed to add account \
                balance with reserved value {}", err)).ok()?;
        //calc difference of new balance from old balance
        let mut balance_diff = acc.get_balance()?.clone();
        //must be succeded, because check is already done
        balance_diff.sub(&new_balance).ok()?;

        //TODO: substract difference from account balance
        if acc_sub_funds(acc, &balance_diff).is_none() {
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
        if let Some(ref fee) = phase.total_action_fees {
            tr.total_fees_mut().grams.add(fee).ok()?;
        }

        //for now we support only this change status
        phase.status_change = AccStatusChange::Unchanged;
        phase.spec_actions = special_action_count;
        phase.msgs_created = msg_action_count;
        phase.skipped_actions = skipped_action_count;
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

        let (storage, fwd_full_fees) = fwd_prices.calc_fwd_fee(&Cell::default());
        let fwd_mine_fees = fwd_prices.calc_mine_fee(fwd_full_fees);
        let fwd_fees = fwd_full_fees - fwd_mine_fees;
        let fwd_full_fees = Grams::from(fwd_full_fees);

        if !header.value.grams.sub(&gas_fee).ok()? {
            return Some(TrBouncePhase::no_funds(storage, fwd_full_fees))
        }
        acc_sub_funds(acc, &header.value)?;
        if !header.value.grams.sub(&fwd_full_fees).ok()? {
            return Some(TrBouncePhase::no_funds(storage, fwd_full_fees))
        }
        header.ihr_disabled = true;
        header.bounce = false;
        header.bounced = true;
        header.ihr_fee = Grams::zero();
        header.fwd_fee = fwd_fees.into();
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
        tr.total_fees_mut().grams.add(&fwd_mine_fees.into()).ok()?;
        Some(TrBouncePhase::ok(storage, fwd_mine_fees.into(), fwd_fees.into()))
    }
}

/// Calculate new account state according to inbound message and current account state.
/// If account does not exist - it can be created with uninitialized state.
/// If account is uninitialized - it can be created with active state.
/// If account exists - it can be frozen.
/// Returns computed initial phase.
fn compute_new_state(acc: &mut Account, in_msg: &Message, now: u32) -> TrComputePhase {
    let bounce = in_msg.int_header().map(|header| header.bounce).unwrap_or_default();
    log::debug!(target: "executor", "compute_account_state");    
    match acc.state() {
        None => {
            log::error!(target: "executor", "account must exist");
            create_account_state(acc, in_msg, bounce, now)
        }
        //Account exists, but can be in different states.
        Some(AccountState::AccountActive(_)) => {
            //account is active, just return it
            log::debug!(target: "executor", "account state: AccountActive");
            TrComputePhase::Vm(TrComputePhaseVm::default())
        }
        Some(AccountState::AccountUninit) => {
            log::debug!(target: "executor", "AccountUninit");
            if let Some(state_init) = in_msg.state_init() {
                // if msg is a constructor message then
                // borrow code and data from it and switch account state to 'active'.
                log::debug!(target: "executor", "external message for uninitialized: activated");
                acc.activate(state_init.clone());
                TrComputePhase::Vm(TrComputePhaseVm::activated(true))
            } else {
                log::debug!(target: "executor", "message for uninitialized: skip computing phase");
                TrComputePhase::default()
            }
        }
        Some(AccountState::AccountFrozen(_)) => {
            log::debug!(target: "executor", "AccountFrozen");
            //account balance was credited and if it positive after that
            //and inbound message bear code and data then make some check and unfreeze account
            if !acc.get_balance().map(|balance| balance.grams.is_zero()).unwrap_or_default() {
                if let Some(state_init) = in_msg.state_init() {
                    log::debug!(target: "executor", "external message for frozen: activated");
                    acc.activate(state_init.clone());
                    return TrComputePhase::Vm(TrComputePhaseVm::activated(true))
                }
            }
            //skip computing phase, because account is frozen (bad state)
            log::debug!(target: "executor", "account is frozen (bad state): skip computing phase");
            TrComputePhase::skipped(ComputeSkipReason::BadState)
        }
    }
}

fn create_account_state(acc: &mut Account, in_msg: &Message, bounce: bool, now: u32) -> TrComputePhase {
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
            return TrComputePhase::Vm(TrComputePhaseVm::activated(true))
        } else {
            log::debug!(target: "executor", "new acc is created and frozen");
            acc.freeze_account();
        }
    //message has no code and data,
    //check bounce flag
    } else if bounce {
        //let skip computing phase, because account not exist and bounce flag is setted. 
        //Account will not be created, return AccountNone
    } else if let Some(balance) =  in_msg.get_value().cloned() {
        //value-bearing message with no bounce: create uninitialized account
        log::debug!(target: "executor", "new uninitialized acc is created");
        let address = in_msg.dst().unwrap_or_default();
        *acc = Account::with_address_and_ballance(&address, &balance);
        acc.set_last_paid(now);
    }
    TrComputePhase::default()
}

fn outmsg_action_handler(
    phase: &mut TrActionPhase, 
    mut mode: u8, 
    msg: &mut Message,
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
    let value = msg.get_value().cloned().unwrap_or_default();
    let mut ihr_fee;
    let fwd_fee;
    let fwd_mine_fee;
    let mut result_grams = value.grams.clone();
    let mut new_msg_value = value.clone();

    // TODO: check and rewrite src and dst adresses (see check_replace_src_addr and 
    // check_rewrite_dest_addr functions)

    let fwd_prices = config.get_fwd_prices(msg);
    let compute_fwd_fee = if is_special {
        Grams::zero()
    } else {
        msg.serialize()
        .map(|cell| fwd_prices.calc_fwd_fee(&cell).1.into())
        .map_err(|err| {
            log::error!(target: "executor", "cannot serialize message in action phase : {}", err);
            RESULT_CODE_ACTIONLIST_INVALID
        })?
    };

    if let Some(int_header) = msg.int_header_mut() {
        ihr_fee = int_header.ihr_fee.clone();
        if !int_header.ihr_disabled {
            let compute_ihr_fee = fwd_prices.calc_ihr_fee(compute_fwd_fee.value().to_u128().unwrap_or(0)).into();
            if ihr_fee < compute_ihr_fee {
                ihr_fee = compute_ihr_fee;
            }
        }
        fwd_fee = std::cmp::max(&int_header.fwd_fee, &compute_fwd_fee).clone();
        fwd_mine_fee = Grams::from(fwd_prices.calc_mine_fee(fwd_fee.value().to_u128().unwrap_or(0)));

        let fwd_remain_fee = fwd_fee.0.clone() - fwd_mine_fee.0.clone();
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
                log::warn!(
                    target: "executor", 
                    "account balance is too small, cannot send {}", result_grams.0
                ); 
                Err(RESULT_CODE_NOT_ENOUGH_GRAMS) 
            };
        }

        //set evaluated fees and value back to msg
        int_header.fwd_fee = fwd_remain_fee.into();
        int_header.ihr_fee = ihr_fee.clone();
        int_header.value = new_msg_value;
    } else if msg.ext_out_header().is_some() {
        ihr_fee = Grams::default();
        fwd_fee = compute_fwd_fee.clone();
        fwd_mine_fee = compute_fwd_fee.clone();
        // TODO: check if account can pay fee
        result_grams = fwd_fee.clone();
    } else {
        return Err(-1)
    }

    // TODO: process SENDMSG_DELETE_IF_EMPTY flag

    // total fwd fees is sum of messages full fwd and ihr fees
    let mut total_fwd_fees = phase.total_fwd_fees.take().unwrap_or(Grams::default());
    total_fwd_fees.0 += fwd_fee.0 + ihr_fee.0;
    if !total_fwd_fees.is_zero() {
        phase.total_fwd_fees = Some(total_fwd_fees);
    }

    // total action fees is sum of messages fwd mine fees
    let mut total_action_fees = phase.total_action_fees.take().unwrap_or(Grams::default());
    total_action_fees.0 += fwd_mine_fee.0;
    if !total_action_fees.is_zero() {
        phase.total_action_fees = Some(total_action_fees);
    }

    let msg_cell = msg.serialize().map_err(|err| {
        log::error!(target: "executor", "cannot serialize message in action phase : {}", err);
        RESULT_CODE_ACTIONLIST_INVALID
    })?;
    phase.tot_msg_size.append(&msg_cell);

    // TODO: subtract all currencies from account balance
    let value = CurrencyCollection::from_grams(result_grams);
    remaining.sub(&value).or(Err(RESULT_CODE_INVALID_BALANCE))?;
    log::info!(target: "executor", "msg exports value {}", value.grams.0);
    Ok(value)
}

/// Reserves some grams from accout balance. 
/// Returns calculated reserved value. its calculation depends on mode.
/// Reduces balance by the amount of the reserved value.
fn reserve_action_handler(
    mode: u8, 
    val: &CurrencyCollection,
    remaining: &mut CurrencyCollection,
) -> std::result::Result<CurrencyCollection, i32> {
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
    acc.sub_funds(funds_to_sub).map_err(|err|
        log::error!(target: "executor", "cannot sub funds {:?} from account balance {:?} : {}",
        funds_to_sub, acc.get_balance(), err)).ok().map(|_|())
}
