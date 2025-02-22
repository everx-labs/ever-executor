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

mod common;

use ever_block::{
    Account, AccountStatus, AccStatusChange, AnycastInfo, CurrencyCollection, GetRepresentationHash,
    GlobalCapabilities, Grams, InternalMessageHeader, Message, MsgAddressInt,
    MsgAddressIntOrNone, OutAction, OutActions, SENDMSG_ORDINARY, Serializable,
    StorageUsedShort, TrActionPhase, Transaction, TransactionDescr, TransactionDescrOrdinary,
    TrBouncePhase, TrBouncePhaseNofunds, TrBouncePhaseOk, TrComputePhase, TrComputePhaseVm,
    TrCreditPhase, TrStoragePhase, UnixTime32
};
use ever_block::TrBouncePhase::Nofunds;
use ever_assembler::compile_code_to_cell;
use ever_block::{AccountId, Cell, SliceData};
use common::*;
use crate::ordinary_transaction::tests7::common::cross_check::disable_cross_check;

#[test]
fn test_action_phase_failed() {
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
        true,
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

    description.credit_first = false;
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
fn test_action_phase_failed_with_cap_but_unsuccess_bounce() {
    let start_balance = 100000000;
    let msg_income = 1_000_000_u64;
    let storage_fee = 6618;
    let gas_used = 1307u32;
    let gas_fees = 13070000;
    let msg_fwd_fee = MSG_FWD_FEE;
    let msg_mine_fee = MSG_MINE_FEE;
    let total_fees = gas_fees + storage_fee;
    let total_balance = start_balance - storage_fee - gas_fees + msg_income;

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
        true,
        PREV_BLOCK_LT
    );

    let mut new_acc = create_test_account(total_balance, acc_id, code, create_two_messages_data());
    new_acc.set_last_paid(BLOCK_UT);
    new_acc.set_last_tr_time(BLOCK_LT + 2);

    let tr_lt = BLOCK_LT + 1;
    let mut good_trans = Transaction::with_account_and_message(&new_acc, &msg, tr_lt).unwrap();

    let config = custom_config(None, Some(GlobalCapabilities::CapBounceAfterFailedAction as u64));
    let acc_before = acc.clone();
    let trans = execute_with_params(config, &msg, &mut acc, tr_lt).unwrap();
    check_account_and_transaction(&acc_before, &acc, &msg, Some(&trans), new_acc.balance().unwrap().grams, 0);
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

    description.credit_first = false;
    description.bounce = Some(
        Nofunds(
            TrBouncePhaseNofunds {
                msg_size: StorageUsedShort::with_values_checked(0, 0).unwrap(),
                req_fwd_fees: Grams::from(10000000)
            }
        )
    );
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
fn test_action_phase_failed_with_cap() {
    let start_balance = 10_000_000;
    let msg_income = 150_000_000_u64;
    let storage_fee = 6606;
    let total_balance = start_balance - storage_fee;
    let gas_used = 1307u32;
    let gas_fees = 13_070_000;
    let msg_fwd_fee = MSG_FWD_FEE;
    let msg_mine_fee = MSG_MINE_FEE;
    let total_fees = 16409888;

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
        true,
        PREV_BLOCK_LT
    );

    let mut new_acc = create_test_account(total_balance, acc_id, code, create_two_messages_data());
    new_acc.set_last_paid(BLOCK_UT);
    new_acc.set_last_tr_time(BLOCK_LT + 3);

    let tr_lt = BLOCK_LT + 1;
    let mut good_trans = Transaction::with_account_and_message(&new_acc, &msg, tr_lt).unwrap();

    let config = custom_config(None, Some(GlobalCapabilities::CapBounceAfterFailedAction as u64));
    let acc_before = acc.clone();
    let trans = execute_with_params(config, &msg, &mut acc, tr_lt).unwrap();
    check_account_and_transaction(&acc_before, &acc, &msg, Some(&trans), new_acc.balance().unwrap().grams, 1);
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
    vm_phase.gas_limit = 15000u32.into();
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

    description.credit_first = false;
    description.bounce = Some(
        TrBouncePhase::Ok(TrBouncePhaseOk {
            msg_size: StorageUsedShort::with_values_checked(0, 0).unwrap(),
            msg_fees: Grams::from(3333282),
            fwd_fees: Grams::from(6666718),
        }),
    );
    description.aborted = true;
    description.destroyed = false;
    let description = TransactionDescr::Ordinary(description);

    good_trans.set_total_fees(CurrencyCollection::with_grams(total_fees));
    good_trans.set_now(BLOCK_UT);

    let mut message = Message::with_int_header(InternalMessageHeader{
        ihr_disabled: true,
        bounce: false,
        bounced: true,
        src: MsgAddressIntOrNone::Some(MsgAddressInt::with_standart(None, -1, AccountId::from([0x11; 32])).unwrap()),
        dst: MsgAddressInt::with_standart(None, -1, AccountId::from([0x33; 32])).unwrap(),
        value: CurrencyCollection::with_grams(126930000),
        ihr_fee: Default::default(),
        fwd_fee: Grams::from(6666718),
        created_lt: 2000000002,
        created_at: BLOCK_UT.into()
    });
    let builder = (-1i32).write_to_new_cell().unwrap();
    message.set_body(SliceData::load_builder(builder).unwrap());
    good_trans.add_out_message(&make_common(message)).unwrap();

    good_trans.write_description(&description).unwrap();

    trans.out_msgs.scan_diff(&good_trans.out_msgs, |key: ever_block::U15, msg1, msg2| {
        assert_eq!(msg1, msg2, "for key {}", key.0);
        Ok(true)
    }).unwrap();
    assert_eq!(trans.read_description().unwrap(), description);
    assert_eq!(trans, good_trans);
}

