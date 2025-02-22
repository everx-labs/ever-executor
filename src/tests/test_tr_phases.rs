/*
* Copyright (C) 2019-2023 EverX. All Rights Reserved.
*
* Licensed under the SOFTWARE EVALUATION License (the "License"); you may not use
* this file except in compliance with the License.
*
* Unless required by applicable law or agreed to in writing, software
* distributed under the License is distributed on an "AS IS" BASIS,
* WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
* See the License for the specific EVERX DEV software governing permissions and
* limitations under the License.
*/
#![allow(clippy::field_reassign_with_default)]

use super::*;
use crate::{
    blockchain_config::{BlockchainConfig, CalcMsgFwdFees},
    error::ExecutorError,
    OrdinaryTransactionExecutor,
};

mod common;
use common::*;
use pretty_assertions::assert_eq;
use std::sync::{atomic::AtomicU64, Arc};
use ever_block::{
    accounts::{Account, AccountStatus},
    generate_test_account_by_init_code_hash,
    messages::{
        ExtOutMessageHeader, ExternalInboundMessageHeader, InternalMessageHeader, Message,
        MsgAddressInt,
    },
    out_actions::{
        OutAction, OutActions, RESERVE_ALL_BUT, RESERVE_EXACTLY, RESERVE_IGNORE_ERROR,
        SENDMSG_ALL_BALANCE, SENDMSG_IGNORE_ERROR, SENDMSG_ORDINARY, SENDMSG_PAY_FEE_SEPARATELY,
    },
    transactions::{
        AccStatusChange, ComputeSkipReason, TrActionPhase, TrComputePhase, TrComputePhaseVm,
        Transaction,
    },
    types::Grams,
    types::VarUInteger32,
    CurrencyCollection, GetRepresentationHash, MsgAddressExt, Serializable, StateInit, 
    StorageUsedShort, UnixTime32,
    AccountId, BuilderData, SliceData, HashmapType,
};
use ever_assembler::compile_code;
use ever_vm::{int, executor::gas::gas_state::Gas, stack::integer::IntegerData, stack::StackItem};

fn create_ext_msg(dest: AccountId) -> Message {
    let body_slice = SliceData::default();
    let hdr = ExternalInboundMessageHeader::new(
        Default::default(),
        MsgAddressInt::with_standart(None, -1, dest).unwrap()
    );
    Message::with_ext_in_header_and_body(hdr, body_slice)
}

const INTERNAL_FWD_FEE: u64 = 5;

fn create_int_msg(src: AccountId, dest: AccountId, value: u64, bounce: bool, lt: u64, fwd_fee: impl Into<Grams>) -> Message {
    let mut hdr = InternalMessageHeader::with_addresses(
        MsgAddressInt::with_standart(None, -1, src).unwrap(),
        MsgAddressInt::with_standart(None, -1, dest).unwrap(),
        CurrencyCollection::with_grams(value),
    );
    hdr.bounce = bounce;
    hdr.ihr_disabled = true;
    hdr.fwd_fee = fwd_fee.into();
    hdr.created_lt = lt;
    hdr.created_at = UnixTime32::default();
    let mut msg = Message::with_int_header(hdr);
    msg.set_body(SliceData::default());
    msg
}

fn create_ext_out_msg(src_addr: AccountId) -> Message {
    let mut hdr = ExtOutMessageHeader::default();
    hdr.set_src(MsgAddressInt::with_standart(None, -1, src_addr).unwrap());
    hdr.created_lt = 1;
    hdr.created_at = 0x12345678.into();
    let mut msg = Message::with_ext_out_header(hdr);
    msg.set_body(SliceData::default());
    msg
}

fn create_state_init() -> StateInit {
    let mut init = StateInit::default();
    let code = compile_code("PUSHINT 1 PUSHINT 1 ACCEPT").unwrap().into_cell();
    let data = SliceData::new(vec![0x22; 32]).into_cell();
    init.code = Some(code);
    init.data = Some(data);
    init
}

fn ordinary_compute_phase(msg: &Message, acc: &mut Account) -> Result<TrComputePhase> {
    let msg_balance = msg.get_value().cloned().unwrap_or_default();
    let mut acc_balance = acc.get_balance().cloned().unwrap_or_else(|| msg_balance.clone());
    let acc_address = msg.dst_ref().unwrap();

    let config = BLOCKCHAIN_CONFIG.to_owned();
    let config_params = config.raw_config().config_params.data().cloned();
    let info = SmartContractInfo {
        capabilities: config.raw_config().capabilities(),
        myself: SliceData::load_builder(acc_address.write_to_new_cell().unwrap_or_default()).unwrap(),
        balance: acc_balance.clone(),
        config_params,
        ..Default::default()
    };
    let executor = OrdinaryTransactionExecutor::new(config);
    let stack = executor.build_stack(Some(msg), acc).unwrap();

    let (phase, _actions, _new_data) = executor.compute_phase(
        Some(msg),
        acc,
        &mut acc_balance,
        &msg_balance,
        info,
        stack,
        0,
        msg.is_masterchain(),
        false,
        &ExecuteParams::default(),
    )?;
    acc.set_balance(acc_balance);
    Ok(phase)
}

#[test]
fn test_computing_phase_extmsg_to_acc_notexist_nogas() {
    let mut msg = create_ext_msg(AccountId::from([0x11; 32]));
    let mut acc = Account::default();
    let phase = ordinary_compute_phase(&msg, &mut acc).unwrap();
    assert_eq!(phase, TrComputePhase::skipped(ComputeSkipReason::NoGas));

    msg.set_state_init(create_state_init());
    let mut acc = Account::default();
    let phase = ordinary_compute_phase(&msg, &mut acc).unwrap();
    assert_eq!(phase, TrComputePhase::skipped(ComputeSkipReason::NoGas));
}

#[test]
fn test_computing_phase_acc_notexist_intmsg_state() {
    let msg = create_int_msg(
        AccountId::from([0x11; 32]),
        AccountId::from([0x22; 32]),
        5_000_000,
        false,
        5,
        INTERNAL_FWD_FEE,
    );

    let mut acc = Account::default();
    assert_eq!(acc.status(), AccountStatus::AccStateNonexist);
    let phase = ordinary_compute_phase(&msg, &mut acc).unwrap();
    assert_eq!(phase, TrComputePhase::skipped(ComputeSkipReason::NoState));
    assert_eq!(acc.status(), AccountStatus::AccStateUninit);
}

#[test]
fn test_computing_phase_acc_uninit_extmsg_nostate() {
    let msg = create_ext_msg(AccountId::from([0x11; 32]));

    //create uninitialized account
    let mut acc = Account::with_address_and_ballance(
        &MsgAddressInt::with_standart(None, -1, AccountId::from([0x11; 32])).unwrap(),
        &CurrencyCollection::with_grams(1000),
    );
    let phase = ordinary_compute_phase(&msg, &mut acc).unwrap();
    assert_eq!(phase, TrComputePhase::skipped(ComputeSkipReason::NoGas));
}

#[test]
fn test_computing_phase_acc_uninit_extmsg_with_state() {
    let state_init = create_state_init();
    let hash = state_init.hash().unwrap();
    let addr = AccountId::from(*hash.as_slice());
    let mut msg = create_ext_msg(addr.clone());
    msg.set_state_init(state_init);

    //create uninitialized account
    let balance = 5000000;
    let mut acc = Account::with_address_and_ballance(
        &MsgAddressInt::with_standart(None, -1, addr).unwrap(),
        &CurrencyCollection::with_grams(balance),
    );
    let phase = ordinary_compute_phase(&msg, &mut acc).unwrap();

    let gas_config = BLOCKCHAIN_CONFIG.get_gas_config(true);
    let mut vm_phase = TrComputePhaseVm::default();
    vm_phase.success = true;
    // vm_phase.msg_state_used = true;
    // vm_phase.account_activated = true;
    vm_phase.exit_code = 0;
    let used = 67u32;
    vm_phase.gas_used = used.into();
    vm_phase.gas_limit = 0.into();
    vm_phase.gas_credit = Some(500.into());
    vm_phase.gas_fees = gas_config.flat_gas_price.into();
    vm_phase.vm_steps = 4;
    assert_eq!(phase, TrComputePhase::Vm(vm_phase));
}

