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
    Account, AccountStatus, AccStatusChange, GasLimitsPrices, GlobalCapabilities,
    AddSub, CurrencyCollection, Grams,
    Message, MsgAddressInt,
    OutAction, OutActions, RESERVE_VALID_MODES, RESERVE_ALL_BUT, RESERVE_IGNORE_ERROR, RESERVE_PLUS_ORIG, RESERVE_REVERSE,
    SENDMSG_ALL_BALANCE, SENDMSG_IGNORE_ERROR, SENDMSG_PAY_FEE_SEPARATELY, SENDMSG_DELETE_IF_EMPTY,
    SENDMSG_REMAINING_MSG_BALANCE, SENDMSG_VALID_FLAGS,
    ComputeSkipReason, Transaction, StorageUsedShort,
    TrActionPhase, TrBouncePhase, TrComputePhase, TrStoragePhase, TrCreditPhase, TrComputePhaseVm, HashUpdate,
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



pub struct ExecuteParams {
    pub state_libs: HashmapE,
    pub block_unixtime: u32,
    pub block_lt: u64,
    pub last_tr_lt: Arc<AtomicU64>,
    pub seed_block: UInt256,
    pub debug: bool
}

impl ExecuteParams {
    pub fn default() -> Self {
        Self {
            state_libs: HashmapE::default(),
            block_unixtime: 0,
            block_lt: 0,
            last_tr_lt: Arc::new(AtomicU64::new(0)),
            seed_block: UInt256::default(),
            debug: false
        }
    }
}

pub trait TransactionExecutor {
    fn execute_with_params(
        &self,
        in_msg: Option<&Message>,
        account: &mut Account,
        params: ExecuteParams,
    ) -> Result<Transaction>;

