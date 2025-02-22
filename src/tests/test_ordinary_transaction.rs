/*
* Copyright (C) 2019-2024 EverX. All Rights Reserved.
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
use pretty_assertions::assert_eq;
use std::sync::{atomic::AtomicU64, Arc};
use ever_block::{
    accounts::{Account, AccountStorage, StorageInfo},
    messages::{CommonMsgInfo, ExternalInboundMessageHeader, Message, MsgAddressInt},
    out_actions::{
        OutAction, OutActions, SENDMSG_ALL_BALANCE, SENDMSG_ORDINARY, SENDMSG_REMAINING_MSG_BALANCE,
    },
    transactions::{
        AccStatusChange, TrActionPhase, TrComputePhase, TrComputePhaseVm, TrCreditPhase,
        TrStoragePhase, Transaction, TransactionDescr,
    },
    AccountStatus, AnycastInfo, ComputeSkipReason, ConfigParam8, ConfigParamEnum, ConfigParams,
    CurrencyCollection, Deserializable, GetRepresentationHash, GlobalCapabilities, GlobalVersion,
    Grams, InternalMessageHeader, MsgAddressIntOrNone, Serializable, StateInit, StorageUsedShort,
    TrBouncePhaseNofunds, TrBouncePhaseOk, UnixTime32, SENDMSG_PAY_FEE_SEPARATELY,
};
use ever_assembler::compile_code_to_cell;
use ever_block::{AccountId, BuilderData, Cell, HashmapE, IBitstring, SliceData, UInt256};
use ever_vm::{
    int,
    stack::{integer::IntegerData, Stack, StackItem},
};

mod common;
use common::*;
use crate::ordinary_transaction::tests1::common::cross_check::{disable_cross_check, DisableCrossCheck};
use crate::VERSION_BLOCK_NEW_CALCULATION_BOUNCED_STORAGE;

#[test]
fn test_simple_transaction() {
    let code = "
        PUSHROOT
        CTOS
        LDREF
        LDREF
        SWAP
        PUSHINT 64
        SENDRAWMSG
        SWAP
        PUSHINT 0
        SENDRAWMSG
    ";
    let start_balance = 100_000_000_000u64;
    let compute_phase_gas_fees = 1317000u64;
    let msg_income = 1_400_200_000;

    let acc_id = MsgAddressInt::with_standart(None, 0, AccountId::from([0x22; 32])).unwrap().address();
    let code = compile_code_to_cell(code).unwrap();

    let (mut msg1, mut msg2) = create_two_internal_messages();
    msg1.set_src_address(MsgAddressInt::with_standart(None, 0, acc_id.clone()).unwrap());
    msg2.set_src_address(MsgAddressInt::with_standart(None, 0, acc_id.clone()).unwrap());
    let mut b = BuilderData::default();
    b.checked_append_reference(msg2.serialize().unwrap()).unwrap();
    b.checked_append_reference(msg1.serialize().unwrap()).unwrap();
    let data = b.into_cell().unwrap();

    let mut acc = create_test_account_workchain(start_balance, 0, acc_id.clone(), code, data);

    let msg = create_int_msg_workchain(
        0,
        AccountId::from([0x33; 32]),
        acc_id,
        msg_income,
        false,
        BLOCK_LT - 2
    );

    let mut new_acc = Account::active_by_init_code_hash(
        acc.get_addr().unwrap().clone(),
        CurrencyCollection::with_grams(99849728119),
        BLOCK_UT,
        acc.state_init().unwrap().clone(),
        false
    ).unwrap();
    new_acc.update_storage_stat().unwrap();
    new_acc.set_last_tr_time(BLOCK_LT + 4);

    let tr_lt = BLOCK_LT + 1;
    let trans = execute_c(&msg, &mut acc, tr_lt, new_acc.balance().unwrap().grams, 2).unwrap();

    let mut description = TransactionDescrOrdinary::default();
    let storage_phase_fees = 271881;
    description.storage_ph = Some(TrStoragePhase::with_params(
        Grams::from(storage_phase_fees),
        None,
        AccStatusChange::Unchanged
    ));
    description.credit_ph = Some(TrCreditPhase::with_params(
        None,
        CurrencyCollection::with_grams(msg_income)
    ));

    let gas_used = (compute_phase_gas_fees / 1000) as u32;
    let gas_fees = compute_phase_gas_fees;
    let mut vm_phase = TrComputePhaseVm::default();
    vm_phase.success = true;
    vm_phase.gas_used = gas_used.into();
    vm_phase.gas_limit = 1000000u32.into();
    vm_phase.gas_fees = gas_fees.into();
    vm_phase.vm_steps = 11;
    description.compute_ph = TrComputePhase::Vm(vm_phase);

    let msg_mine_fee = MSG_MINE_FEE;
    let msg_fwd_fee = MSG_FWD_FEE;
    let msg_remain_fee = msg_fwd_fee - msg_mine_fee;
    let mut actions = OutActions::default();
    actions.push_back(OutAction::new_send(SENDMSG_REMAINING_MSG_BALANCE, msg1.clone()));
    actions.push_back(OutAction::new_send(SENDMSG_ORDINARY, msg2.clone()));
    if let Some(int_header) = msg1.int_header_mut() {
        if let Some(int_header2) = msg2.int_header_mut() {
            int_header.value.grams = Grams::from(MSG1_BALANCE + msg_income - compute_phase_gas_fees - msg_fwd_fee);
            int_header2.value.grams = Grams::from(MSG2_BALANCE - msg_fwd_fee);
            int_header.fwd_fee = msg_remain_fee.into();
            int_header2.fwd_fee = msg_remain_fee.into();
            int_header.created_at = BLOCK_UT.into();
            int_header2.created_at = BLOCK_UT.into();
        }
    }
    let mut action_ph = TrActionPhase::default();
    action_ph.success = true;
    action_ph.valid = true;
    action_ph.status_change = AccStatusChange::Unchanged;
    action_ph.tot_actions = 2;
    action_ph.spec_actions = 0;
    action_ph.msgs_created = 2;
    action_ph.total_fwd_fees = Some((2 * msg_fwd_fee).into());
    action_ph.total_action_fees = Some((2 * msg_mine_fee).into());
    action_ph.action_list_hash = actions.hash().unwrap();
    action_ph.tot_msg_size = StorageUsedShort::calculate_for_struct(&msg1).unwrap();
    action_ph.tot_msg_size.append(&msg2.serialize().unwrap());
    description.action = Some(action_ph);

    description.credit_first = true;
    description.bounce = None;
    description.aborted = false;
    description.destroyed = false;
    let description = TransactionDescr::Ordinary(description);

    let mut good_trans = Transaction::with_account_and_message(&new_acc, &msg, BLOCK_LT + 2).unwrap();

    good_trans.add_out_message(&make_common(msg1)).unwrap();
    good_trans.add_out_message(&make_common(msg2)).unwrap();
    good_trans.set_total_fees(CurrencyCollection::with_grams(gas_fees + storage_phase_fees + msg_mine_fee * 2));
    good_trans.orig_status = AccountStatus::AccStateActive;
    good_trans.set_end_status(AccountStatus::AccStateActive);
    good_trans.set_logical_time(BLOCK_LT + 1);
    good_trans.set_now(BLOCK_UT);

    good_trans.write_description(&description).unwrap();

    assert_eq!(acc, new_acc);
    trans.out_msgs.scan_diff(&good_trans.out_msgs, |key: ever_block::U15, msg1, msg2| {
        assert_eq!(msg1, msg2, "for key {}", key.0);
        Ok(true)
    }).unwrap();
    assert_eq!(trans.read_description().unwrap(), description);
    assert_eq!(trans, good_trans);
}

#[test]
fn test_trexecutor_with_bad_code() {
    let used = 748u32; //gas units
    let gas_fees = used as u64 * 10000;
    let storage_fees = 284115250;
    let total_fees = storage_fees + gas_fees;

    let acc_id = AccountId::from([0x11; 32]);
    let start_balance = 2000000000u64;
    let bad_code = compile_code_to_cell("
        ACCEPT
        NEWC
        ENDC
        CTOS
        LDREF
    ").unwrap();
    let mut acc = create_test_account(start_balance, acc_id.clone(), bad_code.clone(), create_two_messages_data());
    let msg = create_int_msg(
        acc_id.clone(),
        acc_id.clone(),
        1000000,
        false,
        BLOCK_LT
    );
    let mut new_acc = create_test_account(
        Grams::from(start_balance) + msg.value().unwrap().grams - Grams::from(total_fees), 
        acc_id, 
        bad_code, 
        create_two_messages_data()
    );
    new_acc.set_last_paid(BLOCK_UT);
    new_acc.set_last_tr_time(BLOCK_LT + 2);

    let tr_lt = BLOCK_LT + 1;

    let mut good_trans = Transaction::with_account_and_message(&new_acc, &msg, tr_lt).unwrap();

    let mut trans = execute_c(&msg, &mut acc, tr_lt, new_acc.balance().unwrap().grams, 0).unwrap();
    assert_eq!(acc, new_acc);

    good_trans.set_total_fees(total_fees.into());

    let mut description = TransactionDescrOrdinary::default();
    description.storage_ph = Some(TrStoragePhase::with_params(storage_fees.into(), None, AccStatusChange::Unchanged));
    description.credit_ph = Some(TrCreditPhase{ due_fees_collected: None, credit: msg.value().unwrap().clone() });

    let mut vm_phase = TrComputePhaseVm::default();
    vm_phase.success = false;
    vm_phase.exit_code = 9;
    vm_phase.msg_state_used = false;
    vm_phase.account_activated = false;
    vm_phase.gas_used = used.into();
    vm_phase.gas_limit = 100u32.into();
    vm_phase.gas_credit = None;
    vm_phase.gas_fees = gas_fees.into();
    vm_phase.vm_steps = 6;
    description.compute_ph = TrComputePhase::Vm(vm_phase);

    description.action = None;
    description.credit_first = true;
    description.bounce = None;
    description.aborted = true;
    description.destroyed = false;
    let description = TransactionDescr::Ordinary(description);
    good_trans.write_description(&description).unwrap();
    assert_eq!(trans.read_description().unwrap(), description);
    trans.set_now(0);

    assert_eq!(
        trans.read_description().unwrap().compute_phase_ref().unwrap().clone().get_vmphase_mut().unwrap().gas_used,
        good_trans.read_description().unwrap().compute_phase_ref().unwrap().clone().get_vmphase_mut().unwrap().gas_used);

    assert_eq!(trans, good_trans);
}

#[test]
fn test_trexecutor_with_code_without_accept() {
    let acc_id = AccountId::from([0x11; 32]);
    let start_balance = 2000000000u64;
    let msg = create_test_external_msg();
    // balance - (balance of 2 output messages + storage_fee + gas_fee)

    let tr_lt = BLOCK_LT + 1;
    // enough gas for smartcontract - normal termination, but NoAccept error
    let code = compile_code_to_cell("NOP").unwrap();
    let mut acc = create_test_account(start_balance, acc_id.clone(), code, create_two_messages_data());
    let err = execute(&msg, &mut acc, tr_lt).expect_err("no accept error must be generated");
    assert_eq!(err.downcast::<ExecutorError>().unwrap(), ExecutorError::NoAcceptError(0, None));

    // enough gas for smartcontract - but exception during run and NoAccept error
    let code = compile_code_to_cell("CTOS").unwrap();
    let mut acc = create_test_account(start_balance, acc_id.clone(), code, create_two_messages_data());
    let err = execute(&msg, &mut acc, tr_lt).expect_err("no accept error must be generated");
    assert_eq!(err.downcast::<ExecutorError>().unwrap(), ExecutorError::NoAcceptError(7, Some(ever_vm::int!(0))));

    // not enough gas for smartcontract - OutOfGas Exception and NoAccept error
    let code = compile_code_to_cell("AGAINEND NEWC ENDC DROP").unwrap();
    let mut acc = create_test_account(start_balance, acc_id.clone(), code, create_two_messages_data());
    let err = execute(&msg, &mut acc, tr_lt).expect_err("no accept error must be generated");
    assert_eq!(err.downcast::<ExecutorError>().unwrap(), ExecutorError::NoAcceptError(-14, Some(ever_vm::int!(10057))));

    // Due to ACCEPT, the transaction will be completed
    let code = compile_code_to_cell("ACCEPT AGAINEND NEWC ENDC DROP").unwrap();
    let mut acc = create_test_account(start_balance, acc_id.clone(), code, create_two_messages_data());
    execute(&msg, &mut acc, tr_lt).unwrap();

    // not exist code - transaction will be aborted
    let code = compile_code_to_cell("NOP").unwrap();
    let mut acc = create_test_account(start_balance, acc_id, code, create_two_messages_data());
    acc.state_init_mut().unwrap().code = None;
    let err = execute(&msg, &mut acc, tr_lt).expect_err("no accept error must be generated");
    assert_eq!(err.downcast::<ExecutorError>().unwrap(), ExecutorError::NoAcceptError(-13, None));

    // not exist code but special account - transaction will be aborted
    let acc_id = AccountId::from([0x66; 32]);
    assert!(
        BLOCKCHAIN_CONFIG.is_special_account(
            &MsgAddressInt::with_standart(None, -1, acc_id.clone()).unwrap()
        ).unwrap()
    );
    let code = compile_code_to_cell("NOP").unwrap();
    let mut acc = create_test_account(start_balance, acc_id.clone(), code, create_two_messages_data());

    let mut hdr = ExternalInboundMessageHeader::default();
    hdr.dst = MsgAddressInt::with_standart(None, -1, acc_id).unwrap();
    hdr.import_fee = Grams::zero();
    let mut msg_copy = Message::with_ext_in_header(hdr);
    msg_copy.set_body(SliceData::default());

    acc.state_init_mut().unwrap().code = None;
    let err = execute(&msg_copy, &mut acc, tr_lt).expect_err("no accept error must be generated");
    assert_eq!(err.downcast::<ExecutorError>().unwrap(), ExecutorError::NoAcceptError(-13, None));
}

#[test]
fn test_trexecutor_with_no_funds_for_storage() {
    let import_fee = 10000000;
    let balance = 1;
    let start_balance = import_fee + balance; // to pay for import external message and not enough to pay storage
    let last_paid = BLOCK_UT - 100;
    let acc_id = AccountId::from([0x11; 32]);
    let mut acc = create_test_account(start_balance, acc_id, create_send_two_messages_code(), create_two_messages_data());
    acc.set_last_paid(last_paid);
    let msg = create_test_external_msg();
    let mut new_acc = acc.clone();
    new_acc.set_last_paid(BLOCK_UT);
    new_acc.set_due_payment(Some(6605u64.into()));

    execute(&msg, &mut acc, BLOCK_LT + 1).expect_err("no funds for external message");
    assert_eq!(acc, new_acc);
}

#[test]
fn test_trexecutor_active_acc_with_code1_not_enough_balance() {
    let acc_id = AccountId::from([0x11; 32]);
    let start_balance = 9000000u64;
    let mut acc = create_test_account(start_balance, acc_id, create_send_two_messages_code(), create_two_messages_data());
    let msg = create_test_external_msg();
    let trans = execute_c(&msg, &mut acc, BLOCK_LT + 1, start_balance, 0);
    assert!(trans.is_err());
}

#[test]
fn test_trexecutor_active_acc_with_code1() {
    let used = 1307u32; //gas units
    let storage_fees = 288370662u64;
    let msg_fwd_fee = MSG_FWD_FEE;
    let msg_mine_fee = MSG_MINE_FEE;
    let msg_remain_fee = msg_fwd_fee - msg_mine_fee;
    let gas_fees = used as u64 * 10000;
    let gas_credit = 10000;

    let acc_id = AccountId::from([0x11; 32]);
    let start_balance = 2000000000u64;
    let mut acc = create_test_account(start_balance, acc_id.clone(), create_send_two_messages_code(), create_two_messages_data());
    // balance - (balance of 2 output messages + input msg fee + storage_fee + gas_fee)
    let end_balance = start_balance - (150000000 + msg_fwd_fee + storage_fees + gas_fees);
    let mut new_acc = create_test_account(end_balance, acc_id, create_send_two_messages_code(), create_two_messages_data());
    let msg = create_test_external_msg();
    let tr_lt = BLOCK_LT + 1;
    new_acc.set_last_tr_time(tr_lt + 3);

    let trans = execute_c(&msg, &mut acc, tr_lt, end_balance, 2).unwrap();
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

fn create_transfer_ext_msg(src: AccountId, dest: AccountId, value: u64) -> Message {
    let mut hdr = ExternalInboundMessageHeader::default();
    hdr.dst = MsgAddressInt::with_standart(None, -1, src.clone()).unwrap();
    hdr.import_fee = Grams::zero();

    let int_msg = create_int_msg(src, dest, value, true, 0);
    let int_header = match int_msg.withdraw_header() {
        CommonMsgInfo::IntMsgInfo(int_hdr) => int_hdr,
        _ => panic!("must be internal message header"),
    };

    let mut msg = Message::with_ext_in_header(hdr);
    msg.set_body(SliceData::load_builder(int_header.write_to_new_cell().unwrap()).unwrap());
    msg
}

#[test]
fn test_light_wallet_contract() {
    let contract_data = BuilderData::with_raw(vec![0x00; 32], 256).unwrap().into_cell().unwrap();
    let code = "
    ; s1 - body slice
    IFNOTRET
    ACCEPT
    BLOCKLT
    LTIME
    INC         ; increase logical time by 1
    PUSH s2     ; body to top
    PUSHINT 96  ; internal header in body, cut unixtime and lt
    SDSKIPLAST

    NEWC
    STSLICE
    STU 64         ; store tr lt
    STU 32         ; store unixtime
    STSLICECONST 0 ; no init
    STSLICECONST 0 ; body (Either X)
    ENDC
    PUSHINT 0
    SENDRAWMSG
    ";
    let contract_code = compile_code_to_cell(code).unwrap();
    let acc1_id = AccountId::from([0x11; 32]);
    let acc2_id = AccountId::from([0x22; 32]);

    let gas_used1 = 1387;
    let gas_fee1 = gas_used1 * 10000;
    let gas_fee2 = 1000000; // flat_gas_price
    let start_balance1 = u64::MAX / 4;
    let start_balance2 = u64::MAX / 4;
    let fwd_fee = MSG_FWD_FEE;
    let storage_fee1 = 140362109;
    let storage_fee2 = 140362109;

    let mut acc1 = create_test_account(start_balance1, acc1_id.clone(), contract_code.clone(), contract_data.clone());
    let mut acc2 = create_test_account(start_balance2, acc2_id.clone(), contract_code, contract_data);

    let transfer = u64::MAX / 8;
    //new acc.balance = acc.balance - in_fwd_fee - transfer_value - storage_fees - gas_fee
    let newbalance1 = start_balance1 - fwd_fee - transfer - storage_fee1 - gas_fee1;
    let in_msg = create_transfer_ext_msg(acc1_id, acc2_id, transfer);
    let trans = execute_c(&in_msg, &mut acc1, BLOCK_LT + 1, newbalance1, 1).unwrap();
    let msg = trans.get_out_msg(0).unwrap();

    //new acc.balance = acc.balance + transfer_value - fwd_fee - storage_fee - gas_fee
    let newbalance2 = start_balance2 + transfer - fwd_fee - storage_fee2 - gas_fee2;
    let _trans = execute_c(msg.unwrap().get_std().unwrap(), &mut acc2, BLOCK_LT + 1, newbalance2, 0).unwrap();
}

fn test_transfer_code(mode: u8, ending: &str) -> Cell {
    let code = format!("
        PUSHCONT {{
            ACCEPT
            NEWC        ; create builder
            STSLICE     ; store internal msg slice into builder (next in stack - internal message body like a slice)
            ENDC        ; finish cell creating
            PUSHINT {x}
            SENDRAWMSG  ; send message with created cell as a root
            {e}
        }}
        IF ; top-of-stack value is function selector, it is non zero - message is external
    ",
    x = mode,
    e = ending
    );

    compile_code_to_cell(&code).unwrap()
}

fn create_test_transfer_account(amount: u64, mode: u8) -> Account {
    create_test_transfer_account_with_ending(amount, mode, "")
}

fn create_test_transfer_account_with_ending(amount: u64, mode: u8, ending: &str) -> Account {
    let mut acc = Account::with_storage(
        &MsgAddressInt::with_standart(
            None,
            -1,
            SENDER_ACCOUNT.clone()
        ).unwrap(),
        &StorageInfo::with_values(
            ACCOUNT_UT,
            None,
        ),
        &AccountStorage::active_by_init_code_hash(0, CurrencyCollection::with_grams(amount), StateInit::default(), false),
    );
    acc.set_code(test_transfer_code(mode, ending));
    acc.update_storage_stat().unwrap();
    acc
}

fn create_test_external_msg_with_int(transfer_value: u64) -> Message {
    let acc_id = SENDER_ACCOUNT.clone();
    let mut hdr = ExternalInboundMessageHeader::default();
    hdr.dst = MsgAddressInt::with_standart(None, -1, acc_id.clone()).unwrap();
    hdr.import_fee = Grams::zero();
    let mut msg = Message::with_ext_in_header(hdr);

    let int_msg = create_int_msg(
        acc_id,
        RECEIVER_ACCOUNT.clone(),
        transfer_value,
        false,
        BLOCK_LT + 2
    );
    msg.set_body(SliceData::load_builder(int_msg.write_to_new_cell().unwrap()).unwrap());

    msg
}

#[test]
fn test_trexecutor_active_acc_with_code2() {
    let start_balance = 2000000000;
    let gas_used = 1170u32;
    let gas_fees = gas_used as u64 * 10000;
    let transfer = 50000000;
    let storage_fee = 78924597;
    let msg_mine_fee = MSG_MINE_FEE;
    let msg_fwd_fee = MSG_FWD_FEE;
    let msg_remain_fee = msg_fwd_fee - msg_mine_fee;

    let mut acc = create_test_transfer_account(start_balance, SENDMSG_ORDINARY);
    let mut new_acc = create_test_transfer_account(
        start_balance - (msg_fwd_fee + transfer + storage_fee + gas_fees), 0);
    let msg = create_test_external_msg_with_int(transfer);
    let tr_lt = BLOCK_LT + 1;
    new_acc.set_last_tr_time(tr_lt + 2);

    let trans = execute_c(&msg, &mut acc, tr_lt, new_acc.balance().unwrap().grams, 1).unwrap();
    acc.update_storage_stat().unwrap();
    new_acc.set_data(acc.get_data().unwrap());
    new_acc.set_last_paid(acc.last_paid());
    new_acc.update_storage_stat().unwrap();

    let mut good_trans = Transaction::with_account_and_message(&acc, &msg, tr_lt).unwrap();
    good_trans.set_now(BLOCK_UT);

    let msg1 = create_int_msg(
        AccountId::from([0x11; 32]),
        AccountId::from([0x22; 32]),
        transfer,
        false,
        BLOCK_LT + 2
    );
    let mut msg1_new_value = create_int_msg(
        AccountId::from([0x11; 32]),
        AccountId::from([0x22; 32]),
        transfer - msg_fwd_fee,
        false,
        BLOCK_LT + 2
    );
    if let CommonMsgInfo::IntMsgInfo(int_header) = msg1_new_value.header_mut() {
        int_header.fwd_fee = msg_remain_fee.into();
        int_header.created_at = BLOCK_UT.into();
    }

    good_trans.add_out_message(&make_common(msg1_new_value.clone())).unwrap();
    good_trans.set_total_fees((msg_fwd_fee + storage_fee + gas_fees + msg_mine_fee).into());

    let mut description = TransactionDescrOrdinary::default();
    description.storage_ph = Some(TrStoragePhase::with_params(storage_fee.into(), None, AccStatusChange::Unchanged));
    description.credit_ph = None;

    let mut vm_phase = TrComputePhaseVm::default();
    vm_phase.success = true;
    vm_phase.msg_state_used = false;
    vm_phase.account_activated = false;
    vm_phase.gas_used = gas_used.into();
    vm_phase.gas_limit = 0u32.into();
    vm_phase.gas_credit = Some(10000.into());
    vm_phase.gas_fees = gas_fees.into();
    vm_phase.vm_steps = 10;
    description.compute_ph = TrComputePhase::Vm(vm_phase);

    let mut action_ph = TrActionPhase::default();
    action_ph.success = true;
    action_ph.valid = true;
    action_ph.status_change = AccStatusChange::Unchanged;
    action_ph.tot_actions = 1;
    action_ph.msgs_created = 1;
    action_ph.add_fwd_fees(msg_fwd_fee.into());
    action_ph.add_action_fees(msg_mine_fee.into());
    action_ph.tot_msg_size = StorageUsedShort::calculate_for_struct(&msg1_new_value).unwrap();

    let mut actions = OutActions::default();
    actions.push_back(OutAction::new_send(SENDMSG_ORDINARY, msg1));
    action_ph.action_list_hash = actions.hash().unwrap();
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
    assert_eq!(trans, good_trans);
}

#[test]
fn test_trexecutor_active_acc_credit_first_false() {
    let start_balance = 1000000000;
    let mut acc = create_test_transfer_account(start_balance, SENDMSG_ORDINARY);

    let msg = create_int_msg(
        AccountId::from([0x33; 32]),
        AccountId::from([0x11; 32]),
        50000000,
        true,
        0,
    );

    let trans = execute_c(&msg, &mut acc, BLOCK_LT + 1, 970075403, 0).unwrap();
    assert_eq!(trans.read_description().unwrap().is_credit_first().unwrap(), false);
}

#[test]
fn test_trexecutor_active_acc_with_zero_balance() {
    let start_balance = 0;
    let mut acc = create_test_transfer_account(start_balance, SENDMSG_ORDINARY);
    let transfer = 1000000000;
    let storage_fee = 76796891;
    let gas_fee = 1000000; // flat_gas_price

    let msg = create_int_msg(
        AccountId::from([0x33; 32]),
        AccountId::from([0x11; 32]),
        transfer,
        false,
        0,
    );

    let trans = execute_c(&msg, &mut acc, BLOCK_LT + 1, transfer - storage_fee - gas_fee, 0).unwrap();
    assert_eq!(trans.read_description().unwrap().is_aborted(), false);
    let vm_phase_success = trans.read_description().unwrap().compute_phase_ref().unwrap().clone().get_vmphase_mut().unwrap().success;
    assert_eq!(vm_phase_success, true);
}

//contract send all its balance to another account using special mode in SENDRAWMSG.
//contract balance must equal to zero after transaction.
fn active_acc_send_all_balance(ending: &str) {
    let start_balance = 10_000_000_000; //10 grams
    let mut acc = create_test_transfer_account_with_ending(start_balance, SENDMSG_ALL_BALANCE, ending);

    let msg = create_test_external_msg_with_int(start_balance);

    let trans = execute_c(&msg, &mut acc, BLOCK_LT + 1, 0, 1).unwrap();
    assert_eq!(trans.read_description().unwrap().is_aborted(), false);
    let vm_phase_success = trans.read_description().unwrap().compute_phase_ref().unwrap().clone().get_vmphase_mut().unwrap().success;
    assert_eq!(vm_phase_success, true);
    assert!(trans.get_out_msg(0).unwrap().is_some());
    assert!(trans.get_out_msg(1).unwrap().is_none());
}

#[test]
fn test_trexecutor_active_acc_send_all_balance() {
    active_acc_send_all_balance("");
}

#[test]
fn test_trexecutor_active_acc_send_all_balance_with_commit_and_throw() {
    active_acc_send_all_balance("COMMIT THROW 11");
}

#[test]
fn test_trexecutor_active_acc_send_all_balance_with_commit_and_secondmsg_with_throw() {
    active_acc_send_all_balance(
        "COMMIT
         NEWC
         STSLICECONST x1234_
         ENDC
         PUSHINT 10
         SENDRAWMSG
         THROW 11"
    );
}

#[test]
fn test_skip_compute_phase_on_ext_msg() {
    let mut acc = Account::with_address_and_ballance(
        &MsgAddressInt::with_standart(None, -1, SENDER_ACCOUNT.clone()).unwrap(),
        &1_000_000_000u64.into());

    let msg = create_test_external_msg_with_int(1_000_000_000);

    let err = execute(&msg, &mut acc, BLOCK_LT + 1).unwrap_err();
    assert_eq!(err.downcast::<ExecutorError>().unwrap(), ExecutorError::ExtMsgComputeSkipped(ComputeSkipReason::NoState));
}

#[test]
fn test_build_ordinary_empty_stack() {
    let acc_balance = 10_000_000;
    let acc_id = AccountId::from([0x22; 32]);
    let acc = create_test_account(acc_balance, acc_id, create_send_two_messages_code(), create_two_messages_data());
    let executor = OrdinaryTransactionExecutor::new(BLOCKCHAIN_CONFIG.to_owned());

    let test_stack1 = executor.build_stack(None, &acc).unwrap();
    assert_eq!(test_stack1, Stack::new());
}

#[test]
fn test_build_ordinary_stack() {
    let acc_balance = 10_000_000;
    let msg_balance = 15_000;
    let acc_id = AccountId::from([0x22; 32]);
    let msg_int = create_int_msg(
        AccountId::from([0x11; 32]),
        acc_id.clone(),
        msg_balance,
        false,
        0,
    );
    let acc = create_test_account(acc_balance, acc_id, create_send_two_messages_code(), create_two_messages_data());
    let executor = OrdinaryTransactionExecutor::new(BLOCKCHAIN_CONFIG.to_owned());

    let test_stack1 = executor.build_stack(Some(&msg_int), &acc).unwrap();

    let body_slice1: SliceData = msg_int.body().unwrap_or_default();

    //stack for internal msg
    let mut etalon_stack1 = Stack::new();
    etalon_stack1
        .push(int!(acc_balance))
        .push(int!(msg_balance))
        .push(StackItem::Cell(msg_int.serialize().unwrap()))
        .push(StackItem::Slice(body_slice1))
        .push(int!(0));

    assert_eq!(test_stack1, etalon_stack1);

    let msg_ext = create_test_external_msg();
    let executor = OrdinaryTransactionExecutor::new(BLOCKCHAIN_CONFIG.to_owned());
    let test_stack2 = executor.build_stack(Some(&msg_ext), &acc).unwrap();

    let body_slice2: SliceData = msg_ext.body().unwrap_or_default();

    //stack for external msg
    let mut etalon_stack2 = Stack::new();
    etalon_stack2
        .push(int!(acc_balance))
        .push(int!(0))
        .push(StackItem::Cell(msg_ext.serialize().unwrap()))
        .push(StackItem::Slice(body_slice2))
        .push(int!(-1));

    assert_eq!(test_stack2, etalon_stack2);
}

#[test]
fn test_drain_account() {
    let storage_fee = 286774882;
    let start_balance = 1; // not enough to pay storage
    let msg_income = 200000000; //  but enough not to freeze
    let total_balance = start_balance + msg_income;
    let due_payment = storage_fee - total_balance;
    let acc_id = AccountId::from([0x11; 32]);
    let mut acc = create_test_account(start_balance, acc_id, create_send_two_messages_code(), create_two_messages_data());
    // send message to zero account but not freeze
    let msg = create_int_msg(
        AccountId::from([0x33; 32]),
        AccountId::from([0x11; 32]),
        msg_income,
        false,
        BLOCK_LT + 2
    );
    let mut new_acc = acc.clone();
    new_acc.set_last_paid(BLOCK_UT);
    new_acc.set_last_tr_time(BLOCK_LT + 4);
    new_acc.set_balance(CurrencyCollection::default());
    new_acc.update_storage_stat().unwrap();
    new_acc.set_due_payment(Some(86774881u64.into()));

    let tr_lt = BLOCK_LT + 1;
    let trans = execute_c(&msg, &mut acc, tr_lt, new_acc.balance().unwrap().grams, 0).unwrap();
    acc.update_storage_stat().unwrap();

    assert_eq!(acc, new_acc);

    let mut good_trans = Transaction::with_account_and_message(&new_acc, &msg, BLOCK_LT + 3).unwrap();
    let mut description = TransactionDescrOrdinary::default();
    description.storage_ph = Some(TrStoragePhase {
        storage_fees_collected: total_balance.into(), // collect full balance as fee
        storage_fees_due: Some(Grams::from(due_payment)), // also due_payment credit for next transaction
        status_change: AccStatusChange::Unchanged
    });
    description.credit_ph = Some(TrCreditPhase {
        due_fees_collected: None,
        credit: CurrencyCollection::with_grams(msg_income),
    });
    description.compute_ph = TrComputePhase::skipped(ComputeSkipReason::NoGas);

    description.action = None;
    description.credit_first = true;
    description.bounce = None;
    description.aborted = true;
    description.destroyed = false;
    let description = TransactionDescr::Ordinary(description);

    good_trans.set_total_fees(CurrencyCollection::with_grams(total_balance));
    good_trans.orig_status = AccountStatus::AccStateActive;
    good_trans.set_now(BLOCK_UT);

    good_trans.write_description(&description).unwrap();

    assert_eq!(trans.read_description().unwrap(), description);
    assert_eq!(trans, good_trans);
}

// send money to account without money and send all remaining balance to another account
// account must not be freezen
#[test]
fn test_send_value_to_account_without_money_without_bounce() {
    let start_balance = 0; // priviously all balance was moved from contract
    let msg_income = 1_000_000_000u64; // send enough money to pay storage fee and to run contract
    let storage_fee = 3492;
    let total_balance = 0;
    let gas_used = 591u32;
    let gas_fees = 5910000u64;
    let msg_fwd_fee = MSG_FWD_FEE;
    let msg_mine_fee = MSG_MINE_FEE;
    let msg_remain_fee = msg_fwd_fee - msg_mine_fee;
    let total_fees = gas_fees + msg_mine_fee + storage_fee;
    let remain_balance = msg_income - msg_fwd_fee - gas_fees - storage_fee;
    let mut out_msg = create_int_msg([0x11; 32].into(), [0; 32].into(), 100, false, 0);
    let data = out_msg.serialize().unwrap();
    let code = compile_code_to_cell("
        PUSHROOT
        PUSHINT 128
        SENDRAWMSG
    ").unwrap();
    let acc_id = AccountId::from([0x11; 32]);
    let mut acc = create_test_account(start_balance, acc_id.clone(), code.clone(), data.clone());
    acc.set_last_paid(BLOCK_UT - 100);
    let msg = create_int_msg(
        AccountId::from([0x33; 32]),
        AccountId::from([0x11; 32]),
        msg_income,
        false,
        PREV_BLOCK_LT
    );
    // result account is normal with empty balance
    let mut new_acc = create_test_account(total_balance, acc_id, code, data);
    new_acc.set_last_paid(BLOCK_UT);
    new_acc.set_last_tr_time(BLOCK_LT + 3);

    let tr_lt = BLOCK_LT + 1;
    let mut good_trans = Transaction::with_account_and_message(&new_acc, &msg, tr_lt).unwrap();

    let trans = execute_c(&msg, &mut acc, tr_lt, new_acc.balance().unwrap().grams, 1).unwrap();
    assert_eq!(acc, new_acc);

    let mut description = TransactionDescrOrdinary::default();
    description.storage_ph = Some(TrStoragePhase {
        storage_fees_collected: storage_fee.into(),
        storage_fees_due: None,
        status_change: AccStatusChange::Unchanged
    });
    description.credit_ph = Some(TrCreditPhase {
        due_fees_collected: None,
        credit: CurrencyCollection::with_grams(msg_income),
    });
    let mut vm_phase = TrComputePhaseVm::default();
    vm_phase.success = true;
    vm_phase.gas_used = gas_used.into();
    vm_phase.gas_limit = 99999u32.into();
    vm_phase.gas_fees = gas_fees.into();
    vm_phase.vm_steps = 4;
    description.compute_ph = TrComputePhase::Vm(vm_phase);

    let mut actions = OutActions::default();
    actions.push_back(OutAction::new_send(SENDMSG_ALL_BALANCE, out_msg.clone()));
    if let Some(int_header) = out_msg.int_header_mut() {
        int_header.value = remain_balance.into();
        int_header.fwd_fee = msg_remain_fee.into();
        int_header.created_lt = BLOCK_LT + 2;
        int_header.created_at = BLOCK_UT.into();
    }

    let mut account = acc.clone();
    let mut acc_balance = CurrencyCollection::with_grams(start_balance + msg_income - storage_fee);
    account.set_balance(acc_balance.clone());
    let executor = OrdinaryTransactionExecutor::new(BLOCKCHAIN_CONFIG.to_owned());
    let smci = build_contract_info(&account, &msg, BLOCKCHAIN_CONFIG.raw_config());
    let stack = executor.build_stack(Some(&msg), &account).unwrap();
    let (compute_ph, real_actions, _new_data) = executor.compute_phase(
        Some(&msg),
        &mut account,
        &mut acc_balance,
        &CurrencyCollection::with_grams(msg_income),
        smci,
        stack,
        0,
        msg.is_masterchain(),
        false,
        &ExecuteParams::default(),
    ).unwrap();
    assert_eq!(compute_ph, description.compute_ph);
    assert_eq!(OutActions::construct_from_cell(real_actions.unwrap()).unwrap(), actions);

    let mut action_ph = TrActionPhase::default();
    action_ph.success = true;
    action_ph.valid = true;
    action_ph.status_change = AccStatusChange::Unchanged;
    action_ph.tot_actions = 1;
    action_ph.msgs_created = 1;
    action_ph.add_fwd_fees(msg_fwd_fee.into());
    action_ph.add_action_fees(msg_mine_fee.into());
    action_ph.action_list_hash = actions.hash().unwrap();
    action_ph.tot_msg_size = StorageUsedShort::calculate_for_struct(&out_msg).unwrap();

    description.action = Some(action_ph);

    description.credit_first = true;
    description.bounce = None;
    description.aborted = false;
    description.destroyed = false;
    let description = TransactionDescr::Ordinary(description);
    let cmn_msg = make_common(out_msg);
    good_trans.add_out_message(&cmn_msg).unwrap();
    good_trans.set_total_fees(CurrencyCollection::with_grams(total_fees));
    // good_trans.orig_status = AccountStatus::AccStateActive;
    good_trans.set_now(BLOCK_UT);

    good_trans.write_description(&description).unwrap();

    assert_eq!(trans.read_description().unwrap(), description);
    assert_eq!(trans.get_out_msg(0).unwrap().as_ref(), Some(&cmn_msg));
    assert_eq!(trans, good_trans);
}

#[test]
fn test_outmsg_ihr_fee() {
    let start_balance = 100000000000u64;
    let msg_income = 1_000_000_000u64;
    let storage_fee = 3528;
    let ihr_fee = if cfg!(feature = "ihr_disabled") {0} else {15000000};
    let total_balance = 100984246372 - ihr_fee;
    let gas_used = 575u32;
    let gas_fees = 5750000u64;
    let msg_fwd_fee = MSG_FWD_FEE;
    let msg_mine_fee = MSG_MINE_FEE;
    let msg_remain_fee = msg_fwd_fee - msg_mine_fee;
    let total_fees = gas_fees + msg_mine_fee + storage_fee;
    let mut out_msg = create_int_msg([0x11; 32].into(), [0; 32].into(), 100, false, 0);
    if let CommonMsgInfo::IntMsgInfo(header) = out_msg.header_mut() {
        header.ihr_disabled = false;
    } else {
        unreachable!()
    }
    let data = out_msg.serialize().unwrap();
    let code = compile_code_to_cell("
        PUSHROOT
        PUSHINT 1
        SENDRAWMSG
    ").unwrap();
    let acc_id = AccountId::from([0x11; 32]);
    let mut acc = create_test_account(start_balance, acc_id.clone(), code.clone(), data.clone());
    acc.set_last_paid(BLOCK_UT - 100);
    let msg = create_int_msg(
        AccountId::from([0x33; 32]),
        AccountId::from([0x11; 32]),
        msg_income,
        false,
        PREV_BLOCK_LT
    );

    let mut new_acc = create_test_account(total_balance, acc_id, code, data);
    new_acc.set_last_paid(BLOCK_UT);
    new_acc.set_last_tr_time(BLOCK_LT + 3);

    let tr_lt = BLOCK_LT + 1;
    let mut good_trans = Transaction::with_account_and_message(&new_acc, &msg, tr_lt).unwrap();

    let trans = execute_c(&msg, &mut acc, tr_lt, new_acc.balance().unwrap().grams, 1).unwrap();
    assert_eq!(acc, new_acc);

    let mut description = TransactionDescrOrdinary::default();
    description.storage_ph = Some(TrStoragePhase {
        storage_fees_collected: storage_fee.into(),
        storage_fees_due: None,
        status_change: AccStatusChange::Unchanged
    });
    description.credit_ph = Some(TrCreditPhase {
        due_fees_collected: None,
        credit: CurrencyCollection::with_grams(msg_income),
    });
    let mut vm_phase = TrComputePhaseVm::default();
    vm_phase.success = true;
    vm_phase.gas_used = gas_used.into();
    vm_phase.gas_limit = 100000u32.into();
    vm_phase.gas_fees = gas_fees.into();
    vm_phase.vm_steps = 4;
    description.compute_ph = TrComputePhase::Vm(vm_phase);

    let mut actions = OutActions::default();
    actions.push_back(OutAction::new_send(SENDMSG_PAY_FEE_SEPARATELY, out_msg.clone()));
    if let Some(int_header) = out_msg.int_header_mut() {
        int_header.ihr_disabled = cfg!(feature = "ihr_disabled");
        int_header.value = CurrencyCollection::with_grams(100);
        int_header.fwd_fee = msg_remain_fee.into();
        int_header.created_lt = BLOCK_LT + 2;
        int_header.created_at = BLOCK_UT.into();
        int_header.ihr_fee = ihr_fee.into();
    }

    let mut action_ph = TrActionPhase::default();
    action_ph.success = true;
    action_ph.valid = true;
    action_ph.status_change = AccStatusChange::Unchanged;
    action_ph.tot_actions = 1;
    action_ph.msgs_created = 1;
    action_ph.add_fwd_fees((msg_fwd_fee + ihr_fee).into());
    action_ph.add_action_fees(msg_mine_fee.into());
    action_ph.action_list_hash = actions.hash().unwrap();
    action_ph.tot_msg_size = StorageUsedShort::calculate_for_struct(&out_msg).unwrap();

    description.action = Some(action_ph);

    description.credit_first = true;
    description.bounce = None;
    description.aborted = false;
    description.destroyed = false;
    let description = TransactionDescr::Ordinary(description);
    let cmn_msg = make_common(out_msg);
    good_trans.add_out_message(&cmn_msg).unwrap();
    good_trans.set_total_fees(CurrencyCollection::with_grams(total_fees));
    good_trans.set_now(BLOCK_UT);

    good_trans.write_description(&description).unwrap();

    assert_eq!(trans.get_out_msg(0).unwrap().as_ref(), Some(&cmn_msg));
    assert_eq!(trans.read_description().unwrap(), description);
    assert_eq!(trans, good_trans);
}

#[test]
fn test_send_value_to_account_without_money_with_bounce() {
    let start_balance = 0; // priviously all balance was moved from contract
    let msg_income = 1_000_000_000u64; // send enough money to pay storage fee and to run contract
    let storage_fee = 3480;
    let total_balance = 0;
    let gas_used = 591u32;
    let gas_fees = gas_used as u64 * 10000;
    let msg_fwd_fee = MSG_FWD_FEE;
    let msg_mine_fee = MSG_MINE_FEE;
    let msg_remain_fee = msg_fwd_fee - msg_mine_fee;
    let total_fees = gas_fees + msg_mine_fee;
    let remain_balance = msg_income - msg_fwd_fee - gas_fees; // by specs
    let mut out_msg = create_int_msg([0x11; 32].into(), [0; 32].into(), 0, false, 0);
    let data = out_msg.serialize().unwrap();
    let code = compile_code_to_cell("
        PUSHROOT
        PUSHINT 128
        SENDRAWMSG
    ").unwrap();
    let acc_id = AccountId::from([0x11; 32]);
    let mut acc = create_test_account(start_balance, acc_id.clone(), code.clone(), data.clone());
    acc.set_last_paid(BLOCK_UT - 100);
    let msg = create_int_msg(
        AccountId::from([0x33; 32]),
        AccountId::from([0x11; 32]),
        msg_income,
        true,
        PREV_BLOCK_LT
    );
    // result account is normal with empty balance
    let mut new_acc = create_test_account(total_balance, acc_id, code, data);
    new_acc.set_last_paid(BLOCK_UT);
    new_acc.set_last_tr_time(BLOCK_LT + 3);

    let tr_lt = BLOCK_LT + 1;
    let mut good_trans = Transaction::with_account_and_message(&new_acc, &msg, tr_lt).unwrap();

    let trans = execute_c(&msg, &mut acc, tr_lt, new_acc.balance().unwrap().grams, 1).unwrap();

    assert_eq!(acc, new_acc);

    let mut description = TransactionDescrOrdinary::default();
    description.storage_ph = Some(TrStoragePhase {
        storage_fees_collected: Grams::default(),
        storage_fees_due: Some(storage_fee.into()),
        status_change: AccStatusChange::Unchanged
    });
    let due = 3480u64;
    description.credit_ph = Some(TrCreditPhase {
        due_fees_collected: Some(due.into()),
        credit: CurrencyCollection::with_grams(msg_income - due),
    });
    let mut vm_phase = TrComputePhaseVm::default();
    vm_phase.success = true;
    vm_phase.gas_used = gas_used.into();
    vm_phase.gas_limit = 99999u32.into();
    vm_phase.gas_fees = gas_fees.into();
    vm_phase.vm_steps = 4;
    description.compute_ph = TrComputePhase::Vm(vm_phase);

    let mut actions = OutActions::default();
    actions.push_back(OutAction::new_send(SENDMSG_ALL_BALANCE, out_msg.clone()));
    if let Some(int_header) = out_msg.int_header_mut() {
        int_header.value = (remain_balance - due).into();
        int_header.fwd_fee = msg_remain_fee.into();
        int_header.created_lt = BLOCK_LT + 2;
        int_header.created_at = BLOCK_UT.into();
    }

    let mut account = acc.clone();
    let acc_balance = CurrencyCollection::with_grams(acc.balance().unwrap().grams.as_u128() as u64 + msg_income - storage_fee);
    account.set_balance(acc_balance);
    let executor = OrdinaryTransactionExecutor::new(BLOCKCHAIN_CONFIG.to_owned());
    let smci = build_contract_info(&account, &msg, BLOCKCHAIN_CONFIG.raw_config());
    let stack = executor.build_stack(Some(&msg), &account).unwrap();
    let (compute_ph, real_actions, _new_data) = executor.compute_phase(
        Some(&msg),
        &mut account.clone(),
        &mut account.balance().cloned().unwrap_or_default(),
        &msg.get_value().cloned().unwrap_or_default(),
        smci,
        stack,
        0,
        msg.is_masterchain(),
        false,
        &ExecuteParams::default(),
    ).unwrap();
    assert_eq!(compute_ph, description.compute_ph);
    assert_eq!(OutActions::construct_from_cell(real_actions.unwrap()).unwrap(), actions);

    let mut action_ph = TrActionPhase::default();
    action_ph.success = true;
    action_ph.valid = true;
    action_ph.status_change = AccStatusChange::Unchanged;
    action_ph.tot_actions = 1;
    action_ph.msgs_created = 1;
    action_ph.add_fwd_fees(msg_fwd_fee.into());
    action_ph.add_action_fees(msg_mine_fee.into());
    action_ph.action_list_hash = actions.hash().unwrap();
    action_ph.tot_msg_size = StorageUsedShort::calculate_for_struct(&out_msg).unwrap();

    description.action = Some(action_ph);

    description.credit_first = false;
    description.bounce = None;
    description.aborted = false;
    description.destroyed = false;
    let description = TransactionDescr::Ordinary(description);
    let cmn_msg = make_common(out_msg);
    good_trans.add_out_message(&cmn_msg).unwrap();
    good_trans.set_total_fees(CurrencyCollection::with_grams(total_fees + due));
    good_trans.set_now(BLOCK_UT);

    good_trans.write_description(&description).unwrap();

    assert_eq!(trans.read_description().unwrap(), description);
    assert_eq!(trans.get_out_msg(0).unwrap().as_ref(), Some(&cmn_msg));
    assert_eq!(trans, good_trans);
}

#[test]
fn test_send_msg_value_to_account_without_money_with_bounce() {
    let start_balance = 3u64; // priviously all balance was moved from contract
    let msg_income = 1_000_000_000u64; // send enough money to pay storage fee and to run contract
    let storage_fee = 3480;
    let due = storage_fee - start_balance;
    let total_balance = 0;
    let gas_used = 583u32;
    let gas_fees = gas_used as u64 * 10000;
    let msg_fwd_fee = MSG_FWD_FEE;
    let msg_mine_fee = MSG_MINE_FEE;
    let msg_remain_fee = msg_fwd_fee - msg_mine_fee;
    let total_fees = start_balance + gas_fees + msg_mine_fee;
    let remain_balance = msg_income - msg_fwd_fee - gas_fees; // by specs
    let mut out_msg = create_int_msg([0x11; 32].into(), [0; 32].into(), 0, false, 0);
    let data = out_msg.serialize().unwrap();
    let code = compile_code_to_cell("
        PUSHROOT
        PUSHINT 64
        SENDRAWMSG
    ").unwrap();
    let acc_id = AccountId::from([0x11; 32]);
    let mut acc = create_test_account(start_balance, acc_id.clone(), code.clone(), data.clone());
    acc.set_last_paid(BLOCK_UT - 100);
    let msg = create_int_msg(
        AccountId::from([0x33; 32]),
        AccountId::from([0x11; 32]),
        msg_income,
        true,
        PREV_BLOCK_LT
    );
    // result account is normal with empty balance
    let mut new_acc = create_test_account(total_balance, acc_id, code, data);
    new_acc.set_last_paid(BLOCK_UT);
    new_acc.set_last_tr_time(BLOCK_LT + 3);

    let tr_lt = BLOCK_LT + 1;
    let mut good_trans = Transaction::with_account_and_message(&new_acc, &msg, tr_lt).unwrap();

    let trans = execute_c(&msg, &mut acc, tr_lt, new_acc.balance().unwrap().grams, 1).unwrap();
    acc.update_storage_stat().unwrap();

    assert_eq!(acc, new_acc);

    let mut description = TransactionDescrOrdinary::default();
    description.storage_ph = Some(TrStoragePhase {
        storage_fees_collected: Grams::from(start_balance),
        storage_fees_due: Some(due.into()),
        status_change: AccStatusChange::Unchanged
    });
    let due = 3477u64;
    description.credit_ph = Some(TrCreditPhase {
        due_fees_collected: Some(due.into()),
        credit: CurrencyCollection::with_grams(msg_income - due),
    });
    let mut vm_phase = TrComputePhaseVm::default();
    vm_phase.success = true;
    vm_phase.gas_used = gas_used.into();
    vm_phase.gas_limit = 99999u32.into();
    vm_phase.gas_fees = gas_fees.into();
    vm_phase.vm_steps = 4;
    description.compute_ph = TrComputePhase::Vm(vm_phase);

    let mut actions = OutActions::default();
    actions.push_back(OutAction::new_send(SENDMSG_REMAINING_MSG_BALANCE, out_msg.clone()));
    if let Some(int_header) = out_msg.int_header_mut() {
        int_header.value = (remain_balance - due).into();
        int_header.fwd_fee = msg_remain_fee.into();
        int_header.created_lt = BLOCK_LT + 2;
        int_header.created_at = BLOCK_UT.into();
    }

    let mut account = acc.clone();
    let acc_balance = CurrencyCollection::from_grams(acc.balance().unwrap().grams + (msg_income - storage_fee) as u128);
    account.set_balance(acc_balance);
    let executor = OrdinaryTransactionExecutor::new(BLOCKCHAIN_CONFIG.to_owned());
    let smci = build_contract_info(&account, &msg, BLOCKCHAIN_CONFIG.raw_config());
    let stack = executor.build_stack(Some(&msg), &account).unwrap();
    let (compute_ph, real_actions, _new_data) = executor.compute_phase(
        Some(&msg),
        &mut account.clone(),
        &mut account.balance().cloned().unwrap_or_default(),
        &msg.get_value().cloned().unwrap_or_default(),
        smci,
        stack,
        0,
        msg.is_masterchain(),
        false,
        &ExecuteParams::default(),
    ).unwrap();
    assert_eq!(compute_ph, description.compute_ph);
    assert_eq!(OutActions::construct_from_cell(real_actions.unwrap()).unwrap(), actions);

    let mut action_ph = TrActionPhase::default();
    action_ph.success = true;
    action_ph.valid = true;
    action_ph.status_change = AccStatusChange::Unchanged;
    action_ph.tot_actions = 1;
    action_ph.msgs_created = 1;
    action_ph.add_fwd_fees(msg_fwd_fee.into());
    action_ph.add_action_fees(msg_mine_fee.into());
    action_ph.action_list_hash = actions.hash().unwrap();
    action_ph.tot_msg_size = StorageUsedShort::calculate_for_struct(&out_msg).unwrap();

    description.action = Some(action_ph);

    description.credit_first = false;
    description.bounce = None;
    description.aborted = false;
    description.destroyed = false;
    let description = TransactionDescr::Ordinary(description);
    let cmn_msg = make_common(out_msg);
    good_trans.add_out_message(&cmn_msg).unwrap();
    good_trans.set_total_fees(CurrencyCollection::with_grams(total_fees + due));
    good_trans.set_now(BLOCK_UT);

    good_trans.write_description(&description).unwrap();

    assert_eq!(trans.read_description().unwrap(), description);
    assert_eq!(trans.get_out_msg(0).unwrap().as_ref(), Some(&cmn_msg));
    assert_eq!(trans, good_trans);
}

#[test]
fn test_send_bouncable_messages_to_account_without_enough_money_to_pay_storage_and_freeze() {
    // 1. Bad code without accept
    let code = "CTOS";
    let data = Cell::default();
    let start_balance = 100;
    let due = 105786786u64;
    // message value is not enough to pay for storage
    let (acc, tr) = execute_custom_transaction(start_balance, code, data.clone(), 100, true, 0, 0);
    assert_eq!(get_tr_descr(&tr).storage_ph.unwrap().storage_fees_due, Some(due.into()));
    assert_eq!(get_tr_descr(&tr).credit_ph.unwrap().credit, CurrencyCollection::with_grams(0));
    assert!(matches!(get_tr_descr(&tr).compute_ph, TrComputePhase::Skipped(_)));
    assert_eq!(acc.status(), AccountStatus::AccStateFrozen);
    // message value is enough to pay for storage, but not enough to pay for compute value
    let (acc, tr) = execute_custom_transaction(start_balance, code, data.clone(), due + 1, true, 1, 0);
    assert_eq!(get_tr_descr(&tr).credit_ph.unwrap().credit, CurrencyCollection::with_grams(1));
    assert_eq!(get_tr_descr(&tr).compute_ph, TrComputePhase::skipped(ComputeSkipReason::NoGas));
    assert_eq!(acc.status(), AccountStatus::AccStateFrozen);
    // message value is enough to pay for storage and compute, but not enough to pay for bounce value
    let (acc, tr) = execute_custom_transaction(start_balance, code, data.clone(), due + 4_000_000, true, 4_000_000, 0);
    assert_eq!(get_tr_descr(&tr).compute_ph, TrComputePhase::skipped(ComputeSkipReason::NoState));
    assert!(matches!(get_tr_descr(&tr).bounce, Some(TrBouncePhase::Nofunds(_))));
    assert_eq!(acc.status(), AccountStatus::AccStateFrozen);
    // message value is enough to pay for storage,compute and bounce value
    let (acc, tr) = execute_custom_transaction(start_balance, code, data, due + 15_000_000, true, 0, 1);
    assert_eq!(get_tr_descr(&tr).compute_ph, TrComputePhase::skipped(ComputeSkipReason::NoState));
    assert!(matches!(get_tr_descr(&tr).bounce, Some(TrBouncePhase::Ok(_))));
    assert_eq!(acc.status(), AccountStatus::AccStateFrozen);
}

#[test]
fn test_send_bouncable_messages_to_account_without_enough_money_to_pay_storage() {
    // account balance is not enough to pay for storage fee

    // i. Bad code without accept
    let code = "CTOS";
    let data = Cell::default();
    let due = 1000u64;
    let start_balance = 107382665 - due;
    // message value is not enough to pay for storage
    let (acc, tr) = execute_custom_transaction(start_balance, code, data.clone(), 100, true, 0, 0);
    assert_eq!(get_tr_descr(&tr).storage_ph.unwrap().storage_fees_due, Some(due.into()));
    assert_eq!(get_tr_descr(&tr).credit_ph.unwrap().credit, CurrencyCollection::with_grams(0));
    assert!(matches!(get_tr_descr(&tr).compute_ph, TrComputePhase::Skipped(_)));
    assert_eq!(acc.status(), AccountStatus::AccStateActive);
    // message value is enough to pay for storage, but not enough to pay for compute value
    let (acc, tr) = execute_custom_transaction(start_balance, code, data.clone(), 10000, true, 10000 - due, 0);
    assert_eq!(get_tr_descr(&tr).credit_ph.unwrap().credit, CurrencyCollection::with_grams(9000));
    assert!(matches!(get_tr_descr(&tr).compute_ph, TrComputePhase::Skipped(_)));
    assert_eq!(acc.status(), AccountStatus::AccStateActive);
    // message value is enough to pay for storage and compute, but not enough to pay for bounce value
    let (acc, tr) = execute_custom_transaction(start_balance, code, data.clone(), 10_000_000, true, 9000000 - due, 0);
    assert!(matches!(get_tr_descr(&tr).compute_ph, TrComputePhase::Vm(_)));
    assert!(matches!(get_tr_descr(&tr).bounce, Some(TrBouncePhase::Nofunds(_))));
    assert_eq!(acc.status(), AccountStatus::AccStateActive);
    // message value is enough to pay for storage,compute and bounce value
    let (acc, tr) = execute_custom_transaction(start_balance, code, data.clone(), 12_000_000, true, 0, 1);
    assert!(matches!(get_tr_descr(&tr).compute_ph, TrComputePhase::Vm(_)));
    assert!(matches!(get_tr_descr(&tr).bounce, Some(TrBouncePhase::Ok(_))));
    assert_eq!(acc.status(), AccountStatus::AccStateActive);

    // ii. Bad code with accept
    let code = "ACCEPT CTOS";
    let due = 1064853u64;
    // message value is not enough to pay for storage
    let (acc, tr) = execute_custom_transaction(start_balance, code, data.clone(), 100, true, 0, 0);
    assert_eq!(get_tr_descr(&tr).storage_ph.unwrap().storage_fees_due, Some(due.into()));
    assert_eq!(get_tr_descr(&tr).credit_ph.unwrap().credit, CurrencyCollection::with_grams(0));
    assert!(matches!(get_tr_descr(&tr).compute_ph, TrComputePhase::Skipped(_)));
    assert_eq!(acc.status(), AccountStatus::AccStateActive);
    // message value is enough to pay for storage, but not enough to pay for compute value
    let (acc, tr) = execute_custom_transaction(start_balance, code, data.clone(), due + 1, true, 1, 0);
    assert_eq!(get_tr_descr(&tr).credit_ph.unwrap().credit, CurrencyCollection::with_grams(1));
    assert!(matches!(get_tr_descr(&tr).compute_ph, TrComputePhase::Skipped(_)));
    assert_eq!(acc.status(), AccountStatus::AccStateActive);
    // message value is enough to pay for storage and compute, but not enough to pay for bounce value
    let (acc, tr) = execute_custom_transaction(start_balance, code, data.clone(), 10_000_000, true, 9000000 - due, 0);
    assert!(matches!(get_tr_descr(&tr).compute_ph, TrComputePhase::Vm(_)));
    assert!(matches!(get_tr_descr(&tr).bounce, Some(TrBouncePhase::Nofunds(_))));
    assert_eq!(acc.status(), AccountStatus::AccStateActive);
    // message value is enough to pay for storage,compute and bounce value
    let (acc, tr) = execute_custom_transaction(start_balance, code, data.clone(), 13_000_000, true, 0, 1);
    assert!(matches!(get_tr_descr(&tr).compute_ph, TrComputePhase::Vm(_)));
    assert!(matches!(get_tr_descr(&tr).bounce, Some(TrBouncePhase::Ok(_))));
    assert_eq!(acc.status(), AccountStatus::AccStateActive);

    // iii. good code without accept
    let code = "NOP";
    let due = 1000u64;
    // message value is not enough to pay for storage
    let (acc, tr) = execute_custom_transaction(start_balance, code, data.clone(), 100, true, 0, 0);
    assert_eq!(get_tr_descr(&tr).storage_ph.unwrap().storage_fees_due, Some(due.into()));
    assert_eq!(get_tr_descr(&tr).credit_ph.unwrap().credit, CurrencyCollection::with_grams(0));
    assert!(matches!(get_tr_descr(&tr).compute_ph, TrComputePhase::Skipped(_)));
    assert_eq!(acc.status(), AccountStatus::AccStateActive);
    // message value is enough to pay for storage, but not enough to pay for compute value
    let (acc, tr) = execute_custom_transaction(start_balance, code, data.clone(), 10000, true, 10000 - due, 0);
    assert_eq!(get_tr_descr(&tr).credit_ph.unwrap().credit, CurrencyCollection::with_grams(9000));
    assert!(matches!(get_tr_descr(&tr).compute_ph, TrComputePhase::Skipped(_)));
    assert_eq!(acc.status(), AccountStatus::AccStateActive);
    // message value is enough to pay for storage and compute
    let (acc, tr) = execute_custom_transaction(start_balance, code, data.clone(), 12_000_000, true, 11_000_000 - due, 0);
    assert!(matches!(get_tr_descr(&tr).compute_ph, TrComputePhase::Vm(_)));
    assert!(matches!(get_tr_descr(&tr).bounce, None));
    assert_eq!(acc.status(), AccountStatus::AccStateActive);

    // iv. good code with accept
    let code = "ACCEPT";
    let due = 532927u64;
    // message value is not enough to pay for storage
    let (acc, tr) = execute_custom_transaction(start_balance, code, data.clone(), 100, true, 0, 0);
    assert_eq!(get_tr_descr(&tr).storage_ph.unwrap().storage_fees_due, Some(due.into()));
    assert_eq!(get_tr_descr(&tr).credit_ph.unwrap().credit, CurrencyCollection::with_grams(0));
    assert!(matches!(get_tr_descr(&tr).compute_ph, TrComputePhase::Skipped(_)));
    assert_eq!(acc.status(), AccountStatus::AccStateActive);
    // message value is enough to pay for storage, but not enough to pay for compute value
    let (acc, tr) = execute_custom_transaction(start_balance, code, data.clone(), due + 1, true, 1, 0);
    assert_eq!(get_tr_descr(&tr).credit_ph.unwrap().credit, CurrencyCollection::with_grams(1));
    assert!(matches!(get_tr_descr(&tr).compute_ph, TrComputePhase::Skipped(_)));
    assert_eq!(acc.status(), AccountStatus::AccStateActive);
    // message value is enough to pay for storage and compute
    let (acc, tr) = execute_custom_transaction(start_balance, code, data, 12_000_000, true, 11_000_000 - due, 0);
    assert!(matches!(get_tr_descr(&tr).compute_ph, TrComputePhase::Vm(_)));
    assert!(matches!(get_tr_descr(&tr).bounce, None));
    assert_eq!(acc.status(), AccountStatus::AccStateActive);

    // v. good code without accept and with sendmsg
    let code = "
        PUSHCTR C4
        PUSHINT 0
        SENDRAWMSG
    ";
    let out_msg = create_int_msg(
        AccountId::from([0x11; 32]),
        AccountId::from([0x33; 32]),
        11_000_000,
        false,
        BLOCK_LT + 1
    );
    let data = out_msg.serialize().unwrap();
    let due = 1000u64;
    let start_balance = 154258689 - due;
    // message value is not enough to pay for storage
    let (acc, tr) = execute_custom_transaction(start_balance, code, data.clone(), 100, true, 0, 0);
    assert_eq!(get_tr_descr(&tr).storage_ph.unwrap().storage_fees_due, Some(due.into()));
    assert_eq!(get_tr_descr(&tr).credit_ph.unwrap().credit, CurrencyCollection::with_grams(0));
    assert!(matches!(get_tr_descr(&tr).compute_ph, TrComputePhase::Skipped(_)));
    assert_eq!(acc.status(), AccountStatus::AccStateActive);
    // message value is enough to pay for storage, but not enough to pay for compute value
    let (acc, tr) = execute_custom_transaction(start_balance, code, data.clone(), 10000, true, 10000 - due, 0);
    assert_eq!(get_tr_descr(&tr).credit_ph.unwrap().credit, CurrencyCollection::with_grams(9000));
    assert!(matches!(get_tr_descr(&tr).compute_ph, TrComputePhase::Skipped(_)));
    assert_eq!(acc.status(), AccountStatus::AccStateActive);
    // message value is enough to pay for storage and compute, but not enough to send value
    let (acc, tr) = execute_custom_transaction(start_balance, code, data.clone(), 10_000_000, true, 4_250_000 - due, 0);
    assert!(matches!(get_tr_descr(&tr).compute_ph, TrComputePhase::Vm(_)));
    assert!(get_tr_descr(&tr).action.unwrap().no_funds);
    assert!(matches!(get_tr_descr(&tr).bounce, None));
    assert_eq!(acc.status(), AccountStatus::AccStateActive);
    // message value is enough to pay for storage,compute and send value
    let (acc, tr) = execute_custom_transaction(start_balance, code, data, 18_000_000, true, 1_250_000 - due, 1);
    assert!(matches!(get_tr_descr(&tr).compute_ph, TrComputePhase::Vm(_)));
    assert_eq!(get_tr_descr(&tr).action.unwrap().result_code, 0);
    assert!(matches!(get_tr_descr(&tr).bounce, None));
    assert_eq!(acc.status(), AccountStatus::AccStateActive);

    // vi. good code with accept and with sendmsg
    let code = "
        ACCEPT
        PUSHCTR C4
        PUSHINT 0
        SENDRAWMSG
    ";
    let out_msg = create_int_msg(
        AccountId::from([0x11; 32]),
        AccountId::from([0x33; 32]),
        11_000_000,
        false,
        BLOCK_LT + 1
    );
    let data = out_msg.serialize().unwrap();
    let due = 1063853u64;
    let start_balance = 154258689u64;
    // message value is not enough to pay for storage
    let (acc, tr) = execute_custom_transaction(start_balance, code, data.clone(), 100, true, 0, 0);
    assert_eq!(get_tr_descr(&tr).storage_ph.unwrap().storage_fees_due, Some(due.into()));
    assert_eq!(get_tr_descr(&tr).credit_ph.unwrap().credit, CurrencyCollection::with_grams(0));
    assert!(matches!(get_tr_descr(&tr).compute_ph, TrComputePhase::Skipped(_)));
    assert_eq!(acc.status(), AccountStatus::AccStateActive);
    // message value is enough to pay for storage, but not enough to pay for compute value
    let (acc, tr) = execute_custom_transaction(start_balance, code, data.clone(), due + 1, true, 1, 0);
    assert_eq!(get_tr_descr(&tr).credit_ph.unwrap().credit, CurrencyCollection::with_grams(1));
    assert!(matches!(get_tr_descr(&tr).compute_ph, TrComputePhase::Skipped(_)));
    assert_eq!(acc.status(), AccountStatus::AccStateActive);
    // message value is enough to pay for storage and compute, but not enough to send value
    let (acc, tr) = execute_custom_transaction(start_balance, code, data.clone(), 10_000_000, true, 3_990_000 - due, 0);
    assert!(matches!(get_tr_descr(&tr).compute_ph, TrComputePhase::Vm(_)));
    assert!(get_tr_descr(&tr).action.unwrap().no_funds);
    assert!(matches!(get_tr_descr(&tr).bounce, None));
    assert_eq!(acc.status(), AccountStatus::AccStateActive);
    // message value is enough to pay for storage,compute and send value
    let (acc, tr) = execute_custom_transaction(start_balance, code, data, 20_000_000, true, 2990000 - due, 1);
    assert!(matches!(get_tr_descr(&tr).compute_ph, TrComputePhase::Vm(_)));
    assert_eq!(get_tr_descr(&tr).action.unwrap().result_code, 0);
    assert!(matches!(get_tr_descr(&tr).bounce, None));
    assert_eq!(acc.status(), AccountStatus::AccStateActive);
}

fn execute_sendrawmsg_message(mode: u8, send_value: u64, result_balance: u64, count_out_msgs: usize) {
    let code = format!("
        ACCEPT
        PUSHCTR C4
        PUSHINT {}
        SENDRAWMSG
    ", mode);
    let out_msg = create_int_msg(
        AccountId::from([0x11; 32]),
        AccountId::from([0x33; 32]),
        send_value,
        false,
        BLOCK_LT
    );
    let data = out_msg.serialize().unwrap();
    let start_balance = 200_000_000;
    execute_custom_transaction(start_balance, &code, data.clone(), 20_000_000, false, result_balance, count_out_msgs);
    execute_custom_transaction(start_balance, &code, data, 20_000_000, true, result_balance, count_out_msgs);
}

#[test]
fn test_sendrawmsg_message() {
    execute_sendrawmsg_message(0, 14_000_000, 44_667_458, 1);
    execute_sendrawmsg_message(1, 14_000_000, 34_667_458, 1);
    execute_sendrawmsg_message(0, 0, 60_263_237, 0);
    execute_sendrawmsg_message(1, 0, 50_263_237, 1);

    execute_sendrawmsg_message(128, 14_000_000, 0, 1);
    execute_sendrawmsg_message(128+1, 14_000_000, 0, 1);
    execute_sendrawmsg_message(128+32, 14_000_000, 0, 1); // deleted account
    execute_sendrawmsg_message(128, 0, 0, 1);
    execute_sendrawmsg_message(128+1, 0, 0, 1);
    execute_sendrawmsg_message(128+32, 0, 0, 1); // deleted account

    execute_sendrawmsg_message(64, 14_000_000, 30_145_531, 1);
    execute_sendrawmsg_message(64+1, 14_000_000, 14_055_531, 1);
    execute_sendrawmsg_message(64, 0, 45_741_311, 1);
    execute_sendrawmsg_message(64+1, 0, 29_651_311, 1);

    // tests for flag +2 are considered in test_send_rawreserve_messages
}

#[test]
fn test_rand_seed() {
    let run_test = |code: Cell, seed: UInt256| {
        let out_msg = create_int_msg([0x11; 32].into(), [0; 32].into(), 1_000_000_000, false, BLOCK_LT);
        let data = out_msg.serialize().unwrap();
        let msg = create_int_msg(
            AccountId::from([0x33; 32]),
            AccountId::from([0x11; 32]),
            1_000_000_000,
            false,
            PREV_BLOCK_LT
        );
        let tr_lt = BLOCK_LT + 1;
        let lt = Arc::new(AtomicU64::new(tr_lt));
        let acc_id = AccountId::from([0x11; 32]);
        let mut acc = create_test_account(2_000_000_000, acc_id, code, data);
        let executor = OrdinaryTransactionExecutor::new(BLOCKCHAIN_CONFIG.to_owned());
        let trans = executor.execute_with_params(Some(&make_common(msg)), &mut acc, execute_params(lt, HashmapE::default(), seed, 0)).unwrap();
        assert_eq!(trans.out_msgs.len().unwrap(), 1);
    };

    let code = compile_code_to_cell("
        PUSHROOT
        RANDSEED
        THROWIFNOT 5
        PUSHINT 0
        SENDRAWMSG
    ").unwrap();
    run_test(code, UInt256::with_array([15; 32]));

    // if the user forgot to set the rand_seed_block value, then this 0 will be clearly visible on tests
    let code = compile_code_to_cell("
        PUSHROOT
        RANDSEED
        THROWIF 5
        PUSHINT 0
        SENDRAWMSG
    ").unwrap();
    run_test(code, UInt256::with_array([0; 32]));
}

#[test]
fn test_return_ihr_fee_in_message() {
    let mut acc = Account::with_address_and_ballance(
        &MsgAddressInt::with_standart(None, -1, RECEIVER_ACCOUNT.clone()).unwrap(),
        &0u64.into());

    let mut msg = create_int_msg(
        AccountId::from([0x11; 32]),
        RECEIVER_ACCOUNT.clone(),
        33334000000000u64,
        false,
        BLOCK_LT + 2
    );
    msg.int_header_mut().unwrap().ihr_fee = 183814500u64.into();
    msg.int_header_mut().unwrap().ihr_disabled = false;

    execute_c(&msg, &mut acc, BLOCK_LT + 1, 33334183814500u64, 0).unwrap();
}

#[test]
fn test_change_128_flag() {
    // message with 128 flag is processed last
    let code = "
        ACCEPT
        PUSHROOT
        CTOS
        LDREF
        PLDREF
        PUSHINT 128
        SENDRAWMSG
        PUSHINT 1
        SENDRAWMSG
    ";

    let start_balance = 310000000u64;
    let msg_income = 1230000000u64;
    let (acc, trans) = execute_custom_transaction(start_balance, code, create_two_messages_data(), msg_income, false, 0, 2);
    assert!(!acc.is_none());
    assert!(!trans.read_description().unwrap().is_aborted());
    // check ordering messages
    assert_eq!(
        trans.out_msgs.export_vector().unwrap()[1].0.get_std().unwrap().get_value().unwrap().grams,
        CurrencyCollection::with_grams(MSG2_BALANCE).grams,
    );
    for (i, msg) in trans.out_msgs.export_vector().unwrap().iter().enumerate() {
        assert_eq!(msg.0.get_std().unwrap().int_header().unwrap().created_lt, 2000000002 + i as u64);
    }

    // if money is not enough, transaction fail
    let code = "
        PUSHINT 10
        PUSHINT 0
        RAWRESERVE

        ACCEPT
        PUSHROOT
        CTOS
        LDREF
        PLDREF
        PUSHINT 128
        SENDRAWMSG
        PUSHINT 1
        SENDRAWMSG
    ";

    let start_balance = 310000000u64;
    let msg_income = 44404882u64 + 68259633 - 1;
    let (acc, trans) = execute_custom_transaction(start_balance, code, create_two_messages_data(), msg_income, false, 112252293, 0);
    assert!(!acc.is_none());
    assert!(trans.read_description().unwrap().is_aborted());

    // if money is not enough, with mode 2 message will be skipped
    let code = "
        ACCEPT
        PUSHROOT
        CTOS
        LDREF
        PLDREF
        PUSHINT 130
        SENDRAWMSG
        PUSHINT 1
        SENDRAWMSG
    ";

    let start_balance = 310000000u64;
    let msg_income = 44404882u64 + 68259633 - 1;
    let (acc, trans) = execute_custom_transaction(start_balance, code, create_two_messages_data(), msg_income, false, 9999999, 1);
    assert!(!acc.is_none());
    assert!(!trans.read_description().unwrap().is_aborted());

    // two messages with 128 flag is disabled
    let code = "
        ACCEPT
        PUSHROOT
        CTOS
        LDREF
        PLDREF
        PUSHINT 128
        SENDRAWMSG
        PUSHINT 128
        SENDRAWMSG
    ";

    let start_balance = 310000000u64;
    let msg_income = 1230000000u64;
    let (acc, trans) = execute_custom_transaction(start_balance, code, create_two_messages_data(), msg_income, false, 1236111632, 0);
    assert!(!acc.is_none());
    assert!(trans.read_description().unwrap().is_aborted());

    // two messages with 128 flag is disabled, but flag 2 ignores error
    let code = "
        ACCEPT
        PUSHROOT
        CTOS
        LDREF
        PLDREF
        PUSHINT 128
        SENDRAWMSG
        PUSHINT 130
        SENDRAWMSG
    ";

    let start_balance = 310000000u64;
    let msg_income = 1230000000u64;
    let (acc, trans) = execute_custom_transaction(start_balance, code, create_two_messages_data(), msg_income, false, 0, 1);
    assert!(!acc.is_none());
    assert!(!trans.read_description().unwrap().is_aborted());

    // message with 128 flag is processed after rawreserve
    let code = "
        ACCEPT
        PUSHROOT
        CTOS
        LDREF
        PLDREF
        PUSHINT 128
        SENDRAWMSG
        PUSHINT 1000
        PUSHINT 0
        RAWRESERVE
    ";

    let start_balance = 310000000u64;
    let msg_income = 1230000000u64;
    let (acc, trans) = execute_custom_transaction(start_balance, code, create_two_messages_data(), msg_income, false, 1000, 1);
    assert!(!acc.is_none());
    assert!(!trans.read_description().unwrap().is_aborted());

    // message with 128+32 flag is processed last
    let code = "
        ACCEPT
        PUSHROOT
        CTOS
        LDREF
        PLDREF
        PUSHINT 160
        SENDRAWMSG
        PUSHINT 0
        SENDRAWMSG
    ";

    let start_balance = 310000000u64;
    let msg_income = 1230000000u64;
    let (acc, trans) = execute_custom_transaction(start_balance, code, create_two_messages_data(), msg_income, false, 0, 2);
    assert!(acc.is_none());
    assert!(!trans.read_description().unwrap().is_aborted());

    // message with 32 flag is not valid without 128 flag
    let code = "
        ACCEPT
        PUSHROOT
        CTOS
        LDREF
        PLDREF
        PUSHINT 32
        SENDRAWMSG
        PUSHINT 0
        SENDRAWMSG
    ";

    let start_balance = 310000000u64;
    let msg_income = 1230000000u64;
    let (acc, trans) = execute_custom_transaction(start_balance, code, create_two_messages_data(), msg_income, false, 1237947412, 0);
    assert!(!acc.is_none());
    assert!(trans.read_description().unwrap().is_aborted());

    // if there is reserved value, then do not remove account
    let code = "
        PUSHINT 10
        PUSHINT 0
        RAWRESERVE

        ACCEPT
        PUSHROOT
        CTOS
        LDREF
        PLDREF
        PUSHINT 160
        SENDRAWMSG
        PUSHINT 1
        SENDRAWMSG
    ";

    let start_balance = 3100000000u64;
    let msg_income = 444048082u64;
    let (acc, _) = execute_custom_transaction(start_balance, code, create_two_messages_data(), msg_income, false, 10, 2);
    assert!(!acc.is_none());

    // if action phase aborted, account will not destroyed
    let code = "
        ACCEPT
        PUSHROOT
        CTOS
        LDREF
        PLDREF
        PUSHINT 160
        SENDRAWMSG
        PUSHINT 160
        SENDRAWMSG
    ";

    let start_balance = 3100000000u64;
    let msg_income = 444048082u64;
    let (acc, _) = execute_custom_transaction(start_balance, code, create_two_messages_data(), msg_income, false, 3240159714u64, 0);
    assert!(!acc.is_none());
}

#[allow(clippy::too_many_arguments)]
fn test_uninit_account(
    code: Option<&Cell>,
    msg_balance: u64,
    bounce: bool,
    begin_status: AccountStatus,
    end_status: AccountStatus,
    addr_eq_state_hash: bool,
    result_account_balance: u64,
    count_out_msgs: usize,
    config: &ConfigParams,
) {
    let mut state_init = StateInit::default();
    if let Some(code) = code {
        state_init.set_code(code.clone());
    }

    let tr_lt = BLOCK_LT + 1;
    let lt = Arc::new(AtomicU64::new(tr_lt));
    let mut acc;
    let mut acc_id = AccountId::from([0x11; 32]);
    if begin_status == AccountStatus::AccStateNonexist {
        if addr_eq_state_hash {
            acc_id = AccountId::from(state_init.hash().unwrap())
        }
        acc = Account::default();
    } else if begin_status == AccountStatus::AccStateUninit {
        if addr_eq_state_hash {
            acc_id = AccountId::from(state_init.hash().unwrap())
        }
        acc = Account::with_address(MsgAddressInt::with_standart(None, -1, acc_id.clone()).unwrap());
    } else {
        panic!("Incorrect begin Account Status");
    }

    let mut msg = create_int_msg(
        AccountId::from([0x33; 32]),
        acc_id,
        msg_balance,
        bounce,
        PREV_BLOCK_LT
    );
    if code.is_some() {
        msg.set_state_init(state_init);
    }
    let acc_copy = acc.clone();
    let executor = OrdinaryTransactionExecutor::new(BlockchainConfig::with_config(config.clone()).unwrap());
    let trans = executor.execute_with_params(Some(&make_common(msg.clone())), &mut acc, execute_params_none(lt)).unwrap();
    assert_eq!(acc.status(), end_status);

    check_account_and_transaction(&acc_copy, &acc, &msg, Some(&trans), result_account_balance, count_out_msgs);
}

fn test_uninit_account_initstate_default(
    msg_balance: u64,
    bounce: bool,
    begin_status: AccountStatus,
    end_status: AccountStatus,
    result_account_balance: u64,
    count_out_msgs: usize,
    config: &ConfigParams,
) {
    let tr_lt = BLOCK_LT + 1;
    let lt = Arc::new(AtomicU64::new(tr_lt));
    let mut acc;
    let acc_id = AccountId::from([0x11; 32]);
    if begin_status == AccountStatus::AccStateNonexist {
        acc = Account::default();
    } else if begin_status == AccountStatus::AccStateUninit {
        acc = Account::with_address(MsgAddressInt::with_standart(None, -1, acc_id.clone()).unwrap());
    } else {
        panic!("Incorrect begin Account Status");
    }

    let mut msg = create_int_msg(
        AccountId::from([0x33; 32]),
        acc_id,
        msg_balance,
        bounce,
        PREV_BLOCK_LT
    );
    msg.set_state_init(StateInit::default());
    let acc_copy = acc.clone();
    let executor = OrdinaryTransactionExecutor::new(BlockchainConfig::with_config(config.clone()).unwrap());
    let trans = executor.execute_with_params(Some(&make_common(msg.clone())), &mut acc, execute_params_none(lt)).unwrap();
    assert_eq!(acc.status(), end_status);

    check_account_and_transaction(&acc_copy, &acc, &msg, Some(&trans), result_account_balance, count_out_msgs);
}

#[test]
fn test_uninit_accounts() {
    let code = compile_code_to_cell("
        PUSHROOT
        SENDRAWMSG
    ").unwrap();

    let mut config = ConfigParams::construct_from_file("real_ever_boc/config.boc").unwrap();
    let mut capabilities = 0x2e;
    let global_version = GlobalVersion {
        version: 0,
        capabilities,
    };

    config.set_config(ConfigParamEnum::ConfigParam8(ConfigParam8 {global_version})).unwrap();

    // code hash matches with account address
    test_uninit_account(Some(&code), 1_000_000_000, false, AccountStatus::AccStateNonexist, AccountStatus::AccStateActive, true, 990_000_000, 0, &config);
    // code hash does not match with account address
    test_uninit_account(Some(&code), 1_000_000_000, false, AccountStatus::AccStateNonexist, AccountStatus::AccStateUninit, false, 1_000_000_000, 0, &config);
    // code hash does not match with account address
    test_uninit_account(Some(&code), 1_000_000_000, false, AccountStatus::AccStateUninit, AccountStatus::AccStateUninit, false, 1_000_000_000, 0, &config);
    // not enougt money to execute
    test_uninit_account(Some(&code), 1000, false, AccountStatus::AccStateNonexist, AccountStatus::AccStateUninit, false, 1000, 0, &config);
    // code hash matches with account address
    test_uninit_account(Some(&code), 1_000_000_000, false, AccountStatus::AccStateUninit, AccountStatus::AccStateActive, true, 990_000_000, 0, &config);
    // not enougt money to execute
    test_uninit_account(Some(&code), 1000, false, AccountStatus::AccStateUninit, AccountStatus::AccStateUninit, true, 1000, 0, &config);
    // absence of code for init account
    test_uninit_account(None, 1_000_000_000, false, AccountStatus::AccStateUninit, AccountStatus::AccStateUninit, false, 1_000_000_000, 0, &config);

    // if message has money, change account to AccStateUninit state
    test_uninit_account(None, 1_000_000_000, false, AccountStatus::AccStateNonexist, AccountStatus::AccStateUninit, false, 1_000_000_000, 0, &config);
    test_uninit_account(None, 1_000, false, AccountStatus::AccStateNonexist, AccountStatus::AccStateUninit, false, 1_000, 0, &config);
    // if bounce, account no need to create
    test_uninit_account(None, 1_000_000_000, true, AccountStatus::AccStateNonexist, AccountStatus::AccStateNonexist, false, 0, 1, &config);
    // message has not money
    test_uninit_account(None, 0, false, AccountStatus::AccStateNonexist, AccountStatus::AccStateNonexist, false, 0, 0, &config);

    capabilities |= GlobalCapabilities::CapInitCodeHash as u64;
    let global_version = GlobalVersion {
        version: 0,
        capabilities,
    };
    config.set_config(ConfigParamEnum::ConfigParam8(ConfigParam8 {global_version})).unwrap();

    // code hash matches with account address
    test_uninit_account(Some(&code), 1_000_000_000, false, AccountStatus::AccStateNonexist, AccountStatus::AccStateActive, true, 990_000_000, 0, &config);
    // code hash does not match with account address
    test_uninit_account(Some(&code), 1_000_000_000, false, AccountStatus::AccStateNonexist, AccountStatus::AccStateUninit, false, 1_000_000_000, 0, &config);
    // code hash does not match with account address
    test_uninit_account(Some(&code), 1_000_000_000, false, AccountStatus::AccStateUninit, AccountStatus::AccStateUninit, false, 1_000_000_000, 0, &config);
    // not enougt money to execute
    test_uninit_account(Some(&code), 1000, false, AccountStatus::AccStateNonexist, AccountStatus::AccStateUninit, false, 1000, 0, &config);
    // code hash matches with account address
    test_uninit_account(Some(&code), 1_000_000_000, false, AccountStatus::AccStateUninit, AccountStatus::AccStateActive, true, 990_000_000, 0, &config);
    // not enougt money to execute
    test_uninit_account(Some(&code), 1000, false, AccountStatus::AccStateUninit, AccountStatus::AccStateUninit, true, 1000, 0, &config);
    // absence of code for init account
    test_uninit_account(None, 1_000_000_000, false, AccountStatus::AccStateUninit, AccountStatus::AccStateUninit, false, 1_000_000_000, 0, &config);

    // if message has money, change account to AccStateUninit state
    test_uninit_account(None, 1_000_000_000, false, AccountStatus::AccStateNonexist, AccountStatus::AccStateUninit, false, 1_000_000_000, 0, &config);
    test_uninit_account(None, 1_000, false, AccountStatus::AccStateNonexist, AccountStatus::AccStateUninit, false, 1_000, 0, &config);
    // if bounce, account no need to create
    test_uninit_account(None, 1_000_000_000, true, AccountStatus::AccStateNonexist, AccountStatus::AccStateNonexist, false, 0, 1, &config);
    // message has not money
    test_uninit_account(None, 0, false, AccountStatus::AccStateNonexist, AccountStatus::AccStateNonexist, false, 0, 0, &config);

    // if init state is default, change account to AccStateUninit that save moneys
    test_uninit_account_initstate_default(1000, false, AccountStatus::AccStateNonexist, AccountStatus::AccStateUninit, 1000, 0, &config);
}

#[test]
fn adjust_msg_value() {
    let code = "
        ACCEPT
        PUSHCTR C4
        PUSHINT 64
        SENDRAWMSG
    ";
    let start_balance = 0;

    let acc_id = AccountId::from([0x22; 32]);
    let out_msg = create_int_msg(
        acc_id.clone(),
        acc_id.clone(),
        0,
        false,
        BLOCK_LT - 2
    );
    let code = compile_code_to_cell(code).unwrap();
    let data = out_msg.serialize().unwrap();

    let mut acc = create_test_account(start_balance, acc_id.clone(), code, data);

    let msg = create_int_msg(
        AccountId::from([0x33; 32]),
        acc_id,
        1_400_200_000,
        false,
        BLOCK_LT - 2
    );

    execute_c(&msg, &mut acc, BLOCK_LT + 1, 0, 1).unwrap();
}

#[test]
fn test_check_replace_src_addr() {
    let code = "
        ACCEPT
        PUSHCTR C4
        PUSHINT 0
        SENDRAWMSG
    ";
    let start_balance = 0;

    let acc_id = AccountId::from([0x22; 32]);
    let out_msg = create_int_msg(
        AccountId::from([0x11; 32]),
        acc_id.clone(),
        1_000_000_000,
        false,
        BLOCK_LT - 2
    );
    let code = compile_code_to_cell(code).unwrap();
    let data = out_msg.serialize().unwrap();

    let mut acc = create_test_account(start_balance, acc_id.clone(), code, data);

    let msg = create_int_msg(
        AccountId::from([0x33; 32]),
        acc_id,
        1_400_200_000,
        false,
        BLOCK_LT - 2
    );

    let trans = execute_c(&msg, &mut acc, BLOCK_LT + 1, 1_240_463_237, 0).unwrap();
    let descr = trans.read_description().unwrap();
    assert_eq!(descr.action_phase_ref().unwrap().result_code, 35);
}

#[test]
fn special_account() {
    let code = "
        ACCEPT
        PUSHCTR C4
        PUSHINT 0
        SENDRAWMSG
    ";
    let start_balance = 200_000_000;

    let acc_id = AccountId::from([0x33; 32]);
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
    assert!(BLOCKCHAIN_CONFIG.is_special_account(acc.get_addr().unwrap()).unwrap());

    let msg = create_int_msg(
        AccountId::from([0x33; 32]),
        acc_id,
        14_200_000,
        false,
        BLOCK_LT - 2
    );

    execute_c(&msg, &mut acc, BLOCK_LT + 1, 204_200_000, 1).unwrap();
    assert_eq!(acc.last_paid(), 0);
}

#[test]
fn test_fail_bound_message_with_nonexist_account() {
    let mut acc = Account::default();

    let msg_income = 46372;
    let msg = create_int_msg(
        AccountId::from([0x33; 32]),
        AccountId::from([0x11; 32]),
        msg_income,
        true,
        BLOCK_LT
    );

    let tr_lt = BLOCK_LT + 2;
    let trans = execute_c(&msg, &mut acc, tr_lt, 46372, 0).unwrap();

    let mut new_acc = Account::with_address_and_ballance(
        &MsgAddressInt::with_standart(None, -1, AccountId::from([0x11; 32])).unwrap(),
        &msg_income.into());
    new_acc.set_last_paid(1576526553);
    new_acc.set_last_tr_time(BLOCK_LT + 3);
    new_acc.update_storage_stat().unwrap();
    assert_eq!(acc, new_acc);

    let mut description = TransactionDescrOrdinary::default();
    description.storage_ph = Some(TrStoragePhase::with_params(Grams::zero(), None, AccStatusChange::Unchanged));
    description.credit_ph = Some(TrCreditPhase::with_params(None, CurrencyCollection::with_grams(msg_income)));
    description.compute_ph = TrComputePhase::skipped(ComputeSkipReason::NoGas);

    description.bounce = Some(
        TrBouncePhase::Nofunds(TrBouncePhaseNofunds {
            msg_size: StorageUsedShort::default(),
            req_fwd_fees: 10000000u64.into()
        }),
    );

    description.action = None;
    description.credit_first = false;
    description.aborted = true;
    description.destroyed = false;
    let description = TransactionDescr::Ordinary(description);

    let mut good_trans = Transaction::with_account_and_message(&new_acc, &msg, BLOCK_LT + 2).unwrap();

    good_trans.set_total_fees(CurrencyCollection::with_grams(0));
    good_trans.orig_status = AccountStatus::AccStateNonexist;
    good_trans.set_end_status(AccountStatus::AccStateUninit);
    good_trans.set_logical_time(BLOCK_LT + 2);
    good_trans.set_now(BLOCK_UT);

    good_trans.write_description(&description).unwrap();

    assert_eq!(trans.read_description().unwrap(), description);
    assert_eq!(trans, good_trans);
}

#[test]
fn test_fail_bound_message_with_nonexist_account_2() {
    let mut acc = Account::default();

    let msg_income = 0;
    let msg = create_int_msg(
        AccountId::from([0x33; 32]),
        AccountId::from([0x11; 32]),
        msg_income,
        true,
        BLOCK_LT
    );

    let tr_lt = BLOCK_LT + 2;
    let trans = execute_c(&msg, &mut acc, tr_lt, 0, 0).unwrap();

    let new_acc = Account::default();
    assert_eq!(acc, new_acc);

    let mut description = TransactionDescrOrdinary::default();
    description.storage_ph = Some(TrStoragePhase::with_params(Grams::zero(), None, AccStatusChange::Unchanged));
    description.credit_ph = Some(TrCreditPhase::with_params(None, CurrencyCollection::with_grams(msg_income)));
    description.compute_ph = TrComputePhase::skipped(ComputeSkipReason::NoGas);

    description.bounce = Some(
        TrBouncePhase::Nofunds(TrBouncePhaseNofunds {
            msg_size: StorageUsedShort::default(),
            req_fwd_fees: 10000000u64.into()
        }),
    );

    description.action = None;
    description.credit_first = false;
    description.aborted = true;
    description.destroyed = false;
    let description = TransactionDescr::Ordinary(description);

    let mut good_trans = Transaction::with_account_and_message(&new_acc, &msg, BLOCK_LT + 2).unwrap();

    good_trans.set_total_fees(CurrencyCollection::with_grams(msg_income));
    good_trans.orig_status = AccountStatus::AccStateNonexist;
    good_trans.set_end_status(AccountStatus::AccStateNonexist);
    good_trans.set_logical_time(BLOCK_LT + 2);
    good_trans.set_now(BLOCK_UT);

    good_trans.write_description(&description).unwrap();

    assert_eq!(trans.read_description().unwrap(), description);
    assert_eq!(trans, good_trans);
}

#[test]
fn account_without_code() {
    let state_init = StateInit::default();
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

    let trans = execute_c(&msg, &mut acc, BLOCK_LT + 1, 9999999984737712408u64, 0).unwrap();
    if let TrComputePhase::Vm(vm) = trans.read_description().unwrap().compute_phase_ref().unwrap() {
        assert_eq!(vm.exit_code, -13);
    } else {
        unreachable!()
    }
}

#[test]
fn account_without_code2() {
    let state_init = StateInit::default();
    let acc_id = AccountId::from(state_init.hash().unwrap());

    let mut acc = create_test_account(
        10000000000000000000u64, 
        acc_id.clone(), 
        create_send_two_messages_code(), 
        create_two_messages_data()
    );
    *acc.state_init_mut().unwrap() = state_init.clone();

    let mut msg = create_int_msg(
        AccountId::from([0x33; 32]),
        acc_id,
        14_200_000,
        false,
        BLOCK_LT - 2
    );
    msg.set_state_init(state_init);

    let trans = execute_c(&msg, &mut acc, BLOCK_LT + 1, 9999999999722701632u64, 0).unwrap();
    if let TrComputePhase::Vm(vm) = trans.read_description().unwrap().compute_phase_ref().unwrap() {
        assert_eq!(vm.exit_code, -13);
    } else {
        unreachable!()
    }
}

#[test]
fn account_without_data() {
    let mut state_init = StateInit::default();
    state_init.set_code(compile_code_to_cell("NOP").unwrap());
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

    let trans = execute_c(&msg, &mut acc, BLOCK_LT + 1, 9999999984737712408u64, 0).unwrap();
    if let TrComputePhase::Vm(vm) = trans.read_description().unwrap().compute_phase_ref().unwrap() {
        assert_eq!(vm.exit_code, 0);
    } else {
        unreachable!()
    }
}

#[test]
fn incorrect_acc_timestamp() {
    let acc_id = AccountId::from([0x11; 32]);
    let start_balance = 20000000000000u64;
    let msg = create_int_msg(
        AccountId::from([0x33; 32]),
        acc_id.clone(),
        14_200_000,
        false,
        BLOCK_LT - 2
    );

    let tr_lt = BLOCK_LT + 1;
    let code = compile_code_to_cell("NOP").unwrap();
    let mut acc = create_test_account(start_balance, acc_id, code, create_two_messages_data());
    acc.set_last_paid(acc.last_paid() + 1000000000);
    let err = execute(&msg, &mut acc, tr_lt).expect_err("no accept error must be generated");
    assert!(matches!(err.downcast::<ExecutorError>().unwrap(), ExecutorError::TrExecutorError(_)));
}

#[test]
fn sendmsg_64_fail() {
    let code = "
        PUSHROOT
        CTOS
        LDREF
        PLDREF

        SWAP

        PUSHINT 67
        SENDRAWMSG
        PUSHINT 64
        SENDRAWMSG
    ";
    let data = create_two_messages_data();
    execute_custom_transaction(410_000_000 - 21097413, code, data, 41_000_000, false, 49999999, 1);
}

#[test]
fn test_account_with_enought_balance_to_run_compute_phase() {
    let acc_id = AccountId::from([0x66; 32]);
    let mut acc = create_test_account(0, acc_id.clone(), create_send_two_messages_code(), create_two_messages_data());
    let mut msg = create_test_external_msg();
    if let CommonMsgInfo::ExtInMsgInfo(header) = msg.header_mut() {
        header.dst = MsgAddressInt::with_standart(None, -1, acc_id).unwrap();
    } else {
        unreachable!()
    }

    let err = execute(&msg, &mut acc, BLOCK_LT + 1).expect_err("no funds for external message");
    assert_eq!(err.downcast::<ExecutorError>().unwrap(), ExecutorError::ExtMsgComputeSkipped(ComputeSkipReason::NoGas));
}

fn make_transaction_from_workchain_to_masterchain(from_worckchain_to_masterchain: bool, result_acc_balance: u64, bounce: bool) {
    let (w_id_src, w_id_dst) = if from_worckchain_to_masterchain {
        (0, -1)
    } else {
        (-1, 0)
    };
    let code = if bounce {"CTOS"} else {"
        ACCEPT
        PUSHCTR C4
        PUSHINT 0
        SENDRAWMSG
    "};
    let start_balance = 200_000_000;

    let acc_id = AccountId::from([0x11; 32]);
    let in_msg = create_int_msg_workchain(
        w_id_dst,
        acc_id.clone(),
        acc_id.clone(),
        10_000_000,
        false,
        BLOCK_LT - 2
    );
    let code = compile_code_to_cell(code).unwrap();
    let data = in_msg.serialize().unwrap();

    let mut acc = create_test_account_workchain(start_balance, w_id_dst, acc_id.clone(), code, data);

    let mut msg = create_int_msg_workchain(
        w_id_dst,
        AccountId::from([0x22; 32]),
        acc_id,
        14_200_000,
        bounce,
        BLOCK_LT - 2
    );
    msg.set_src_address(MsgAddressInt::with_standart(None, w_id_src, AccountId::from([0x22; 32])).unwrap());

    let tr = execute_c(&msg, &mut acc, BLOCK_LT + 1, result_acc_balance, 1).unwrap();
    if bounce {
        assert!(matches!(get_tr_descr(&tr).bounce.unwrap(), TrBouncePhase::Ok(_)));
    }
}

#[test]
fn send_message_from_workchain_to_masterchain() {
    make_transaction_from_workchain_to_masterchain(true, 42867458, false);
    make_transaction_from_workchain_to_masterchain(false, 203443677, false);

    make_transaction_from_workchain_to_masterchain(true, 47869017, true);
    make_transaction_from_workchain_to_masterchain(false, 199847869, true);
}

fn make_transaction_from_workchain_to_masterchain2(from_worckchain_to_masterchain: bool, result_acc_balance: u64) {
    let (w_id_src, w_id_dst) = if from_worckchain_to_masterchain {
        (0, -1)
    } else {
        (-1, 0)
    };
    let code = "
        ACCEPT
        PUSHCTR C4
        PUSHINT 0
        SENDRAWMSG
    ";
    let start_balance = 200_000_000;

    let acc_id = AccountId::from([0x11; 32]);
    let mut in_msg = create_int_msg_workchain(
        w_id_dst,
        acc_id.clone(),
        acc_id.clone(),
        10_000_000,
        false,
        BLOCK_LT - 2
    );
    in_msg.set_src_address(MsgAddressInt::with_standart(None, w_id_src, AccountId::from([0x11; 32])).unwrap());
    let code = compile_code_to_cell(code).unwrap();
    let data = in_msg.serialize().unwrap();

    let mut acc = create_test_account_workchain(start_balance, w_id_src, acc_id.clone(), code, data);

    let msg = create_int_msg_workchain(
        w_id_src,
        AccountId::from([0x22; 32]),
        acc_id,
        14_200_000,
        false,
        BLOCK_LT - 2
    );

    execute_c(&msg, &mut acc, BLOCK_LT + 1, result_acc_balance, 1).unwrap();
}

#[test]
fn send_message_from_workchain_to_masterchain2() {
    make_transaction_from_workchain_to_masterchain2(true, 203443677);
    make_transaction_from_workchain_to_masterchain2(false, 42867458);
}

#[test]
fn test_message_with_anycast_output_address() {
    let start_balance = 300_000_000_000u64;
    let msg_income = 100_000_000_000u64;

    let dst = MsgAddressInt::with_standart(
        Some(AnycastInfo::with_rewrite_pfx(SliceData::new(vec![0x22; 3])).unwrap()), 
        0, 
        AccountId::from([0x11; 32])
    ).unwrap();
    let mut hdr = InternalMessageHeader::with_addresses(
        MsgAddressInt::with_standart(None, 0, AccountId::from([0x11; 32])).unwrap(),
        dst,
        CurrencyCollection::with_grams(msg_income),
    );
    hdr.bounce = false;
    hdr.ihr_disabled = true;
    hdr.created_lt = PREV_BLOCK_LT;
    hdr.created_at = UnixTime32::default();
    let out_msg = Message::with_int_header(hdr);

    let data = out_msg.serialize().unwrap();
    let code = compile_code_to_cell("
        PUSHROOT
        PUSHINT 2
        SENDRAWMSG
    ").unwrap();
    let acc_id = AccountId::from([0x11; 32]);
    let mut acc = create_test_account_workchain(start_balance, 0, acc_id.clone(), code, data);
    acc.set_last_paid(BLOCK_UT - 100);
    let msg = create_int_msg_workchain(
        0,
        AccountId::from([0x33; 32]),
        AccountId::from([0x11; 32]),
        msg_income,
        true,
        PREV_BLOCK_LT
    );

    disable_cross_check();
    let tr_lt = BLOCK_LT + 1;
    let trans = execute_c(&msg, &mut acc, tr_lt, 399999424996u64, 0).unwrap();

    let mut new_acc = create_test_account_workchain(
        399999424996u64,
        0, 
        acc_id, 
        acc.get_code().unwrap(), 
        acc.get_data().unwrap()
    );
    new_acc.set_last_paid(BLOCK_UT);
    new_acc.set_last_tr_time(BLOCK_LT + 2);
    assert_eq!(acc, new_acc);

    let mut description = TransactionDescrOrdinary::default();
    description.storage_ph = Some(TrStoragePhase {
        storage_fees_collected: 4u64.into(),
        storage_fees_due: None,
        status_change: AccStatusChange::Unchanged
    });
    description.credit_ph = Some(TrCreditPhase {
        due_fees_collected: None,
        credit: CurrencyCollection::with_grams(msg_income),
    });
    let mut vm_phase = TrComputePhaseVm::default();
    vm_phase.success = true;
    vm_phase.gas_used = 575u32.into();
    vm_phase.gas_limit = 1000000u32.into();
    vm_phase.gas_fees = 575000u64.into();
    vm_phase.vm_steps = 4;
    description.compute_ph = TrComputePhase::Vm(vm_phase);

    description.action = Some(TrActionPhase {
        success: true,
        valid: true,
        tot_actions: 1,
        msgs_created: 0, // incorrect value ignore, if success == false
        action_list_hash: "0x0695bc3c098f64c83a812ddfb987e1aa1e463fe90f663e182ba02ef5aab9de24".parse().unwrap(),
        ..TrActionPhase::default()
    });

    description.credit_first = false;
    description.bounce = None;
    description.aborted = false;
    description.destroyed = false;
    let description = TransactionDescr::Ordinary(description);

    let mut good_trans = Transaction::with_account_and_message(&new_acc, &msg, tr_lt).unwrap();

    good_trans.set_total_fees(CurrencyCollection::with_grams(575004));
    good_trans.set_now(BLOCK_UT);

    good_trans.write_description(&description).unwrap();

    assert_eq!(trans.read_description().unwrap(), description);
    assert_eq!(trans, good_trans);
}

#[test]
fn test_message_with_anycast_output_address_2() {
    let start_balance = 300_000_000_000u64;
    let msg_income = 100_000_000_000u64;

    let dst = MsgAddressInt::with_standart(
        Some(AnycastInfo::with_rewrite_pfx(SliceData::new(vec![0x22; 3])).unwrap()), 
        0, 
        AccountId::from([0x11; 32])
    ).unwrap();
    let mut hdr = InternalMessageHeader::with_addresses(
        MsgAddressInt::with_standart(None, 0, AccountId::from([0x11; 32])).unwrap(),
        dst,
        CurrencyCollection::with_grams(msg_income),
    );
    hdr.bounce = false;
    hdr.ihr_disabled = true;
    hdr.created_lt = PREV_BLOCK_LT;
    hdr.created_at = UnixTime32::default();
    let out_msg = Message::with_int_header(hdr);

    let data = out_msg.serialize().unwrap();
    let code = compile_code_to_cell("
        PUSHROOT
        PUSHINT 0
        SENDRAWMSG
    ").unwrap();
    let acc_id = AccountId::from([0x11; 32]);
    let mut acc = create_test_account_workchain(start_balance, 0, acc_id.clone(), code, data);
    acc.set_last_paid(BLOCK_UT - 100);
    let msg = create_int_msg_workchain(
        0,
        AccountId::from([0x33; 32]),
        AccountId::from([0x11; 32]),
        msg_income,
        true,
        PREV_BLOCK_LT
    );

    disable_cross_check();
    let tr_lt = BLOCK_LT + 1;
    let trans = execute_c(&msg, &mut acc, tr_lt, 399999424996u64, 0).unwrap();

    let mut new_acc = create_test_account_workchain(399999424996u64, 0, acc_id, acc.get_code().unwrap(), acc.get_data().unwrap());
    new_acc.set_last_paid(BLOCK_UT);
    new_acc.set_last_tr_time(BLOCK_LT + 2);
    assert_eq!(acc, new_acc);

    let mut description = TransactionDescrOrdinary::default();
    description.storage_ph = Some(TrStoragePhase {
        storage_fees_collected: 4u64.into(),
        storage_fees_due: None,
        status_change: AccStatusChange::Unchanged
    });
    description.credit_ph = Some(TrCreditPhase {
        due_fees_collected: None,
        credit: CurrencyCollection::with_grams(msg_income),
    });
    let mut vm_phase = TrComputePhaseVm::default();
    vm_phase.success = true;
    vm_phase.gas_used = 575u32.into();
    vm_phase.gas_limit = 1000000u32.into();
    vm_phase.gas_fees = 575000u64.into();
    vm_phase.vm_steps = 4;
    description.compute_ph = TrComputePhase::Vm(vm_phase);

    let mut action_ph = TrActionPhase::default();
    action_ph.success = false;
    action_ph.valid = true;
    action_ph.status_change = AccStatusChange::Unchanged;
    action_ph.tot_actions = 1;
    action_ph.msgs_created = 0; // incorrect value ignore, if success == false
    action_ph.action_list_hash = "0x221ff324345e3bb2b1f6d6277d3613c9a17048d4d3b6b34f6103c93cf4b427d1".parse().unwrap();
    action_ph.result_code = 50;
    action_ph.result_arg = None;
    action_ph.no_funds = false;
    action_ph.tot_msg_size = StorageUsedShort::default();  // incorrect value ignore, if success == false

    description.action = Some(action_ph);

    description.credit_first = false;
    description.bounce = None;
    description.aborted = true;
    description.destroyed = false;
    let description = TransactionDescr::Ordinary(description);

    let mut good_trans = Transaction::with_account_and_message(&new_acc, &msg, tr_lt).unwrap();

    good_trans.set_total_fees(CurrencyCollection::with_grams(575004));
    good_trans.set_now(BLOCK_UT);

    good_trans.write_description(&description).unwrap();

    assert_eq!(trans.read_description().unwrap(), description);
    assert_eq!(trans, good_trans);
}

#[test]
fn test_message_with_anycast_address() {
    let msg_income = 100_000_000_000u64; // send enough money to pay storage fee and to run contract
    let mut acc = Account::default();

    let dst = MsgAddressInt::with_standart(
        Some(AnycastInfo::with_rewrite_pfx(SliceData::new(vec![0x22; 3])).unwrap()),
        0,
        AccountId::from([0x11; 32])
    ).unwrap();
    let mut hdr = InternalMessageHeader::with_addresses(
        MsgAddressInt::with_standart(None, 0, AccountId::from([0x33; 32])).unwrap(),
        dst,
        CurrencyCollection::with_grams(msg_income),
    );
    hdr.bounce = false;
    hdr.ihr_disabled = true;
    hdr.created_lt = PREV_BLOCK_LT;
    hdr.created_at = UnixTime32::default();
    let msg = Message::with_int_header(hdr);

    let config = custom_config(
        Some(VERSION_BLOCK_REVERT_MESSAGES_WITH_ANYCAST_ADDRESSES),
        None
    );

    let tr_lt = BLOCK_LT + 1;
    disable_cross_check();
    let trans = execute_with_params(config, &msg, &mut acc, tr_lt).unwrap();

    let new_acc = Account::default();
    assert_eq!(acc, new_acc);

    let mut description = TransactionDescrOrdinary::default();

    description.credit_first = true;
    description.bounce = None;
    description.aborted = true;
    description.destroyed = false;
    let description = TransactionDescr::Ordinary(description);

    let mut good_trans = Transaction::with_account_and_message(&new_acc, &msg, tr_lt).unwrap();

    good_trans.set_total_fees(CurrencyCollection::with_grams(0));
    good_trans.set_now(BLOCK_UT);

    good_trans.write_description(&description).unwrap();

    assert_eq!(trans.read_description().unwrap(), description);
    assert_eq!(trans, good_trans);
}

#[test]
fn test_action_phase_fields_with_unsuccess_action() {
    let start_balance = 100000000;
    let msg_income = 1_000_000_u64;
    let storage_fee = 6618;
    let total_balance = 87923382;
    let gas_used = 1307u32;
    let gas_fees = 13070000;
    let msg_fwd_fee = MSG_FWD_FEE;
    let msg_mine_fee = MSG_MINE_FEE;
    let total_fees = gas_fees + storage_fee;

    let code = compile_code_to_cell("
        ACCEPT
        PUSHROOT
        CTOS
        LDREF
        PLDREF
        PUSHINT 0
        SENDRAWMSG
        PUSHINT 0
        SENDRAWMSG
    ").unwrap();
    let acc_id = AccountId::from([0x11; 32]);
    let mut acc = create_test_account(start_balance, acc_id.clone(), code.clone(), create_two_messages_data());
    acc.set_last_paid(BLOCK_UT - 100);
    let msg = create_int_msg(
        AccountId::from([0x33; 32]),
        AccountId::from([0x11; 32]),
        msg_income,
        false,
        PREV_BLOCK_LT
    );

    let mut new_acc = create_test_account(total_balance, acc_id, code, create_two_messages_data());
    new_acc.set_last_paid(BLOCK_UT);
    new_acc.set_last_tr_time(BLOCK_LT + 2);

    let tr_lt = BLOCK_LT + 1;
    let mut good_trans = Transaction::with_account_and_message(&new_acc, &msg, tr_lt).unwrap();

    let trans = execute_c(&msg, &mut acc, tr_lt, new_acc.balance().unwrap().grams, 0).unwrap();
    assert_eq!(acc, new_acc);

    let mut description = TransactionDescrOrdinary::default();
    description.storage_ph = Some(TrStoragePhase {
        storage_fees_collected: storage_fee.into(),
        storage_fees_due: None,
        status_change: AccStatusChange::Unchanged
    });
    description.credit_ph = Some(TrCreditPhase {
        due_fees_collected: None,
        credit: CurrencyCollection::with_grams(msg_income),
    });
    let mut vm_phase = TrComputePhaseVm::default();
    vm_phase.success = true;
    vm_phase.gas_used = gas_used.into();
    vm_phase.gas_limit = 100u32.into();
    vm_phase.gas_fees = gas_fees.into();
    vm_phase.vm_steps = 10;
    description.compute_ph = TrComputePhase::Vm(vm_phase);

    let mut actions = OutActions::default();
    let (msg1, msg2) = create_two_internal_messages();
    actions.push_back(OutAction::new_send(SENDMSG_ORDINARY, msg1));
    actions.push_back(OutAction::new_send(SENDMSG_ORDINARY, msg2));

    let mut action_ph = TrActionPhase::default();
    action_ph.success = false;
    action_ph.valid = true;
    action_ph.status_change = AccStatusChange::Unchanged;
    action_ph.tot_actions = 2;
    action_ph.msgs_created = 1; // incorrect value ignore, if success == false
    action_ph.add_fwd_fees((msg_fwd_fee).into()); // incorrect value ignore, if success == false
    action_ph.add_action_fees(msg_mine_fee.into()); // incorrect value ignore, if success == false
    action_ph.action_list_hash = actions.hash().unwrap();
    action_ph.result_code = 37;
    action_ph.result_arg = Some(1);
    action_ph.no_funds = true;
    action_ph.tot_msg_size = StorageUsedShort::with_values_checked(1, 705).unwrap();  // incorrect value ignore, if success == false

    description.action = Some(action_ph);

    description.credit_first = true;
    description.bounce = None;
    description.aborted = true;
    description.destroyed = false;
    let description = TransactionDescr::Ordinary(description);

    good_trans.set_total_fees(CurrencyCollection::with_grams(total_fees));
    good_trans.set_now(BLOCK_UT);

    good_trans.write_description(&description).unwrap();

    assert_eq!(trans.read_description().unwrap(), description);
    assert_eq!(trans, good_trans);
}

#[test]
fn test_message_with_zero_value() {
    let mut acc = Account::default();

    let msg = create_int_msg(
        AccountId::from([0x33; 32]),
        AccountId::from([0x11; 32]),
        0,
        false,
        BLOCK_LT + 1
    );

    let tr_lt = BLOCK_LT + 1;

    let trans = execute_c(&msg, &mut acc, tr_lt, 0, 0).unwrap();
    assert_eq!(acc.status(), AccountStatus::AccStateNonexist);
    let credit_phase = get_tr_descr(&trans).credit_ph.unwrap();
    assert_eq!(credit_phase.credit, CurrencyCollection::default());
}

#[test]
fn test_bounce_with_body() {
    let begin_currencies = CurrencyCollection::with_grams(1_400_200_000);

    let mut acc = Account::uninit(
        MsgAddressInt::with_standart(None, 0, AccountId::from([0x11; 32])).unwrap(), 
        10, 
        10, 
        begin_currencies
    );

    let mut msg = create_int_msg_workchain(
        0,
        AccountId::from([0x33; 32]),
        AccountId::from([0x11; 32]),
        1_000_000_000,
        true,
        BLOCK_LT + 1
    );

    let mut data = BuilderData::default();
    data.append_raw(&[1; 780 / 8 + 2], 780).unwrap();
    msg.set_body(SliceData::load_builder(data.clone()).unwrap());
    let mut data2 = BuilderData::default();
    data2.append_raw(&[2; 780 / 8 + 2], 780).unwrap();
    let mut state_init = StateInit::default();
    state_init.data = Some(data2.clone().into_cell().unwrap());
    msg.set_state_init(state_init.clone());

    let tr_lt = BLOCK_LT + 1;

    let config = custom_config(None, Some(GlobalCapabilities::CapFullBodyInBounced as u64 | GlobalCapabilities::CapBounceMsgBody as u64));

    let acc_before = acc.clone();
    disable_cross_check();
    let trans = execute_with_block_version(config, &msg, &mut acc, tr_lt, VERSION_BLOCK_NEW_CALCULATION_BOUNCED_STORAGE);
    check_account_and_transaction(&acc_before, &acc, &msg, trans.as_ref().ok(), 1385694300, 1);
    let trans = trans.unwrap();

    assert_eq!(acc.status(), AccountStatus::AccStateUninit);

    let mut description = TransactionDescrOrdinary::default();
    let storage_fee = 14505700u64;
    description.storage_ph = Some(TrStoragePhase::with_params(
        Grams::from(storage_fee),
        None,
        AccStatusChange::Unchanged
    ));
    description.credit_ph = Some(TrCreditPhase::with_params(None, CurrencyCollection::with_grams(1_000_000_000)));
    description.compute_ph = TrComputePhase::skipped(ComputeSkipReason::BadState);

    description.action = None;
    description.credit_first = false;

    let msg_fee = 919985u64;
    let fwd_fees = 1840015u64;
    description.bounce = Some(
        TrBouncePhase::Ok(TrBouncePhaseOk {
            msg_size: StorageUsedShort::with_values_checked(2, 1560).unwrap(),
            msg_fees: msg_fee.into(),
            fwd_fees: fwd_fees.into(),
        }),
    );

    description.aborted = true;
    description.destroyed = false;
    let description = TransactionDescr::Ordinary(description);

    let mut good_trans = Transaction::with_account_and_message(&acc, &msg, BLOCK_LT + 3).unwrap();

    let mut message = Message::with_int_header(InternalMessageHeader{
        ihr_disabled: true,
        bounce: false,
        bounced: true,
        src: MsgAddressIntOrNone::Some(MsgAddressInt::with_standart(None, 0, AccountId::from([0x11; 32])).unwrap()),
        dst: MsgAddressInt::with_standart(None, 0, AccountId::from([0x33; 32])).unwrap(),
        value: CurrencyCollection::with_grams(1_000_000_000 - msg_fee - fwd_fees),
        ihr_fee: Default::default(),
        fwd_fee: fwd_fees.into(),
        created_lt: 2000000003,
        created_at: BLOCK_UT.into()
    });
    let mut builder = (-1i32).write_to_new_cell().unwrap();
    let mut body_copy = SliceData::load_builder(data.clone()).unwrap();
    body_copy.shrink_data(0..256);
    builder.append_bytestring(&body_copy).unwrap();
    builder.checked_append_reference(data.clone().into_cell().unwrap()).unwrap();
    message.set_body(SliceData::load_builder(builder.clone()).unwrap());
    message.set_state_init(state_init);
    good_trans.add_out_message(&make_common(message)).unwrap();

    good_trans.set_total_fees(CurrencyCollection::with_grams(storage_fee + msg_fee));
    good_trans.orig_status = AccountStatus::AccStateUninit;
    good_trans.set_end_status(AccountStatus::AccStateUninit);

    good_trans.set_logical_time(BLOCK_LT + 2);
    good_trans.set_now(BLOCK_UT);

    good_trans.write_description(&description).unwrap();

    assert_eq!(trans.read_description().unwrap(), description);
    trans.out_msgs.scan_diff(&good_trans.out_msgs, |key: ever_block::U15, msg1, msg2| {
        assert_eq!(msg1, msg2, "for key {}", key.0);
        Ok(true)
    }).unwrap();
    assert_eq!(trans, good_trans);
}

fn build_contract_info(
    acc: &Account,
    msg: &Message,
    config: &ConfigParams,
) -> SmartContractInfo {
    let config_params = config.config_params.data().cloned();
    SmartContractInfo {
        capabilities: config.capabilities(),
        myself: SliceData::load_builder(msg.dst_ref().unwrap().write_to_new_cell().unwrap()).unwrap(),
        block_lt: BLOCK_LT,
        trans_lt: BLOCK_LT + 2,
        unix_time: BLOCK_UT,
        balance: acc.balance().unwrap().clone(),
        config_params,
        ..Default::default()
    }
}

fn bounced_message_in_several_cells(block_version: u32, count_cells: u64, fwd_fee: Grams) {
    let begin_currencies = CurrencyCollection::with_grams(1_400_200_000);

    let mut acc = Account::uninit(
        MsgAddressInt::with_standart(None, 0, AccountId::from([0x11; 32])).unwrap(), 
        10, 
        10, 
        begin_currencies
    );
    acc.set_last_tr_time(27916231000002);

    let mut msg = create_int_msg_workchain(
        0,
        AccountId::from([0x33; 32]),
        AccountId::from([0x11; 32]),
        494915282787965,
        true,
        27916231000002 + 1
    );
    msg.set_at_and_lt(1656081021, 27916231000002 + 1);
    match msg.header_mut() {
        CommonMsgInfo::IntMsgInfo(header) => {
            header.fwd_fee = 17673469.into();
        },
        _ => unreachable!()
    }

    let mut data = BuilderData::default();
    data.append_raw(&[1; 780 / 8 + 2], 780).unwrap();
    msg.set_body(SliceData::load_builder(data.clone()).unwrap());
    let mut data2 = BuilderData::default();
    data2.append_raw(&[2; 780 / 8 + 2], 780).unwrap();
    let mut state_init = StateInit::default();
    state_init.data = Some(data2.clone().into_cell().unwrap());
    msg.set_state_init(state_init);

    let tr_lt = 27916231000002 + 1;

    let acc_before = acc.clone();

    let lt = Arc::new(AtomicU64::new(tr_lt));
    let executor = OrdinaryTransactionExecutor::new(BLOCKCHAIN_CONFIG.clone());
    let params = ExecuteParams {
        state_libs: HashmapE::default(),
        block_unixtime: BLOCK_UT,
        block_lt: BLOCK_LT,
        last_tr_lt: lt,
        seed_block: UInt256::default(),
        debug: true,
        block_version,
        ..ExecuteParams::default()
    };
    let trans= executor.execute_with_params(Some(&make_common(msg.clone())), &mut acc, params);

    check_account_and_transaction(&acc_before, &acc, &msg, trans.as_ref().ok(), 1385694300, 1);
    let trans = trans.unwrap();

    assert_eq!(acc.status(), AccountStatus::AccStateUninit);
    let descr = get_tr_descr(&trans);
    if let TrBouncePhase::Ok(bounce) = descr.bounce.unwrap() {
        assert_eq!(bounce.msg_size.cells(), count_cells);
        assert_eq!(bounce.fwd_fees, fwd_fee);
    } else {
        unreachable!()
    }
}

#[test]
fn test_bounced_message_in_two_cells() {
    let _ = DisableCrossCheck::new();
    bounced_message_in_several_cells(29, 0, 666672u64.into());
    bounced_message_in_several_cells(30, 1, 925341u64.into());
}

#[test]
fn test_message_with_var_address() {
    let start_balance = 300_000_000_000u64;
    let msg_income = 100_000_000_000u64;

    let dst = MsgAddressInt::with_variant(None, 0, SliceData::new(vec![6; 31])).unwrap();
    let mut hdr = InternalMessageHeader::with_addresses(
        MsgAddressInt::with_standart(None, 0, AccountId::from([0x11; 32])).unwrap(),
        dst,
        CurrencyCollection::with_grams(msg_income),
    );
    hdr.bounce = false;
    hdr.ihr_disabled = true;
    hdr.created_lt = PREV_BLOCK_LT;
    hdr.created_at = UnixTime32::default();
    let out_msg = Message::with_int_header(hdr);

    let data = out_msg.serialize().unwrap();
    let code = compile_code_to_cell("
        PUSHROOT
        PUSHINT 2
        SENDRAWMSG
    ").unwrap();
    let acc_id = AccountId::from([0x11; 32]);
    let mut acc = create_test_account_workchain(start_balance, 0, acc_id.clone(), code, data);
    acc.set_last_paid(BLOCK_UT - 100);
    let msg = create_int_msg_workchain(
        0,
        AccountId::from([0x33; 32]),
        AccountId::from([0x11; 32]),
        msg_income,
        true,
        PREV_BLOCK_LT
    );

    let tr_lt = BLOCK_LT + 1;
    let trans = execute_c(&msg, &mut acc, tr_lt, 399999424996u64, 0).unwrap();

    let mut new_acc = create_test_account_workchain(
        399999424996u64,
        0,
        acc_id,
        acc.get_code().unwrap(),
        acc.get_data().unwrap()
    );
    new_acc.set_last_paid(BLOCK_UT);
    new_acc.set_last_tr_time(BLOCK_LT + 2);
    assert_eq!(acc, new_acc);

    let mut description = TransactionDescrOrdinary::default();
    description.storage_ph = Some(TrStoragePhase {
        storage_fees_collected: 4u64.into(),
        storage_fees_due: None,
        status_change: AccStatusChange::Unchanged
    });
    description.credit_ph = Some(TrCreditPhase {
        due_fees_collected: None,
        credit: CurrencyCollection::with_grams(msg_income),
    });
    let mut vm_phase = TrComputePhaseVm::default();
    vm_phase.success = true;
    vm_phase.gas_used = 575u32.into();
    vm_phase.gas_limit = 1000000u32.into();
    vm_phase.gas_fees = 575000u64.into();
    vm_phase.vm_steps = 4;
    description.compute_ph = TrComputePhase::Vm(vm_phase);

    let mut action_ph = TrActionPhase::default();
    action_ph.success = true;
    action_ph.valid = true;
    action_ph.status_change = AccStatusChange::Unchanged;
    action_ph.tot_actions = 1;
    action_ph.msgs_created = 0;
    action_ph.action_list_hash = "0xd95f4f546c3d9df56a578939486abbfd78b8bd53a820350aab3cf896a8924669".parse().unwrap();
    action_ph.result_code = 0;
    action_ph.result_arg = None;
    action_ph.no_funds = false;
    action_ph.tot_msg_size = StorageUsedShort::default();

    description.action = Some(action_ph);

    description.credit_first = false;
    description.bounce = None;
    description.aborted = false;
    description.destroyed = false;
    let description = TransactionDescr::Ordinary(description);

    let mut good_trans = Transaction::with_account_and_message(&new_acc, &msg, tr_lt).unwrap();

    good_trans.set_total_fees(CurrencyCollection::with_grams(575004));
    good_trans.set_now(BLOCK_UT);

    good_trans.write_description(&description).unwrap();

    assert_eq!(trans.read_description().unwrap(), description);
    assert_eq!(trans, good_trans);
}

#[test]
fn test_message_with_var_address_in_masterchain() {
    let start_balance = 300_000_000_000u64;
    let msg_income = 100_000_000_000u64;

    let dst = MsgAddressInt::with_variant(None, -1, SliceData::new(vec![6; 31])).unwrap();
    let mut hdr = InternalMessageHeader::with_addresses(
        MsgAddressInt::with_standart(None, -1, AccountId::from([0x11; 32])).unwrap(),
        dst,
        CurrencyCollection::with_grams(msg_income),
    );
    hdr.bounce = false;
    hdr.ihr_disabled = true;
    hdr.created_lt = PREV_BLOCK_LT;
    hdr.created_at = UnixTime32::default();
    let out_msg = Message::with_int_header(hdr);

    let data = out_msg.serialize().unwrap();
    let code = compile_code_to_cell("
        PUSHROOT
        PUSHINT 2
        SENDRAWMSG
    ").unwrap();
    let acc_id = AccountId::from([0x11; 32]);
    let mut acc = create_test_account_workchain(start_balance, -1, acc_id.clone(), code, data);
    acc.set_last_paid(BLOCK_UT - 100);
    let msg = create_int_msg_workchain(
        -1,
        AccountId::from([0x33; 32]),
        AccountId::from([0x11; 32]),
        msg_income,
        true,
        PREV_BLOCK_LT
    );

    disable_cross_check();
    let tr_lt = BLOCK_LT + 1;
    let trans = execute_c(&msg, &mut acc, tr_lt, 399994246388u64, 0).unwrap();

    let mut new_acc = create_test_account_workchain(
        399994246388u64,
        -1,
        acc_id,
        acc.get_code().unwrap(),
        acc.get_data().unwrap()
    );
    new_acc.set_last_paid(BLOCK_UT);
    new_acc.set_last_tr_time(BLOCK_LT + 2);
    assert_eq!(acc, new_acc);

    let mut description = TransactionDescrOrdinary::default();
    description.storage_ph = Some(TrStoragePhase {
        storage_fees_collected: 3612u64.into(),
        storage_fees_due: None,
        status_change: AccStatusChange::Unchanged
    });
    description.credit_ph = Some(TrCreditPhase {
        due_fees_collected: None,
        credit: CurrencyCollection::with_grams(msg_income),
    });
    let mut vm_phase = TrComputePhaseVm::default();
    vm_phase.success = true;
    vm_phase.gas_used = 575u32.into();
    vm_phase.gas_limit = 1000000u32.into();
    vm_phase.gas_fees = 5750000u64.into();
    vm_phase.vm_steps = 4;
    description.compute_ph = TrComputePhase::Vm(vm_phase);

    let mut action_ph = TrActionPhase::default();
    action_ph.success = true;
    action_ph.valid = true;
    action_ph.status_change = AccStatusChange::Unchanged;
    action_ph.tot_actions = 1;
    action_ph.msgs_created = 0;
    action_ph.action_list_hash = "0x6e4d01ecc1c74a741dacd6d4250bc9f7b72b519abbaa57c255f781fd305288c7".parse().unwrap();
    action_ph.result_code = 0;
    action_ph.result_arg = None;
    action_ph.no_funds = false;
    action_ph.tot_msg_size = StorageUsedShort::default();

    description.action = Some(action_ph);

    description.credit_first = false;
    description.bounce = None;
    description.aborted = false;
    description.destroyed = false;
    let description = TransactionDescr::Ordinary(description);

    let mut good_trans = Transaction::with_account_and_message(&new_acc, &msg, tr_lt).unwrap();

    good_trans.set_total_fees(CurrencyCollection::with_grams(5753612));
    good_trans.set_now(BLOCK_UT);

    good_trans.write_description(&description).unwrap();

    assert_eq!(trans.read_description().unwrap(), description);
    assert_eq!(trans, good_trans);
}

#[test]
fn test_send_message_to_nonexisting_workchain() {
    let start_balance = 300_000_000_000u64;
    let msg_income = 100_000_000_000u64;

    let dst = MsgAddressInt::with_standart(None, 1, AccountId::from([0x44; 32])).unwrap();
    let mut hdr = InternalMessageHeader::with_addresses(
        MsgAddressInt::with_standart(None, -1, AccountId::from([0x11; 32])).unwrap(),
        dst,
        CurrencyCollection::with_grams(msg_income),
    );
    hdr.bounce = false;
    hdr.ihr_disabled = true;
    hdr.created_lt = PREV_BLOCK_LT;
    hdr.created_at = UnixTime32::default();
    let out_msg = Message::with_int_header(hdr);

    let data = out_msg.serialize().unwrap();
    let code = compile_code_to_cell("
        PUSHROOT
        PUSHINT 2
        SENDRAWMSG
    ").unwrap();
    let acc_id = AccountId::from([0x11; 32]);
    let mut acc = create_test_account_workchain(start_balance, -1, acc_id.clone(), code, data);
    acc.set_last_paid(BLOCK_UT - 100);
    let msg = create_int_msg_workchain(
        -1,
        AccountId::from([0x33; 32]),
        AccountId::from([0x11; 32]),
        msg_income,
        true,
        PREV_BLOCK_LT
    );

    disable_cross_check();
    let tr_lt = BLOCK_LT + 1;
    let trans = execute_c(&msg, &mut acc, tr_lt, 399994246423u64, 0).unwrap();

    let mut new_acc = create_test_account_workchain(
        399994246423u64,
        -1,
        acc_id,
        acc.get_code().unwrap(),
        acc.get_data().unwrap()
    );
    new_acc.set_last_paid(BLOCK_UT);
    new_acc.set_last_tr_time(BLOCK_LT + 2);
    assert_eq!(acc, new_acc);

    let mut description = TransactionDescrOrdinary::default();
    description.storage_ph = Some(TrStoragePhase {
        storage_fees_collected: 3577u64.into(),
        storage_fees_due: None,
        status_change: AccStatusChange::Unchanged
    });
    description.credit_ph = Some(TrCreditPhase {
        due_fees_collected: None,
        credit: CurrencyCollection::with_grams(msg_income),
    });
    let mut vm_phase = TrComputePhaseVm::default();
    vm_phase.success = true;
    vm_phase.gas_used = 575u32.into();
    vm_phase.gas_limit = 1000000u32.into();
    vm_phase.gas_fees = 5750000u64.into();
    vm_phase.vm_steps = 4;
    description.compute_ph = TrComputePhase::Vm(vm_phase);

    let mut action_ph = TrActionPhase::default();
    action_ph.success = true;
    action_ph.valid = true;
    action_ph.status_change = AccStatusChange::Unchanged;
    action_ph.tot_actions = 1;
    action_ph.msgs_created = 0;
    action_ph.action_list_hash = "0xa8d20bc0b9058e2ea89c21595fecae517f3a18c49a5492df8675b85ca8aed63f".parse().unwrap();
    action_ph.result_code = 0;
    action_ph.result_arg = None;
    action_ph.no_funds = false;
    action_ph.tot_msg_size = StorageUsedShort::default();

    description.action = Some(action_ph);

    description.credit_first = false;
    description.bounce = None;
    description.aborted = false;
    description.destroyed = false;
    let description = TransactionDescr::Ordinary(description);

    let mut good_trans = Transaction::with_account_and_message(&new_acc, &msg, tr_lt).unwrap();

    good_trans.set_total_fees(CurrencyCollection::with_grams(5753577));
    good_trans.set_now(BLOCK_UT);

    good_trans.write_description(&description).unwrap();

    assert_eq!(trans.read_description().unwrap(), description);
    assert_eq!(trans, good_trans);
}

#[test]
fn bounced_message_with_special_account() {
    let acc_id = AccountId::from([0x66; 32]);
    assert!(
        BLOCKCHAIN_CONFIG.is_special_account(
            &MsgAddressInt::with_standart(None, -1, acc_id.clone()).unwrap()
        ).unwrap()
    );

    let mut acc = Account::uninit(
        MsgAddressInt::with_standart(None, -1, acc_id.clone()).unwrap(),
        10,
        10,
        CurrencyCollection::with_grams(1_400_200_000)
    );

    let msg = create_int_msg_workchain(
        -1,
        AccountId::from([0x33; 32]),
        acc_id,
        494915282787965u64,
        true,
        BLOCK_LT
    );

    let trans = execute_c(&msg, &mut acc, BLOCK_LT + 1, 1400200000, 1).unwrap();

    assert_eq!(acc.status(), AccountStatus::AccStateUninit);
    let descr = get_tr_descr(&trans);
    if let TrBouncePhase::Ok(bounce) = descr.bounce.unwrap() {
        assert_eq!(bounce.fwd_fees, Grams::from(6666718u64));
    } else {
        unreachable!()
    }
}

#[test]
fn test_cap_fee_in_gas_units() {
    let code = "
        PUSHROOT
        CTOS
        LDREF
        LDREF
        SWAP
        PUSHINT 65
        SENDRAWMSG
        SWAP
        PUSHINT 1
        SENDRAWMSG
    ";
    let start_balance = 500_000_000_000u64;
    let compute_phase_gas_fees = 1317000u64;
    let msg_income = 10_400_200_000;

    let acc_id = MsgAddressInt::with_standart(None, 0, AccountId::from([0x22; 32])).unwrap().address();
    let code = compile_code_to_cell(code).unwrap();

    let (mut msg1, mut msg2) = create_two_internal_messages();
    msg1.set_src_address(MsgAddressInt::with_standart(None, 0, acc_id.clone()).unwrap());
    msg2.set_src_address(MsgAddressInt::with_standart(None, 0, acc_id.clone()).unwrap());
    let mut b = BuilderData::default();
    b.checked_append_reference(msg2.serialize().unwrap()).unwrap();
    b.checked_append_reference(msg1.serialize().unwrap()).unwrap();
    let data = b.into_cell().unwrap();

    let mut acc = create_test_account_workchain(start_balance, 0, acc_id.clone(), code, data);

    let msg = create_int_msg_workchain(
        0,
        AccountId::from([0x33; 32]),
        acc_id,
        msg_income,
        false,
        BLOCK_LT - 2
    );

    let mut new_acc = Account::active_by_init_code_hash(
        acc.get_addr().unwrap().clone(),
        CurrencyCollection::with_grams(299576802000),
        BLOCK_UT,
        acc.state_init().unwrap().clone(),
        false
    ).unwrap();
    new_acc.update_storage_stat().unwrap();
    new_acc.set_last_tr_time(BLOCK_LT + 4);

    let config = custom_config(
        None,
        Some(GlobalCapabilities::CapFeeInGasUnits as u64)
    );
    let tr_lt = BLOCK_LT + 1;
    disable_cross_check();
    let acc_before = acc.clone();
    let trans = execute_with_params(config, &msg, &mut acc, tr_lt).unwrap();
    check_account_and_transaction_balances(&acc_before, &acc, &msg, Some(&trans));

    let mut description = TransactionDescrOrdinary::default();
    let storage_phase_fees = 271881 * 1000;
    description.storage_ph = Some(TrStoragePhase::with_params(
        Grams::from(storage_phase_fees),
        None,
        AccStatusChange::Unchanged
    ));
    description.credit_ph = Some(TrCreditPhase::with_params(
        None,
        CurrencyCollection::with_grams(msg_income)
    ));

    let gas_used = (compute_phase_gas_fees / 1000) as u32;
    let gas_fees = compute_phase_gas_fees;
    let mut vm_phase = TrComputePhaseVm::default();
    vm_phase.success = true;
    vm_phase.gas_used = gas_used.into();
    vm_phase.gas_limit = 1000000u32.into();
    vm_phase.gas_fees = gas_fees.into();
    vm_phase.vm_steps = 11;
    description.compute_ph = TrComputePhase::Vm(vm_phase);

    let msg_fwd_fee = MSG_FWD_FEE * 10000;
    let msg_remain_fee = 66667175293;
    let msg_mine_fee = msg_fwd_fee - msg_remain_fee;
    let mut actions = OutActions::default();
    actions.push_back(OutAction::new_send(SENDMSG_REMAINING_MSG_BALANCE + SENDMSG_PAY_FEE_SEPARATELY, msg1.clone()));
    actions.push_back(OutAction::new_send(SENDMSG_PAY_FEE_SEPARATELY, msg2.clone()));
    if let Some(int_header) = msg1.int_header_mut() {
        if let Some(int_header2) = msg2.int_header_mut() {
            int_header.value.grams = Grams::from(MSG1_BALANCE + msg_income);
            int_header2.value.grams = Grams::from(MSG2_BALANCE);
            int_header.fwd_fee = msg_remain_fee.into();
            int_header2.fwd_fee = msg_remain_fee.into();
            int_header.created_at = BLOCK_UT.into();
            int_header2.created_at = BLOCK_UT.into();
        }
    }
    let mut action_ph = TrActionPhase::default();
    action_ph.success = true;
    action_ph.valid = true;
    action_ph.status_change = AccStatusChange::Unchanged;
    action_ph.tot_actions = 2;
    action_ph.spec_actions = 0;
    action_ph.msgs_created = 2;
    action_ph.total_fwd_fees = Some((2 * msg_fwd_fee).into());
    action_ph.total_action_fees = Some((2 * msg_mine_fee).into());
    action_ph.action_list_hash = actions.hash().unwrap();
    action_ph.tot_msg_size = StorageUsedShort::calculate_for_struct(&msg1).unwrap();
    action_ph.tot_msg_size.append(&msg2.serialize().unwrap());
    description.action = Some(action_ph);

    description.credit_first = true;
    description.bounce = None;
    description.aborted = false;
    description.destroyed = false;
    let description = TransactionDescr::Ordinary(description);

    let mut good_trans = Transaction::with_account_and_message(&new_acc, &msg, BLOCK_LT + 2).unwrap();

    good_trans.add_out_message(&make_common(msg1)).unwrap();
    good_trans.add_out_message(&make_common(msg2)).unwrap();
    good_trans.set_total_fees(CurrencyCollection::with_grams(gas_fees + storage_phase_fees + msg_mine_fee * 2));
    good_trans.orig_status = AccountStatus::AccStateActive;
    good_trans.set_end_status(AccountStatus::AccStateActive);
    good_trans.set_logical_time(BLOCK_LT + 1);
    good_trans.set_now(BLOCK_UT);

    good_trans.write_description(&description).unwrap();

    assert_eq!(acc, new_acc);
    trans.out_msgs.scan_diff(&good_trans.out_msgs, |key: ever_block::U15, msg1, msg2| {
        assert_eq!(msg1, msg2, "for key {}", key.0);
        Ok(true)
    }).unwrap();
    assert_eq!(trans.read_description().unwrap(), description);
    assert_eq!(trans, good_trans);
}

static ACCOUNT_ID: [u8; 32] = [0x11; 32];

fn create_account(balance: u64, code: &str) -> Account {
    let code = compile_code_to_cell(code).unwrap();
    create_test_account_workchain(
        balance,
        0,
        ACCOUNT_ID.into(),
        code,
        BuilderData::default().into_cell().unwrap()
    )
}

fn create_bouncable_message(value: u64) -> Message {
    create_int_msg_workchain(
        0,
        [1; 32].into(),
        ACCOUNT_ID.into(),
        value,
        true,
        BLOCK_LT
    )
}

#[test]
fn test_bouncable() -> Result<()> {
    let throw_code = "THROW 123";
    let bounce_gas_leftover_code = "
        PUSH s2
        CTOS
        PUSHINT 4
        SDSKIPFIRST
        PUSHINT 267 ; 2 + 1 + 8 + 256 = 267 source address as destination
        SDCUTFIRST
        NEWC
        STSLICECONST x62_
        STSLICE
        PUSHINT 111
        STZEROES
        ENDC
        PUSHINT 64
        SENDRAWMSG
    ";
    let config = custom_config(None, None);
    // account with tiny balance and big dues
    let tiny_balance = 1;
    let msg_value = 2_800_000;
    let due_payment = 2_000_000;

    // account has not enough gas because of due payment and not enough value to send bounced message
    let mut account = create_account(tiny_balance, bounce_gas_leftover_code);
    account.set_due_payment(Some(due_payment.into()));
    let msg = create_bouncable_message(msg_value);
    let tr = execute_with_params(config.clone(), &msg, &mut account, BLOCK_LT)?;
    assert!(tr.get_out_msg(0)?.is_none());
    assert_eq!(account.balance().unwrap().grams.as_u128(), 979);

    // account has not enough value to send bounced message
    let mut account = create_account(tiny_balance, throw_code);
    account.set_due_payment(Some(due_payment.into()));
    let msg = create_bouncable_message(msg_value);
    let tr = execute_with_params(config.clone(), &msg, &mut account, BLOCK_LT)?;
    assert!(tr.get_out_msg(0)?.is_none());
    assert_eq!(account.balance().unwrap().grams.as_u128(), 593_150);

    // change capabilities to fix problem with due payment
    let config = custom_config(None, Some(GlobalCapabilities::CapDuePaymentFix as u64));

    // account has enough gas and can send message, but due payment is growing
    let mut account = create_account(tiny_balance, bounce_gas_leftover_code);
    account.set_due_payment(Some(due_payment.into()));
    let msg = create_bouncable_message(msg_value);
    let tr = execute_with_params(config.clone(), &msg, &mut account, BLOCK_LT)?;
    let msg = tr.get_out_msg(0)?.unwrap();
    let hdr = msg.get_std()?.int_header().unwrap();
    assert!(!hdr.bounced);
    assert_eq!(hdr.value().grams.as_u128(), 373_000);
    assert_eq!(account.balance().unwrap().grams.as_u128(), 0);
    assert_eq!(account.due_payment().unwrap().as_u128(), 2_118_021);

    // account has enough value to send bounced message, but due payment is growing
    // we get not enough value to send bounced message
    let mut account = create_account(tiny_balance, throw_code);
    account.set_due_payment(Some(due_payment.into()));
    let msg = create_bouncable_message(msg_value);
    let tr = execute_with_params(config.clone(), &msg, &mut account, BLOCK_LT)?;
    let msg = tr.get_out_msg(0)?.unwrap();
    let hdr = msg.get_std()?.int_header().unwrap();
    assert!(hdr.bounced);
    assert_eq!(hdr.value().grams.as_u128(), 1_700_000);
    assert_eq!(account.balance().unwrap().grams.as_u128(), 0);
    assert_eq!(account.due_payment().unwrap().as_u128(), 2_106_850);

    Ok(())
}