#[test]
fn test_computing_phase_acc_uninit_intmsg_with_nostate() {
    let mut acc = Account::with_address_and_ballance(
        &MsgAddressInt::with_standart(None, -1, AccountId::from([0x22; 32])).unwrap(),
        &CurrencyCollection::with_grams(5_000_000),
    );
    let msg = create_int_msg(
        AccountId::from([0x11; 32]),
        AccountId::from([0x22; 32]),
        1_000_000, // it is enough grams by config to buy gas for uninit account but message has no state
        false,
        5,
        INTERNAL_FWD_FEE,
    );
    let phase = ordinary_compute_phase(&msg, &mut acc.clone()).unwrap();
    assert_eq!(phase, TrComputePhase::skipped(ComputeSkipReason::NoState));

    let msg = create_int_msg(
        AccountId::from([0x11; 32]),
        AccountId::from([0x22; 32]),
        500, // it is not enough by config to buy gas for uninit account
        false,
        5,
        INTERNAL_FWD_FEE,
    );

    let phase = ordinary_compute_phase(&msg, &mut acc).unwrap();
    assert_eq!(phase, TrComputePhase::skipped(ComputeSkipReason::NoGas));
}

#[test]
fn test_computing_phase_acc_active_extmsg() {
    
    //external inbound msg for account
    let msg = create_ext_msg(AccountId::from([0x11; 32]));

    //msg just for creating account with active state
    let balance = 5000000;
    let mut ctor_msg = create_int_msg(
        AccountId::from([0x11; 32]), 
        AccountId::from([0x11; 32]),
        balance,
        false,
        5,
        INTERNAL_FWD_FEE,
    );
    let mut init = StateInit::default();
    let code = compile_code("PUSHINT 1 PUSHINT 2 ADD ACCEPT").unwrap().into_cell();
    let data = SliceData::new(vec![0x22; 32]).into_cell();
    init.code = Some(code);
    init.data = Some(data);
    ctor_msg.set_state_init(init);

    let gas_config = BLOCKCHAIN_CONFIG.get_gas_config(true);
    let mut acc = account_from_message(&ctor_msg, ctor_msg.value().unwrap(), false, false, false).unwrap();
    let phase = ordinary_compute_phase(&msg, &mut acc).unwrap();
    let mut vm_phase = TrComputePhaseVm::default();
    vm_phase.success = true;
    vm_phase.msg_state_used = false;
    vm_phase.account_activated = false;
    vm_phase.exit_code = 0;
    let used = 85u32;
    vm_phase.gas_used = used.into();
    vm_phase.gas_limit = 0u32.into();
    vm_phase.gas_credit = Some(500u16.into());
    vm_phase.gas_fees = gas_config.flat_gas_price.into();
    vm_phase.vm_steps = 5;
    assert_eq!(phase, TrComputePhase::Vm(vm_phase));
}

fn create_account(balance: u64, address: &[u8; 32], code: SliceData, data: SliceData) -> Account {    
    //msg just for creating account with active state
    let mut ctor_msg = create_int_msg(
        AccountId::from([0x11; 32]), 
        AccountId::from(*address),
        balance,
        false,
        5,
        INTERNAL_FWD_FEE,
    );

    let mut init = StateInit::default();
    init.code = Some(code.into_cell());
    init.data = Some(data.into_cell());
    ctor_msg.set_state_init(init);

    account_from_message(&ctor_msg, ctor_msg.value().unwrap(), false, false, false).unwrap()
}

#[test]
fn test_computing_phase_activeacc_gas_not_accepted() {
    let balance = 100000000;
    let address = [0x33; 32];
    let mut acc = create_account(
        balance, 
        &address, 
        compile_code("PUSHINT 1 PUSHINT 2 THROW 100 ADD ACCEPT").unwrap(),
        SliceData::new(vec![0x22; 32]),
    );

    //external inbound msg for account
    let msg = create_ext_msg(AccountId::from(address));

    let result = ordinary_compute_phase(&msg, &mut acc);
    println!("{:?}", result);
    let e = result.expect_err("Must generate ExecutorError::NoAcceptError()");
    assert_eq!(e.downcast_ref(), Some(&ExecutorError::NoAcceptError(100, Some(int!(0)))));
}

#[test]
fn test_computing_phase_activeacc_gas_consumed_after_accept() {
    let balance = 10000000;
    let address = [0x33; 32];
    let mut acc = create_account(
        balance, 
        &address, 
        compile_code("PUSHINT 1 PUSHINT 2 ADD ACCEPT PUSHINT 3 THROW 100").unwrap(),
        SliceData::new(vec![0x22; 32]),
    );

    //external inbound msg for account
    let msg = create_ext_msg(AccountId::from(address));
    let phase = ordinary_compute_phase(&msg, &mut acc).unwrap();

    let gas_config = BLOCKCHAIN_CONFIG.get_gas_config(true);
    let mut vm_phase = TrComputePhaseVm::default();
    vm_phase.success = false;
    vm_phase.msg_state_used = false;
    vm_phase.account_activated = false;
    vm_phase.exit_code = 100;
    vm_phase.gas_used = 182u32.into();
    vm_phase.gas_limit = 0u32.into();
    vm_phase.gas_credit = Some(1000u16.into());
    let gas_fees = 182u64 * gas_config.get_real_gas_price();
    vm_phase.gas_fees = gas_fees.into();
    vm_phase.vm_steps = 6;
    assert_eq!(phase, TrComputePhase::Vm(vm_phase));
    assert_eq!(acc.balance().unwrap().grams, Grams::from(balance - gas_fees));
}

fn call_action_phase(
    start_acc_balance: u64,
    out_msg_value: u64,
    must_succeded: bool,
    no_funds: bool,
    fwd_fees: u64,
    action_fees: impl Into<Grams>,
    res: i32,
    res_arg: Option<i32>
) {
    let msg = create_ext_msg(AccountId::from([0x11; 32]));

    //msg just for creating account with active state
    let mut ctor_msg = create_int_msg(
        AccountId::from([0x12; 32]), 
        AccountId::from([0x11; 32]),
        start_acc_balance,
        false,
        5,
        INTERNAL_FWD_FEE,
    );
    let mut init = StateInit::default();
    let code = SliceData::new_empty().into_cell();
    let data = SliceData::new(vec![0x22; 32]).into_cell();
    init.code = Some(code);
    init.data = Some(data);
    ctor_msg.set_state_init(init);

    let mut acc = account_from_message(&ctor_msg, ctor_msg.value().unwrap(), false, false, false).unwrap();
    let mut tr = Transaction::with_account_and_message(&acc, &msg, 1).unwrap();

    let mut actions = OutActions::default();
    let msg = create_ext_out_msg(AccountId::from([0x11; 32]));
    let mut storage = StorageUsedShort::calculate_for_struct(&msg).unwrap();
    actions.push_back(OutAction::new_send(SENDMSG_ORDINARY, msg));

    let config = BLOCKCHAIN_CONFIG.to_owned();
    let executor = OrdinaryTransactionExecutor::new(config.clone());
    let fwd_prices = config.get_fwd_prices(ctor_msg.is_masterchain());
    let msg_fwd_fees = Grams::from(fwd_prices.lump_price) - fwd_prices.mine_fee_checked(&fwd_prices.lump_price.into()).unwrap();
    let mut msg = create_int_msg(
        AccountId::from([0x11; 32]),
        AccountId::from([0x22; 32]),
        out_msg_value,
        true,
        5,
        //INTERNAL_FWD_FEE
        msg_fwd_fees,
    );
    let mut msg_remaining_balance = msg.get_value().cloned().unwrap_or_default();
    actions.push_back(OutAction::new_send(SENDMSG_ORDINARY, msg.clone()));
    if must_succeded {
        msg.get_value_mut().unwrap().grams = (out_msg_value - fwd_prices.lump_price).into();
        storage.append(&msg.serialize().unwrap());
    }

    // this message costs 10000000
    let msg = create_ext_out_msg(AccountId::from([0x11; 32]));
    if must_succeded { storage.append(&msg.serialize().unwrap()); }
    actions.push_back(OutAction::new_send(SENDMSG_ORDINARY, msg));
    let actions_hash = actions.hash().unwrap();
    let mut acc_balance = CurrencyCollection::with_grams(start_acc_balance);
    let original_acc_balance = acc_balance.clone();

    let my_addr = acc.get_addr().unwrap().clone();

    let result = executor.action_phase_with_copyleft(
        &mut tr,
        &mut acc,
        &original_acc_balance,
        &mut acc_balance,
        &mut msg_remaining_balance,
        &Grams::zero(),
        actions.serialize().unwrap(),
        None,
        &my_addr,
        false
    ).unwrap();
    let phase = result.phase;
    let mut phase2 = TrActionPhase::default();
    phase2.success = must_succeded;
    phase2.valid = true;
    phase2.no_funds = no_funds;
    phase2.msgs_created = if must_succeded {3} else {1};
    phase2.tot_actions = 3;
    phase2.status_change = AccStatusChange::Unchanged;
    phase2.action_list_hash = actions_hash;
    phase2.add_fwd_fees(fwd_fees.into());
    phase2.add_action_fees(action_fees.into());
    phase2.result_code = res;
    phase2.result_arg = res_arg;
    phase2.tot_msg_size = storage;
    assert_eq!(phase, phase2);
    if !no_funds {
        let balance = start_acc_balance - out_msg_value - 2 * fwd_prices.lump_price;
        assert_eq!(acc_balance.grams.as_u128(), balance.into());
    }
}