    #[deprecated]
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
        let params = ExecuteParams {
            state_libs,
            block_unixtime,
            block_lt,
            last_tr_lt,
            seed_block: UInt256::default(),
            debug,
        };
        let mut account_root = account.serialize()?;
        let transaction = self.execute_with_libs_and_params(in_msg, &mut account_root, params)?;
        *account = Account::construct_from_cell(account_root)?;
        Ok(transaction)
    }
    fn execute_with_libs_and_params(
        &self,
        in_msg: Option<&Message>,
        account_root: &mut Cell,
        params: ExecuteParams,
    ) -> Result<Transaction> {
        let old_hash = account_root.repr_hash();
        let mut account = Account::construct_from_cell(account_root.clone())?;
        let mut transaction = self.execute_with_params(
            in_msg,
            &mut account,
            params,
        )?;
        if self.config().raw_config().has_capability(GlobalCapabilities::CapFastStorageStat) {
            account.update_storage_stat_fast()?;
        } else {
            account.update_storage_stat()?;
        }
        *account_root = account.serialize()?;
        let new_hash = account_root.repr_hash();
        transaction.write_state_update(&HashUpdate::with_hashes(old_hash, new_hash))?;
        Ok(transaction)
    }

    #[deprecated]
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
        let params = ExecuteParams {
            state_libs,
            block_unixtime,
            block_lt,
            last_tr_lt,
            seed_block: UInt256::default(),
            debug,
        };
        self.execute_with_libs_and_params(in_msg, account_root, params)
    }

    #[deprecated]
    fn execute(
        &self,
        in_msg: Option<&Message>,
        account_root: &mut Cell,
        block_unixtime: u32,
        block_lt: u64,
        last_tr_lt: Arc<AtomicU64>,
        debug: bool
    ) -> Result<Transaction> {
        let params = ExecuteParams {
            state_libs: HashmapE::default(),
            block_unixtime,
            block_lt,
            last_tr_lt,
            seed_block: UInt256::default(),
            debug,
        };
        self.execute_with_libs_and_params(in_msg, account_root, params)
    }

    fn build_contract_info(&self, acc_balance: &CurrencyCollection, acc_address: &MsgAddressInt, block_unixtime: u32, block_lt: u64, tr_lt: u64, seed_block: UInt256) -> SmartContractInfo {
        let mut info = SmartContractInfo::with_myself(acc_address.serialize().unwrap_or_default().into());
        *info.block_lt_mut() = block_lt;
        *info.trans_lt_mut() = tr_lt;
        *info.unix_time_mut() = block_unixtime;
        *info.balance_remaining_grams_mut() = acc_balance.grams.0;
        *info.balance_remaining_other_mut() = acc_balance.other_as_hashmap();
        if let Some(data) = self.config().raw_config().config_params.data() {
            info.set_config_params(data.clone());
        }
        info.calc_rand_seed(seed_block, &acc_address.address().get_bytestring(0));
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
        acc_balance: &mut CurrencyCollection,
        tr: &mut Transaction,
        is_masterchain: bool,
        is_special: bool
    ) -> Option<TrStoragePhase> {
        log::debug!(target: "executor", "storage_phase");
        if is_special {
            log::debug!(target: "executor", "Special account: AccStatusChange::Unchanged");
            return Some(TrStoragePhase::default())
        }
        if acc.is_none() {
            log::debug!(target: "executor", "Account::None");
            return Some(TrStoragePhase::default())
        }

        let mut fee = Grams::from(self.config().calc_storage_fee(
            acc.storage_info()?,
            is_masterchain,
            tr.now(),
        ));
        if let Some(due_payment) = acc.due_payment() {
            fee.add(&due_payment).ok()?;
        }

        if acc_balance.grams >= fee {
            log::debug!(target: "executor", "acc_balance: {}, storage fee: {}", acc_balance.grams, fee);
            acc_balance.grams.sub(&fee).ok()?;
            tr.add_fee_grams(&fee).ok()?;
            Some(TrStoragePhase::with_params(fee, None, AccStatusChange::Unchanged))
        } else {
            log::debug!(target: "executor", "acc_balance: {} is storage fee from total: {}", acc_balance.grams, fee);
            let storage_fees_collected = std::mem::replace(&mut acc_balance.grams, Grams::default());
            tr.add_fee_grams(&storage_fees_collected).ok()?;
            fee.sub(&storage_fees_collected).ok()?;
            if fee.0 > self.config().get_gas_config(is_masterchain).freeze_due_limit.into() {
                log::debug!(target: "executor", "freeze account");
                acc.try_freeze().unwrap();
                Some(TrStoragePhase::with_params(storage_fees_collected, Some(fee), AccStatusChange::Frozen))
            } else {
                Some(TrStoragePhase::with_params(storage_fees_collected, Some(fee), AccStatusChange::Unchanged))
            }
        }
    }

    /// Implementation of transaction's credit phase.
    /// Increases account balance by the amount that appears in the internal message header.
    /// If account does not exist - phase skipped.
    /// If message is not internal - phase skipped.
    fn credit_phase(
        &self,
        msg_balance: &CurrencyCollection,
        acc_balance: &mut CurrencyCollection,
    ) -> Option<TrCreditPhase> {
        log::debug!(target: "executor", "credit_phase: add funds {} to {}", msg_balance.grams, acc_balance.grams);
        acc_balance.add(msg_balance).ok()?;
        Some(TrCreditPhase::with_params(None, msg_balance.clone()))
        //TODO: Is it need to credit with ihr_fee value in internal messages?
    }

    /// Implementation of transaction's computing phase.
    /// Evaluates new accout state and invokes TVM if account has contract code.
    fn compute_phase(
        &self,
        msg: Option<&Message>,
        acc: &mut Account,
        acc_balance: &mut CurrencyCollection,
        msg_balance: &CurrencyCollection,
        state_libs: HashmapE, // masterchain libraries
        smc_info: SmartContractInfo, 
        stack: Stack,
        is_masterchain: bool,
        is_special: bool,
        debug: bool,
    ) -> Result<(TrComputePhase, Option<Cell>, Option<Cell>)> {
        let mut vm_phase = TrComputePhaseVm::default();
        let is_external = if let Some(msg) = msg {
            if let Some(header) = msg.int_header() {
                log::debug!(target: "executor", "msg internal, bounce: {}", header.bounce);
                if acc.is_none() {
                    if let Some(new_acc) = Account::from_message(msg) {
                        *acc = new_acc;
                        acc.set_last_paid(smc_info.unix_time())
                    }
                }
                false
            } else {
                log::debug!(target: "executor", "msg external");
                true
            }
        } else {
            debug_assert!(!acc.is_none());
            false
        };
        log::debug!(target: "executor", "acc balance: {}", acc_balance.grams);
        log::debug!(target: "executor", "msg balance: {}", msg_balance.grams);
        let is_ordinary = self.ordinary_transaction();
        let gas_config = self.config().get_gas_config(is_masterchain);
        let gas = init_gas(acc_balance.grams.0, msg_balance.grams.0, is_external, is_special, is_ordinary, gas_config);
        if gas.get_gas_limit() == 0 && gas.get_gas_credit() == 0 {
            log::debug!(target: "executor", "skip computing phase no gas");
            return Ok((TrComputePhase::skipped(ComputeSkipReason::NoGas), None, None))
        }

        let mut libs = vec![];
        if let Some(ref msg) = msg {
            if let Some(state_init) = msg.state_init() {
                libs.push(state_init.libraries().inner());
            }
            if let Some(reason) = compute_new_state(acc, msg) {
                return Ok((TrComputePhase::skipped(reason), None, None))
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
            .set_contract_info(smc_info)
            .set_stack(stack)
            .set_data(data)
            .set_libraries(libs)
            .set_gas(gas)
            .set_debug(debug)
            .create();
        
        //TODO: set vm_init_state_hash

        let result = vm.execute();
        log::trace!(target: "executor", "execute result: {:?}", result);
        let mut raw_exit_arg = None;
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
                raw_exit_arg = Some(exception.value);
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
                fail!(ExecutorError::NoAcceptError(vm_phase.exit_code, raw_exit_arg))
            }
            vm_phase.gas_fees = Grams::zero();
        } else { // credit == 0 means contract accepted
            let gas_fees = if is_special { 0 } else { gas_config.calc_gas_fee(used) };
            vm_phase.gas_fees = gas_fees.into();
        };

        log::debug!(
            target: "executor", 
            "gas after: gl: {}, gc: {}, gu: {}, fees: {}", 
            gas.get_gas_limit() as u64, credit, used, vm_phase.gas_fees
        );
    
        //set mode
        vm_phase.mode = 0;
        vm_phase.vm_steps = vm.steps();
        //TODO: vm_final_state_hash
        log::debug!(target: "executor", "acc_balance: {}, gas fees: {}", acc_balance.grams, vm_phase.gas_fees);
        if !acc_balance.grams.sub(&vm_phase.gas_fees)? {
            log::debug!(target: "executor", "can't sub funds: {} from acc_balance: {}", vm_phase.gas_fees, acc_balance.grams);
        }

        let new_data = if let StackItem::Cell(cell) = vm.get_committed_state().get_root() {
            Some(cell)
        } else {
            log::debug!(target: "executor", "invalid contract, it must be cell in c4 register");
            vm_phase.success = false;
            None
        };

        let out_actions = if let StackItem::Cell(root_cell) = vm.get_committed_state().get_actions() {
            Some(root_cell)
        } else {
            log::debug!(target: "executor", "invalid contract, it must be cell in c5 register");
            vm_phase.success = false;
            None
        };

        Ok((TrComputePhase::Vm(vm_phase), out_actions, new_data))
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
        original_acc_balance: &CurrencyCollection,
        acc_balance: &mut CurrencyCollection,
        msg_remaining_balance: &mut CurrencyCollection,
        actions_cell: Cell,
        new_data: Option<Cell>,
        is_special: bool,
    ) -> Option<(TrActionPhase, Vec<Message>)> {
        let mut acc_copy = acc.clone();
        let mut acc_remaining_balance = acc_balance.clone();
        let mut phase = TrActionPhase::default();
        let mut total_reserved_value = CurrencyCollection::default();
        let mut out_msgs = vec![];
        let mut actions = match OutActions::construct_from_cell(actions_cell) {
            Err(err) => {
                log::debug!(
                    target: "executor", 
                    "cannot parse action list: format is invalid, err: {}", 
                    err
                );
                phase.result_code = RESULT_CODE_ACTIONLIST_INVALID;
                return Some((phase, out_msgs))
            }
            Ok(actions) => actions
        };

        if actions.len() > MAX_ACTIONS {
            log::debug!(target: "executor", "too many actions: {}", actions.len());
            phase.result_code = RESULT_CODE_TOO_MANY_ACTIONS;
            return Some((phase, out_msgs))
        }
        phase.action_list_hash = actions.hash().ok()?;
        phase.tot_actions = actions.len() as i16;

        let my_addr = acc_copy.get_addr()?.clone();
        for (i, action) in actions.iter_mut().enumerate() {
            let err_code = match std::mem::replace(action, OutAction::None) {
                OutAction::SendMsg{ mode, mut out_msg } => {
                    out_msg.set_src_address(my_addr.clone());
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
                    match reserve_action_handler(mode, &value, &original_acc_balance, &mut acc_remaining_balance) {
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
                    match setcode_action_handler(&mut acc_copy, code) {
                        None => {
                            phase.spec_actions += 1;
                            0
                        }
                        Some(code) => code
                    }
                }
                OutAction::ChangeLibrary{ mode, code, hash} => {
                    match change_library_action_handler(&mut acc_copy, mode, code, hash) {
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
                return Some((phase, vec![]));
            }
        }

        //calc new account balance
        if let Err(err) = acc_remaining_balance.add(&total_reserved_value) {
            log::debug!(target: "executor", "failed to add account balance with reserved value {}", err);
            return None
        }

        if let Some(fee) = phase.total_action_fees.as_ref() {
            log::debug!(target: "executor", "action fees: {}", fee);
            tr.add_fee_grams(fee).ok()?;
        }

        phase.valid = true;
        phase.success = true;
        *acc_balance = acc_remaining_balance;
        *acc = acc_copy;
        if let Some(new_data) = new_data {
            acc.set_data(new_data);
        }
        Some((phase, out_msgs))
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
        tr: &mut Transaction,
        gas_fee: Grams
    ) -> Option<(TrBouncePhase, Option<Message>)> {
        let header = msg.int_header()?;
        if !header.bounce {
            return None
        }
        // create bounced message and swap src and dst addresses
        let mut header = header.clone();
        let msg_src = header.src_ref()?.clone();
        let msg_dst = std::mem::replace(&mut header.dst, msg_src);
        header.set_src(msg_dst);

        // gas for compute from message
        if !header.value.grams.sub(&gas_fee).ok()? {
            log::debug!(
                target: "executor",
                "bounce phase - not enough grams {} to get gas fee {}",
                header.value.grams,
                gas_fee,
            );
            return Some((TrBouncePhase::Negfunds, None))
        }

        // calculated storage for bounced message is empty
        let storage = StorageUsedShort::default();
        let fwd_prices = self.config().get_fwd_prices(msg.is_masterchain());
        let fwd_full_fees = fwd_prices.fwd_fee(&Cell::default());
        let fwd_mine_fees = fwd_prices.mine_fee(&fwd_full_fees);
        let fwd_fees = Grams::from(fwd_full_fees.0 - fwd_mine_fees.0);

        if header.value.grams.0 < fwd_full_fees.0 {
            log::debug!(
                target: "executor", "bounce phase - not enough grams {} to get fwd fee {}",
                header.value.grams, fwd_full_fees
            );
            return Some((TrBouncePhase::no_funds(storage, fwd_full_fees), None))
        }

        // create header for new bounced message and swap src and dst addresses
        log::debug!(target: "executor", "get fee {} from bounce msg {}", fwd_full_fees, header.value.grams);
        header.value.grams.sub(&fwd_full_fees).ok()?;
        header.ihr_disabled = true;
        header.bounce = false;
        header.bounced = true;
        header.ihr_fee = Grams::zero();
        header.fwd_fee = fwd_fees.clone();

        let mut bounce_msg = Message::with_int_header(header);
        if self.config().has_capability(GlobalCapabilities::CapBounceMsgBody) {
            let mut builder = (-1i32).write_to_new_cell().ok()?;
            if let Some(mut body) = msg.body() {
                body.shrink_data(0..256);
                builder.append_bytestring(&body).ok()?;
            }
            bounce_msg.set_body(builder.into_cell().ok()?.into());
        }

        log::debug!(target: "executor", "bounce fees: {} bounce value: {}", fwd_mine_fees, bounce_msg.get_value().unwrap().grams);
        tr.add_fee_grams(&fwd_mine_fees).ok()?;
        Some((TrBouncePhase::ok(storage, fwd_mine_fees, fwd_fees), Some(bounce_msg)))
    }

    fn add_messages(&self, tr: &mut Transaction, out_msgs: Vec<Message>, lt: Arc<AtomicU64>) -> Result<u64> {
        let mut lt = lt.fetch_add(1 + out_msgs.len() as u64, Ordering::Relaxed);
        lt += 1;
        for mut msg in out_msgs {
            msg.set_at_and_lt(tr.now(), lt);
            tr.add_out_message(&msg)?;
            lt += 1;
        }
        Ok(lt)
    }
}

/// Calculate new account state according to inbound message and current account state.
/// If account does not exist - it can be created with uninitialized state.
/// If account is uninitialized - it can be created with active state.
/// If account exists - it can be frozen.
/// Returns computed initial phase.
fn compute_new_state(acc: &mut Account, in_msg: &Message) -> Option<ComputeSkipReason> {
    log::debug!(target: "executor", "compute_account_state");
    match acc.status() {
        AccountStatus::AccStateNonexist => {
            log::error!(target: "executor", "account must exist");
            Some(ComputeSkipReason::BadState)
        }
        //Account exists, but can be in different states.
        AccountStatus::AccStateActive => {
            //account is active, just return it
            log::debug!(target: "executor", "account state: AccountActive");
            None
        }
        AccountStatus::AccStateUninit => {
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
        AccountStatus::AccStateFrozen => {
            log::debug!(target: "executor", "AccountFrozen");
            //account balance was credited and if it positive after that
            //and inbound message bear code and data then make some check and unfreeze account
            if !acc.balance().map(|balance| balance.grams.is_zero()).unwrap_or_default() {
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
        .map(|cell| fwd_prices.fwd_fee(&cell))
        .map_err(|err| {
            log::error!(target: "executor", "cannot serialize message in action phase : {}", err);
            RESULT_CODE_ACTIONLIST_INVALID
        })?
    };

    if let Some(int_header) = msg.int_header_mut() {
        int_header.bounced = false;
        result_value = int_header.value.clone();
        int_header.ihr_disabled = true; // ihr is disabled because it does not work
        if int_header.ihr_disabled {
            int_header.ihr_fee = Grams::default();
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
            *msg_balance = CurrencyCollection::default();
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

    log::debug!(target: "executor", "sub funds {} from {}", result_value, acc_balance.grams);
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

    log::info!(target: "executor", "msg with flags: {} exports value {}", mode, result_value.grams.0);
    Ok(result_value)
}

/// Reserves some grams from accout balance. 
/// Returns calculated reserved value. its calculation depends on mode.
/// Reduces balance by the amount of the reserved value.
fn reserve_action_handler(
    mode: u8, 
    val: &CurrencyCollection,
    original_acc_balance: &CurrencyCollection,
    acc_remaining_balance: &mut CurrencyCollection,
) -> std::result::Result<CurrencyCollection, i32> {
    if mode & !RESERVE_VALID_MODES != 0 {
        return Err(RESULT_CODE_ACTIONLIST_INVALID);
    }

    let mut reserved;
    if mode & RESERVE_PLUS_ORIG != 0 {
        // Append all currencies
        if mode & RESERVE_REVERSE != 0 {
            reserved = original_acc_balance.clone();
            let result = reserved.sub(&val);
            match result {
                Err(_) => return Err(RESULT_CODE_INVALID_BALANCE),
                Ok(false) => return Err(RESULT_CODE_NOT_ENOUGH_GRAMS),
                Ok(true) => ()
            }
        } else {
            reserved = val.clone();
            reserved.add(&original_acc_balance).or(Err(RESULT_CODE_INVALID_BALANCE))?;
        }
    } else {
        if mode & RESERVE_REVERSE != 0 { // flag 8 without flag 4 unacceptable
            return Err(RESULT_CODE_ACTIONLIST_INVALID);
        }
        reserved = val.clone();
    }
    if mode & RESERVE_IGNORE_ERROR != 0 {
        // Only grams
        reserved.grams.0 = std::cmp::min(reserved.grams.0, acc_remaining_balance.grams.0);
    }

    let mut remaining = acc_remaining_balance.clone();
    let result = remaining.sub(&reserved);
    match result {
        Err(_) => return Err(RESULT_CODE_INVALID_BALANCE),
        Ok(false) => return Err(RESULT_CODE_NOT_ENOUGH_GRAMS),
        Ok(true) => ()
    }
    std::mem::swap(&mut remaining, acc_remaining_balance);

    if mode & RESERVE_ALL_BUT != 0 {
        // swap all currencies
        std::mem::swap(&mut reserved, acc_remaining_balance);
    }

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
            if mode == 0 { // TODO: Wrong codes. Look ton_block/out_actions::SET_LIB_CODE_REMOVE
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