#[test]
fn test_message_with_anycast_output_address_bounced_action() {
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
    let config = custom_config(None, Some(GlobalCapabilities::CapBounceAfterFailedAction as u64));
    let trans = execute_with_params(config, &msg, &mut acc, tr_lt).unwrap();

    let mut new_acc = create_test_account_workchain(299999999996u64, 0, acc_id, acc.get_code().unwrap(), acc.get_data().unwrap());
    new_acc.set_last_paid(BLOCK_UT);
    new_acc.set_last_tr_time(BLOCK_LT + 3);
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
    description.bounce = Some(
        TrBouncePhase::Ok(TrBouncePhaseOk {
            msg_size: StorageUsedShort::with_values_checked(0, 0).unwrap(),
            msg_fees: Grams::from(333328),
            fwd_fees: Grams::from(666672),
        }),
    );
    description.aborted = true;
    description.destroyed = false;
    let description = TransactionDescr::Ordinary(description);

    let mut good_trans = Transaction::with_account_and_message(&new_acc, &msg, tr_lt).unwrap();

    let mut message = Message::with_int_header(InternalMessageHeader{
        ihr_disabled: true,
        bounce: false,
        bounced: true,
        src: MsgAddressIntOrNone::Some(MsgAddressInt::with_standart(None, 0, AccountId::from([0x11; 32])).unwrap()),
        dst: MsgAddressInt::with_standart(None, 0, AccountId::from([0x33; 32])).unwrap(),
        value: CurrencyCollection::with_grams(99998425000),
        ihr_fee: Default::default(),
        fwd_fee: Grams::from(666672),
        created_lt: 2000000002,
        created_at: BLOCK_UT.into()
    });
    let builder = (-1i32).write_to_new_cell().unwrap();
    message.set_body(SliceData::load_builder(builder).unwrap());
    good_trans.add_out_message(&make_common(message)).unwrap();

    good_trans.set_total_fees(CurrencyCollection::with_grams(908332));
    good_trans.set_now(BLOCK_UT);

    good_trans.write_description(&description).unwrap();

    trans.out_msgs.scan_diff(&good_trans.out_msgs, |key: ever_block::U15, msg1, msg2| {
        assert_eq!(msg1, msg2, "for key {}", key.0);
        Ok(true)
    }).unwrap();
    assert_eq!(trans.read_description().unwrap(), description);
    assert_eq!(trans, good_trans);
}

#[allow(clippy::too_many_arguments)]
pub fn execute_custom_transaction_with_caps(
    capabilities: u64,
    start_balance: u64,
    code: &str,
    data: Cell,
    msg_balance: u64,
    bounce: bool,
    result_account_balance: impl Into<Grams>,
    count_out_msgs: usize,
) -> (Account, Transaction) {
    let acc_id = AccountId::from([0x11; 32]);
    let code = compile_code_to_cell(code).unwrap();
    let mut acc = create_test_account(start_balance, acc_id.clone(), code, data);
    let msg = create_int_msg(
        AccountId::from([0x33; 32]),
        acc_id,
        msg_balance,
        bounce,
        BLOCK_LT - 2
    );

    let config = custom_config(None, Some(capabilities));

    let acc_before = acc.clone();
    let trans = execute_with_params(config, &msg, &mut acc, BLOCK_LT + 1);
    check_account_and_transaction(&acc_before, &acc, &msg, trans.as_ref().ok(), result_account_balance, count_out_msgs);
    (acc, trans.unwrap())
}

#[test]
fn test_send_bouncable_messages_to_account_without_enough_money_to_pay_storage() {
    // account balance is not enough to pay for storage fee

    // i. Bad code without accept
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
    // message value is enough to pay for storage and compute, but not enough to send value
    let (acc, tr) = execute_custom_transaction_with_caps(
        GlobalCapabilities::CapBounceAfterFailedAction as u64,
        start_balance,
        code,
        data,
        10_000_000,
        true,
        4_250_000 - due,
        0
    );
    assert!(matches!(get_tr_descr(&tr).compute_ph, TrComputePhase::Vm(_)));
    assert!(get_tr_descr(&tr).action.unwrap().no_funds);
    assert!(matches!(get_tr_descr(&tr).bounce, Some(TrBouncePhase::Nofunds(_))));
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
    // message value is enough to pay for storage and compute, but not enough to send value
    let (acc, tr) = execute_custom_transaction_with_caps(
        GlobalCapabilities::CapBounceAfterFailedAction as u64,
        start_balance,
        code,
        data,
        10_000_000,
        true,
        3_990_000 - due,
        0
    );
    assert!(matches!(get_tr_descr(&tr).compute_ph, TrComputePhase::Vm(_)));
    assert!(get_tr_descr(&tr).action.unwrap().no_funds);
    assert!(matches!(get_tr_descr(&tr).bounce, Some(TrBouncePhase::Nofunds(_))));
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
    execute_custom_transaction_with_caps(
        GlobalCapabilities::CapBounceAfterFailedAction as u64,
        start_balance,
        &code,
        data,
        20_000_000,
        true,
        result_balance,
        count_out_msgs
    );
}

#[test]
fn test_sendrawmsg_message() {
    execute_sendrawmsg_message(0, 0, 46_273_237, 1);
}