#[test]
fn test_action_phase_active_acc_with_actions_nofunds() {
    let fwd_config = BLOCKCHAIN_CONFIG.get_fwd_prices(true);
    let fwd_fee = fwd_config.lump_price;
    // will send one external message then fails
    call_action_phase(100000200, 300, false, true, fwd_fee, fwd_fee, RESULT_CODE_NOT_ENOUGH_GRAMS, Some(1));
}

#[test]
fn test_action_phase_active_acc_with_actions_success() {
    let fwd_config = BLOCKCHAIN_CONFIG.get_fwd_prices(true);
    let fwd_fee = fwd_config.lump_price * 3;
    let mine_fee = Grams::from(fwd_config.lump_price * 2) + fwd_config.mine_fee_checked(&fwd_config.lump_price.into()).unwrap();
    call_action_phase(5000000000, 100000000, true, false, fwd_fee, mine_fee, 0, None);
}


#[test]
fn test_gas_init1() {
    let gas_test = init_gas(15000000, 0, true, false, true, BLOCKCHAIN_CONFIG.get_gas_config(false));
    let gas_etalon = Gas::new(0, 10000, 15000, 1000);
    assert_eq!(gas_test, gas_etalon);
}

#[test]
fn test_gas_init2() {
    let gas_test = init_gas(4000000, 0, true, false, true, BLOCKCHAIN_CONFIG.get_gas_config(false));
    let gas_etalon = Gas::new(0, 4000, 4000, 1000);
    assert_eq!(gas_test, gas_etalon);
}

#[test]
fn test_gas_init3() {
    let gas_test = init_gas(10000000, 100000, false, false, true, BLOCKCHAIN_CONFIG.get_gas_config(false));
    let gas_etalon = Gas::new(100, 0, 10000, 1000);
    assert_eq!(gas_test, gas_etalon);
}

#[test]
fn test_gas_init4() {
    let gas_test = init_gas(1_000_000_000_000_000_000, 1_000_000_000_000, false, false, true, BLOCKCHAIN_CONFIG.get_gas_config(false));
    let gas_etalon = Gas::new(1000000, 0, 1000000, 1000);
    assert_eq!(gas_test, gas_etalon);
}


mod actions {
    use super::*;
    use pretty_assertions::assert_eq;
    use ever_block::{AddSub, RESERVE_PLUS_ORIG, RESERVE_REVERSE, ConfigParamEnum, ConfigParam12, WorkchainFormat0, WorkchainDescr, Workchains, ConfigParam18, StoragePrices, ConfigParam31, AnycastInfo};

    #[test]
    fn test_reserve_exactly() {
        // enough balance and extra
        let mut value = CurrencyCollection::with_grams(123);
        value.other.set(&11111111u32, &VarUInteger32::from_two_u128(0, 123).unwrap()).unwrap();
        let mut original_acc_balance = CurrencyCollection::with_grams(500);
        original_acc_balance.other.set(&11111111u32, &VarUInteger32::from_two_u128(0, 500).unwrap()).unwrap();
        let mut acc_remaining_balance = original_acc_balance.clone();
        let result = reserve_action_handler(RESERVE_EXACTLY, &value, &original_acc_balance, &mut acc_remaining_balance);
        let mut balance = original_acc_balance.clone();
        balance.sub(&value).unwrap();
        assert_eq!(result, Ok(value));
        assert_eq!(acc_remaining_balance, balance);

        // not enough balance
        let mut value = CurrencyCollection::with_grams(123);
        value.other.set(&11111111u32, &VarUInteger32::from_two_u128(0, 123).unwrap()).unwrap();
        let mut original_acc_balance = CurrencyCollection::with_grams(100);
        original_acc_balance.other.set(&11111111u32, &VarUInteger32::from_two_u128(0, 100).unwrap()).unwrap();
        let mut acc_remaining_balance = original_acc_balance.clone();
        let result = reserve_action_handler(RESERVE_EXACTLY, &value, &original_acc_balance, &mut acc_remaining_balance);
        assert_eq!(result, Err(RESULT_CODE_NOT_ENOUGH_GRAMS));
        assert_eq!(acc_remaining_balance, original_acc_balance);

        // not enough extra
        let mut value = CurrencyCollection::with_grams(100);
        value.other.set(&11111111u32, &VarUInteger32::from_two_u128(0, 123).unwrap()).unwrap();
        let mut original_acc_balance = CurrencyCollection::with_grams(123);
        original_acc_balance.other.set(&11111111u32, &VarUInteger32::from_two_u128(0, 100).unwrap()).unwrap();
        let mut acc_remaining_balance = original_acc_balance.clone();
        let result = reserve_action_handler(RESERVE_EXACTLY, &value, &original_acc_balance, &mut acc_remaining_balance);
        assert_eq!(result, Err(RESULT_CODE_NOT_ENOUGH_EXTRA));
        assert_eq!(acc_remaining_balance, original_acc_balance);
    }

    #[test]
    fn test_reserve_all_but() {
        // reserve = remaining_balance - value
        // enough balance and extra
        let mut value = CurrencyCollection::with_grams(123);
        value.other.set(&11111111u32, &VarUInteger32::from_two_u128(0, 123).unwrap()).unwrap();
        let mut original_acc_balance = CurrencyCollection::with_grams(500);
        original_acc_balance.other.set(&11111111u32, &VarUInteger32::from_two_u128(0, 500).unwrap()).unwrap();
        let mut acc_remaining_balance = original_acc_balance.clone();
        let result = reserve_action_handler(RESERVE_ALL_BUT, &value, &original_acc_balance, &mut acc_remaining_balance);
        let mut reserved = original_acc_balance.clone();
        reserved.sub(&value).unwrap();
        assert_eq!(result, Ok(reserved));
        assert_eq!(acc_remaining_balance, value);

        // not enough balance and extra
        let mut value = CurrencyCollection::with_grams(123);
        value.other.set(&11111111u32, &VarUInteger32::from_two_u128(0, 123).unwrap()).unwrap();
        let mut original_acc_balance = CurrencyCollection::with_grams(100);
        original_acc_balance.other.set(&11111111u32, &VarUInteger32::from_two_u128(0, 100).unwrap()).unwrap();
        let mut acc_remaining_balance = original_acc_balance.clone();
        let result = reserve_action_handler(RESERVE_ALL_BUT, &value, &original_acc_balance, &mut acc_remaining_balance);
        assert_eq!(result, Err(RESULT_CODE_NOT_ENOUGH_GRAMS));
        assert_eq!(acc_remaining_balance, original_acc_balance);
    }

