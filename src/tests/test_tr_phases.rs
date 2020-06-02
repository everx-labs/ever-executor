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

use super::*;
use crate::{
    blockchain_config::{
        BlockchainConfig, CalcMsgFwdFees, GasConfigFull, TONDefaultConfig
    }, 
    error::ExecutorError, 
    OrdinaryTransactionExecutor,
};

use pretty_assertions::assert_eq;
use std::sync::{atomic::AtomicU64, Arc};
use ton_block::{
    CurrencyCollection, GetRepresentationHash, Serializable, StateInit, 
    StorageUsedShort, UnixTime32, 
    accounts::{Account, AccountStatus}, config_params::MsgForwardPrices,
    messages::{
        CommonMsgInfo, ExternalInboundMessageHeader, ExtOutMessageHeader,
        InternalMessageHeader, Message, MsgAddressInt, MsgAddressIntOrNone
    },
    out_actions::{
        OutAction, OutActions, RESERVE_ALL_BUT, RESERVE_IGNORE_ERROR, 
        SENDMSG_ALL_BALANCE, SENDMSG_IGNORE_ERROR, SENDMSG_ORDINARY, 
        SENDMSG_PAY_FEE_SEPARATELY
    },
    transactions::{
        AccStatusChange, ComputeSkipReason, Transaction, TrActionPhase, TrComputePhase, 
        TrComputePhaseSkipped, TrComputePhaseVm
    },
    types::{Grams}
};
use ton_types::{AccountId, SliceData};
use ton_vm::{
    assembler::compile_code, executor::gas::gas_state::Gas, 
    smart_contract_info::SmartContractInfo, 
};

fn create_ext_msg(dest: AccountId) -> Message {
    let body_slice = SliceData::default();
    let mut hdr = ExternalInboundMessageHeader::default();
    hdr.dst = MsgAddressInt::with_standart(None, -1, dest).unwrap();
    let mut msg = Message::with_ext_in_header(hdr);
    msg.set_body(body_slice);
    msg
}

const INTERNAL_FWD_FEE: u64 = 5;

