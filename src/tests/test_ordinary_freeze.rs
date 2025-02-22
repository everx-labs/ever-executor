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

mod common;
use common::*;

use pretty_assertions::assert_eq;
use ever_block::{
    accounts::Account,
    out_actions::{OutAction, OutActions, SENDMSG_ORDINARY},
    transactions::{
        AccStatusChange, TrActionPhase, TrComputePhase, TrComputePhaseVm, TrCreditPhase,
        TrStoragePhase, Transaction, TransactionDescr,
    },
    AccountStatus, ComputeSkipReason, CurrencyCollection, GetRepresentationHash, Grams,
    InternalMessageHeader, MsgAddressInt, MsgAddressIntOrNone, Serializable, StateInit,
    StorageUsedShort, TrBouncePhaseNofunds, TrBouncePhaseOk,
};
use ever_assembler::compile_code_to_cell;
use ever_block::{AccountId, SliceData, UInt256};

fn send_message_to_freeze_and_immediately_unfreeze_account(mut acc: Account, state_init: &StateInit, bounce: bool) -> Account {
    let msg_income = 150000000 + 1000000000; // and not enough to not be frozen
    let due = 286774881u64;
    let mut msg = create_int_msg(
        AccountId::from([0x33; 32]),
        AccountId::from([0x11; 32]),
        msg_income,
        bounce,
        BLOCK_LT
    );
    msg.set_state_init(state_init.clone());

    // send message to freeze and unfreeze account
    let mut new_acc = Account::active_by_init_code_hash(
        acc.get_addr().unwrap().clone(),
        CurrencyCollection::with_grams(700155119),
        BLOCK_UT,
        state_init.clone(),
        false
    ).unwrap();
    new_acc.update_storage_stat().unwrap();
    new_acc.set_last_tr_time(BLOCK_LT + 4);

    let tr_lt = BLOCK_LT + 1;
    let trans = execute_c(&msg, &mut acc, tr_lt, new_acc.balance().unwrap().grams, 2).unwrap();
    acc.update_storage_stat().unwrap();

    assert_eq!(acc, new_acc);

    let mut description = TransactionDescrOrdinary::default();
    description.storage_ph = Some(TrStoragePhase::with_params(
        (1 + if bounce {0} else {due}).into(),
        if bounce {Some(due.into())} else {None},
        if bounce {AccStatusChange::Frozen} else {AccStatusChange::Unchanged}
    ));
    description.credit_ph = Some(TrCreditPhase::with_params(
        if bounce {Some(due.into())} else {None},
        CurrencyCollection::with_grams(msg_income - if bounce {due} else {0})
    ));

    let gas_used = 1307u32;
    let gas_fees = gas_used as u64 * 10000;
    let mut vm_phase = TrComputePhaseVm::default();
    vm_phase.success = true;
    vm_phase.gas_used = gas_used.into();
    vm_phase.gas_limit = 86322.into();
    vm_phase.gas_fees = gas_fees.into();
    vm_phase.vm_steps = 10;
    description.compute_ph = TrComputePhase::Vm(vm_phase);

    let msg_mine_fee = MSG_MINE_FEE;
    let msg_fwd_fee = MSG_FWD_FEE;
    let msg_remain_fee = msg_fwd_fee - msg_mine_fee;
    let (mut msg1, mut msg2) = create_two_internal_messages();
    let mut actions = OutActions::default();
    actions.push_back(OutAction::new_send(SENDMSG_ORDINARY, msg1.clone()));
    actions.push_back(OutAction::new_send(SENDMSG_ORDINARY, msg2.clone()));
    if let Some(int_header) = msg1.int_header_mut() {
        if let Some(int_header2) = msg2.int_header_mut() {
            int_header.value.grams = (MSG1_BALANCE - msg_fwd_fee).into();
            int_header2.value.grams = (MSG2_BALANCE - msg_fwd_fee).into();
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
    action_ph.msgs_created = 2;
    action_ph.add_fwd_fees((2 * msg_fwd_fee).into());
    action_ph.add_action_fees((2 * msg_mine_fee).into());
    action_ph.action_list_hash = actions.hash().unwrap();
    action_ph.tot_msg_size = StorageUsedShort::calculate_for_struct(&msg1).unwrap();
    action_ph.tot_msg_size.append(&msg2.serialize().unwrap());
    description.action = Some(action_ph);

    description.credit_first = !bounce;
    description.bounce = None;
    description.aborted = false;
    description.destroyed = false;
    let description = TransactionDescr::Ordinary(description);

    let mut good_trans = Transaction::with_account_and_message(&new_acc, &msg, BLOCK_LT + 2).unwrap();

    good_trans.add_out_message(&make_common(msg1)).unwrap();
    good_trans.add_out_message(&make_common(msg2)).unwrap();
    let storage_fee_collected = 1;
    good_trans.set_total_fees(CurrencyCollection::with_grams(gas_fees + msg_mine_fee * 2 + due + storage_fee_collected));
    good_trans.orig_status = AccountStatus::AccStateActive;
    good_trans.set_end_status(AccountStatus::AccStateActive);
    good_trans.set_logical_time(BLOCK_LT + 1);
    good_trans.set_now(BLOCK_UT);

    good_trans.write_description(&description).unwrap();

    trans.out_msgs.scan_diff(&good_trans.out_msgs, |key: ever_block::U15, msg1, msg2| {
        assert_eq!(msg1, msg2, "for key {}", key.0);
        Ok(true)
    }).unwrap();
    assert_eq!(trans.read_description().unwrap(), description);
    assert_eq!(trans, good_trans);
    acc
}

fn send_money_to_frozen_account(mut acc: Account, bounce: bool) -> Account {
    let msg_income = 150000000;
    let msg = create_int_msg(
        AccountId::from([0x33; 32]),
        AccountId::from([0x11; 32]),
        msg_income,
        bounce,
        BLOCK_LT
    );

    let new_acc_balance = msg_income;

    let tr_lt = BLOCK_LT + 2;
    let due = acc.due_payment().cloned().unwrap_or_default();
    let trans = execute_c(&msg, &mut acc, tr_lt, if bounce {0.into()} else {Grams::from(new_acc_balance) - due}, if bounce {1} else {0}).unwrap();
    acc.update_storage_stat().unwrap();

    let new_acc = Account::frozen(
        acc.get_addr().unwrap().clone(),
        BLOCK_LT + 3 + trans.msg_count() as u64, BLOCK_UT,
        acc.frozen_hash().unwrap().clone(),
        None,
        CurrencyCollection::from_grams(if bounce {0.into()} else {Grams::from(new_acc_balance) - due})
    );

    assert_eq!(acc, new_acc);

    let mut description = TransactionDescrOrdinary::default();
    description.storage_ph = Some(TrStoragePhase::with_params(Grams::zero(), if bounce {Some(136774881u64.into())} else {None}, AccStatusChange::Unchanged));
    description.credit_ph = Some(TrCreditPhase::with_params(Some(due), CurrencyCollection::from_grams(Grams::from(msg_income) - due)));
    description.compute_ph = TrComputePhase::skipped(ComputeSkipReason::NoState);

    description.action = None;
    description.credit_first = !bounce;
    description.aborted = true;
    description.destroyed = false;

    let mut good_trans = Transaction::with_account_and_message(&new_acc, &msg, BLOCK_LT + 2).unwrap();

    let msg_fee = 3333282u64;
    if bounce {
        description.bounce = Some(
            TrBouncePhase::Ok(TrBouncePhaseOk {
                msg_size: StorageUsedShort::default(),
                msg_fees: msg_fee.into(),
                fwd_fees: 6666718u64.into(),
            }),
        );

        let mut message = Message::with_int_header(InternalMessageHeader{
            ihr_disabled: true,
            bounce: false,
            bounced: true,
            src: MsgAddressIntOrNone::Some(MsgAddressInt::with_standart(None, -1, AccountId::from([0x11; 32])).unwrap()),
            dst: MsgAddressInt::with_standart(None, -1, AccountId::from([0x33; 32])).unwrap(),
            value: CurrencyCollection::with_grams(3225119),
            ihr_fee: Default::default(),
            fwd_fee: 6666718u64.into(),
            created_lt: 2000000003,
            created_at: 1576526553.into()
        });
        message.set_body(SliceData::from_raw(vec![0xff; 4], 32));
        good_trans.add_out_message(&make_common(message)).unwrap();
    } else {
        description.bounce = None;
    }

    good_trans.set_total_fees(CurrencyCollection::from_grams(due + if bounce {msg_fee as u128} else {0}));
    good_trans.orig_status = AccountStatus::AccStateFrozen;
    good_trans.set_end_status(AccountStatus::AccStateFrozen);
    good_trans.set_logical_time(BLOCK_LT + 2);
    good_trans.set_now(BLOCK_UT);

    let description = TransactionDescr::Ordinary(description);
    good_trans.write_description(&description).unwrap();

    assert_eq!(trans.read_description().unwrap(), description);
    trans.out_msgs.scan_diff(&good_trans.out_msgs, |key: ever_block::U15, msg1, msg2| {
        assert_eq!(msg1, msg2, "for key {}", key.0);
        Ok(true)
    }).unwrap();
    assert_eq!(trans, good_trans);
    acc
}

fn lead_account_into_even_more_debt(mut acc: Account, bounce: bool) -> Account {
    let msg_income = 15;
    let msg = create_int_msg(
        AccountId::from([0x33; 32]),
        AccountId::from([0x11; 32]),
        msg_income,
        bounce,
        BLOCK_LT
    );

    let tr_lt = BLOCK_LT + 2;
    let old_due = acc.due_payment().cloned().unwrap_or_default();
    acc.set_last_paid(acc.last_paid() - 100);
    let trans = execute_c(&msg, &mut acc, tr_lt, 0, 0).unwrap();
    acc.update_storage_stat().unwrap();

    assert!(old_due < acc.due_payment().cloned().unwrap_or_default());

    let new_acc = Account::frozen(
        acc.get_addr().unwrap().clone(),
        BLOCK_LT + 3, BLOCK_UT,
        acc.frozen_hash().unwrap().clone(),
        Some(acc.due_payment().cloned().unwrap_or_default()),
        CurrencyCollection::with_grams(0)
    );

    assert_eq!(acc, new_acc);

    let mut description = TransactionDescrOrdinary::default();
    description.storage_ph = Some(TrStoragePhase::with_params(
        Grams::zero(),
        Some(acc.due_payment().cloned().unwrap_or_default() + if bounce {msg_income as u128} else {0}),
        AccStatusChange::Unchanged)
    );
    description.credit_ph = Some(TrCreditPhase::with_params(Some(msg_income.into()), CurrencyCollection::with_grams(0)));
    description.compute_ph = TrComputePhase::skipped(ComputeSkipReason::NoGas);
    if bounce {
        description.bounce = Some(
            TrBouncePhase::Nofunds(TrBouncePhaseNofunds {
                msg_size: StorageUsedShort::default(),
                req_fwd_fees: 10000000u64.into()
            }),
        );
    } else {
        description.bounce = None;
    }

    description.action = None;
    description.credit_first = !bounce;
    description.aborted = true;
    description.destroyed = false;
    let description = TransactionDescr::Ordinary(description);

    let mut good_trans = Transaction::with_account_and_message(&new_acc, &msg, BLOCK_LT + 2).unwrap();

    good_trans.set_total_fees(CurrencyCollection::with_grams(msg_income));
    good_trans.orig_status = AccountStatus::AccStateFrozen;
    good_trans.set_end_status(AccountStatus::AccStateFrozen);
    good_trans.set_logical_time(BLOCK_LT + 2);
    good_trans.set_now(BLOCK_UT);

    good_trans.write_description(&description).unwrap();

    assert_eq!(trans.read_description().unwrap(), description);
    assert_eq!(trans, good_trans);
    acc
}

fn send_message_to_freeze_account(mut acc: Account, start_balance: u64, bounce: bool) -> Account {
    let msg_income = 150000000;
    let storage_fee = 286774882;
    let total_balance = start_balance + if !bounce {msg_income} else {0};
    let due_payment = storage_fee - total_balance;

    let msg = create_int_msg(
        AccountId::from([0x33; 32]),
        AccountId::from([0x11; 32]),
        msg_income,
        bounce,
        BLOCK_LT
    );

    let state_init = acc.state_init().unwrap().clone();
    let frozen_hash = state_init.serialize().unwrap().repr_hash();

    // send message to freeze account
    let mut new_acc = Account::frozen(
        acc.get_addr().unwrap().clone(),
        BLOCK_LT + 2, BLOCK_UT,
        frozen_hash,
        Some(136774881u64.into()),
        CurrencyCollection::default()
    );
    new_acc.update_storage_stat().unwrap();

    let tr_lt = BLOCK_LT + 1;
    let trans = execute_c(&msg, &mut acc, tr_lt, 0, 0).unwrap();
    acc.update_storage_stat().unwrap();

    assert_eq!(acc, new_acc);

    let mut good_trans = Transaction::with_account_and_message(&new_acc, &msg, BLOCK_LT + 1).unwrap();
    let mut description = TransactionDescrOrdinary::default();
    description.storage_ph = Some(TrStoragePhase {
        storage_fees_collected: (total_balance).into(), // collect full balance as fee
        storage_fees_due: Some(Grams::from(due_payment)), // also due_payment credit for next transaction
        status_change: AccStatusChange::Frozen // freeze account
    });
    description.credit_ph = Some(TrCreditPhase {
        due_fees_collected: if bounce {Some(msg_income.into())} else {None},
        credit: CurrencyCollection::with_grams(if bounce {0} else {msg_income}),
    });
    description.compute_ph = TrComputePhase::skipped(ComputeSkipReason::NoGas);

    if bounce {
        description.bounce = Some(
            TrBouncePhase::Nofunds(TrBouncePhaseNofunds {
                msg_size: StorageUsedShort::default(),
                req_fwd_fees: 10000000u64.into()
            }),
        );
    } else {
        description.bounce = None;
    }

    description.action = None;
    description.credit_first = !bounce;
    description.aborted = true;
    description.destroyed = false;
    let description = TransactionDescr::Ordinary(description);

    good_trans.set_total_fees(CurrencyCollection::with_grams(total_balance + if bounce {msg_income} else {0}));
    good_trans.orig_status = AccountStatus::AccStateActive;
    good_trans.set_end_status(AccountStatus::AccStateFrozen);
    good_trans.set_now(BLOCK_UT);

    good_trans.write_description(&description).unwrap();

    assert_eq!(trans.read_description().unwrap(), description);
    assert_eq!(trans, good_trans);
    acc
}

fn unfreeze_account(mut acc: Account, state_init: &StateInit, bounce: bool) -> Account {
    let msg_income= 1000000000 + 150000000;
    let mut msg = create_int_msg(
        AccountId::from([0x33; 32]),
        AccountId::from([0x11; 32]),
        msg_income,
        bounce,
        BLOCK_LT
    );

    let gas_used = 1307u32;
    let gas_fees = gas_used as u64 * 10000;

    let msg_mine_fee = MSG_MINE_FEE;
    let msg_fwd_fee = MSG_FWD_FEE;
    let msg_remain_fee = msg_fwd_fee - msg_mine_fee;

    let acc_balance = acc.get_balance().unwrap().grams;

    let due = acc.due_payment().cloned().unwrap_or_default();
    let new_acc_balance = acc_balance + (msg_income - gas_fees - MSG1_BALANCE - MSG2_BALANCE) as u128 - due;
    let tr_lt = BLOCK_LT + 3;
    msg.set_state_init(state_init.clone());
    let trans = execute_c(&msg, &mut acc, tr_lt, new_acc_balance, 2).unwrap();
    acc.update_storage_stat().unwrap();

    let mut new_acc = acc.clone();
    new_acc.set_last_paid(BLOCK_UT);
    new_acc.set_last_tr_time(acc.last_tr_time().unwrap());
    new_acc.set_balance(CurrencyCollection::from_grams(new_acc_balance));
    new_acc.update_storage_stat().unwrap();

    let (mut msg1, mut msg2) = create_two_internal_messages();
    let mut actions = OutActions::default();
    actions.push_back(OutAction::new_send(SENDMSG_ORDINARY, msg1.clone()));
    actions.push_back(OutAction::new_send(SENDMSG_ORDINARY, msg2.clone()));
    if let (Some(int_header), Some(int_header2)) = (msg1.int_header_mut(), msg2.int_header_mut()) {
        int_header.value.grams = Grams::from(MSG1_BALANCE - msg_fwd_fee);
        int_header2.value.grams = Grams::from(MSG2_BALANCE - msg_fwd_fee);
        int_header.fwd_fee = msg_remain_fee.into();
        int_header2.fwd_fee = msg_remain_fee.into();
        int_header.created_at = BLOCK_UT.into();
        int_header2.created_at = BLOCK_UT.into();
        int_header.created_lt = acc.last_tr_time().unwrap() - 2;
        int_header2.created_lt = acc.last_tr_time().unwrap() - 1;
    }

    assert_eq!(acc.status(), AccountStatus::AccStateActive);
    assert_eq!(acc, new_acc);

    let mut description = TransactionDescrOrdinary::default();
    description.storage_ph = Some(TrStoragePhase {
        storage_fees_collected: Grams::zero(),
        storage_fees_due: if bounce && !due.is_zero() {Some(due)} else {None},
        status_change: AccStatusChange::Unchanged
    });
    description.credit_ph = Some(TrCreditPhase {
        due_fees_collected: if !due.is_zero() {Some(due)} else {None},
        credit: CurrencyCollection::from_grams(Grams::from(msg_income) - due),
    });
    let mut vm_phase = TrComputePhaseVm::default();
    vm_phase.success = true;
    vm_phase.gas_used = gas_used.into();
    vm_phase.gas_limit = ((msg_income as u32 - due.as_u128() as u32) / 10000).into();
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
    description.credit_first = !bounce;
    description.bounce = None;
    description.aborted = false;
    description.destroyed = false;
    let description = TransactionDescr::Ordinary(description);

    let mut good_trans = Transaction::with_account_and_message(&new_acc, &msg, BLOCK_LT + 3).unwrap();
    good_trans.write_in_msg(Some(&make_common(msg))).unwrap();
    good_trans.add_out_message(&make_common(msg1)).unwrap();
    good_trans.add_out_message(&make_common(msg2)).unwrap();
    good_trans.set_total_fees(CurrencyCollection::from_grams(due + (gas_fees + msg_mine_fee * 2) as u128));
    good_trans.orig_status = AccountStatus::AccStateFrozen;
    good_trans.set_end_status(AccountStatus::AccStateActive);
    good_trans.set_logical_time(acc.last_tr_time().unwrap() - 3);
    good_trans.set_now(BLOCK_UT);

    good_trans.write_description(&description).unwrap();

    assert_eq!(trans.read_description().unwrap(), description);
    trans.out_msgs.scan_diff(&good_trans.out_msgs, |key: ever_block::U15, msg1, msg2| {
        assert_eq!(msg1, msg2, "for key {}", key.0);
        Ok(true)
    }).unwrap();
    assert_eq!(trans, good_trans);
    acc
}

fn try_unfreeze_account_with_small_value(mut acc: Account, state_init: &StateInit, bounce: bool) -> Account {
    let due = acc.due_payment().cloned().unwrap_or_default();
    let msg_income= due + 1;
    let mut msg = create_int_msg(
        AccountId::from([0x33; 32]),
        AccountId::from([0x11; 32]),
        msg_income,
        bounce,
        BLOCK_LT
    );

    let new_acc_balance = 1;
    let tr_lt = BLOCK_LT + 3;
    msg.set_state_init(state_init.clone());
    let trans = execute_c(&msg, &mut acc, tr_lt, new_acc_balance, 0).unwrap();
    acc.update_storage_stat().unwrap();

    let mut new_acc = acc.clone();
    new_acc.set_last_paid(BLOCK_UT);
    new_acc.set_last_tr_time(tr_lt + 1);
    new_acc.set_balance(CurrencyCollection::with_grams(new_acc_balance));
    new_acc.update_storage_stat().unwrap();

    assert_eq!(acc.status(), AccountStatus::AccStateFrozen);
    assert_eq!(acc, new_acc);

    let mut description = TransactionDescrOrdinary::default();
    description.storage_ph = Some(TrStoragePhase {
        storage_fees_collected: Grams::zero(),
        storage_fees_due: if bounce {Some(due)} else {None},
        status_change: AccStatusChange::Unchanged
    });
    description.credit_ph = Some(TrCreditPhase {
        due_fees_collected: if !due.is_zero() {Some(due)} else {None},
        credit: CurrencyCollection::from_grams(msg_income - due),
    });
    description.compute_ph = TrComputePhase::skipped(ComputeSkipReason::NoGas);
    if bounce {
        description.bounce = Some(
            TrBouncePhase::Nofunds(TrBouncePhaseNofunds {
                msg_size: StorageUsedShort::default(),
                req_fwd_fees: 10000000u64.into()
            }),
        );
    } else {
        description.bounce = None;
    }

    description.action = None;
    description.credit_first = !bounce;
    description.aborted = true;
    description.destroyed = false;
    let description = TransactionDescr::Ordinary(description);

    let mut good_trans = Transaction::with_account_and_message(&new_acc, &msg, BLOCK_LT + 3).unwrap();
    good_trans.write_in_msg(Some(&make_common(msg))).unwrap();
    good_trans.set_total_fees(CurrencyCollection::from_grams(due));
    good_trans.orig_status = AccountStatus::AccStateFrozen;
    good_trans.set_end_status(AccountStatus::AccStateFrozen);
    good_trans.set_logical_time(BLOCK_LT + 3);
    good_trans.set_now(BLOCK_UT);

    good_trans.write_description(&description).unwrap();

    assert_eq!(trans.read_description().unwrap(), description);
    assert_eq!(trans, good_trans);
    acc
}

fn test_freeze_and_unfreeze_account(bounce: bool) {
    let start_balance = 1; // not enough to pay storage
    let acc_id = AccountId::from([0x11; 32]);
    let acc = create_test_account(start_balance, acc_id, create_send_two_messages_code(), create_two_messages_data());
    let state_init = acc.state_init().unwrap().clone();

    send_message_to_freeze_and_immediately_unfreeze_account(acc.clone(), &state_init, bounce);

    let acc_frozen = send_message_to_freeze_account(acc, start_balance, bounce);
    try_unfreeze_account_with_small_value(acc_frozen.clone(), &state_init, bounce); // account will remain frozen

    unfreeze_account(acc_frozen.clone(), &state_init, bounce);

    lead_account_into_even_more_debt(acc_frozen.clone(), bounce);

    let acc_frozen = send_money_to_frozen_account(acc_frozen, bounce);
    unfreeze_account(acc_frozen, &state_init, bounce);
}

#[test]
fn test_freeze_and_unfreeze_account_without_bounce() {
    test_freeze_and_unfreeze_account(false);
}

#[test]
fn test_freeze_and_unfreeze_account_with_bounce() {
    test_freeze_and_unfreeze_account(true);
}

fn delete_frozen_account(mut acc: Account, bounce: bool) -> Account {
    let acc_addr = acc.get_addr().unwrap().address();
    let msg_income = 15;
    let msg = create_int_msg(
        AccountId::from([0x33; 32]),
        acc_addr.clone(),
        msg_income,
        bounce,
        BLOCK_LT
    );

    let tr_lt = BLOCK_LT + 2;
    let trans = execute_c(&msg, &mut acc, tr_lt, if bounce {15} else {0}, 0).unwrap();

    let new_acc = if bounce {
        let mut new_acc = Account::with_address_and_ballance(
            &MsgAddressInt::with_standart(None, -1, acc_addr).unwrap(),
            &msg_income.into());
        new_acc.set_last_paid(acc.last_paid());
        new_acc.set_last_tr_time(BLOCK_LT + 4);
        new_acc.update_storage_stat().unwrap();
        new_acc
    } else {
        Account::default()
    };
    assert_eq!(acc, new_acc);

    let mut description = TransactionDescrOrdinary::default();
    description.storage_ph = Some(TrStoragePhase::with_params(Grams::zero(), Some(Grams::from(1262901841 + if bounce {msg_income} else {0})), AccStatusChange::Deleted));
    description.credit_ph = Some(TrCreditPhase::with_params(if bounce {None} else {Some(msg_income.into())}, CurrencyCollection::with_grams(if bounce {msg_income} else {0})));
    description.compute_ph = TrComputePhase::skipped(ComputeSkipReason::NoGas);

    if bounce {
        description.bounce = Some(
            TrBouncePhase::Nofunds(TrBouncePhaseNofunds {
                msg_size: StorageUsedShort::default(),
                req_fwd_fees: 10000000u64.into()
            }),
        );
    } else {
        description.bounce = None;
    }

    description.action = None;
    description.credit_first = !bounce;
    description.aborted = true;
    description.destroyed = false;
    let description = TransactionDescr::Ordinary(description);

    let mut good_trans = Transaction::with_account_and_message(&new_acc, &msg, BLOCK_LT + 2).unwrap();

    good_trans.set_total_fees(CurrencyCollection::with_grams(if bounce {0} else {msg_income}));
    good_trans.orig_status = AccountStatus::AccStateFrozen;
    good_trans.set_end_status(if bounce {AccountStatus::AccStateUninit} else {AccountStatus::AccStateNonexist});
    good_trans.set_logical_time(BLOCK_LT + 3);
    good_trans.set_now(BLOCK_UT);

    good_trans.write_description(&description).unwrap();

    assert_eq!(trans.read_description().unwrap(), description);
    assert_eq!(trans, good_trans);
    acc
}

fn delete_account_and_get_bounced_message(mut acc: Account) -> Account {
    let acc_address = acc.get_addr().unwrap().address();
    let msg_income = 150000000000000000u64;
    let msg = create_int_msg(
        AccountId::from([0x33; 32]),
        acc_address.clone(),
        msg_income,
        true,
        BLOCK_LT
    );

    let tr_lt = BLOCK_LT + 2;
    let trans = execute_c(&msg, &mut acc, tr_lt, 0, 1).unwrap();

    let new_acc = Account::default();
    assert_eq!(acc, new_acc);

    let mut description = TransactionDescrOrdinary::default();
    description.storage_ph = Some(TrStoragePhase::with_params(Grams::zero(), Some(1262901856u64.into()), AccStatusChange::Deleted));
    description.credit_ph = Some(TrCreditPhase::with_params(None, CurrencyCollection::with_grams(150000000000000000)));
    description.compute_ph = TrComputePhase::skipped(ComputeSkipReason::NoState);

    description.action = None;
    description.credit_first = false;
    description.bounce = Some(
        TrBouncePhase::Ok(TrBouncePhaseOk {
            msg_size: StorageUsedShort::default(),
            msg_fees: 3333282u64.into(),
            fwd_fees: 6666718u64.into(),
        }),
    );
    description.aborted = true;
    description.destroyed = false;
    let description = TransactionDescr::Ordinary(description);

    let mut message = Message::with_int_header(InternalMessageHeader{
        ihr_disabled: true,
        bounce: false,
        bounced: true,
        src: MsgAddressIntOrNone::Some(MsgAddressInt::with_standart(None, -1, acc_address).unwrap()),
        dst: MsgAddressInt::with_standart(None, -1, AccountId::from([0x33; 32])).unwrap(),
        value: CurrencyCollection::with_grams(149999999990000000),
        ihr_fee: Default::default(),
        fwd_fee: 6666718u64.into(),
        created_lt: 2000000004,
        created_at: 1576526553.into()
    });
    message.set_body(SliceData::from_raw(vec![0xff; 4], 32));

    let mut good_trans = Transaction::with_account_and_message(&new_acc, &msg, BLOCK_LT + 2).unwrap();

    good_trans.add_out_message(&make_common(message)).unwrap();
    good_trans.set_total_fees(CurrencyCollection::with_grams(3333282));
    good_trans.orig_status = AccountStatus::AccStateFrozen;
    good_trans.set_end_status(AccountStatus::AccStateNonexist);
    good_trans.set_logical_time(BLOCK_LT + 3);
    good_trans.set_now(BLOCK_UT);

    good_trans.write_description(&description).unwrap();

    assert_eq!(trans.read_description().unwrap(), description);
    trans.out_msgs.scan_diff(&good_trans.out_msgs, |key: ever_block::U15, msg1, msg2| {
        assert_eq!(msg1, msg2, "for key {}", key.0);
        Ok(true)
    }).unwrap();
    assert_eq!(trans, good_trans);
    acc
}

fn delete_and_immediately_restore_account(mut acc: Account, state_init: &StateInit) -> Account {
    let msg_income = 150000000000000000u64;
    let mut msg = create_int_msg(
        AccountId::from([0x33; 32]),
        SliceData::from(state_init.hash().unwrap()),
        msg_income,
        true,
        BLOCK_LT
    );
    msg.set_state_init(state_init.clone());

    let tr_lt = BLOCK_LT + 2;
    let trans = execute_c(&msg, &mut acc, tr_lt, 149999999999000000u64, 0).unwrap();

    let mut new_acc = Account::active_by_init_code_hash(
        MsgAddressInt::with_standart(None, -1, SliceData::from(state_init.hash().unwrap())).unwrap(),
        CurrencyCollection::with_grams(149999999999000000),
        1576526553,
        state_init.clone(),
        false
    ).unwrap();
    new_acc.set_last_tr_time(2000000004);
    new_acc.update_storage_stat().unwrap();
    new_acc.set_data(acc.get_data().unwrap());
    assert_eq!(acc, new_acc);

    assert_eq!(get_tr_descr(&trans).storage_ph.unwrap().status_change, AccStatusChange::Deleted);
    assert_eq!(trans.end_status, AccountStatus::AccStateActive);

    acc
}

fn try_delete_active_account(mut acc: Account, bounce: bool) -> Account {
    let msg_income = 15;
    let msg = create_int_msg(
        AccountId::from([0x33; 32]),
        acc.get_addr().unwrap().address(),
        msg_income,
        bounce,
        BLOCK_LT
    );

    let acc_balance_before = acc.balance().unwrap().grams;

    let due = 1664733855;
    let tr_lt = BLOCK_LT + 2;
    let trans = execute_c(&msg, &mut acc, tr_lt, 0, 0).unwrap();

    let mut new_acc = Account::frozen(
        acc.get_addr().unwrap().clone(),
        BLOCK_LT + 3, BLOCK_UT,
        acc.frozen_hash().unwrap().clone(),
        Some(Grams::from(due)),
        CurrencyCollection::with_grams(0)
    );
    new_acc.update_storage_stat().unwrap();
    acc.update_storage_stat().unwrap();
    assert_eq!(acc, new_acc);

    let mut description = TransactionDescrOrdinary::default();
    description.storage_ph = Some(TrStoragePhase::with_params(
        acc_balance_before + if bounce {0} else {msg_income as u128},
        Some(Grams::from(due + if bounce {msg_income} else {0})),
        AccStatusChange::Frozen
    ));
    description.credit_ph = Some(TrCreditPhase::with_params(
        if bounce {Some(msg_income.into())} else {None},
        CurrencyCollection::with_grams(if bounce {0} else {15})
    ));
    description.compute_ph = TrComputePhase::skipped(ComputeSkipReason::NoGas);

    description.action = None;
    description.credit_first = !bounce;
    if bounce {
        description.bounce = Some(
            TrBouncePhase::Nofunds(TrBouncePhaseNofunds {
                msg_size: StorageUsedShort::default(),
                req_fwd_fees: 10000000u64.into()
            }),
        );
    } else {
        description.bounce = None;
    }
    description.aborted = true;
    description.destroyed = false;
    let description = TransactionDescr::Ordinary(description);

    let mut good_trans = Transaction::with_account_and_message(&new_acc, &msg, BLOCK_LT + 2).unwrap();

    good_trans.set_total_fees(CurrencyCollection::from_grams(acc_balance_before + msg_income as u128));
    good_trans.orig_status = AccountStatus::AccStateActive;
    good_trans.set_end_status(AccountStatus::AccStateFrozen);
    good_trans.set_logical_time(BLOCK_LT + 2);
    good_trans.set_now(BLOCK_UT);

    good_trans.write_description(&description).unwrap();

    assert_eq!(trans.read_description().unwrap(), description);
    assert_eq!(trans, good_trans);
    acc
}

fn delete_uninit_account(mut acc: Account, bounce: bool) -> Account {
    let acc_addr = acc.get_addr().unwrap().address();
    let msg_income = 15;
    let msg = create_int_msg(
        AccountId::from([0x33; 32]),
        acc_addr.clone(),
        msg_income,
        bounce,
        BLOCK_LT
    );

    let acc_balance = acc.balance().unwrap().grams;
    let tr_lt = BLOCK_LT + 2;
    let trans = execute_c(&msg, &mut acc, tr_lt, if bounce { msg_income } else { 0 }, 0).unwrap();

    let new_acc = if bounce {
        let mut new_acc = Account::with_address_and_ballance(
            &MsgAddressInt::with_standart(None, -1, acc_addr).unwrap(),
            &msg_income.into());
        new_acc.set_last_paid(acc.last_paid());
        new_acc.set_last_tr_time(BLOCK_LT + 4);
        new_acc.update_storage_stat().unwrap();
        new_acc
    } else {
        Account::default()
    };
    assert_eq!(acc, new_acc);

    let mut description = TransactionDescrOrdinary::default();
    description.storage_ph = Some(TrStoragePhase::with_params(
        acc_balance + if bounce {0} else {msg_income as u128},
        Some((2650451629 + if bounce { msg_income } else { 0 }).into()),
        AccStatusChange::Deleted
    ));
    description.credit_ph = Some(TrCreditPhase::with_params(None, CurrencyCollection::with_grams(msg_income)));
    description.compute_ph = TrComputePhase::skipped(ComputeSkipReason::NoGas);

    description.action = None;
    description.credit_first = !bounce;
    if bounce {
        description.bounce = Some(
            TrBouncePhase::Nofunds(TrBouncePhaseNofunds {
                msg_size: StorageUsedShort::default(),
                req_fwd_fees: 10000000u64.into()
            }),
        );
    } else {
        description.bounce = None;
    }
    description.aborted = true;
    description.destroyed = false;
    let description = TransactionDescr::Ordinary(description);

    let mut good_trans = Transaction::with_account_and_message(&new_acc, &msg, BLOCK_LT + 3).unwrap();

    good_trans.set_total_fees(CurrencyCollection::from_grams(acc_balance + if bounce {0} else {msg_income as u128}));
    good_trans.orig_status = AccountStatus::AccStateUninit;
    good_trans.set_end_status(if bounce {AccountStatus::AccStateUninit} else {AccountStatus::AccStateNonexist});
    good_trans.set_logical_time(BLOCK_LT + 3);
    good_trans.set_now(BLOCK_UT);

    good_trans.write_description(&description).unwrap();

    assert_eq!(trans.read_description().unwrap(), description);
    assert_eq!(trans, good_trans);
    acc
}

fn delete_uninit_account_with_small_due(mut acc: Account, bounce: bool) {
    let msg_income = 15;
    let msg = create_int_msg(
        AccountId::from([0x33; 32]),
        acc.get_addr().unwrap().address(),
        msg_income,
        bounce,
        BLOCK_LT
    );

    let acc_balance = acc.balance().unwrap().grams;
    let tr_lt = BLOCK_LT + 2;
    let trans = execute_c(&msg, &mut acc, tr_lt, 0, 0).unwrap();

    let due = 8803;
    let mut new_acc = Account::default();
    new_acc.set_due_payment(Some(due.into()));
    new_acc.update_storage_stat().unwrap();
    acc.update_storage_stat().unwrap();
    assert_eq!(acc, new_acc);

    let mut description = TransactionDescrOrdinary::default();
    description.storage_ph = Some(TrStoragePhase::with_params(
        acc_balance + if bounce {0} else {msg_income as u128},
        Some((due + if bounce {msg_income} else {0}).into()),
        AccStatusChange::Unchanged
    ));
    description.credit_ph = Some(TrCreditPhase::with_params(
        if bounce {Some(msg_income.into())} else {None},
        CurrencyCollection::with_grams(if bounce {0} else {msg_income})
    ));
    description.compute_ph = TrComputePhase::skipped(ComputeSkipReason::NoGas);

    description.action = None;
    description.credit_first = !bounce;
    if bounce {
        description.bounce = Some(
            TrBouncePhase::Nofunds(TrBouncePhaseNofunds {
                msg_size: StorageUsedShort::default(),
                req_fwd_fees: 10000000u64.into()
            }),
        );
    } else {
        description.bounce = None;
    }
    description.aborted = true;
    description.destroyed = false;
    let description = TransactionDescr::Ordinary(description);

    let mut good_trans = Transaction::with_account_and_message(&new_acc, &msg, BLOCK_LT + 2).unwrap();

    good_trans.set_total_fees(CurrencyCollection::from_grams(acc_balance + msg_income as u128));
    good_trans.orig_status = AccountStatus::AccStateUninit;
    good_trans.set_end_status(AccountStatus::AccStateNonexist);
    good_trans.set_logical_time(BLOCK_LT + 3);
    good_trans.set_now(BLOCK_UT);

    good_trans.write_description(&description).unwrap();

    assert_eq!(trans.read_description().unwrap(), description);
    assert_eq!(trans, good_trans);
}

fn test_delete_account(bounce: bool) {
    let mut state_init = StateInit::default();
    state_init.set_code(compile_code_to_cell("NOP").unwrap());

    let acc_id = SliceData::from(state_init.hash().unwrap());
    let acc_frozen = Account::frozen(
        MsgAddressInt::with_standart(None, -1, acc_id.clone()).unwrap(),
        BLOCK_LT + 3, BLOCK_UT - 100000000,
        UInt256::default(),
        Some(1000000u64.into()),
        CurrencyCollection::with_grams(0)
    );

    delete_frozen_account(acc_frozen.clone(), bounce);
    delete_account_and_get_bounced_message(acc_frozen.clone());
    delete_and_immediately_restore_account(acc_frozen, &state_init);

    let mut state_init = StateInit::default();
    state_init.set_code(compile_code_to_cell("NOP").unwrap());
    let acc_active = Account::active_by_init_code_hash(
        MsgAddressInt::with_standart(None, -1, acc_id.clone()).unwrap(),
        CurrencyCollection::with_grams(17),
        BLOCK_UT - 100000000,
        state_init,
        false
    ).unwrap();

    try_delete_active_account(acc_active, bounce); // account will be frozen instead delete

    let acc_uninit = Account::uninit(
        MsgAddressInt::with_standart(None, -1, acc_id.clone()).unwrap(),
        BLOCK_LT + 3, BLOCK_UT - 300000000,
        CurrencyCollection::with_grams(17)
    );

    delete_uninit_account(acc_uninit, bounce);

    let acc_uninit = Account::uninit(
        MsgAddressInt::with_standart(None, -1, acc_id).unwrap(),
        BLOCK_LT + 3, BLOCK_UT - 1000,
        CurrencyCollection::with_grams(17)
    );

    delete_uninit_account_with_small_due(acc_uninit, bounce); // account will be uninit
}

fn try_delete_uninit_account_with_capundeletableaccounts(mut acc: Account, bounce: bool) -> Account {
    let acc_addr = acc.get_addr().unwrap().address();
    let msg_income = 15;
    let msg = create_int_msg(
        AccountId::from([0x33; 32]),
        acc_addr.clone(),
        msg_income,
        bounce,
        BLOCK_LT
    );

    let acc_balance = acc.balance().unwrap().grams;
    let tr_lt = BLOCK_LT + 2;
    let config = custom_config(None, Some(GlobalCapabilities::CapUndeletableAccounts as u64));
    let trans = execute_with_params_c(config,&msg, &mut acc, tr_lt, 0, 0).unwrap();

    let new_acc = Account::default();
    assert_eq!(acc, new_acc);

    let mut description = TransactionDescrOrdinary::default();
    description.storage_ph = Some(TrStoragePhase::with_params(
        acc_balance + if bounce {0} else {msg_income as u128},
        Some((2650451629 + if bounce { msg_income } else { 0 }).into()),
        AccStatusChange::Unchanged
    ));
    description.credit_ph = Some(TrCreditPhase::with_params(if bounce {Some(Grams::from(msg_income))} else {None}, CurrencyCollection::with_grams(if bounce {0} else {msg_income})));
    description.compute_ph = TrComputePhase::skipped(ComputeSkipReason::NoGas);

    description.action = None;
    description.credit_first = !bounce;
    if bounce {
        description.bounce = Some(
            TrBouncePhase::Nofunds(TrBouncePhaseNofunds {
                msg_size: StorageUsedShort::default(),
                req_fwd_fees: 10000000u64.into()
            }),
        );
    } else {
        description.bounce = None;
    }
    description.aborted = true;
    description.destroyed = false;
    let description = TransactionDescr::Ordinary(description);

    let mut good_trans = Transaction::with_account_and_message(&new_acc, &msg, BLOCK_LT + 3).unwrap();

    good_trans.set_total_fees(CurrencyCollection::from_grams(acc_balance + msg_income as u128));
    good_trans.orig_status = AccountStatus::AccStateUninit;
    good_trans.set_end_status(AccountStatus::AccStateNonexist);
    good_trans.set_logical_time(BLOCK_LT + 3);
    good_trans.set_now(BLOCK_UT);

    good_trans.write_description(&description).unwrap();

    assert_eq!(trans.read_description().unwrap(), description);
    assert_eq!(trans, good_trans);
    acc
}

fn try_delete_uninit_account_with_small_due_with_capundeletableaccounts(mut acc: Account, bounce: bool) {
    let msg_income = 15;
    let msg = create_int_msg(
        AccountId::from([0x33; 32]),
        acc.get_addr().unwrap().address(),
        msg_income,
        bounce,
        BLOCK_LT
    );

    let acc_balance = acc.balance().unwrap().grams;
    let tr_lt = BLOCK_LT + 2;
    let config = custom_config(None, Some(GlobalCapabilities::CapUndeletableAccounts as u64));
    let trans = execute_with_params_c(config,&msg, &mut acc, tr_lt, 0, 0).unwrap();

    let due = 8803;
    let mut new_acc = Account::default();
    new_acc.set_due_payment(Some(due.into()));
    new_acc.update_storage_stat().unwrap();
    acc.update_storage_stat().unwrap();
    assert_eq!(acc, new_acc);

    let mut description = TransactionDescrOrdinary::default();
    description.storage_ph = Some(TrStoragePhase::with_params(
        acc_balance + if bounce {0} else {msg_income as u128},
        Some((due + if bounce {msg_income} else {0}).into()),
        AccStatusChange::Unchanged
    ));
    description.credit_ph = Some(TrCreditPhase::with_params(
        if bounce {Some(msg_income.into())} else {None},
        CurrencyCollection::with_grams(if bounce {0} else {msg_income})
    ));
    description.compute_ph = TrComputePhase::skipped(ComputeSkipReason::NoGas);

    description.action = None;
    description.credit_first = !bounce;
    if bounce {
        description.bounce = Some(
            TrBouncePhase::Nofunds(TrBouncePhaseNofunds {
                msg_size: StorageUsedShort::default(),
                req_fwd_fees: 10000000u64.into()
            }),
        );
    } else {
        description.bounce = None;
    }
    description.aborted = true;
    description.destroyed = false;
    let description = TransactionDescr::Ordinary(description);

    let mut good_trans = Transaction::with_account_and_message(&new_acc, &msg, BLOCK_LT + 2).unwrap();

    good_trans.set_total_fees(CurrencyCollection::from_grams(acc_balance + msg_income as u128));
    good_trans.orig_status = AccountStatus::AccStateUninit;
    good_trans.set_end_status(AccountStatus::AccStateNonexist);
    good_trans.set_logical_time(BLOCK_LT + 3);
    good_trans.set_now(BLOCK_UT);

    good_trans.write_description(&description).unwrap();

    assert_eq!(trans.read_description().unwrap(), description);
    assert_eq!(trans, good_trans);
}

fn try_delete_active_account_with_capundeletableaccounts(mut acc: Account, bounce: bool) -> Account {
    let msg_income = 15;
    let msg = create_int_msg(
        AccountId::from([0x33; 32]),
        acc.get_addr().unwrap().address(),
        msg_income,
        bounce,
        BLOCK_LT
    );

    let acc_balance_before = acc.balance().unwrap().grams;

    let due = 1664733855;
    let tr_lt = BLOCK_LT + 2;
    let config = custom_config(None, Some(GlobalCapabilities::CapUndeletableAccounts as u64));
    let trans = execute_with_params_c(config,&msg, &mut acc, tr_lt, 0, 0).unwrap();

    let mut new_acc = Account::frozen(
        acc.get_addr().unwrap().clone(),
        BLOCK_LT + 3, BLOCK_UT,
        acc.frozen_hash().unwrap().clone(),
        Some(Grams::from(due)),
        CurrencyCollection::with_grams(0)
    );
    new_acc.update_storage_stat().unwrap();
    acc.update_storage_stat().unwrap();
    assert_eq!(acc, new_acc);

    let mut description = TransactionDescrOrdinary::default();
    description.storage_ph = Some(TrStoragePhase::with_params(
        acc_balance_before + if bounce {0} else {msg_income as u128},
        Some(Grams::from(due + if bounce {msg_income} else {0})),
        AccStatusChange::Frozen
    ));
    description.credit_ph = Some(TrCreditPhase::with_params(
        if bounce {Some(msg_income.into())} else {None},
        CurrencyCollection::with_grams(if bounce {0} else {15})
    ));
    description.compute_ph = TrComputePhase::skipped(ComputeSkipReason::NoGas);

    description.action = None;
    description.credit_first = !bounce;
    if bounce {
        description.bounce = Some(
            TrBouncePhase::Nofunds(TrBouncePhaseNofunds {
                msg_size: StorageUsedShort::default(),
                req_fwd_fees: 10000000u64.into()
            }),
        );
    } else {
        description.bounce = None;
    }
    description.aborted = true;
    description.destroyed = false;
    let description = TransactionDescr::Ordinary(description);

    let mut good_trans = Transaction::with_account_and_message(&new_acc, &msg, BLOCK_LT + 2).unwrap();

    good_trans.set_total_fees(CurrencyCollection::from_grams(acc_balance_before + msg_income as u128));
    good_trans.orig_status = AccountStatus::AccStateActive;
    good_trans.set_end_status(AccountStatus::AccStateFrozen);
    good_trans.set_logical_time(BLOCK_LT + 2);
    good_trans.set_now(BLOCK_UT);

    good_trans.write_description(&description).unwrap();

    assert_eq!(trans.read_description().unwrap(), description);
    assert_eq!(trans, good_trans);
    acc
}

fn try_delete_frozen_account_with_capundeletableaccounts(mut acc: Account, bounce: bool) -> Account {
    let acc_addr = acc.get_addr().unwrap().address();
    let msg_income = 15;
    let msg = create_int_msg(
        AccountId::from([0x33; 32]),
        acc_addr.clone(),
        msg_income,
        bounce,
        BLOCK_LT
    );

    let tr_lt = BLOCK_LT + 2;
    let config = custom_config(None, Some(GlobalCapabilities::CapUndeletableAccounts as u64));
    let trans = execute_with_params_c(config,&msg, &mut acc, tr_lt, 0, 0).unwrap();

    let new_acc = Account::frozen(
        MsgAddressInt::with_standart(None, -1, acc_addr).unwrap(),
        BLOCK_LT + 4,
        acc.last_paid(),
        UInt256::default(),
        Some(1262901841u64.into()),
        CurrencyCollection::with_grams(0)
    );

    assert_eq!(acc, new_acc);

    let mut description = TransactionDescrOrdinary::default();
    description.storage_ph = Some(TrStoragePhase::with_params(Grams::zero(), Some(Grams::from(1262901841 + if bounce {msg_income} else {0})), AccStatusChange::Unchanged));
    description.credit_ph = Some(TrCreditPhase::with_params(Some(msg_income.into()), CurrencyCollection::with_grams(0)));
    description.compute_ph = TrComputePhase::skipped(ComputeSkipReason::NoGas);

    if bounce {
        description.bounce = Some(
            TrBouncePhase::Nofunds(TrBouncePhaseNofunds {
                msg_size: StorageUsedShort::default(),
                req_fwd_fees: 10000000u64.into()
            }),
        );
    } else {
        description.bounce = None;
    }

    description.action = None;
    description.credit_first = !bounce;
    description.aborted = true;
    description.destroyed = false;
    let description = TransactionDescr::Ordinary(description);

    let mut good_trans = Transaction::with_account_and_message(&new_acc, &msg, BLOCK_LT + 2).unwrap();

    good_trans.set_total_fees(CurrencyCollection::with_grams(msg_income));
    good_trans.orig_status = AccountStatus::AccStateFrozen;
    good_trans.set_end_status(AccountStatus::AccStateFrozen);
    good_trans.set_logical_time(BLOCK_LT + 3);
    good_trans.set_now(BLOCK_UT);

    good_trans.write_description(&description).unwrap();

    assert_eq!(trans.read_description().unwrap(), description);
    assert_eq!(trans, good_trans);
    acc
}

fn test_delete_account_with_capundeletableaccounts(bounce: bool) {
    let mut state_init = StateInit::default();
    state_init.set_code(compile_code_to_cell("NOP").unwrap());

    let acc_id = SliceData::from(state_init.hash().unwrap());
    let acc_frozen = Account::frozen(
        MsgAddressInt::with_standart(None, -1, acc_id.clone()).unwrap(),
        BLOCK_LT + 3, BLOCK_UT - 100000000,
        UInt256::default(),
        Some(1000000u64.into()),
        CurrencyCollection::with_grams(0)
    );

    try_delete_frozen_account_with_capundeletableaccounts(acc_frozen.clone(), bounce);

    let mut state_init = StateInit::default();
    state_init.set_code(compile_code_to_cell("NOP").unwrap());
    let acc_active = Account::active_by_init_code_hash(
        MsgAddressInt::with_standart(None, -1, acc_id.clone()).unwrap(),
        CurrencyCollection::with_grams(17),
        BLOCK_UT - 100000000,
        state_init,
        false
    ).unwrap();

    try_delete_active_account_with_capundeletableaccounts(acc_active, bounce); 

    let acc_uninit = Account::uninit(
        MsgAddressInt::with_standart(None, -1, acc_id.clone()).unwrap(),
        BLOCK_LT + 3, BLOCK_UT - 300000000,
        CurrencyCollection::with_grams(17)
    );

    try_delete_uninit_account_with_capundeletableaccounts(acc_uninit, bounce);
 
    let acc_uninit = Account::uninit(
        MsgAddressInt::with_standart(None, -1, acc_id).unwrap(),
        BLOCK_LT + 3, BLOCK_UT - 1000,
        CurrencyCollection::with_grams(17)
    );

    try_delete_uninit_account_with_small_due_with_capundeletableaccounts(acc_uninit, bounce); 
    
}

#[test]
fn test_delete_account_with_capundeletableaccounts_bounce() {
    test_delete_account_with_capundeletableaccounts(false);
    test_delete_account_with_capundeletableaccounts(true);
}

#[test]
fn test_delete_account_without_bounce() {
    test_delete_account(false);
}

#[test]
fn test_delete_account_with_bounce() {
    test_delete_account(true);
}

fn delete_frozen_account_and_bounce(mut acc: Account) -> Account {
    let msg_income = 1500000000000000;
    let msg_balance_before = acc.balance().unwrap().grams;
    let msg = create_int_msg(
        AccountId::from([0x33; 32]),
        AccountId::from([0x11; 32]),
        msg_income,
        true,
        BLOCK_LT
    );

    let tr_lt = BLOCK_LT + 2;
    let trans = execute_c(&msg, &mut acc, tr_lt, 0, 1).unwrap();

    let new_acc = Account::default();
    assert_eq!(acc, new_acc);

    let mut description = TransactionDescrOrdinary::default();
    description.storage_ph = Some(TrStoragePhase::with_params(msg_balance_before, Some(1275108872u64.into()), AccStatusChange::Deleted));
    description.credit_ph = Some(TrCreditPhase::with_params(None, CurrencyCollection::with_grams(msg_income)));
    description.compute_ph = TrComputePhase::skipped(ComputeSkipReason::NoState);

    let msg_fee = 3333282u64;
    description.bounce = Some(
        TrBouncePhase::Ok(TrBouncePhaseOk {
            msg_size: StorageUsedShort::default(),
            msg_fees: msg_fee.into(),
            fwd_fees: 6666718u64.into(),
        }),
    );

    let mut good_trans = Transaction::with_account_and_message(&new_acc, &msg, BLOCK_LT + 2).unwrap();

    let mut message = Message::with_int_header(InternalMessageHeader{
        ihr_disabled: true,
        bounce: false,
        bounced: true,
        src: MsgAddressIntOrNone::Some(MsgAddressInt::with_standart(None, -1, AccountId::from([0x11; 32])).unwrap()),
        dst: MsgAddressInt::with_standart(None, -1, AccountId::from([0x33; 32])).unwrap(),
        value: CurrencyCollection::with_grams(1499999990000000),
        ihr_fee: Default::default(),
        fwd_fee: 6666718u64.into(),
        created_lt: 2000000004,
        created_at: 1576526553.into()
    });
    message.set_body(SliceData::from_raw(vec![0xff; 4], 32));
    good_trans.add_out_message(&make_common(message)).unwrap();

    description.action = None;
    description.credit_first = false;
    description.aborted = true;
    description.destroyed = false;
    let description = TransactionDescr::Ordinary(description);

    good_trans.set_total_fees(CurrencyCollection::with_grams(3333297));
    good_trans.orig_status = AccountStatus::AccStateFrozen;
    good_trans.set_end_status(AccountStatus::AccStateNonexist);
    good_trans.set_logical_time(BLOCK_LT + 3);
    good_trans.set_now(BLOCK_UT);

    good_trans.write_description(&description).unwrap();

    assert_eq!(trans.read_description().unwrap(), description);
    trans.out_msgs.scan_diff(&good_trans.out_msgs, |key: ever_block::U15, msg1, msg2| {
        assert_eq!(msg1, msg2, "for key {}", key.0);
        Ok(true)
    }).unwrap();
    assert_eq!(trans, good_trans);
    acc
}

#[test]
fn delete_account_and_bounce_message() {
    let acc_id = AccountId::from([0x11; 32]);
    let acc_frozen = Account::frozen(
        MsgAddressInt::with_standart(None, -1, acc_id).unwrap(),
        BLOCK_LT + 3, BLOCK_UT - 100000000,
        UInt256::default(),
        Some(1000000u64.into()),
        CurrencyCollection::with_grams(15)
    );

    delete_frozen_account_and_bounce(acc_frozen);
}