    #[test]
    fn test_reserve_exactly_skip_error() {
        // reserve = min(value, remaining_balance)
        // reserved less than needed
        let value = CurrencyCollection::with_grams(123);
        let original_acc_balance = CurrencyCollection::with_grams(100);
        let mut acc_remaining_balance = original_acc_balance.clone();
        let result = reserve_action_handler(RESERVE_IGNORE_ERROR, &value, &original_acc_balance, &mut acc_remaining_balance);
        assert_eq!(result, Ok(original_acc_balance));
        assert_eq!(acc_remaining_balance, CurrencyCollection::with_grams(0));

        // balance and extra enough
        let mut value = CurrencyCollection::with_grams(100);
        value.other.set(&11111111u32, &VarUInteger32::from_two_u128(0, 100).unwrap()).unwrap();
        let mut original_acc_balance = CurrencyCollection::with_grams(123);
        original_acc_balance.other.set(&11111111u32, &VarUInteger32::from_two_u128(0, 123).unwrap()).unwrap();
        let mut acc_remaining_balance = original_acc_balance.clone();
        let result = reserve_action_handler(RESERVE_IGNORE_ERROR, &value, &original_acc_balance, &mut acc_remaining_balance);
        assert_eq!(result, Ok(value));
        let mut answer = original_acc_balance.clone();
        answer.grams = 23u64.into();
        answer.other.set(&11111111u32, &VarUInteger32::from_two_u128(0, 23).unwrap()).unwrap();
        assert_eq!(acc_remaining_balance, answer);

        // extra not enough
        let mut value = CurrencyCollection::with_grams(123);
        value.other.set(&11111111u32, &VarUInteger32::from_two_u128(0, 123).unwrap()).unwrap();
        let mut original_acc_balance = CurrencyCollection::with_grams(100);
        original_acc_balance.other.set(&11111111u32, &VarUInteger32::from_two_u128(0, 100).unwrap()).unwrap();
        let mut acc_remaining_balance = original_acc_balance.clone();
        let result = reserve_action_handler(RESERVE_IGNORE_ERROR, &value, &original_acc_balance, &mut acc_remaining_balance);
        assert_eq!(result, Err(RESULT_CODE_NOT_ENOUGH_EXTRA));
        assert_eq!(acc_remaining_balance, original_acc_balance);
    }

    #[test]
    fn test_reserve_allbut_skip_error() {
        // reserve = remaining_balance - min(value, remaining_balance)
        // reserved become less than needed
        let value = CurrencyCollection::with_grams(123);
        let original_acc_balance = CurrencyCollection::with_grams(100);
        let mut acc_remaining_balance = original_acc_balance.clone();
        let result = reserve_action_handler(RESERVE_IGNORE_ERROR | RESERVE_ALL_BUT, &value, &original_acc_balance, &mut acc_remaining_balance);
        assert_eq!(result.unwrap(), CurrencyCollection::with_grams(0));
        assert_eq!(acc_remaining_balance, CurrencyCollection::with_grams(100));

        // balance and extra enough
        let mut value = CurrencyCollection::with_grams(100);
        value.other.set(&11111111u32, &VarUInteger32::from_two_u128(0, 100).unwrap()).unwrap();
        let mut original_acc_balance = CurrencyCollection::with_grams(123);
        original_acc_balance.other.set(&11111111u32, &VarUInteger32::from_two_u128(0, 123).unwrap()).unwrap();
        let mut acc_remaining_balance = original_acc_balance.clone();
        let result = reserve_action_handler(RESERVE_IGNORE_ERROR | RESERVE_ALL_BUT, &value, &original_acc_balance, &mut acc_remaining_balance);
        let mut answer = CurrencyCollection::with_grams(23);
        answer.other.set(&11111111u32, &VarUInteger32::from_two_u128(0, 23).unwrap()).unwrap();
        assert_eq!(result.unwrap(), answer);
        let mut answer = CurrencyCollection::with_grams(100);
        answer.other.set(&11111111u32, &VarUInteger32::from_two_u128(0, 100).unwrap()).unwrap();
        assert_eq!(acc_remaining_balance, answer);

        // extra not enough
        let mut value = CurrencyCollection::with_grams(123);
        value.other.set(&11111111u32, &VarUInteger32::from_two_u128(0, 123).unwrap()).unwrap();
        let mut original_acc_balance = CurrencyCollection::with_grams(100);
        original_acc_balance.other.set(&11111111u32, &VarUInteger32::from_two_u128(0, 100).unwrap()).unwrap();
        let mut acc_remaining_balance = original_acc_balance.clone();
        let result = reserve_action_handler(RESERVE_IGNORE_ERROR | RESERVE_ALL_BUT, &value, &original_acc_balance, &mut acc_remaining_balance);
        assert_eq!(result, Err(RESULT_CODE_NOT_ENOUGH_EXTRA));
        assert_eq!(acc_remaining_balance, original_acc_balance);
    }

    #[test]
    fn test_reserve_sum_mode() {
        // reserve = original_balance + value
        // reserve exceed balance
        let mut value = CurrencyCollection::with_grams(123);
        value.other.set(&11111111u32, &VarUInteger32::from_two_u128(0, 123).unwrap()).unwrap();
        let mut original_acc_balance = CurrencyCollection::with_grams(100);
        original_acc_balance.other.set(&11111111u32, &VarUInteger32::from_two_u128(0, 100).unwrap()).unwrap();
        let mut acc_remaining_balance = original_acc_balance.clone();
        let result = reserve_action_handler(RESERVE_PLUS_ORIG, &value, &original_acc_balance, &mut acc_remaining_balance);
        assert_eq!(result, Err(RESULT_CODE_NOT_ENOUGH_GRAMS));
        assert_eq!(acc_remaining_balance, original_acc_balance);

        // message added money, so reserve don't exceed balance
        let mut value = CurrencyCollection::with_grams(123);
        value.other.set(&11111111u32, &VarUInteger32::from_two_u128(0, 123).unwrap()).unwrap();
        let mut original_acc_balance = CurrencyCollection::with_grams(100);
        original_acc_balance.other.set(&11111111u32, &VarUInteger32::from_two_u128(0, 100).unwrap()).unwrap();
        let mut message_balance = CurrencyCollection::with_grams(300);
        message_balance.other.set(&11111111u32, &VarUInteger32::from_two_u128(0, 300).unwrap()).unwrap();
        let mut acc_remaining_balance = original_acc_balance.clone();
        acc_remaining_balance.add(&message_balance).unwrap();
        let result = reserve_action_handler(RESERVE_PLUS_ORIG, &value, &original_acc_balance, &mut acc_remaining_balance);
        let mut answer = CurrencyCollection::with_grams(223);
        answer.other.set(&11111111u32, &VarUInteger32::from_two_u128(0, 223).unwrap()).unwrap();
        assert_eq!(result.unwrap(), answer);
        let mut answer = CurrencyCollection::with_grams(177);
        answer.other.set(&11111111u32, &VarUInteger32::from_two_u128(0, 177).unwrap()).unwrap();
        assert_eq!(acc_remaining_balance, answer);
    }