fn create_int_msg(src: AccountId, dest: AccountId, value: u64, bounce: bool, lt: u64, fwd_fee: u64) -> Message {
    let mut balance = CurrencyCollection::new();
    balance.grams = value.into();
    let mut hdr = InternalMessageHeader::with_addresses(
        MsgAddressInt::with_standart(None, -1, src).unwrap(),
        MsgAddressInt::with_standart(None, -1, dest).unwrap(),
        balance,
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
    hdr.src = MsgAddressIntOrNone::Some(MsgAddressInt::with_standart(None, -1, src_addr.clone()).unwrap());
    hdr.created_lt = 1;
    hdr.created_at = UnixTime32(0x12345678);
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

#[test]
fn test_computing_phase_acc_notexist_extmsg_nostate() {
    let msg = create_ext_msg(AccountId::from_raw(vec![0x11; 32], 256));
    let mut acc = Account::default();
    let info = SmartContractInfo::default();
    let config = BlockchainConfig::default();
    let executor = OrdinaryTransactionExecutor::new(config);
    let (phase, _actions) = executor.compute_phase(Some(&msg), &mut acc, &info, &executor, &GasConfigFull::default_mc(), false, true).unwrap();
    assert_eq!(
        phase, 
        TrComputePhase::Skipped(TrComputePhaseSkipped { reason: ComputeSkipReason::NoState })
    );
}

#[test]
fn test_computing_phase_acc_notexist_extmsg_state() {
    let mut msg = create_ext_msg(AccountId::from_raw(vec![0x11; 32], 256));
    msg.set_state_init(create_state_init());
    let mut acc = Account::default();
    let info = SmartContractInfo::default();
    let config = BlockchainConfig::default();
    let executor = OrdinaryTransactionExecutor::new(config);
    let (phase, _actions) = executor.compute_phase(Some(&msg), &mut acc, &info, &executor, &GasConfigFull::default_mc(), false, true).unwrap();
    assert_eq!(
        phase, 
        TrComputePhase::Skipped(TrComputePhaseSkipped { reason: ComputeSkipReason::NoState })
    );
}

#[test]
fn test_computing_phase_acc_notexist_intmsg_state() {
    let balance = 5_000_000;
    let msg = create_int_msg(
        AccountId::from_raw(vec![0x11; 32], 256),
        AccountId::from_raw(vec![0x22; 32], 256),
        balance,
        false,
        5,
        INTERNAL_FWD_FEE,
    );

    let gas_config = GasConfigFull::default_mc();
    
    let mut acc = Account::default();
    let info = SmartContractInfo::default();
    assert_eq!(acc.status(), AccountStatus::AccStateNonexist);
    let config = BlockchainConfig::default();
    let executor = OrdinaryTransactionExecutor::new(config);
    let (phase, _actions) = executor.compute_phase(Some(&msg), &mut acc, &info, &executor, &gas_config, false, true).unwrap();
    let mut vm_phase = TrComputePhaseVm::default();
    vm_phase.success = true;
    vm_phase.msg_state_used = false;
    vm_phase.account_activated = false;
    vm_phase.exit_code = 0;
    let used = 5u64;
    vm_phase.gas_used = used.into();
    vm_phase.gas_limit = 1000000.into();
    vm_phase.gas_credit = None;
    vm_phase.gas_fees = (gas_config.flat_gas_price).into();
    vm_phase.vm_steps = 1;
    assert_eq!(phase, TrComputePhase::Vm(vm_phase));
    assert_eq!(acc.status(), AccountStatus::AccStateUninit);
}

#[test]
fn test_computing_phase_acc_uninit_extmsg_nostate() {
    let msg = create_ext_msg(AccountId::from_raw(vec![0x11; 32], 256));

    //create uninitialized account
    let mut acc = Account::with_address_and_ballance(
        &MsgAddressInt::with_standart(None, -1, AccountId::from_raw(vec![0x11; 32], 256)).unwrap(),
        &CurrencyCollection::with_grams(1000),
    );
    let info = SmartContractInfo::default();
    let config = BlockchainConfig::default();
    let executor = OrdinaryTransactionExecutor::new(config);
    let (phase, _actions) = executor.compute_phase(Some(&msg), &mut acc, &info, &executor, &GasConfigFull::default_mc(), false, true).unwrap();
    assert_eq!(
        phase, 
        TrComputePhase::Skipped(TrComputePhaseSkipped { reason: ComputeSkipReason::NoState })
    );
}

#[test]
fn test_computing_phase_acc_uninit_extmsg_with_state() {
    let mut msg = create_ext_msg(AccountId::from_raw(vec![0x11; 32], 256));
    msg.set_state_init(create_state_init());

    //create uninitialized account
    let balance = 5000000;
    let mut acc = Account::with_address_and_ballance(
        &MsgAddressInt::with_standart(None, -1, AccountId::from_raw(vec![0x11; 32], 256)).unwrap(),
        &CurrencyCollection::with_grams(balance),
    );
    let gas_config = GasConfigFull::default_mc();
    let info = SmartContractInfo::default();
    let config = BlockchainConfig::default();
    let executor = OrdinaryTransactionExecutor::new(config);
    let (phase, _actions) = executor.compute_phase(Some(&msg), &mut acc, &info, &executor, &gas_config, false, true).unwrap();

    let mut vm_phase = TrComputePhaseVm::default();
    vm_phase.success = true;
    vm_phase.msg_state_used = true;
    vm_phase.account_activated = true;
    vm_phase.exit_code = 0;
    let used = 67u64;
    vm_phase.gas_used = used.into();
    vm_phase.gas_limit = 0.into();
    vm_phase.gas_credit = Some(10000.into());
    vm_phase.gas_fees = gas_config.flat_gas_price.into();
    vm_phase.vm_steps = 4;
    assert_eq!(phase, TrComputePhase::Vm(vm_phase));
}

#[test]
fn test_computing_phase_acc_uninit_intmsg_with_nostate() {
    let acc_balance = 5_000_000;
    let msg = create_int_msg(
        AccountId::from_raw(vec![0x11; 32], 256),
        AccountId::from_raw(vec![0x22; 32], 256),
        acc_balance,
        false,
        5,
        INTERNAL_FWD_FEE,
    );
    //create uninitialized account
    let msg_balance = 1_000_000;
    let mut acc = Account::with_address_and_ballance(
        &MsgAddressInt::with_standart(None, -1, AccountId::from_raw(vec![0x11; 32], 256)).unwrap(),
        &CurrencyCollection::with_grams(msg_balance),
    );
    let mut acc2 = acc.clone();

    let gas_config = GasConfigFull::default_mc();
    let info = SmartContractInfo::default();
    let config = BlockchainConfig::default();
    let executor = OrdinaryTransactionExecutor::new(config);
    let (phase, _actions) = executor.compute_phase(Some(&msg), &mut acc, &info, &executor, &gas_config, false, true).unwrap();

    let mut vm_phase = TrComputePhaseVm::default();
    vm_phase.success = true;
    vm_phase.msg_state_used = false;
    vm_phase.account_activated = false;
    vm_phase.exit_code = 0;
    let used = 5u64;
    vm_phase.gas_used = used.into();
    let limit = gas_config.calc_gas(msg_balance as u128);
    vm_phase.gas_limit = limit.into();
    vm_phase.gas_credit = None;
    vm_phase.gas_fees = gas_config.flat_gas_price.into();
    vm_phase.vm_steps = 1;
    assert_eq!(phase, TrComputePhase::Vm(vm_phase));

    let msg = create_int_msg(
        AccountId::from_raw(vec![0x11; 32], 256),
        AccountId::from_raw(vec![0x22; 32], 256),
        500,
        true,
        5,
        INTERNAL_FWD_FEE,
    );

    let info = SmartContractInfo::default();
    let config = BlockchainConfig::default();
    let executor = OrdinaryTransactionExecutor::new(config);
    let (phase, _actions) = executor.compute_phase(Some(&msg), &mut acc2, &info, &executor, &gas_config, false, true).unwrap();
    assert_eq!(
        phase, 
        TrComputePhase::Skipped(TrComputePhaseSkipped { reason: ComputeSkipReason::NoState })
    );
}

#[test]
fn test_computing_phase_acc_active_extmsg() {
    
    //external inbound msg for account
    let msg = create_ext_msg(AccountId::from_raw(vec![0x11; 32], 256));

    //msg just for creating account with active state
    let balance = 5000000;
    let mut ctor_msg = create_int_msg(
        AccountId::from_raw(vec![0x11; 32], 256), 
        AccountId::from_raw(vec![0x11; 32], 256),
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

    let gas_config = GasConfigFull::default_mc();
    let mut acc = Account::with_message(&ctor_msg).unwrap();
    let info = SmartContractInfo::default();
    let config = BlockchainConfig::default();
    let executor = OrdinaryTransactionExecutor::new(config);
    let (phase, _actions) = executor.compute_phase(Some(&msg), &mut acc, &info, &executor, &gas_config, false, true).unwrap();
    let mut vm_phase = TrComputePhaseVm::default();
    vm_phase.success = true;
    vm_phase.msg_state_used = false;
    vm_phase.account_activated = false;
    vm_phase.exit_code = 0;
    let used = 85u64;
    vm_phase.gas_used = used.into();
    vm_phase.gas_limit = 0.into();
    vm_phase.gas_credit = Some(10000.into());
    vm_phase.gas_fees = gas_config.flat_gas_price.into();
    vm_phase.vm_steps = 5;
    assert_eq!(phase, TrComputePhase::Vm(vm_phase));
}

fn create_account(balance: u64, address: &[u8; 32], code: SliceData, data: SliceData) -> Account {    
    //msg just for creating account with active state
    let mut ctor_msg = create_int_msg(
        AccountId::from_raw(vec![0x11; 32], 256), 
        AccountId::from_raw(address.to_vec(), 256),
        balance,
        false,
        5,
        INTERNAL_FWD_FEE,
    );

    let mut init = StateInit::default();
    init.code = Some(code.into_cell());
    init.data = Some(data.into_cell());
    ctor_msg.set_state_init(init);

    Account::with_message(&ctor_msg).unwrap()
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
    let msg = create_ext_msg(AccountId::from_raw(address.to_vec(), 256));

    let gas_config = GasConfigFull::default_mc();
    let info = SmartContractInfo::default();
    let config = BlockchainConfig::default();
    let executor = OrdinaryTransactionExecutor::new(config);
    let result = executor.compute_phase(Some(&msg), &mut acc, &info, &executor, &gas_config, false, true);
    println!("{:?}", result);
    if let Err(e) = result {
        if let Some(ExecutorError::NoAcceptError(100)) = e.downcast_ref::<ExecutorError>() {
            return;
        }
    }
    panic!("unexpected result");
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
    let msg = create_ext_msg(AccountId::from_raw(address.to_vec(), 256));

    let gas_config = GasConfigFull::default_mc();
    let info = SmartContractInfo::default();
    let config = BlockchainConfig::default();
    let executor = OrdinaryTransactionExecutor::new(config);
    let (phase, _actions) = executor.compute_phase(
        Some(&msg), &mut acc, &info, &executor, &gas_config, false, true
    ).unwrap();
    let mut vm_phase = TrComputePhaseVm::default();
    vm_phase.success = false;
    vm_phase.msg_state_used = false;
    vm_phase.account_activated = false;
    vm_phase.exit_code = 100;
    vm_phase.gas_used = 182u64.into();
    vm_phase.gas_limit = 0.into();
    vm_phase.gas_credit = Some(10000.into());
    let gas_fees = 182u64 * gas_config.get_real_gas_price();
    vm_phase.gas_fees = gas_fees.into();
    vm_phase.vm_steps = 6;
    assert_eq!(phase, TrComputePhase::Vm(vm_phase));
    assert_eq!(acc.get_balance().unwrap().grams, (balance - gas_fees).into());
}

fn call_action_phase(
    acc_balance: u64,
    out_msg_value: u64,
    must_succeded: bool,
    no_funds: bool,
    fwd_fees: u64,
    action_fees: u64,
    res: i32,
    res_arg: Option<i32>) {
    let msg = create_ext_msg(AccountId::from_raw(vec![0x11; 32], 256));

    //msg just for creating account with active state
    let mut ctor_msg = create_int_msg(
        AccountId::from_raw(vec![0x12; 32], 256), 
        AccountId::from_raw(vec![0x11; 32], 256),
        acc_balance,
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

    let mut acc = Account::with_message(&ctor_msg).unwrap();
    let mut tr = Transaction::with_account_and_message(&acc, &msg, 1).unwrap();

    let mut actions = OutActions::default();
    let msg = create_ext_out_msg(AccountId::from_raw(vec![0x11; 32], 256));
    let mut storage = StorageUsedShort::calculate_for_struct(&msg).unwrap();
    actions.push_back(OutAction::new_send(SENDMSG_ORDINARY, msg));

    let config = BlockchainConfig::default();
    let executor = OrdinaryTransactionExecutor::new(config.clone());
    let fwd_prices = config.get_fwd_prices(&ctor_msg);
    let msg_fwd_fees = fwd_prices.lump_price - fwd_prices.calc_mine_fee(fwd_prices.lump_price as u128) as u64;
    let mut msg = create_int_msg(
        AccountId::from_raw(vec![0x22; 32], 256),
        AccountId::from_raw(vec![0x22; 32], 256),
        out_msg_value,
        true,
        5,
        //INTERNAL_FWD_FEE
        msg_fwd_fees,
    );
    actions.push_back(OutAction::new_send(SENDMSG_ORDINARY, msg.clone()));
    if must_succeded {
        msg.get_value_mut().unwrap().grams = (out_msg_value - fwd_prices.lump_price).into();
        storage.append(&msg.write_to_new_cell().unwrap().into());
    }

    let msg = create_ext_out_msg(AccountId::from_raw(vec![0x11; 32], 256));
    if must_succeded { storage.append(&msg.write_to_new_cell().unwrap().into()); }
    actions.push_back(OutAction::new_send(SENDMSG_ORDINARY, msg));
    let actions_hash = actions.hash().unwrap();

    let tr_lt = Arc::new(AtomicU64::new(2));
    let phase = executor.action_phase(&mut tr, &mut acc, Some(actions.write_to_new_cell().unwrap().into()), &config, tr_lt, false).unwrap();
    let mut phase2 = TrActionPhase::default();
    phase2.success = must_succeded;
    phase2.valid = true;
    phase2.no_funds = no_funds;
    phase2.msgs_created = if must_succeded {3} else {1};
    phase2.tot_actions = 3;
    phase2.status_change = AccStatusChange::Unchanged;
    phase2.action_list_hash = actions_hash;
    phase2.total_fwd_fees = Some(fwd_fees.into());
    phase2.total_action_fees = Some(action_fees.into());
    phase2.result_code = res;
    phase2.result_arg = res_arg;
    phase2.tot_msg_size = storage;
    assert_eq!(phase, phase2);
    let balance = if no_funds {acc_balance} else {acc_balance - out_msg_value - 2 * fwd_prices.lump_price};
    println!("{} {}", balance, acc.get_balance().unwrap().grams);
    assert_eq!(acc.get_balance().unwrap().grams, balance.into());
}

#[test]
fn test_action_phase_active_acc_with_actions_nofunds() {
    let fwd_config = MsgForwardPrices::default_mc();
    let fwd_fee = fwd_config.lump_price;
    call_action_phase(200, 300, false, true, fwd_fee, fwd_fee, RESULT_CODE_NOT_ENOUGH_GRAMS, Some(1));
}

#[test]
fn test_action_phase_active_acc_with_actions_success() {
    let fwd_config = MsgForwardPrices::default_mc();
    let fwd_fee = fwd_config.lump_price * 3;
    let mine_fee = fwd_config.lump_price * 2 + fwd_config.calc_mine_fee(fwd_config.lump_price as u128) as u64;
    call_action_phase(5000000000, 100000000, true, false, fwd_fee, mine_fee, 0, None);
}


#[test]
fn test_gas_init1() {
    let gas_test = init_gas(15000000, 0, true, false, true, &GasConfigFull::default_wc());
    let gas_etalon = Gas::new(0, 10000, 1000000, 1000);
    assert_eq!(gas_test, gas_etalon);
}

#[test]
fn test_gas_init2() {
    let gas_test = init_gas(4000000, 0, true, false, true, &GasConfigFull::default_wc());
    let gas_etalon = Gas::new(0, 10000, 1000000, 1000);
    assert_eq!(gas_test, gas_etalon);
}

#[test]
fn test_gas_init3() {
    let gas_test = init_gas(10000000, 100000, false, false, true, &GasConfigFull::default_wc());
    let gas_etalon = Gas::new(100, 0, 1000000, 1000);
    assert_eq!(gas_test, gas_etalon);
}

#[test]
fn test_gas_init4() {
    let gas_test = init_gas(1000000000_000_000_000, 1000_000_000_000, false, false, true, &GasConfigFull::default_wc());
    let gas_etalon = Gas::new(1000000, 0, 1000000, 1000);
    assert_eq!(gas_test, gas_etalon);
}


mod actions {
    use super::*;
    use pretty_assertions::assert_eq;
    use ton_block::AddSub;

    #[test]
    fn test_reserve_exactly() {
        let value = CurrencyCollection::with_grams(123);
        let mut balance = CurrencyCollection::with_grams(500);
        let mut remaining_balance = balance.clone();
        let result = reserve_action_handler(0, &value, &mut remaining_balance);
        balance.sub(&value).unwrap();
        assert_eq!(result, Ok(value));
        assert_eq!(remaining_balance, balance);
    }

    #[test]
    fn test_reserve_no_funds() {
        let value = CurrencyCollection::with_grams(123);
        let balance = CurrencyCollection::with_grams(100);
        let mut remaining_balance = balance.clone();
        let result = reserve_action_handler(0, &value, &mut remaining_balance);
        assert_eq!(result, Err(RESULT_CODE_NOT_ENOUGH_GRAMS));
        assert_eq!(remaining_balance, balance);

        let result = reserve_action_handler(RESERVE_ALL_BUT, &value, &mut remaining_balance);
        assert_eq!(result, Err(RESULT_CODE_NOT_ENOUGH_GRAMS));
        assert_eq!(remaining_balance, balance);
    }

    #[test]
    fn test_reserve_wrong_mode() {
        let value = CurrencyCollection::with_grams(123);
        let balance = CurrencyCollection::with_grams(100);
        let mut remaining_balance = balance.clone();
        let result = reserve_action_handler(4, &value, &mut remaining_balance);
        assert_eq!(result, Err(RESULT_CODE_UNSUPPORTED));
        assert_eq!(remaining_balance, balance);
    }


    #[test]
    fn test_reserve_all_but() {
        let value = CurrencyCollection::with_grams(123);
        let balance = CurrencyCollection::with_grams(500);
        let mut reserved = balance.clone();
        reserved.sub(&value).unwrap();
        let mut remaining_balance = balance.clone();
        let result = reserve_action_handler(RESERVE_ALL_BUT, &value, &mut remaining_balance);
        assert_eq!(result, Ok(reserved));
        assert_eq!(remaining_balance, value);
    }

    #[test]
    fn test_reserve_allbut_skip_error() {
        let value = CurrencyCollection::with_grams(123);
        let balance = CurrencyCollection::with_grams(100);
        let mut remaining_balance = balance.clone();
        let result = reserve_action_handler(RESERVE_IGNORE_ERROR | RESERVE_ALL_BUT, &value, &mut remaining_balance);
        assert_eq!(result, Ok(balance));
        assert_eq!(remaining_balance, CurrencyCollection::with_grams(0));
    }

     #[test]
    fn test_reserve_exactly_skip_error() {
        let value = CurrencyCollection::with_grams(123);
        let balance = CurrencyCollection::with_grams(100);
        let mut remaining_balance = balance.clone();
        let result = reserve_action_handler(RESERVE_IGNORE_ERROR, &value, &mut remaining_balance);
        assert_eq!(result, Ok(balance));
        assert_eq!(remaining_balance, CurrencyCollection::with_grams(0));
    }
   
    fn test_sendmsg_action(mode: u8, val: u64, bal: u64, lt: Arc<AtomicU64>, fwd_fee: u64, mine_fee: u64, error: Option<i32>) {
        let mut balance = CurrencyCollection::with_grams(bal);
        let mut remaining_balance = balance.clone();
        let mut phase = TrActionPhase::default();
        phase.total_fwd_fees = Some(3u64.into());
        phase.total_action_fees = Some(5u64.into());
        let address = MsgAddressInt::with_standart(None, -1, AccountId::from_raw(vec![0x11; 32], 256)).unwrap();
        let mut msg = create_int_msg(AccountId::from_raw(vec![0x22; 32], 256), AccountId::from_raw(vec![0x22; 32], 256), val, false, 0, fwd_fee);
        
        let msg_lt = lt.load(Ordering::SeqCst);
        msg.set_at_and_lt(0, msg_lt);
        msg.set_src(MsgAddressIntOrNone::Some(address.clone()));
        let res = outmsg_action_handler(
            &mut phase,
            mode,
            &mut msg,
            &mut remaining_balance,
            &BlockchainConfig::default(),
            false
        );

        let mut res_val = CurrencyCollection::with_grams(val.clone());
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
        assert_eq!(remaining_balance, balance);

        let valid_address = match msg.header() {
            CommonMsgInfo::IntMsgInfo(ref header) => header.src.clone(),
            _ => panic!("must be internal msg"),
        };
        assert_eq!(valid_address, MsgAddressIntOrNone::Some(address));

        assert_eq!(msg.at_and_lt().unwrap(), (0, msg_lt));
        assert_eq!(msg.get_fee().unwrap(), Some((fwd_fee - mine_fee).into()));

        res_val.sub(&CurrencyCollection::with_grams(fwd_fee)).unwrap();
        assert_eq!(msg.get_value().unwrap().clone(), res_val);

        let mut total_fwd_fees = Grams::default();
        total_fwd_fees.add(&3u64.into()).unwrap();
        total_fwd_fees.add(&fwd_fee.into()).unwrap();
        assert_eq!(phase.total_fwd_fees, Some(total_fwd_fees));

        let mut total_action_fees = Grams::default();
        total_action_fees.add(&5u64.into()).unwrap();
        total_action_fees.add(&mine_fee.into()).unwrap();
        assert_eq!(phase.total_action_fees, Some(total_action_fees));
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
}
