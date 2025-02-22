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

use std::sync::{atomic::AtomicU64, Arc};

use pretty_assertions::assert_eq;
use ever_block::{
    CurrencyCollection, GetRepresentationHash, Grams, Serializable, StateInit, StorageUsedShort, ConfigParamEnum, ConfigParam8, GlobalCapabilities, MsgAddressInt,
    messages::CommonMsgInfo,
    out_actions::{OutAction, OutActions, SENDMSG_ORDINARY, SET_LIB_CODE_ADD_PRIVATE},
    transactions::{
        AccStatusChange, TrActionPhase, TrComputePhase, TrComputePhaseVm, TrCreditPhase,
        TrStoragePhase, Transaction, TransactionDescr,
    },
    AccountId, BuilderData, Cell, HashmapE, SliceData, Status, UInt256
};
use ever_assembler::compile_code_to_cell;

use super::*;

mod common;
use common::*;
use crate::ordinary_transaction::tests2::common::cross_check::disable_cross_check;

fn create_test_code_with_using_libs() -> Cell {
    compile_code_to_cell("
        ACCEPT
        PUSHROOT
        CTOS
        LDREF
        PLDREF
        PUSHINT 0
        SENDRAWMSG
        PUSHINT 0
        SENDRAWMSG
        NEWC
        PUSHINT 2
        STUR 8
        PUSHSLICE xd816dc4ba685aed03aacac298a2beb6bcd67241e35ddcf39c4020c7430b3cf8f
        STSLICER
        TRUE
        ENDXC
        CTOS
        BLESS
        POP C0
    ").unwrap()
}

#[test]
fn test_trexecutor_active_acc_with_code_with_libs_in_msg() {
    cross_check::disable_cross_check(); // libraries are not supported thoroughly

    let used = 2368u32; //gas units
    let storage_fees = 314966987;
    let msg_mine_fee = MSG_MINE_FEE;
    let msg_fwd_fee = MSG_FWD_FEE;
    let msg_remain_fee = msg_fwd_fee - msg_mine_fee;
    let gas_fees = used as u64 * 10000;
    let gas_limit = 12300u32;
    let msg_income = 123000000u64;

    let acc_id = AccountId::from([0x11; 32]);
    let start_balance = 2000000000u64;
    let mut acc = create_test_account(start_balance, acc_id.clone(), create_test_code_with_using_libs(), create_two_messages_data());
    // balance - (balance of 2 output messages + input msg fee + storage_fee + gas_fee)
    let end_balance = start_balance + msg_income - (150000000 + storage_fees + gas_fees);
    let mut new_acc = create_test_account(end_balance, acc_id, create_test_code_with_using_libs(), create_two_messages_data());
    let mut msg = create_int_msg(
        AccountId::from([0x22; 32]),
        AccountId::from([0x11; 32]),
        msg_income,
        false,
        BLOCK_LT - 1_000_000 + 1,
    );
    let mut state_init = StateInit::default();
    state_init.set_library_code(BuilderData::with_raw(vec![0x72], 8).unwrap().into_cell().unwrap(), true).unwrap();
    msg.set_state_init(state_init);
    let tr_lt = BLOCK_LT + 1;
    new_acc.set_last_tr_time(tr_lt + 3);

    let config = custom_config(None, Some(GlobalCapabilities::CapSetLibCode as u64));
    let acc_before = acc.clone();
    let trans = execute_with_params(config, &msg, &mut acc, tr_lt);
    check_account_and_transaction(&acc_before, &acc, &msg, trans.as_ref().ok(), 1634353013, 2);
    let trans = trans.unwrap();

    new_acc.set_last_paid(acc.last_paid());

    let mut good_trans = Transaction::with_account_and_message(&acc, &msg, tr_lt).unwrap();
    good_trans.set_now(BLOCK_UT);

    let (mut msg1, mut msg2) = create_two_internal_messages();
    let mut actions = OutActions::default();
    actions.push_back(OutAction::new_send(SENDMSG_ORDINARY, msg1.clone()));
    actions.push_back(OutAction::new_send(SENDMSG_ORDINARY, msg2.clone()));
    if let CommonMsgInfo::IntMsgInfo(int_header) = msg1.header_mut() {
        if let CommonMsgInfo::IntMsgInfo(int_header2) = msg2.header_mut() {
            int_header.value.grams = Grams::from(MSG1_BALANCE - msg_fwd_fee);
            int_header2.value.grams = Grams::from(MSG2_BALANCE - msg_fwd_fee);
            int_header.fwd_fee = msg_remain_fee.into();
            int_header2.fwd_fee = msg_remain_fee.into();
            int_header.created_at = BLOCK_UT.into();
            int_header2.created_at = BLOCK_UT.into();
        }
    }
    let msg1 = make_common(msg1);
    let msg2 = make_common(msg2);
    good_trans.add_out_message(&msg1).unwrap();
    good_trans.add_out_message(&msg2).unwrap();
    good_trans.set_total_fees((storage_fees + gas_fees + msg_mine_fee * 2).into());

    let mut description = TransactionDescrOrdinary::default();
    description.storage_ph = Some(TrStoragePhase::with_params(storage_fees.into(), None, AccStatusChange::Unchanged));

    description.credit_ph = Some(TrCreditPhase {
        due_fees_collected: None,
        credit: CurrencyCollection::with_grams(msg_income),
    });

    let mut vm_phase = TrComputePhaseVm::default();
    vm_phase.success = true;
    vm_phase.msg_state_used = false;
    vm_phase.account_activated = false;
    vm_phase.gas_used = used.into();
    vm_phase.gas_limit = gas_limit.into();
    vm_phase.gas_credit = None;
    vm_phase.gas_fees = gas_fees.into();
    vm_phase.vm_steps = 22;
    description.compute_ph = TrComputePhase::Vm(vm_phase);

    let mut action_ph = TrActionPhase::default();
    action_ph.success = true;
    action_ph.valid = true;
    action_ph.status_change = AccStatusChange::Unchanged;
    action_ph.tot_actions = 2;
    action_ph.msgs_created = 2;
    action_ph.add_fwd_fees((2 * msg_fwd_fee).into());
    action_ph.add_action_fees((2 * msg_mine_fee).into());
    action_ph.action_list_hash = actions.hash().unwrap();
    action_ph.tot_msg_size = StorageUsedShort::calculate_for_struct(&msg1).unwrap();
    action_ph.tot_msg_size.append(&msg2.serialize().unwrap());

    description.action = Some(action_ph);
    description.credit_first = true;
    description.bounce = None;
    description.aborted = false;
    description.destroyed = false;
    good_trans.write_description(&TransactionDescr::Ordinary(description)).unwrap();

    assert_eq!(
        trans.read_description().unwrap().compute_phase_ref().unwrap().clone().get_vmphase_mut().unwrap().gas_used,
        good_trans.read_description().unwrap().compute_phase_ref().unwrap().clone().get_vmphase_mut().unwrap().gas_used);

    assert_eq!(trans.read_description().unwrap(), good_trans.read_description().unwrap());

    assert_eq!(acc, new_acc);
    trans.out_msgs.scan_diff(&good_trans.out_msgs, |key: ever_block::U15, msg1, msg2| {
        assert_eq!(msg1, msg2, "for key {}", key.0);
        Ok(true)
    }).unwrap();
    assert_eq!(trans, good_trans);
}

#[test]
fn test_trexecutor_active_acc_with_code_with_libs_in_state() {
    let used = 2368u32; //gas units
    let storage_fees = 314966987;
    let msg_mine_fee = MSG_MINE_FEE;
    let msg_fwd_fee = MSG_FWD_FEE;
    let msg_remain_fee = msg_fwd_fee - msg_mine_fee;
    let gas_fees = used as u64 * 10000;
    let gas_credit = 10000u16;

    let acc_id = AccountId::from([0x11; 32]);
    let start_balance = 2000000000u64;
    let mut acc = create_test_account(start_balance, acc_id.clone(), create_test_code_with_using_libs(), create_two_messages_data());
    // balance - (balance of 2 output messages + input msg fee + storage_fee + gas_fee)
    let end_balance = start_balance - (150000000 + msg_fwd_fee + storage_fees + gas_fees);
    let mut new_acc = create_test_account(end_balance, acc_id, create_test_code_with_using_libs(), create_two_messages_data());
    let msg = create_test_external_msg();
    let tr_lt = BLOCK_LT + 1;
    let lt = Arc::new(AtomicU64::new(tr_lt));
    new_acc.set_last_tr_time(tr_lt + 3);

    let lib_code = BuilderData::with_raw(vec![0x72], 8).unwrap().into_cell().unwrap();
    let mut state_lib = HashmapE::with_bit_len(256);
    state_lib.setref(lib_code.repr_hash().into(), &lib_code).unwrap();
    let config = custom_config(None, Some(GlobalCapabilities::CapSetLibCode as u64));
    let executor = OrdinaryTransactionExecutor::new(config);
    let acc_before = acc.clone();
    let trans = executor.execute_with_params(Some(&make_common(msg.clone())), &mut acc, execute_params(lt, state_lib, UInt256::default(), 0)).unwrap();
    check_account_and_transaction(&acc_before, &acc, &msg, Some(&trans), end_balance, 2);
    new_acc.set_last_paid(acc.last_paid());

    let mut good_trans = Transaction::with_account_and_message(&acc, &msg, tr_lt).unwrap();
    good_trans.set_now(BLOCK_UT);

    let (mut msg1, mut msg2) = create_two_internal_messages();
    let mut actions = OutActions::default();
    actions.push_back(OutAction::new_send(SENDMSG_ORDINARY, msg1.clone()));
    actions.push_back(OutAction::new_send(SENDMSG_ORDINARY, msg2.clone()));
    if let CommonMsgInfo::IntMsgInfo(int_header) = msg1.header_mut() {
        if let CommonMsgInfo::IntMsgInfo(int_header2) = msg2.header_mut() {
            int_header.value.grams = Grams::from(MSG1_BALANCE - msg_fwd_fee);
            int_header2.value.grams = Grams::from(MSG2_BALANCE - msg_fwd_fee);
            int_header.fwd_fee = msg_remain_fee.into();
            int_header2.fwd_fee = msg_remain_fee.into();
            int_header.created_at = BLOCK_UT.into();
            int_header2.created_at = BLOCK_UT.into();
        }
    }
    good_trans.add_out_message(&make_common(msg1.clone())).unwrap();
    good_trans.add_out_message(&make_common(msg2.clone())).unwrap();
    good_trans.set_total_fees((msg_fwd_fee + storage_fees + gas_fees + msg_mine_fee * 2).into());

    let mut description = TransactionDescrOrdinary::default();
    description.storage_ph = Some(TrStoragePhase::with_params(storage_fees.into(), None, AccStatusChange::Unchanged));
    description.credit_ph = None;

    let mut vm_phase = TrComputePhaseVm::default();
    vm_phase.success = true;
    vm_phase.msg_state_used = false;
    vm_phase.account_activated = false;
    vm_phase.gas_used = used.into();
    vm_phase.gas_limit = 0u32.into();
    vm_phase.gas_credit = Some(gas_credit.into());
    vm_phase.gas_fees = gas_fees.into();
    vm_phase.vm_steps = 22;
    description.compute_ph = TrComputePhase::Vm(vm_phase);

    let mut action_ph = TrActionPhase::default();
    action_ph.success = true;
    action_ph.valid = true;
    action_ph.status_change = AccStatusChange::Unchanged;
    action_ph.tot_actions = 2;
    action_ph.msgs_created = 2;
    action_ph.add_fwd_fees((2 * msg_fwd_fee).into());
    action_ph.add_action_fees((2 * msg_mine_fee).into());
    action_ph.action_list_hash = actions.hash().unwrap();
    action_ph.tot_msg_size = StorageUsedShort::calculate_for_struct(&msg1).unwrap();
    action_ph.tot_msg_size.append(&msg2.serialize().unwrap());

    description.action = Some(action_ph);
    description.credit_first = true;
    description.bounce = None;
    description.aborted = false;
    description.destroyed = false;
    good_trans.write_description(&TransactionDescr::Ordinary(description)).unwrap();

    assert_eq!(
        trans.read_description().unwrap().compute_phase_ref().unwrap().clone().get_vmphase_mut().unwrap().gas_used,
        good_trans.read_description().unwrap().compute_phase_ref().unwrap().clone().get_vmphase_mut().unwrap().gas_used);

    assert_eq!(trans.read_description().unwrap(), good_trans.read_description().unwrap());

    assert_eq!(acc, new_acc);
    trans.out_msgs.scan_diff(&good_trans.out_msgs, |key: ever_block::U15, msg1, msg2| {
        assert_eq!(msg1, msg2, "for key {}", key.0);
        Ok(true)
    }).unwrap();
    assert_eq!(trans, good_trans);
}

#[test]
fn set_library_test() {
    // library code and data
    let new_code = "ACCEPT";
    let mut state = StateInit::default();
    state.set_code(compile_code_to_cell(new_code).unwrap());

    let code = "
        ACCEPT
        PUSHCTR C4
        PUSHINT 1
        SETLIBCODE
    ";

    let code = compile_code_to_cell(code).unwrap();
    let start_balance = 1_000_000_000;

    let acc_id = AccountId::from([0x11; 32]);
    let mut acc = create_test_account(start_balance, acc_id.clone(), code, state.serialize().unwrap());
    let msg = create_int_msg(
        AccountId::from([0x33; 32]),
        acc_id,
        20_000_000,
        false,
        BLOCK_LT - 2
    );

    let tr_lt = BLOCK_LT + 1;
    let lt = Arc::new(AtomicU64::new(tr_lt));
    let executor = OrdinaryTransactionExecutor::new(
        custom_config(
            None,
            Some(GlobalCapabilities::CapSetLibCode as u64)
        )
    );
    assert_eq!(acc.libraries().len().unwrap(), 0);
    let params = execute_params_none(lt);
    let trans = executor.execute_with_params(Some(&make_common(msg)), &mut acc, params).unwrap();
    assert_eq!(trans.out_msgs.len().unwrap(), 0);
    assert_eq!(acc.libraries().len().unwrap(), 1);

    /*
    let code = format!("
        ACCEPT
        PUSHCTR C4
        PUSHINT 0
        SETLIBCODE
    ");
    acc.set_code(compile_code_to_cell(code).unwrap());
    let trans = executor.execute_for_account(Some(&msg), &mut acc, HashmapE::default(), BLOCK_UT, BLOCK_LT, lt, true).unwrap();
    assert_eq!(acc.libraries().len().unwrap(), 0);
    */
}

#[ignore]
#[test]
fn set_ext_library_test() {
    // library code and data
    let lib_code = BuilderData::with_raw(vec![0x72], 8).unwrap().into_cell().unwrap();
    let mut state_lib = HashmapE::with_bit_len(256);
    state_lib.setref(lib_code.repr_hash().into(), &lib_code).unwrap();

    let code = format!("
        ACCEPT
        PUSHCTR C4
        HASHCU
        PUSHINT {}
        CHANGELIB
    ", SET_LIB_CODE_ADD_PRIVATE);
    dbg!(format!("UUU {} {}", lib_code.repr_hash(), code));

    let code = compile_code_to_cell(&code).unwrap();
    let start_balance = 1_000_000_000;

    let acc_id = AccountId::from([0x11; 32]);
    let mut acc = create_test_account(start_balance, acc_id.clone(), code, lib_code);
    let msg = create_int_msg(
        AccountId::from([0x33; 32]),
        acc_id,
        20_000_000,
        false,
        BLOCK_LT - 2
    );

    let tr_lt = BLOCK_LT + 1;
    let lt = Arc::new(AtomicU64::new(tr_lt));
    let executor = OrdinaryTransactionExecutor::new(BLOCKCHAIN_CONFIG.to_owned());
    assert_eq!(acc.libraries().len().unwrap(), 0);
    let params = execute_params(lt, state_lib, UInt256::default(), 0);
    let trans = executor.execute_with_params(Some(&make_common(msg)), &mut acc, params).unwrap();
    assert_eq!(trans.out_msgs.len().unwrap(), 0);
    assert_eq!(acc.libraries().len().unwrap(), 1);
}

#[test]
fn set_code_test() {
    let code = "
        ACCEPT
        PUSHCTR C4
        SETCODE
    ";
    let new_code = "
        ACCEPT
        PUSHCTR C4
        PUSHINT 0
        SENDRAWMSG
    ";
    let code = compile_code_to_cell(code).unwrap();
    let new_code = compile_code_to_cell(new_code).unwrap();
    let start_balance = 500_000_000;

    let acc_id = AccountId::from([0x11; 32]);
    let mut acc = create_test_account(start_balance, acc_id.clone(), code, new_code);
    let msg = create_int_msg(
        AccountId::from([0x33; 32]),
        acc_id,
        20_000_000,
        false,
        BLOCK_LT - 2
    );

    // set new code send tx
    let tr_lt = BLOCK_LT + 1;
    execute_c(&msg, &mut acc, tr_lt, 400404216, 0).unwrap();

    // set new data for code send tx
    let out_msg = create_int_msg(
        AccountId::from([0x11; 32]),
        AccountId::from([0x33; 32]),
        11_000_000,
        false,
        BLOCK_LT + 1
    );
    let data = out_msg.serialize().unwrap();
    acc.set_data(data);

    // run send tx code
    execute_c(&msg, &mut acc, tr_lt, 403394216, 1).unwrap();
}

#[test]
fn set_code_unsuccess_test() {
    let code = "
        ACCEPT
        PUSHCTR C4
        SETCODE
        PUSHCTR C4
        SETCODE
        PUSHCTR C4
        SETCODE
        PUSHCTR C4
        SETCODE
        PUSHCTR C4
        SETCODE
        PUSHCTR C4
        SETCODE
        PUSHCTR C4
        SETCODE
        PUSHINT 1000000000
        PUSHINT 0
        RAWRESERVE
    ";
    let new_code = "
        ACCEPT
        PUSHCTR C4
        PUSHINT 0
        SENDRAWMSG
    ";
    let code = compile_code_to_cell(code).unwrap();
    let new_code = compile_code_to_cell(new_code).unwrap();
    let start_balance = 500_000_000;

    let acc_id = AccountId::from([0x11; 32]);
    let mut acc = create_test_account(start_balance, acc_id.clone(), code.clone(), new_code);
    let msg = create_int_msg(
        AccountId::from([0x33; 32]),
        acc_id,
        20_000_000,
        false,
        BLOCK_LT - 2
    );

    // set new code send tx
    let tr_lt = BLOCK_LT + 1;
    execute_c(&msg, &mut acc, tr_lt, 344060641, 0).unwrap();
    assert_eq!(acc.get_code().unwrap(), code);
}

#[test]
fn my_code() {
    let code = "
        ACCEPT
        MYCODE
        CTOS
        SBITS
        THROWIF 62
    ";
    let start_balance = 200_000_000;

    let acc_id = AccountId::from([0x11; 32]);
    let in_msg = create_int_msg(
        acc_id.clone(),
        acc_id.clone(),
        10_000_000,
        false,
        BLOCK_LT - 2
    );
    let code = compile_code_to_cell(code).unwrap();
    let data = in_msg.serialize().unwrap();

    let mut acc = create_test_account(start_balance, acc_id.clone(), code, data);

    let msg = create_int_msg(
        AccountId::from([0x11; 32]),
        acc_id,
        14_200_000,
        false,
        BLOCK_LT - 2
    );

    let mut config = read_config().unwrap();
    let mut param8 = ConfigParam8 {
        global_version: config.get_global_version().unwrap()
    };
    param8.global_version.capabilities |= GlobalCapabilities::CapMycode as u64;
    config.set_config(ConfigParamEnum::ConfigParam8(param8)).unwrap();
    let blockchain_config = BlockchainConfig::with_config(config).unwrap();

    let acc_before = acc.clone();
    disable_cross_check();
    let trans = execute_with_params(blockchain_config, &msg, &mut acc, BLOCK_LT + 1);
    check_account_and_transaction_balances(&acc_before, &acc, &msg, None);
    let trans = trans.unwrap();

    if let TrComputePhase::Vm(vm) = trans.read_description().unwrap().compute_phase_ref().unwrap() {
        assert_eq!(vm.exit_code, 62);
    } else {
        unreachable!()
    }
}

#[test]
fn test_init_code_hash() {
    let code = "
        ACCEPT
        PUSHROOT
        CTOS
        LDU 256
        DROP
        INITCODEHASH
        CMP
        THROWIFNOT 62
    ";
    let code = compile_code_to_cell(code).unwrap();
    let codehash = code.repr_hash();
    let data = codehash.serialize().unwrap();

    let mut acc = Account::default();

    let mut state_init = StateInit::default();
    state_init.set_code(code);
    state_init.set_data(data);

    let acc_id = AccountId::from(state_init.hash().unwrap());
    let mut msg = create_int_msg(
        AccountId::from([0x11; 32]),
        acc_id,
        140_200_000,
        false,
        BLOCK_LT - 2
    );
    msg.set_state_init(state_init);

    let mut config = read_config().unwrap();
    let mut param8 = ConfigParam8 {
        global_version: config.get_global_version().unwrap()
    };
    param8.global_version.capabilities |= GlobalCapabilities::CapInitCodeHash as u64;
    config.set_config(ConfigParamEnum::ConfigParam8(param8)).unwrap();
    let blockchain_config = BlockchainConfig::with_config(config).unwrap();

    let acc_before = acc.clone();
    disable_cross_check();
    let trans = execute_with_params(blockchain_config, &msg, &mut acc, BLOCK_LT + 1);
    check_account_and_transaction_balances(&acc_before, &acc, &msg, None);
    let trans = trans.unwrap();

    if let TrComputePhase::Vm(vm) = trans.read_description().unwrap().compute_phase_ref().unwrap() {
        assert_eq!(vm.exit_code, 62);
    } else {
        unreachable!()
    }
}

fn storage_fee_instruction(start_balance: u64, begin_due: u64, bounce: bool, storage_phase_due: bool) {
    let storage_fee_collected = 158514102;
    let code = format!("
        ACCEPT
        STORAGEFEE
        PUSHINT {}
        CMP
        THROWIFNOT 62
    ", storage_fee_collected);

    let acc_id = AccountId::from([0x11; 32]);
    let in_msg = create_int_msg(
        acc_id.clone(),
        acc_id.clone(),
        10_000_000,
        false,
        BLOCK_LT - 2
    );
    let code = compile_code_to_cell(&code).unwrap();
    let data = in_msg.serialize().unwrap();

    let mut acc = create_test_account(start_balance, acc_id.clone(), code, data);
    acc.set_due_payment(Some(Grams::from(begin_due)));

    let msg = create_int_msg(
        AccountId::from([0x11; 32]),
        acc_id,
        200_000_000,
        bounce,
        BLOCK_LT - 2
    );

    let mut config = read_config().unwrap();
    let mut param8 = ConfigParam8 {
        global_version: config.get_global_version().unwrap()
    };
    param8.global_version.capabilities |= GlobalCapabilities::CapStorageFeeToTvm as u64;
    config.set_config(ConfigParamEnum::ConfigParam8(param8)).unwrap();
    let blockchain_config = BlockchainConfig::with_config(config).unwrap();

    let acc_before = acc.clone();
    disable_cross_check();
    let trans = execute_with_params(blockchain_config, &msg, &mut acc, BLOCK_LT + 1);
    check_account_and_transaction_balances(&acc_before, &acc, &msg, None);
    let trans = trans.unwrap();

    if let TrComputePhase::Vm(vm) = trans.read_description().unwrap().compute_phase_ref().unwrap() {
        assert_eq!(vm.exit_code, 62);
    } else {
        panic!("wrong transaction desciption")
    }

    let storage_ph = get_tr_descr(&trans).storage_ph.unwrap();
    assert_eq!(storage_ph.storage_fees_due.is_some(), storage_phase_due);
    assert_eq!(
        Grams::from(storage_fee_collected),
        storage_ph.storage_fees_collected + storage_ph.storage_fees_due.unwrap_or_default() - if bounce {begin_due as u128} else {0}
    );
}

#[test]
fn test_storage_fee_instruction() {
    storage_fee_instruction(200_000_000, 0, false, false);
    storage_fee_instruction(200_000_000, 0, true, false);

    storage_fee_instruction(141782075, 0, false, false);
    storage_fee_instruction(141782075, 0, true, true);

    storage_fee_instruction(60000000, 1000, false, false);
    storage_fee_instruction(60000000, 1000, true, true);
}

#[test]
fn account_uninit_with_libs_disabled() {
    disable_cross_check();

    let mut state_init = StateInit::default();
    state_init.set_code(compile_code_to_cell("NOP").unwrap());
    state_init.set_library_code(BuilderData::with_raw(vec![0x72], 8).unwrap().into_cell().unwrap(), true).unwrap();

    let acc_id = AccountId::from(state_init.hash().unwrap());

    let mut acc = Account::uninit(MsgAddressInt::with_standart(
        None,
        -1,
        acc_id.clone()
    ).unwrap(), 10, 10, CurrencyCollection::with_grams(10000000000000000000));

    let mut msg = create_int_msg(
        AccountId::from([0x33; 32]),
        acc_id,
        14_200_000,
        false,
        BLOCK_LT - 2
    );
    msg.set_state_init(state_init);

    let mut acc_copy = acc.clone();
    execute_c(&msg, &mut acc_copy, BLOCK_LT + 1, 9999999984738712408u64, 0).unwrap();
    assert_eq!(acc_copy.status(), AccountStatus::AccStateUninit);

    let config = custom_config(None, Some(GlobalCapabilities::CapSetLibCode as u64));
    let acc_before = acc.clone();
    let trans = execute_with_params(config, &msg, &mut acc, BLOCK_LT + 1);
    check_account_and_transaction(&acc_before, &acc, &msg, trans.as_ref().ok(), 9999999984737712408u64, 0);
    trans.unwrap();
    assert_eq!(acc.status(), AccountStatus::AccStateActive);
}

#[test]
fn account_nonexist_with_libs_disabled() {
    disable_cross_check();

    let mut state_init = StateInit::default();
    state_init.set_code(compile_code_to_cell("NOP").unwrap());
    state_init.set_library_code(BuilderData::with_raw(vec![0x72], 8).unwrap().into_cell().unwrap(), true).unwrap();

    let acc_id = AccountId::from(state_init.hash().unwrap());

    let mut acc = Account::default();

    let mut msg = create_int_msg(
        AccountId::from([0x33; 32]),
        acc_id,
        14_200_000_000u64,
        false,
        BLOCK_LT - 2
    );
    msg.set_state_init(state_init);

    let mut acc_copy = acc.clone();
    execute_c(&msg, &mut acc_copy, BLOCK_LT + 1, 14200000000u64, 0).unwrap();
    assert_eq!(acc_copy.status(), AccountStatus::AccStateUninit);

    let config = custom_config(None, Some(GlobalCapabilities::CapSetLibCode as u64));
    let acc_before = acc.clone();
    let trans = execute_with_params(config, &msg, &mut acc, BLOCK_LT + 1);
    check_account_and_transaction(&acc_before, &acc, &msg, trans.as_ref().ok(), 14199000000u64, 0);
    trans.unwrap();
    assert_eq!(acc.status(), AccountStatus::AccStateActive);
}

#[test]
fn account_frozen_with_libs_disabled() {
    disable_cross_check();

    let mut state_init = StateInit::default();
    state_init.set_code(compile_code_to_cell("NOP").unwrap());
    state_init.set_library_code(BuilderData::with_raw(vec![0x72], 8).unwrap().into_cell().unwrap(), true).unwrap();

    let frozen_hash = state_init.serialize().unwrap().repr_hash();
    let acc_id = SliceData::from(state_init.hash().unwrap());
    let mut acc = Account::frozen(
        MsgAddressInt::with_standart(None, -1, acc_id.clone()).unwrap(),
        BLOCK_LT + 2, BLOCK_UT,
        frozen_hash,
        Some(136774881u64.into()),
        CurrencyCollection::default()
    );

    let mut msg = create_int_msg(
        AccountId::from([0x33; 32]),
        acc_id,
        14_200_000_000u64,
        false,
        BLOCK_LT - 2
    );
    msg.set_state_init(state_init);

    let mut acc_copy = acc.clone();
    execute_c(&msg, &mut acc_copy, BLOCK_LT + 1, 14063225119u64, 0).unwrap();
    assert_eq!(acc_copy.status(), AccountStatus::AccStateFrozen);

    let config = custom_config(None, Some(GlobalCapabilities::CapSetLibCode as u64));
    let acc_before = acc.clone();
    let trans = execute_with_params(config, &msg, &mut acc, BLOCK_LT + 1);
    check_account_and_transaction(&acc_before, &acc, &msg, trans.as_ref().ok(), 14062225119u64, 0);
    trans.unwrap();
    assert_eq!(acc.status(), AccountStatus::AccStateActive);
}

#[test]
fn test_simple_account_with_libs() {
    cross_check::disable_cross_check(); // libraries are not supported thoroughly

    let used = 1307u32; //gas units
    let storage_fees = 288370662;
    let msg_mine_fee = MSG_MINE_FEE;
    let msg_fwd_fee = MSG_FWD_FEE;
    let msg_remain_fee = msg_fwd_fee - msg_mine_fee;
    let gas_fees = used as u64 * 10000;
    let gas_limit = 12300u32;
    let msg_income = 123000000u64;

    let acc_id = AccountId::from([0x11; 32]);
    let start_balance = 2000000000u64;
    let mut acc = create_test_account(start_balance, acc_id.clone(), create_send_two_messages_code(), create_two_messages_data());
    // balance - (balance of 2 output messages + input msg fee + storage_fee + gas_fee)
    let end_balance = start_balance + msg_income - (150000000 + storage_fees + gas_fees);
    let mut new_acc = create_test_account(end_balance, acc_id, create_send_two_messages_code(), create_two_messages_data());
    let msg = create_int_msg(
        AccountId::from([0x22; 32]),
        AccountId::from([0x11; 32]),
        msg_income,
        false,
        BLOCK_LT - 1_000_000 + 1,
    );
    assert!(acc.set_library(BuilderData::with_raw(vec![0x72], 8).unwrap().into_cell().unwrap(), true));
    assert!(new_acc.set_library(BuilderData::with_raw(vec![0x72], 8).unwrap().into_cell().unwrap(), true));
    let tr_lt = BLOCK_LT + 1;
    new_acc.set_last_tr_time(tr_lt + 3);

    let trans = execute_c(&msg, &mut acc, tr_lt, 1671559338, 2).unwrap();

    new_acc.set_last_paid(acc.last_paid());

    let mut good_trans = Transaction::with_account_and_message(&acc, &msg, tr_lt).unwrap();
    good_trans.set_now(BLOCK_UT);

    let (mut msg1, mut msg2) = create_two_internal_messages();
    let mut actions = OutActions::default();
    actions.push_back(OutAction::new_send(SENDMSG_ORDINARY, msg1.clone()));
    actions.push_back(OutAction::new_send(SENDMSG_ORDINARY, msg2.clone()));
    if let CommonMsgInfo::IntMsgInfo(int_header) = msg1.header_mut() {
        if let CommonMsgInfo::IntMsgInfo(int_header2) = msg2.header_mut() {
            int_header.value.grams = Grams::from(MSG1_BALANCE - msg_fwd_fee);
            int_header2.value.grams = Grams::from(MSG2_BALANCE - msg_fwd_fee);
            int_header.fwd_fee = msg_remain_fee.into();
            int_header2.fwd_fee = msg_remain_fee.into();
            int_header.created_at = BLOCK_UT.into();
            int_header2.created_at = BLOCK_UT.into();
        }
    }

    good_trans.add_out_message(&make_common(msg1.clone())).unwrap();
    good_trans.add_out_message(&make_common(msg2.clone())).unwrap();
    good_trans.set_total_fees((storage_fees + gas_fees + msg_mine_fee * 2).into());

    let mut description = TransactionDescrOrdinary::default();
    description.storage_ph = Some(TrStoragePhase::with_params(storage_fees.into(), None, AccStatusChange::Unchanged));

    description.credit_ph = Some(TrCreditPhase {
        due_fees_collected: None,
        credit: CurrencyCollection::with_grams(msg_income),
    });

    let mut vm_phase = TrComputePhaseVm::default();
    vm_phase.success = true;
    vm_phase.msg_state_used = false;
    vm_phase.account_activated = false;
    vm_phase.gas_used = used.into();
    vm_phase.gas_limit = gas_limit.into();
    vm_phase.gas_credit = None;
    vm_phase.gas_fees = gas_fees.into();
    vm_phase.vm_steps = 10;
    description.compute_ph = TrComputePhase::Vm(vm_phase);

    let mut action_ph = TrActionPhase::default();
    action_ph.success = true;
    action_ph.valid = true;
    action_ph.status_change = AccStatusChange::Unchanged;
    action_ph.tot_actions = 2;
    action_ph.msgs_created = 2;
    action_ph.add_fwd_fees((2 * msg_fwd_fee).into());
    action_ph.add_action_fees((2 * msg_mine_fee).into());
    action_ph.action_list_hash = actions.hash().unwrap();
    action_ph.tot_msg_size = StorageUsedShort::calculate_for_struct(&msg1).unwrap();
    action_ph.tot_msg_size.append(&msg2.serialize().unwrap());

    description.action = Some(action_ph);
    description.credit_first = true;
    description.bounce = None;
    description.aborted = false;
    description.destroyed = false;
    good_trans.write_description(&TransactionDescr::Ordinary(description)).unwrap();

    assert_eq!(
        trans.read_description().unwrap().compute_phase_ref().unwrap().clone().get_vmphase_mut().unwrap().gas_used,
        good_trans.read_description().unwrap().compute_phase_ref().unwrap().clone().get_vmphase_mut().unwrap().gas_used);

    assert_eq!(trans.read_description().unwrap(), good_trans.read_description().unwrap());

    assert_eq!(acc, new_acc);
    trans.out_msgs.scan_diff(&good_trans.out_msgs, |key: ever_block::U15, msg1, msg2| {
        assert_eq!(msg1, msg2, "for key {}", key.0);
        Ok(true)
    }).unwrap();
    assert_eq!(trans, good_trans);
}

fn create_non_zero_level_cell() -> Result<Cell> {
    use ever_block::{IBitstring, CellType};

    let mut b = BuilderData::new();
    b.append_u8(1)?;
    b.append_u8(1)?;
    b.append_raw(UInt256::default().as_array(), 256)?;
    b.append_u16(0)?;
    b.set_type(CellType::PrunedBranch);
    //b.set_level_mask(LevelMask::with_level(1));
    let c = b.into_cell()?;

    let mut b = BuilderData::new();
    b.checked_append_reference(c)?;
    b.into_cell()
}

fn check_aborted_and_zero_level(tr: &Transaction, acc: &Account) -> Status {
    if !tr.read_description()?.is_aborted() {
        fail!("not aborted")
    }
    if acc.serialize()?.level() > 0 {
        fail!("non-zero level")
    }
    Ok(())
}

fn run_account_non_zero_level(code: Cell, body: SliceData, external: bool, bounce: bool) -> Status {
    let acc_id = AccountId::from([0x11; 32]);
    let mut acc = create_test_account(1_000_000_000, acc_id.clone(), code.clone(), Cell::default());
    let orig_acc = acc.clone();
    let config = custom_config(None, Some(GlobalCapabilities::CapSetLibCode as u64));
    let tr_lt = BLOCK_LT + 1;
    let lt = Arc::new(AtomicU64::new(tr_lt));
    let params = execute_params_none(lt);

    let mut msg = if external {
        create_test_external_msg()
    } else {
        create_int_msg(
            AccountId::from([0x33; 32]),
            acc_id,
            1_000_000_000u64,
            bounce,
            BLOCK_LT - 2
        )
    };
    msg.set_body(body);
    let executor = OrdinaryTransactionExecutor::new(config);
    let trans = executor.execute_with_params(Some(&make_common(msg.clone())), &mut acc, params)?;

    check_account_and_transaction_balances(&orig_acc, &acc, &msg, None);
    assert_eq!(acc.status(), orig_acc.status());
    assert_eq!(acc.state_init().cloned(), orig_acc.state_init().cloned());
    check_aborted_and_zero_level(&trans, &acc)
}

#[derive(Debug, PartialEq)]
enum Mode {
    Uninit,
    Frozen,
    Empty,
}

fn run_account_non_zero_level_stateinit(state_init: StateInit, mode: &Mode, external: bool, bounce: bool) -> Status {
    let acc_id = SliceData::from_raw(state_init.hash()?.as_slice().to_vec(), 256);
    let acc_addr = MsgAddressInt::with_standart(None, -1, acc_id.clone())?;
    let mut acc = match mode {
        Mode::Frozen => Account::frozen(
            acc_addr,
            BLOCK_LT + 2, BLOCK_UT,
            state_init.hash()?,
            Some(100_000_000u64.into()),
            CurrencyCollection::default()
        ),
        Mode::Uninit => Account::uninit(
            acc_addr,
            BLOCK_LT + 3, BLOCK_UT - 300000000,
            CurrencyCollection::with_grams(1_000_000_000_000),
        ),
        Mode::Empty => Account::default()
    };
    let orig_acc = acc.clone();
    let mut msg = if external {
        create_test_external_msg()
    } else {
        create_int_msg(
            AccountId::from([0x33; 32]),
            acc_id,
            1_000_000_000u64,
            bounce,
            BLOCK_LT - 2
        )
    };
    msg.set_state_init(state_init);

    let tr_lt = BLOCK_LT + 1;
    let lt = Arc::new(AtomicU64::new(tr_lt));
    let config = custom_config(None, Some(GlobalCapabilities::CapSetLibCode as u64));
    let executor = OrdinaryTransactionExecutor::new(config);
    let params = execute_params_none(lt);
    let trans_res = executor.execute_with_params(Some(&make_common(msg.clone())), &mut acc, params);
    if external && mode == &Mode::Uninit {
        assert_eq!(
            trans_res.unwrap_err().downcast::<ExecutorError>().unwrap(),
            ExecutorError::ExtMsgComputeSkipped(ever_block::ComputeSkipReason::BadState)
        );
    } else {
        check_aborted_and_zero_level(&trans_res?, &acc)?;
    };
    match mode {
        Mode::Empty => {
            if bounce {
                assert_eq!(acc.status(), AccountStatus::AccStateNonexist)
            } else {
                assert_eq!(acc.status(), AccountStatus::AccStateUninit)
            }
        }
        Mode::Frozen | Mode::Uninit => assert_eq!(acc.status(), orig_acc.status()),
    }
    check_account_and_transaction_balances(&orig_acc, &acc, &msg, None);
    assert_eq!(acc.state_init().cloned(), orig_acc.state_init().cloned());
    Ok(())
}

#[test]
fn test_account_non_zero_level() -> Status {
    let poison = create_non_zero_level_cell()?;

    let save_data = compile_code_to_cell("
        DROP LDREF DROP
        POPCTR c4
        ACCEPT
    ").unwrap();
    let setcode = compile_code_to_cell("
        DROP LDREF DROP
        SETCODE
        ACCEPT
    ").unwrap();
    let setlibcode = compile_code_to_cell("
        DROP LDREF DROP
        PUSHINT 1
        SETLIBCODE
        ACCEPT
    ").unwrap();

    let body = SliceData::load_cell_ref(&poison)?;
    for code in [save_data, setcode, setlibcode] {
        for bounce in [false, true] {
            run_account_non_zero_level(code.clone(), body.clone(), false, bounce)?;
        }
        for external in [false, true] {
            run_account_non_zero_level(code.clone(), body.clone(), external, false)?;
        }
    }

    let check_with_statinit = |mode: &Mode, external: bool, bounce: bool| -> Status {
        let mut state_init = StateInit::default();
        let mut b = BuilderData::from_cell(&compile_code_to_cell("PUSHREF DROP").unwrap())?;
        b.checked_append_reference(poison.clone())?;
        state_init.set_code(b.into_cell()?);
        run_account_non_zero_level_stateinit(state_init, &mode, external, bounce)?;

        let mut state_init = StateInit::default();
        state_init.set_code(Cell::default());
        state_init.set_data(poison.clone());
        run_account_non_zero_level_stateinit(state_init, &mode, external, bounce)?;

        let mut libs = HashmapE::with_bit_len(256);
        let key = SliceData::from_raw(poison.repr_hash().as_slice().to_vec(), 256);
        libs.setref(key, &poison)?;

        let mut state_init = StateInit::default();
        state_init.set_code(Cell::default());
        state_init.set_library(libs.data().unwrap().clone());
        run_account_non_zero_level_stateinit(state_init, &mode, external, bounce)?;

        Ok(())
    };

    for mode in [Mode::Empty, Mode::Frozen, Mode::Uninit] {
        for bounce in [false, true] {
            check_with_statinit(&mode, false, bounce)?;
        }
    }

    check_with_statinit(&Mode::Uninit, true, false)?;

    Ok(())

}

#[test]
fn fix_libs_disabled() {
    disable_cross_check();

    // manually create an output action list with one ChangeLib action
    let code = compile_code_to_cell("
        PUSHREF
        .cell {
            .blob x26fa1dd403
            .cell {}
            .cell {
                .blob x00
            }
        }
        POPCTR c5
    ").unwrap();

    let start_balance = 1_000_000_000;
    let acc_id = AccountId::from([0x11; 32]);
    let mut acc = create_test_account(start_balance, acc_id.clone(), code, Cell::default());

    let msg = create_int_msg(
        AccountId::from([0x33; 32]),
        acc_id,
        14_200_000_000u64,
        false,
        BLOCK_LT - 2
    );
    let tr_lt = BLOCK_LT + 1;
    let lt = Arc::new(AtomicU64::new(tr_lt));
    let executor = OrdinaryTransactionExecutor::new(
        custom_config(
            None,
            Some(GlobalCapabilities::CapTvmV19 as u64)
        )
    );
    assert_eq!(acc.libraries().len().unwrap(), 0);
    let params = execute_params_none(lt);
    let trans = executor.execute_with_params(Some(&make_common(msg)), &mut acc, params).unwrap();
    assert_eq!(
        34, // RESULT_CODE_UNKNOWN_OR_INVALID_ACTION,
        trans.read_description().unwrap().action_phase_ref().unwrap().result_code
    );
    assert_eq!(acc.libraries().len().unwrap(), 0);
}