    #[test]
    fn test_reserve_minus_sum_mode() {
        // reserve = remaining_balance - (original_balance + value)
        // not enough balance and extra
        let mut value = CurrencyCollection::with_grams(123);
        value.other.set(&11111111u32, &VarUInteger32::from_two_u128(0, 123).unwrap()).unwrap();
        let mut original_acc_balance = CurrencyCollection::with_grams(100);
        original_acc_balance.other.set(&11111111u32, &VarUInteger32::from_two_u128(0, 100).unwrap()).unwrap();
        let mut acc_remaining_balance = original_acc_balance.clone();
        let result = reserve_action_handler(RESERVE_PLUS_ORIG | RESERVE_ALL_BUT, &value, &original_acc_balance, &mut acc_remaining_balance);
        assert_eq!(result, Err(RESULT_CODE_NOT_ENOUGH_GRAMS));
        assert_eq!(acc_remaining_balance, original_acc_balance);

        // message added money, so balance is enough
        let mut value = CurrencyCollection::with_grams(123);
        value.other.set(&11111111u32, &VarUInteger32::from_two_u128(0, 123).unwrap()).unwrap();
        let mut original_acc_balance = CurrencyCollection::with_grams(100);
        original_acc_balance.other.set(&11111111u32, &VarUInteger32::from_two_u128(0, 100).unwrap()).unwrap();
        let mut message_balance = CurrencyCollection::with_grams(300);
        message_balance.other.set(&11111111u32, &VarUInteger32::from_two_u128(0, 300).unwrap()).unwrap();
        let mut acc_remaining_balance = original_acc_balance.clone();
        acc_remaining_balance.add(&message_balance).unwrap();
        let result = reserve_action_handler(RESERVE_PLUS_ORIG | RESERVE_ALL_BUT, &value, &original_acc_balance, &mut acc_remaining_balance);
        let mut answer = CurrencyCollection::with_grams(177);
        answer.other.set(&11111111u32, &VarUInteger32::from_two_u128(0, 177).unwrap()).unwrap();
        assert_eq!(result.unwrap(), answer);
        let mut answer = CurrencyCollection::with_grams(223);
        answer.other.set(&11111111u32, &VarUInteger32::from_two_u128(0, 223).unwrap()).unwrap();
        assert_eq!(acc_remaining_balance, answer);
    }

    #[test]
    fn test_reserve_remaining_mode() {
        // reserve = min(original_balance + value, remaining_balance)
        // reserved become less than needed
        let value = CurrencyCollection::with_grams(123);
        let original_acc_balance = CurrencyCollection::with_grams(100);
        let mut acc_remaining_balance = original_acc_balance.clone();
        let result = reserve_action_handler(RESERVE_PLUS_ORIG | RESERVE_IGNORE_ERROR, &value, &original_acc_balance, &mut acc_remaining_balance);
        assert_eq!(result.unwrap(), original_acc_balance);
        assert_eq!(acc_remaining_balance, CurrencyCollection::with_grams(0));

        // message added money, so balance is enough
        let mut value = CurrencyCollection::with_grams(123);
        value.other.set(&11111111u32, &VarUInteger32::from_two_u128(0, 123).unwrap()).unwrap();
        let mut original_acc_balance = CurrencyCollection::with_grams(100);
        original_acc_balance.other.set(&11111111u32, &VarUInteger32::from_two_u128(0, 100).unwrap()).unwrap();
        let mut message_balance = CurrencyCollection::with_grams(300);
        message_balance.other.set(&11111111u32, &VarUInteger32::from_two_u128(0, 300).unwrap()).unwrap();
        let mut acc_remaining_balance = original_acc_balance.clone();
        acc_remaining_balance.add(&message_balance).unwrap();
        let result = reserve_action_handler(RESERVE_PLUS_ORIG | RESERVE_IGNORE_ERROR, &value, &original_acc_balance, &mut acc_remaining_balance);
        let mut answer = CurrencyCollection::with_grams(223);
        answer.other.set(&11111111u32, &VarUInteger32::from_two_u128(0, 223).unwrap()).unwrap();
        assert_eq!(result.unwrap(), answer);
        let mut answer = CurrencyCollection::with_grams(177);
        answer.other.set(&11111111u32, &VarUInteger32::from_two_u128(0, 177).unwrap()).unwrap();
        assert_eq!(acc_remaining_balance, answer);

        // extra not enough
        let mut value = CurrencyCollection::with_grams(123);
        value.other.set(&11111111u32, &VarUInteger32::from_two_u128(0, 123).unwrap()).unwrap();
        let mut original_acc_balance = CurrencyCollection::with_grams(100);
        original_acc_balance.other.set(&11111111u32, &VarUInteger32::from_two_u128(0, 100).unwrap()).unwrap();
        let mut acc_remaining_balance = original_acc_balance.clone();
        let result = reserve_action_handler(RESERVE_PLUS_ORIG | RESERVE_IGNORE_ERROR, &value, &original_acc_balance, &mut acc_remaining_balance);
        assert_eq!(result, Err(RESULT_CODE_NOT_ENOUGH_EXTRA));
        let mut answer = CurrencyCollection::with_grams(100);
        answer.other.set(&11111111u32, &VarUInteger32::from_two_u128(0, 100).unwrap()).unwrap();
        assert_eq!(acc_remaining_balance, answer);
    }

    #[test]
    fn test_reserve_7_mode() {
        // reserve = remaining_balance - min(original_balance + value, remaining_balance)
        let value = CurrencyCollection::with_grams(123);
        let original_acc_balance = CurrencyCollection::with_grams(100);
        let message_balance = CurrencyCollection::with_grams(300);
        let mut acc_remaining_balance = original_acc_balance.clone();
        acc_remaining_balance.add(&message_balance).unwrap();
        let result = reserve_action_handler(RESERVE_PLUS_ORIG | RESERVE_IGNORE_ERROR | RESERVE_ALL_BUT, &value, &original_acc_balance, &mut acc_remaining_balance);
        assert_eq!(result.unwrap(), CurrencyCollection::with_grams(177));
        assert_eq!(acc_remaining_balance, CurrencyCollection::with_grams(223));

        let value = CurrencyCollection::with_grams(123);
        let original_acc_balance = CurrencyCollection::with_grams(100);
        let mut acc_remaining_balance = original_acc_balance.clone();
        let result = reserve_action_handler(RESERVE_PLUS_ORIG | RESERVE_IGNORE_ERROR | RESERVE_ALL_BUT, &value, &original_acc_balance, &mut acc_remaining_balance);
        assert_eq!(result.unwrap(), CurrencyCollection::with_grams(0));
        assert_eq!(acc_remaining_balance, CurrencyCollection::with_grams(100));
    }

    #[test]
    fn test_reserve_unsupported_mode() {
        let value = CurrencyCollection::with_grams(123);
        let original_acc_balance = CurrencyCollection::with_grams(100);
        let mut acc_remaining_balance = original_acc_balance.clone();

        for mode in 8..=11 {
            let result = reserve_action_handler(mode, &value, &original_acc_balance, &mut acc_remaining_balance);
            assert_eq!(result, Err(RESULT_CODE_UNKNOWN_OR_INVALID_ACTION));
            assert_eq!(acc_remaining_balance, original_acc_balance);
        }

        let result = reserve_action_handler(16, &value, &original_acc_balance, &mut acc_remaining_balance);
        assert_eq!(result, Err(RESULT_CODE_UNKNOWN_OR_INVALID_ACTION));
        assert_eq!(acc_remaining_balance, original_acc_balance);
    }

    #[test]
    fn test_reserve_all_except_mode() {
        // reserve = original_balance - value
        // balance enough
        let value = CurrencyCollection::with_grams(10);
        let original_acc_balance = CurrencyCollection::with_grams(100);
        let mut acc_remaining_balance = original_acc_balance.clone();
        let result = reserve_action_handler(RESERVE_REVERSE | RESERVE_PLUS_ORIG, &value, &original_acc_balance, &mut acc_remaining_balance);
        assert_eq!(result.unwrap(), CurrencyCollection::with_grams(90));
        assert_eq!(acc_remaining_balance, CurrencyCollection::with_grams(10));

        // balance not enough
        let value = CurrencyCollection::with_grams(100);
        let original_acc_balance = CurrencyCollection::with_grams(10);
        let mut acc_remaining_balance = original_acc_balance.clone();
        let result = reserve_action_handler(RESERVE_REVERSE | RESERVE_PLUS_ORIG, &value, &original_acc_balance, &mut acc_remaining_balance);
        assert_eq!(result, Err(RESULT_CODE_UNSUPPORTED));
        assert_eq!(acc_remaining_balance, original_acc_balance);

        // balance not enough despite message balance
        let value = CurrencyCollection::with_grams(100);
        let original_acc_balance = CurrencyCollection::with_grams(10);
        let message_balance = CurrencyCollection::with_grams(300);
        let mut acc_remaining_balance = original_acc_balance.clone();
        acc_remaining_balance.add(&message_balance).unwrap();
        let result = reserve_action_handler(RESERVE_REVERSE | RESERVE_PLUS_ORIG, &value, &original_acc_balance, &mut acc_remaining_balance);
        assert_eq!(result, Err(RESULT_CODE_UNSUPPORTED));
        assert_eq!(acc_remaining_balance, CurrencyCollection::with_grams(310));

        // balance not enough because of fee
        let value = CurrencyCollection::with_grams(10);
        let original_acc_balance = CurrencyCollection::with_grams(100);
        let fee_values = CurrencyCollection::with_grams(99);
        let mut acc_remaining_balance = original_acc_balance.clone();
        acc_remaining_balance.sub(&fee_values).unwrap();
        let result = reserve_action_handler(RESERVE_REVERSE | RESERVE_PLUS_ORIG, &value, &original_acc_balance, &mut acc_remaining_balance);
        assert_eq!(result, Err(RESULT_CODE_NOT_ENOUGH_GRAMS));
        assert_eq!(acc_remaining_balance, CurrencyCollection::with_grams(1));
    }

    #[test]
    fn test_reserve_13_mode() {
        // reserve = remaining_balance - (original_balance - value)
        let value = CurrencyCollection::with_grams(100);
        let original_acc_balance = CurrencyCollection::with_grams(123);
        let message_balance = CurrencyCollection::with_grams(300);
        let mut acc_remaining_balance = original_acc_balance.clone();
        acc_remaining_balance.add(&message_balance).unwrap();
        let result = reserve_action_handler(RESERVE_REVERSE | RESERVE_PLUS_ORIG | RESERVE_ALL_BUT, &value, &original_acc_balance, &mut acc_remaining_balance);
        assert_eq!(result.unwrap(), CurrencyCollection::with_grams(400));
        assert_eq!(acc_remaining_balance, CurrencyCollection::with_grams(23));

        let value = CurrencyCollection::with_grams(123);
        let original_acc_balance = CurrencyCollection::with_grams(100);
        let message_balance = CurrencyCollection::with_grams(300);
        let mut acc_remaining_balance = original_acc_balance.clone();
        acc_remaining_balance.add(&message_balance).unwrap();
        let result = reserve_action_handler(RESERVE_REVERSE | RESERVE_PLUS_ORIG | RESERVE_ALL_BUT, &value, &original_acc_balance, &mut acc_remaining_balance);
        assert_eq!(result, Err(RESULT_CODE_UNSUPPORTED));
        assert_eq!(acc_remaining_balance, CurrencyCollection::with_grams(400));

        let value = CurrencyCollection::with_grams(10);
        let original_acc_balance = CurrencyCollection::with_grams(123);
        let fee_value = CurrencyCollection::with_grams(23);
        let mut acc_remaining_balance = original_acc_balance.clone();
        acc_remaining_balance.sub(&fee_value).unwrap();
        let result = reserve_action_handler(RESERVE_REVERSE | RESERVE_PLUS_ORIG | RESERVE_ALL_BUT, &value, &original_acc_balance, &mut acc_remaining_balance);
        assert_eq!(result, Err(RESULT_CODE_NOT_ENOUGH_GRAMS));
        assert_eq!(acc_remaining_balance, CurrencyCollection::with_grams(100));
    }

    #[test]
    fn test_reserve_14_mode() {
        // reserve = min(original_balance - value, remaining_balance)
        let value = CurrencyCollection::with_grams(100);
        let original_acc_balance = CurrencyCollection::with_grams(123);
        let message_balance = CurrencyCollection::with_grams(300);
        let mut acc_remaining_balance = original_acc_balance.clone();
        acc_remaining_balance.add(&message_balance).unwrap();
        let result = reserve_action_handler(RESERVE_REVERSE | RESERVE_PLUS_ORIG | RESERVE_IGNORE_ERROR, &value, &original_acc_balance, &mut acc_remaining_balance);
        assert_eq!(result.unwrap(), CurrencyCollection::with_grams(23));
        assert_eq!(acc_remaining_balance, CurrencyCollection::with_grams(400));

        let value = CurrencyCollection::with_grams(10);
        let original_acc_balance = CurrencyCollection::with_grams(123);
        let fees = CurrencyCollection::with_grams(23);
        let mut acc_remaining_balance = original_acc_balance.clone();
        acc_remaining_balance.sub(&fees).unwrap();
        let result = reserve_action_handler(RESERVE_REVERSE | RESERVE_PLUS_ORIG | RESERVE_IGNORE_ERROR, &value, &original_acc_balance, &mut acc_remaining_balance);
        assert_eq!(result.unwrap(), CurrencyCollection::with_grams(100));
        assert_eq!(acc_remaining_balance, CurrencyCollection::with_grams(0));

        let value = CurrencyCollection::with_grams(123);
        let original_acc_balance = CurrencyCollection::with_grams(100);
        let message_balance = CurrencyCollection::with_grams(300);
        let mut acc_remaining_balance = original_acc_balance.clone();
        acc_remaining_balance.add(&message_balance).unwrap();
        let result = reserve_action_handler(RESERVE_REVERSE | RESERVE_PLUS_ORIG | RESERVE_IGNORE_ERROR, &value, &original_acc_balance, &mut acc_remaining_balance);
        assert_eq!(result, Err(RESULT_CODE_UNSUPPORTED));
        assert_eq!(acc_remaining_balance, CurrencyCollection::with_grams(400));
    }

    #[test]
    fn test_reserve_15_mode() {
        // reserve = remaining_balance - min(original_balance - value, remaining_balance)
        let value = CurrencyCollection::with_grams(100);
        let original_acc_balance = CurrencyCollection::with_grams(123);
        let message_balance = CurrencyCollection::with_grams(300);
        let mut acc_remaining_balance = original_acc_balance.clone();
        acc_remaining_balance.add(&message_balance).unwrap();
        let result = reserve_action_handler(RESERVE_REVERSE | RESERVE_PLUS_ORIG | RESERVE_IGNORE_ERROR | RESERVE_ALL_BUT, &value, &original_acc_balance, &mut acc_remaining_balance);
        assert_eq!(result.unwrap(), CurrencyCollection::with_grams(400));
        assert_eq!(acc_remaining_balance, CurrencyCollection::with_grams(23));

        let value = CurrencyCollection::with_grams(10);
        let original_acc_balance = CurrencyCollection::with_grams(123);
        let fees = CurrencyCollection::with_grams(23);
        let mut acc_remaining_balance = original_acc_balance.clone();
        acc_remaining_balance.sub(&fees).unwrap();
        let result = reserve_action_handler(RESERVE_REVERSE | RESERVE_PLUS_ORIG | RESERVE_IGNORE_ERROR | RESERVE_ALL_BUT, &value, &original_acc_balance, &mut acc_remaining_balance);
        assert_eq!(result.unwrap(), CurrencyCollection::with_grams(0));
        assert_eq!(acc_remaining_balance, CurrencyCollection::with_grams(100));

        let value = CurrencyCollection::with_grams(123);
        let original_acc_balance = CurrencyCollection::with_grams(100);
        let message_balance = CurrencyCollection::with_grams(300);
        let mut acc_remaining_balance = original_acc_balance.clone();
        acc_remaining_balance.add(&message_balance).unwrap();
        let result = reserve_action_handler(RESERVE_REVERSE | RESERVE_PLUS_ORIG | RESERVE_IGNORE_ERROR | RESERVE_ALL_BUT, &value, &original_acc_balance, &mut acc_remaining_balance);
        assert_eq!(result, Err(RESULT_CODE_UNSUPPORTED));
        assert_eq!(acc_remaining_balance, CurrencyCollection::with_grams(400));
    }

    fn test_sendmsg_action(mode: u8, val: u64, bal: u64, lt: Arc<AtomicU64>, fwd_fee: u64, mine_fee: u64, error: Option<i32>) {
        let mut balance = CurrencyCollection::with_grams(bal);
        let mut acc_remaining_balance = balance.clone();
        let mut msg_remaining_balance = CurrencyCollection::with_grams(val);
        let mut phase = TrActionPhase::default();
        phase.add_fwd_fees(3u64.into());
        phase.add_action_fees(5u64.into());
        let address = MsgAddressInt::with_standart(None, -1, AccountId::from([0x11; 32])).unwrap();
        let mut msg = create_int_msg(AccountId::from([0x11; 32]), AccountId::from([0x22; 32]), val, false, 0, fwd_fee);

        let msg_lt = lt.load(Ordering::SeqCst);
        msg.set_at_and_lt(0, msg_lt);
        let res = outmsg_action_handler(
            &mut phase,
            mode,
            &mut msg,
            &mut acc_remaining_balance,
            &mut msg_remaining_balance,
            &Grams::zero(),
            &BLOCKCHAIN_CONFIG,
            false,
            &address,
            &CurrencyCollection::default(),
            &mut false
        );

        let mut res_val = CurrencyCollection::with_grams(val);
        if (mode & SENDMSG_ALL_BALANCE) != 0 {
            res_val = CurrencyCollection::with_grams(bal);
        } else if (mode & SENDMSG_PAY_FEE_SEPARATELY) != 0 {
            res_val.add(&CurrencyCollection::with_grams(fwd_fee)).unwrap();
        }

        if error.is_some() {
            assert_eq!(res, Err(error.unwrap()));
            return;
        } 
        assert_eq!(res, Ok(res_val.clone()));

        balance.sub(&res_val).unwrap();
        assert_eq!(acc_remaining_balance, balance);

        assert_eq!(msg.src_ref().expect("must be internal msg"), &address);
        assert_eq!(msg.at_and_lt().unwrap(), (0, msg_lt));
        assert_eq!(msg.get_fee().unwrap(), Some((fwd_fee - mine_fee).into()));

        res_val.sub(&CurrencyCollection::with_grams(fwd_fee)).unwrap();
        assert_eq!(msg.get_value().unwrap().clone(), res_val);

        let mut total_fwd_fees = Grams::zero();
        total_fwd_fees.add(&3u64.into()).unwrap();
        total_fwd_fees.add(&fwd_fee.into()).unwrap();
        assert_eq!(phase.total_fwd_fees(), total_fwd_fees);

        let mut total_action_fees = Grams::zero();
        total_action_fees.add(&5u64.into()).unwrap();
        total_action_fees.add(&mine_fee.into()).unwrap();
        assert_eq!(phase.total_action_fees(), total_action_fees);
    }

    #[test]
    fn test_sendmsg_internal_fees_separately() {
        test_sendmsg_action(SENDMSG_PAY_FEE_SEPARATELY, 10000000, 50000000, Arc::new(AtomicU64::new(12)), 10000000, 3333282, None);
    }

    #[test]
    fn test_sendmsg_internal_ordinary_skip_error() {
        test_sendmsg_action(SENDMSG_IGNORE_ERROR, 15000000, 9000000, Arc::new(AtomicU64::new(12)), 10000000, 3333282, Some(0));
    }

    #[test]
    fn test_sendmsg_internal_fees_separately_skip_error() {
        test_sendmsg_action(SENDMSG_IGNORE_ERROR | SENDMSG_PAY_FEE_SEPARATELY, 15000000, 9000000, Arc::new(AtomicU64::new(12)), 10000000, 3333282, Some(0));
    }

    #[test]
    fn test_sendmsg_internal_ordinary() {
        test_sendmsg_action(SENDMSG_ORDINARY, 10000000, 50000000, Arc::new(AtomicU64::new(12)), 10000000, 3333282, None);
    }

    #[test]
    fn test_sendmsg_internal_ordinary_no_funds() {
        test_sendmsg_action(SENDMSG_ORDINARY, 15000000, 8000000, Arc::new(AtomicU64::new(12)), 10000000, 3333282, Some(RESULT_CODE_NOT_ENOUGH_GRAMS));
    }

    #[test]
    fn test_sendmsg_internal_all_balance() {
        //test case:
        //balance was 120, contract sent msg with value = 120 
        //then vm executed, gas exacted and balance became 100
        //then in action phase need to transfer all remaining balance (100)
        test_sendmsg_action(SENDMSG_ALL_BALANCE, 12000000, 10000000, Arc::new(AtomicU64::new(3)), 10000000, 3333282, None);
    }

    #[test]
    fn test_sendmsg_internal_wrong_mode() {
        test_sendmsg_action(4, 15000000, 8000000, Arc::new(AtomicU64::new(12)), 10000000, 3333282, Some(RESULT_CODE_UNSUPPORTED));
    }

    #[test]
    fn test_check_rewrite_dst() {
        let mut wc0 = WorkchainDescr::default();
        wc0.accept_msgs = true;
        let wc1 = wc0.clone();
        let mut wcs = Workchains::default();
        wcs.set(&0, &wc0).unwrap();
        wcs.set(&-1, &wc1).unwrap();

        let mut wc3 = wc0.clone();
        wc3.format = WorkchainFormat::Extended(WorkchainFormat0::with_params(128, 512, 128, 3).unwrap());
        wcs.set(&3, &wc3).unwrap();
        let mut wc4 = wc0;
        wc4.accept_msgs = false;
        wcs.set(&4, &wc4).unwrap();
        let mut raw_cfg = BLOCKCHAIN_CONFIG.raw_config().clone();
        raw_cfg.set_config(ConfigParamEnum::ConfigParam12(ConfigParam12{ workchains: wcs})).unwrap();

        let mut cf18 = ConfigParam18::default();
        cf18.insert(&StoragePrices::default()).unwrap();
        raw_cfg.set_config(ConfigParamEnum::ConfigParam18(cf18)).unwrap();
        raw_cfg.set_config(ConfigParamEnum::ConfigParam31(ConfigParam31::default())).unwrap();
        let cfg = BlockchainConfig::with_config(raw_cfg).unwrap();

        // simple masterchain
        let dst = MsgAddressInt::with_standart(None, -1, AccountId::from([0x22; 32])).unwrap();
        assert_eq!(check_rewrite_dest_addr(&dst, &cfg, &dst).unwrap(), dst);

        // from masterchain to workchain
        let dst = MsgAddressInt::with_standart(None, 0, AccountId::from([0x33; 32])).unwrap();
        let src = MsgAddressInt::with_standart(None, -1, AccountId::from([0x22; 32])).unwrap();
        assert_eq!(check_rewrite_dest_addr(&dst, &cfg, &src).unwrap(), dst);

        // from workchain to masterchain
        let dst = MsgAddressInt::with_standart(None, -1, AccountId::from([0x33; 32])).unwrap();
        let src = MsgAddressInt::with_standart(None, 0, AccountId::from([0x22; 32])).unwrap();
        assert_eq!(check_rewrite_dest_addr(&dst, &cfg, &src).unwrap(), dst);

        // from workchain to masterchain
        let dst = MsgAddressInt::with_standart(None, -1, AccountId::from([0x33; 32])).unwrap();
        let src = MsgAddressInt::with_standart(None, 2, AccountId::from([0x22; 32])).unwrap();
        assert_eq!(check_rewrite_dest_addr(&dst, &cfg, &src), Err(IncorrectCheckRewrite::Other));

        // from workchain to workchain
        let dst = MsgAddressInt::with_standart(None, 0, AccountId::from([0x33; 32])).unwrap();
        let src = MsgAddressInt::with_standart(None, 2, AccountId::from([0x22; 32])).unwrap();
        assert_eq!(check_rewrite_dest_addr(&dst, &cfg, &src), Err(IncorrectCheckRewrite::Other));

        // anycast masterchain
        let dst = MsgAddressInt::with_standart(Some(AnycastInfo::with_rewrite_pfx(SliceData::new(vec![0x33; 3])).unwrap()), -1, AccountId::from([0x22; 32])).unwrap();
        assert_eq!(check_rewrite_dest_addr(&dst, &cfg, &dst), Err(IncorrectCheckRewrite::Anycast));

        // simple workchain
        let dst = MsgAddressInt::with_standart(None, 0, AccountId::from([0x22; 32])).unwrap();
        assert_eq!(check_rewrite_dest_addr(&dst, &cfg, &dst).unwrap(), dst);

        // unknown workchain
        let dst = MsgAddressInt::with_standart(None, 10, AccountId::from([0x22; 32])).unwrap();
        assert_eq!(check_rewrite_dest_addr(&dst, &cfg, &dst), Err(IncorrectCheckRewrite::Other));

        // anycast
        let dst = MsgAddressInt::with_standart(Some(AnycastInfo::with_rewrite_pfx(SliceData::new(vec![0x33; 3])).unwrap()), 0, AccountId::from([0x22; 32])).unwrap();
        assert_eq!(check_rewrite_dest_addr(&dst, &cfg, &dst), Err(IncorrectCheckRewrite::Anycast));

        // addr_var
        let dst = MsgAddressInt::with_variant(None, 0, SliceData::from_raw(vec![0x22; 32], 256)).unwrap();
        let answer = MsgAddressInt::with_standart(None, 0, AccountId::from([0x22; 32])).unwrap();
        assert_eq!(check_rewrite_dest_addr(&dst, &cfg, &dst).unwrap(), answer);

        // incorrect len address
        let dst = MsgAddressInt::with_variant(None, 0, SliceData::from_raw(vec![0x22; 32], 255)).unwrap();
        assert_eq!(check_rewrite_dest_addr(&dst, &cfg, &dst), Err(IncorrectCheckRewrite::Other));

        // custom workchain
        let dst = MsgAddressInt::with_standart(None, 3, AccountId::from([0x22; 32])).unwrap();
        assert_eq!(check_rewrite_dest_addr(&dst, &cfg, &dst).unwrap(), dst);

        // custom workchain not accept msgs
        let dst = MsgAddressInt::with_standart(None, 4, AccountId::from([0x22; 32])).unwrap();
        assert_eq!(check_rewrite_dest_addr(&dst, &cfg, &dst), Err(IncorrectCheckRewrite::Other));

        // custom workchain incorrect len address
        let dst = MsgAddressInt::with_variant(None, 3, SliceData::from_raw(vec![0x22; 20], 160)).unwrap();
        assert_eq!(check_rewrite_dest_addr(&dst, &cfg, &dst), Err(IncorrectCheckRewrite::Other));

        // custom workchain addr_var
        let dst = MsgAddressInt::with_variant(None, 3, SliceData::from_raw(vec![0x22; 16], 128)).unwrap();
        assert_eq!(check_rewrite_dest_addr(&dst, &cfg, &dst).unwrap(), dst);

        // masterchain addr_var
        let dst = MsgAddressInt::with_variant(None, -1, SliceData::from_raw(vec![0x22; 16], 128)).unwrap();
        assert_eq!(check_rewrite_dest_addr(&dst, &cfg, &dst), Err(IncorrectCheckRewrite::Other));

        // custom workchain anycast
        let dst = MsgAddressInt::with_variant(Some(AnycastInfo::with_rewrite_pfx(SliceData::new(vec![0x33; 3])).unwrap()), 3, SliceData::from_raw(vec![0x22; 16], 128)).unwrap();
        assert_eq!(check_rewrite_dest_addr(&dst, &cfg, &dst), Err(IncorrectCheckRewrite::Anycast));
    }
}

#[test]
fn test_account_from_message() {
    let src = MsgAddressInt::with_standart(None, 0, [0x11; 32].into()).unwrap();
    let dst = MsgAddressInt::with_standart(None, 0, [0x22; 32].into()).unwrap();
    let ext = MsgAddressExt::with_extern([0x99; 32].into()).unwrap();
    // external inbound message
    let hdr = ExternalInboundMessageHeader::new(ext.clone(), dst.clone());
    let msg = Message::with_ext_in_header(hdr);
    assert!(account_from_message(&msg, msg.value().unwrap_or(&CurrencyCollection::default()), false, false, false).is_none(), "account mustn't be constructed using external message");

    // external outbound message
    let hdr = ExtOutMessageHeader::with_addresses(src.clone(), ext);
    let msg = Message::with_ext_out_header(hdr);
    assert!(account_from_message(&msg, msg.value().unwrap_or(&CurrencyCollection::default()), false, false, false).is_none(), "account mustn't be constructed using external message");

    // message without StateInit and with bounce
    let value = CurrencyCollection::with_grams(0);
    let hdr = InternalMessageHeader::with_addresses_and_bounce(src.clone(), dst.clone(), value, true);
    let msg = Message::with_int_header(hdr);
    assert!(account_from_message(&msg, msg.value().unwrap_or(&CurrencyCollection::default()), false, false, false).is_none(), "account mustn't be counstructed without StateInit and with bounce and zero msg balance");

    // message without code
    let value = CurrencyCollection::with_grams(0);
    let hdr = InternalMessageHeader::with_addresses_and_bounce(src.clone(), dst.clone(), value, true);
    let mut msg = Message::with_int_header(hdr);
    let init = StateInit::default();
    msg.set_state_init(init);
    assert!(account_from_message(&msg, msg.value().unwrap(), false, false, false).is_none(), "account mustn't be constructed without code");

    // message without balance
    let hdr = InternalMessageHeader::with_addresses_and_bounce(src.clone(), dst.clone(), Default::default(), true);
    let mut msg = Message::with_int_header(hdr);
    let mut init = StateInit::default();
    init.set_code(SliceData::new(vec![0x71, 0x80]).into_cell());
    msg.set_state_init(init);
    assert_eq!(account_from_message(&msg, msg.value().unwrap(), false, false, false).unwrap().status(), AccountStatus::AccStateActive, "account must be constructed without balance");

    // message without StateInit and without bounce
    let value = CurrencyCollection::with_grams(100);
    let hdr = InternalMessageHeader::with_addresses_and_bounce(src, dst, value, false);
    let mut msg = Message::with_int_header(hdr);
    assert_eq!(account_from_message(&msg, msg.value().unwrap(), false, false, false).unwrap().status(), AccountStatus::AccStateUninit, "account must be constructed without StateInit and without bounce");

    // message with code and without bounce
    let mut init = StateInit::default();
    init.set_code(BuilderData::with_bitstring(vec![0x71, 0x80]).unwrap().into_cell().unwrap());
    msg.set_state_init(init);
    assert_eq!(account_from_message(&msg, msg.value().unwrap(), false, false, false).unwrap().status(), AccountStatus::AccStateActive, "account must be constructed with code and without bounce");

    // message with code and with bounce
    let mut init = StateInit::default();
    init.set_code(BuilderData::with_bitstring(vec![0x71, 0x80]).unwrap().into_cell().unwrap());
    msg.set_state_init(init);
    assert_eq!(account_from_message(&msg, msg.value().unwrap(), false, false, false).unwrap().status(), AccountStatus::AccStateActive, "account must be constructed with code and with bounce");

    // message with libraries
    let mut init = StateInit::default();
    init.set_code(BuilderData::with_bitstring(vec![0x71, 0x80]).unwrap().into_cell().unwrap());
    init.set_library_code(BuilderData::with_raw(vec![0x72], 8).unwrap().into_cell().unwrap(), true).unwrap();
    msg.set_state_init(init);
    assert_eq!(account_from_message(&msg, msg.value().unwrap(), false, false, true).unwrap().status(), AccountStatus::AccStateUninit, "account mustn't be constructed if libraries disabled");

    // message with libraries
    let mut init = StateInit::default();
    init.set_code(BuilderData::with_bitstring(vec![0x71, 0x80]).unwrap().into_cell().unwrap());
    init.set_library_code(BuilderData::with_raw(vec![0x72], 8).unwrap().into_cell().unwrap(), true).unwrap();
    msg.set_state_init(init);
    assert_eq!(account_from_message(&msg, msg.value().unwrap(), false, false, false).unwrap().status(), AccountStatus::AccStateActive, "account must be constructed if libraries enabled");
}

#[test]
fn test_generate_account_and_update() {
    let mut account = generate_test_account_by_init_code_hash(false);
    account.set_code(Cell::default()); // set code does not update storage stat
    let cell = account.serialize().unwrap(); // serialization doesn't update storage stat
    let account2 = Account::construct_from_cell(cell).unwrap();
    assert_eq!(account, account2);
    account.update_storage_stat().unwrap();
    assert_ne!(account, account2);
}
