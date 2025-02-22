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
use common::*;

use super::*;
use pretty_assertions::assert_eq;
use ever_block::{
    accounts::Account,
    messages::{Message, MsgAddressInt},
    out_actions::{OutAction, OutActions, SENDMSG_REMAINING_MSG_BALANCE},
    transactions::{
        AccStatusChange, TrActionPhase, TrComputePhase, TrComputePhaseVm, TrCreditPhase,
        TrStoragePhase, Transaction, TransactionDescr,
    },
    AccountStatus, ComputeSkipReason, CurrencyCollection, GetRepresentationHash, Grams,
    InternalMessageHeader, MsgAddressIntOrNone, Serializable, StateInit, StorageUsedShort,
    TrBouncePhaseNofunds, TrBouncePhaseOk, UnixTime32, VarUInteger32, SENDMSG_PAY_FEE_SEPARATELY,
};
use ever_assembler::compile_code_to_cell;
use ever_block::{AccountId, SliceData};
use crate::VERSION_BLOCK_NEW_CALCULATION_BOUNCED_STORAGE;

fn create_msg_currency_workchain(w_id: i8, dest: AccountId, currencies: &CurrencyCollection, bounce: bool) -> Message {
    let mut hdr = InternalMessageHeader::with_addresses(
        MsgAddressInt::with_standart(None, w_id, AccountId::from([0x33; 32])).unwrap(),
        MsgAddressInt::with_standart(None, w_id, dest).unwrap(),
        currencies.clone(),
    );
    hdr.bounce = bounce;
    hdr.ihr_disabled = true;
    hdr.created_lt = BLOCK_LT - 2;
    hdr.created_at = UnixTime32::default();
    Message::with_int_header(hdr)
}

fn create_msg_currency(dest: AccountId, currencies: &CurrencyCollection, bounce: bool) -> Message {
    create_msg_currency_workchain(0, dest, currencies, bounce)
}

pub fn check_account_and_transaction_with_currencies(
    acc_before: &Account,
    acc_after: &Account,
    msg: &Message,
    trans: Option<&Transaction>,
    result_account_balance: &CurrencyCollection,
    count_out_msgs: usize,
) {
    if let Some(trans) = trans {
        assert_eq!(
            (
                trans.out_msgs.len().unwrap(),
                &acc_after.balance().cloned().unwrap_or_default().grams,
                acc_after.balance().cloned().unwrap_or_default().other.get(&11111111u32).unwrap().unwrap_or_default()
            ),
            (
                count_out_msgs,
                &result_account_balance.grams,
                result_account_balance.other.get(&11111111u32).unwrap().unwrap_or_default()
            )
        );
    }
    check_account_and_transaction_balances(acc_before, acc_after, msg, trans);
}

pub fn execute_currencies(msg: &Message, acc: &mut Account, tr_lt: u64, result_account_balance: &CurrencyCollection, count_out_msgs: usize) -> Result<Transaction> {
    let acc_before = acc.clone();
    let config = custom_config(None, None);
    let trans = execute_with_block_version(config, msg, acc, tr_lt, VERSION_BLOCK_NEW_CALCULATION_BOUNCED_STORAGE);
    check_account_and_transaction_with_currencies(&acc_before, acc, msg, trans.as_ref().ok(), result_account_balance, count_out_msgs);
    trans
}

#[test]
fn test_currency_collection_uninit_account() {
    let mut acc = Account::default();

    let mut currencies = CurrencyCollection::with_grams(1_400_200_000);
    currencies.other.set(&11111111u32, &VarUInteger32::from_two_u128(0, 123000).unwrap()).unwrap();

    let msg = create_msg_currency(
        AccountId::from([0x11; 32]),
        &currencies,
        false,
    );

    let tr_lt = BLOCK_LT + 1;

    let result_currencies = currencies.clone();

    let trans = execute_currencies(&msg, &mut acc, tr_lt, &result_currencies, 0).unwrap();
    assert_eq!(acc.status(), AccountStatus::AccStateUninit);
    let credit_phase = get_tr_descr(&trans).credit_ph.unwrap();
    assert_eq!(credit_phase.credit, currencies);
}

#[test]
fn test_currency_collection_message_with_zero_grams() {
    let mut acc = Account::default();

    let mut currencies = CurrencyCollection::default();
    currencies.other.set(&11111111u32, &VarUInteger32::from_two_u128(0, 123000).unwrap()).unwrap();

    let msg = create_msg_currency(
        AccountId::from([0x11; 32]),
        &currencies,
        false,
    );

    let tr_lt = BLOCK_LT + 1;

    let result_currencies = currencies.clone();

    let trans = execute_currencies(&msg, &mut acc, tr_lt, &result_currencies, 0).unwrap();
    assert_eq!(acc.status(), AccountStatus::AccStateUninit);
    let credit_phase = get_tr_descr(&trans).credit_ph.unwrap();
    assert_eq!(credit_phase.credit, currencies);
}

#[test]
fn test_currency_collection_activate_account() {
    let code = "
        NOP
    ";

    let code = compile_code_to_cell(code).unwrap();
    let mut state_init = StateInit::default();
    state_init.set_code(code);

    let mut acc = Account::default();

    let mut currencies = CurrencyCollection::with_grams(1_400_200_000);
    currencies.other.set(&11111111u32, &VarUInteger32::from_two_u128(0, 123000).unwrap()).unwrap();

    let mut msg = create_msg_currency(
        AccountId::from(state_init.hash().unwrap()),
        &currencies,
        false,
    );
    msg.set_state_init(state_init.clone());

    let tr_lt = BLOCK_LT + 1;

    let mut result_currencies = CurrencyCollection::with_grams(1_400_100_000);
    result_currencies.other.set(&11111111u32, &VarUInteger32::from_two_u128(0, 123000).unwrap()).unwrap();

    let trans = execute_currencies(&msg, &mut acc, tr_lt, &result_currencies, 0).unwrap();
    assert_eq!(acc.status(), AccountStatus::AccStateActive);
    let credit_phase = get_tr_descr(&trans).credit_ph.unwrap();
    assert_eq!(credit_phase.credit, currencies);
}

#[test]
fn test_balance_instruction_with_currency() {
    let begin_extra = 123000;
    let message_extra = 100000;

    let code = &*format!("
        PUSHINT  11111111
        BALANCE SECOND
        PUSHINT 32
        DICTUGET

        PUSHCONT {{
            LDVARUINT32
            DROP
            PUSHINT {}
            CMP
            THROWIFNOT 13
        }}
        PUSHCONT {{
            THROW 12
        }}
        IFELSE
    ", begin_extra + message_extra);

    let code = compile_code_to_cell(code).unwrap();
    let mut state_init = StateInit::default();
    state_init.set_code(code);

    let mut begin_currencies = CurrencyCollection::with_grams(1_400_200_000);
    begin_currencies.other.set(&11111111u32, &VarUInteger32::from_two_u128(0, begin_extra).unwrap()).unwrap();

    let mut acc = Account::active_by_init_code_hash(
        MsgAddressInt::with_standart(None, 0, AccountId::from(state_init.hash().unwrap())).unwrap(),
        begin_currencies.clone(),
        10,
        state_init.clone(),
        false
    ).unwrap();

    let mut currencies = CurrencyCollection::with_grams(1_000_000_000);
    currencies.other.set(&11111111u32, &VarUInteger32::from_two_u128(0, message_extra).unwrap()).unwrap();

    let mut msg = create_msg_currency(
        AccountId::from(state_init.hash().unwrap()),
        &currencies,
        false,
    );
    msg.set_state_init(state_init.clone());

    let tr_lt = BLOCK_LT + 1;

    let mut result_currencies = CurrencyCollection::with_grams(2354051816);
    result_currencies.other.set(&11111111u32, &VarUInteger32::from_two_u128(0, 223000).unwrap()).unwrap();

    let trans = execute_currencies(&msg, &mut acc, tr_lt, &result_currencies, 0).unwrap();
    assert_eq!(acc.status(), AccountStatus::AccStateActive);
    let credit_phase = get_tr_descr(&trans).credit_ph.unwrap();
    assert_eq!(credit_phase.credit, currencies);
    let vm_phase = get_tr_descr(&trans).compute_ph;
    if let TrComputePhase::Vm(vm) = vm_phase {
        assert_eq!(vm.exit_code, 13);
    } else {
        unreachable!();
    }
    assert_eq!(credit_phase.credit, currencies);
}

#[test]
fn test_add_currency_collection_and_activate_account() {
    let code = "
        NOP
    ";

    let code = compile_code_to_cell(code).unwrap();
    let mut state_init = StateInit::default();
    state_init.set_code(code);

    let mut begin_currencies = CurrencyCollection::with_grams(1_400_200_000);
    let begin_extra = 123000;
    begin_currencies.other.set(&11111111u32, &VarUInteger32::from_two_u128(0, begin_extra).unwrap()).unwrap();

    let mut acc = Account::uninit(MsgAddressInt::with_standart(None, 0, AccountId::from(state_init.hash().unwrap())).unwrap(), 10, 10, begin_currencies.clone());

    let mut currencies = CurrencyCollection::with_grams(1_000_000_000);
    let new_extra = 1230000;
    currencies.other.set(&11111111u32, &VarUInteger32::from_two_u128(0, 1230000).unwrap()).unwrap();

    let mut msg = create_msg_currency(
        AccountId::from(state_init.hash().unwrap()),
        &currencies,
        false,
    );
    msg.set_state_init(state_init.clone());

    let tr_lt = BLOCK_LT + 1;

    let mut result_currencies = CurrencyCollection::with_grams(2385594300);
    result_currencies.other.set(&11111111u32, &VarUInteger32::from_two_u128(0, begin_extra + new_extra).unwrap()).unwrap();

    let trans = execute_currencies(&msg, &mut acc, tr_lt, &result_currencies, 0).unwrap();
    assert_eq!(acc.status(), AccountStatus::AccStateActive);
    let credit_phase = get_tr_descr(&trans).credit_ph.unwrap();
    assert_eq!(credit_phase.credit, currencies);
}

fn add_currency_collection_to_active_account_with_bounce_flag(bounce: bool) {
    let code = "
        NOP
    ";

    let code = compile_code_to_cell(code).unwrap();
    let mut state_init = StateInit::default();
    state_init.set_code(code);

    let mut begin_currencies = CurrencyCollection::with_grams(1_400_200_000);
    let begin_extra = 123000;
    begin_currencies.other.set(&11111111u32, &VarUInteger32::from_two_u128(0, begin_extra).unwrap()).unwrap();

    let mut acc = Account::active_by_init_code_hash(
        MsgAddressInt::with_standart(None, 0, AccountId::from(state_init.hash().unwrap())).unwrap(),
        begin_currencies.clone(),
        10,
        state_init.clone(),
        false
    ).unwrap();

    let mut currencies = CurrencyCollection::with_grams(1_000_000_000);
    let new_extra = 1230000;
    currencies.other.set(&11111111u32, &VarUInteger32::from_two_u128(0, 1230000).unwrap()).unwrap();

    let mut msg = create_msg_currency(
        AccountId::from(state_init.hash().unwrap()),
        &currencies,
        bounce,
    );
    msg.set_state_init(state_init.clone());

    let tr_lt = BLOCK_LT + 1;

    let mut result_currencies = CurrencyCollection::with_grams(2359589888);
    result_currencies.other.set(&11111111u32, &VarUInteger32::from_two_u128(0, begin_extra + new_extra).unwrap()).unwrap();

    let trans = execute_currencies(&msg, &mut acc, tr_lt, &result_currencies, 0).unwrap();
    assert_eq!(acc.status(), AccountStatus::AccStateActive);
    let credit_phase = get_tr_descr(&trans).credit_ph.unwrap();
    assert_eq!(credit_phase.credit, currencies);
}

#[test]
fn test_add_currency_collection_to_active_account() {
    add_currency_collection_to_active_account_with_bounce_flag(false);
    add_currency_collection_to_active_account_with_bounce_flag(true);
}

#[test]
fn test_delete_uninit_account_with_currency_collections() {
    let mut begin_currencies = CurrencyCollection::with_grams(17);
    let begin_extra = 123000;
    begin_currencies.other.set(&11111111u32, &VarUInteger32::from_two_u128(0, begin_extra).unwrap()).unwrap();

    let mut acc = Account::uninit(
        MsgAddressInt::with_standart(None, -1, AccountId::from([0x11; 32])).unwrap(),
        BLOCK_LT + 3, BLOCK_UT - 300000000,
        begin_currencies.clone()
    );

    let acc_addr = acc.get_addr().unwrap().address();
    let mut currencies = CurrencyCollection::with_grams(15);
    let new_extra = 1230000;
    currencies.other.set(&11111111u32, &VarUInteger32::from_two_u128(0, new_extra).unwrap()).unwrap();
    let msg = create_msg_currency_workchain(
        -1,
        acc_addr,
        &currencies,
        false,
    );

    let acc_balance = acc.balance().unwrap().grams.as_u128();
    let tr_lt = BLOCK_LT + 2;
    let trans = execute_currencies(&msg, &mut acc, tr_lt, &CurrencyCollection::default(), 0).unwrap();

    let new_acc = Account::default();
    assert_eq!(acc, new_acc);

    let mut description = TransactionDescrOrdinary::default();
    description.storage_ph = Some(TrStoragePhase::with_params(
        currencies.grams + acc_balance,
        Some(Grams::from(2650451629u64)),
        AccStatusChange::Deleted
    ));
    description.credit_ph = Some(TrCreditPhase::with_params(None, currencies.clone()));
    description.compute_ph = TrComputePhase::skipped(ComputeSkipReason::NoGas);

    description.action = None;
    description.credit_first = true;
    description.bounce = None;
    description.aborted = true;
    description.destroyed = false;
    let description = TransactionDescr::Ordinary(description);

    let mut good_trans = Transaction::with_account_and_message(&new_acc, &msg, BLOCK_LT + 3).unwrap();

    let mut sum_fees = currencies.clone();
    sum_fees.add(&begin_currencies).unwrap();
    good_trans.set_total_fees(sum_fees);
    good_trans.orig_status = AccountStatus::AccStateUninit;
    good_trans.set_end_status(AccountStatus::AccStateNonexist);
    good_trans.set_logical_time(BLOCK_LT + 3);
    good_trans.set_now(BLOCK_UT);

    good_trans.write_description(&description).unwrap();

    assert_eq!(trans.read_description().unwrap(), description);
    assert_eq!(trans, good_trans);
}

#[test]
fn test_delete_uninit_account_with_currency_collections_with_bounce() {
    let mut begin_currencies = CurrencyCollection::with_grams(17);
    let begin_extra = 123000;
    begin_currencies.other.set(&11111111u32, &VarUInteger32::from_two_u128(0, begin_extra).unwrap()).unwrap();

    let mut acc = Account::uninit(
        MsgAddressInt::with_standart(None, -1, AccountId::from([0x11; 32])).unwrap(),
        BLOCK_LT + 3, BLOCK_UT - 300000000,
        begin_currencies.clone()
    );

    let acc_addr = acc.get_addr().unwrap().address();
    let mut currencies = CurrencyCollection::with_grams(15);
    let new_extra = 1230000;
    currencies.other.set(&11111111u32, &VarUInteger32::from_two_u128(0, new_extra).unwrap()).unwrap();
    let msg = create_msg_currency_workchain(
        -1,
        acc_addr,
        &currencies,
        true,
    );

    let acc_balance = acc.balance().unwrap().grams;
    let tr_lt = BLOCK_LT + 2;
    let trans = execute_currencies(&msg, &mut acc, tr_lt, &currencies, 0).unwrap();

    let mut description = TransactionDescrOrdinary::default();
    description.storage_ph = Some(TrStoragePhase::with_params(
        acc_balance,
        Some(Grams::from(2650451644u64)),
        AccStatusChange::Deleted
    ));
    description.credit_ph = Some(TrCreditPhase::with_params(None, currencies.clone()));
    description.compute_ph = TrComputePhase::skipped(ComputeSkipReason::NoGas);

    description.action = None;
    description.credit_first = false;
    description.bounce = Some(
        TrBouncePhase::Nofunds(TrBouncePhaseNofunds {
            msg_size: StorageUsedShort::with_values_checked(1, 69).unwrap(),
            req_fwd_fees: Grams::from(11690000u64)
        }),
    );
    description.aborted = true;
    description.destroyed = false;
    let description = TransactionDescr::Ordinary(description);

    let mut good_trans = Transaction::with_account_and_message(&acc, &msg, BLOCK_LT + 3).unwrap();

    let sum_fees = begin_currencies.clone();
    good_trans.set_total_fees(sum_fees);
    good_trans.orig_status = AccountStatus::AccStateUninit;
    good_trans.set_end_status(AccountStatus::AccStateUninit);
    good_trans.set_logical_time(BLOCK_LT + 3);
    good_trans.set_now(BLOCK_UT);

    good_trans.write_description(&description).unwrap();

    assert_eq!(trans.read_description().unwrap(), description);
    assert_eq!(trans, good_trans);
}

#[test]
fn test_freeze_uninit_account_with_currency_collections() {
    let code = "
        NOP
    ";

    let code = compile_code_to_cell(code).unwrap();
    let mut state_init = StateInit::default();
    state_init.set_code(code);

    let mut begin_currencies = CurrencyCollection::with_grams(17);
    let begin_extra = 123000;
    begin_currencies.other.set(&11111111u32, &VarUInteger32::from_two_u128(0, begin_extra).unwrap()).unwrap();

    let mut acc = Account::active_by_init_code_hash(
        MsgAddressInt::with_standart(None, -1, AccountId::from([0x11; 32])).unwrap(),
        begin_currencies.clone(),
        BLOCK_UT - 30000000,
        state_init.clone(),
        false
    ).unwrap();

    let acc_addr = acc.get_addr().unwrap().address();
    let mut currencies = CurrencyCollection::with_grams(15);
    let new_extra = 1230000;
    currencies.other.set(&11111111u32, &VarUInteger32::from_two_u128(0, new_extra).unwrap()).unwrap();
    let msg = create_msg_currency_workchain(
        -1,
        acc_addr,
        &currencies,
        false,
    );

    let acc_balance = acc.balance().unwrap().grams;
    let tr_lt = BLOCK_LT + 2;
    let mut new_currency = begin_currencies.clone();
    new_currency.add(&currencies).unwrap();
    new_currency.grams = Grams::default();
    let trans = execute_currencies(&msg, &mut acc, tr_lt, &new_currency, 0).unwrap();
    assert_eq!(acc.status(), AccountStatus::AccStateFrozen);

    let mut description = TransactionDescrOrdinary::default();
    description.storage_ph = Some(TrStoragePhase::with_params(
        currencies.grams + acc_balance,
        Some(Grams::from(759887664u64)),
        AccStatusChange::Frozen
    ));
    description.credit_ph = Some(TrCreditPhase::with_params(None, currencies.clone()));
    description.compute_ph = TrComputePhase::skipped(ComputeSkipReason::NoGas);

    description.action = None;
    description.credit_first = true;
    description.bounce = None;
    description.aborted = true;
    description.destroyed = false;
    let description = TransactionDescr::Ordinary(description);

    let mut good_trans = Transaction::with_account_and_message(&acc, &msg, BLOCK_LT + 3).unwrap();

    let sum_fees = CurrencyCollection::from_grams(begin_currencies.grams + currencies.grams);
    good_trans.set_total_fees(sum_fees);
    good_trans.orig_status = AccountStatus::AccStateActive;
    good_trans.set_end_status(AccountStatus::AccStateFrozen);
    good_trans.set_logical_time(BLOCK_LT + 2);
    good_trans.set_now(BLOCK_UT);

    good_trans.write_description(&description).unwrap();

    assert_eq!(trans.read_description().unwrap(), description);
    assert_eq!(trans, good_trans);
}

#[test]
fn test_bounce_with_currency_collection() {
    let mut begin_currencies = CurrencyCollection::with_grams(1_400_200_000);
    let begin_extra = 123000;
    begin_currencies.other.set(&11111111u32, &VarUInteger32::from_two_u128(0, begin_extra).unwrap()).unwrap();

    let mut acc = Account::uninit(MsgAddressInt::with_standart(None, 0, AccountId::from([0x11; 32])).unwrap(), 10, 10, begin_currencies.clone());

    let mut currencies = CurrencyCollection::with_grams(1_000_000_000);
    currencies.other.set(&11111111u32, &VarUInteger32::from_two_u128(0, 1230000).unwrap()).unwrap();

    let msg = create_msg_currency(
        AccountId::from([0x11; 32]),
        &currencies,
        true,
    );

    let tr_lt = BLOCK_LT + 1;

    let mut result_currencies = CurrencyCollection::with_grams(1385694300);
    result_currencies.other.set(&11111111u32, &VarUInteger32::from_two_u128(0, begin_extra).unwrap()).unwrap();

    let trans = execute_currencies(&msg, &mut acc, tr_lt, &result_currencies, 1).unwrap();
    assert_eq!(acc.status(), AccountStatus::AccStateUninit);

    let mut description = TransactionDescrOrdinary::default();
    description.storage_ph = Some(TrStoragePhase::with_params(
        Grams::from(14505700),
        None,
        AccStatusChange::Unchanged
    ));
    description.credit_ph = Some(TrCreditPhase::with_params(None, currencies.clone()));
    description.compute_ph = TrComputePhase::skipped(ComputeSkipReason::NoState);

    description.action = None;
    description.credit_first = false;

    let msg_fee = 389660u64;
    let fwd_fees = 779340u64;
    description.bounce = Some(
        TrBouncePhase::Ok(TrBouncePhaseOk {
            msg_size: StorageUsedShort::with_values_checked(1, 69).unwrap(),
            msg_fees: Grams::from(msg_fee),
            fwd_fees: Grams::from(fwd_fees),
        }),
    );

    description.aborted = true;
    description.destroyed = false;
    let description = TransactionDescr::Ordinary(description);

    let mut good_trans = Transaction::with_account_and_message(&acc, &msg, BLOCK_LT + 3).unwrap();

    let mut outmsg_currency = currencies.clone();
    outmsg_currency.grams -= Grams::from(msg_fee + fwd_fees);
    let mut message = Message::with_int_header(InternalMessageHeader{
        ihr_disabled: true,
        bounce: false,
        bounced: true,
        src: MsgAddressIntOrNone::Some(MsgAddressInt::with_standart(None, 0, AccountId::from([0x11; 32])).unwrap()),
        dst: MsgAddressInt::with_standart(None, 0, AccountId::from([0x33; 32])).unwrap(),
        value: outmsg_currency,
        ihr_fee: Default::default(),
        fwd_fee: fwd_fees.into(),
        created_lt: 2000000002,
        created_at: 1576526553.into()
    });
    message.set_body(SliceData::from_raw(vec![0xff; 4], 32));
    good_trans.add_out_message(&make_common(message)).unwrap();

    good_trans.set_total_fees(CurrencyCollection::with_grams(14895360));
    good_trans.orig_status = AccountStatus::AccStateUninit;
    good_trans.set_end_status(AccountStatus::AccStateUninit);

    good_trans.set_logical_time(BLOCK_LT + 1);
    good_trans.set_now(BLOCK_UT);

    good_trans.write_description(&description).unwrap();

    assert_eq!(trans.read_description().unwrap(), description);
    trans.out_msgs.scan_diff(&good_trans.out_msgs, |key: ever_block::U15, msg1, msg2| {
        assert_eq!(msg1, msg2, "for key {}", key.0);
        Ok(true)
    }).unwrap();
    assert_eq!(trans, good_trans);
}

#[test]
fn test_currencies_with_sendmsg() {
    let code = "
        ACCEPT
        PUSHCTR C4
        PUSHINT 1
        SENDRAWMSG
    ";

    let mut out_msg_currencies = CurrencyCollection::with_grams(1100000000);
    out_msg_currencies.other.set(&11111111u32, &VarUInteger32::from_two_u128(0, 120).unwrap()).unwrap();

    let mut out_msg = create_msg_currency(
        AccountId::from([0x33; 32]),
        &out_msg_currencies,
        false
    );
    out_msg.set_src(MsgAddressIntOrNone::Some(MsgAddressInt::with_standart(None, 0, AccountId::from([0x11; 32])).unwrap()));
    let data = out_msg.serialize().unwrap();

    let acc_id = AccountId::from([0x11; 32]);
    let code = compile_code_to_cell(code).unwrap();

    let mut state_init = StateInit::default();
    state_init.set_code(code);
    state_init.set_data(data);

    let mut begin_currencies = CurrencyCollection::with_grams(2000000000);
    begin_currencies.other.set(&11111111u32, &VarUInteger32::from_two_u128(0, 100).unwrap()).unwrap();

    let mut acc = Account::active_by_init_code_hash(
        MsgAddressInt::with_standart(None, 0, acc_id).unwrap(),
        begin_currencies.clone(),
        10,
        state_init.clone(),
        false
    ).unwrap();

    let mut in_msg_currencies = CurrencyCollection::with_grams(1000000000);
    in_msg_currencies.other.set(&11111111u32, &VarUInteger32::from_two_u128(0, 100).unwrap()).unwrap();

    let msg = create_msg_currency(
        AccountId::from([0x11; 32]),
        &in_msg_currencies,
        false
    );

    let mut currencies_result = CurrencyCollection::with_grams(1815253193);
    currencies_result.other.set(&11111111u32, &VarUInteger32::from_two_u128(0, 80).unwrap()).unwrap();

    let trans = execute_currencies(&msg, &mut acc, BLOCK_LT + 1, &currencies_result, 1).unwrap();

    let mut description = TransactionDescrOrdinary::default();
    let storage_phase_fees = 82992807;
    description.storage_ph = Some(TrStoragePhase::with_params(
        Grams::from(storage_phase_fees),
        None,
        AccStatusChange::Unchanged
    ));
    description.credit_ph = Some(TrCreditPhase::with_params(
        None,
        in_msg_currencies.clone()
    ));

    let mut vm_phase = TrComputePhaseVm::default();
    vm_phase.success = true;
    vm_phase.gas_used = 601.into();
    vm_phase.gas_limit = 1000000.into();
    vm_phase.gas_fees = 601000.into();
    vm_phase.vm_steps = 5;
    description.compute_ph = TrComputePhase::Vm(vm_phase);

    let mut actions = OutActions::default();
    actions.push_back(OutAction::new_send(SENDMSG_PAY_FEE_SEPARATELY, out_msg.clone()));
    if let Some(int_header) = out_msg.int_header_mut() {
        int_header.fwd_fee = 768673.into();
        int_header.created_lt = 2000000002;
        int_header.created_at = 1576526553.into();
    }
    let mut action_ph = TrActionPhase::default();
    action_ph.success = true;
    action_ph.valid = true;
    action_ph.status_change = AccStatusChange::Unchanged;
    action_ph.tot_actions = 1;
    action_ph.spec_actions = 0;
    action_ph.msgs_created = 1;
    action_ph.total_fwd_fees = Some(1153000.into());
    action_ph.total_action_fees = Some(384327.into());
    action_ph.action_list_hash = actions.hash().unwrap();
    action_ph.tot_msg_size = StorageUsedShort::calculate_for_struct(&out_msg).unwrap();
    description.action = Some(action_ph);

    description.credit_first = true;
    description.bounce = None;
    description.aborted = false;
    description.destroyed = false;
    let description = TransactionDescr::Ordinary(description);

    let mut good_trans = Transaction::with_account_and_message(&acc, &msg, BLOCK_LT + 2).unwrap();

    good_trans.add_out_message(&make_common(out_msg)).unwrap();
    good_trans.set_total_fees(CurrencyCollection::with_grams(83978134));
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
}

fn execute_sendrawmsg_message(mode: u8, send_value_grams: u64, send_value_tokens: u64, result_balance_grams: u64, result_balance_tokens: u128, count_out_msgs: usize) -> Account {
    let code = format!("
        ACCEPT
        PUSHCTR C4
        PUSHINT {}
        SENDRAWMSG
    ", mode);

    let mut out_msg_currencies = CurrencyCollection::with_grams(1100000000);
    out_msg_currencies.other.set(&11111111u32, &VarUInteger32::from_two_u128(0, 120).unwrap()).unwrap();

    let mut out_msg = create_msg_currency(
        AccountId::from([0x33; 32]),
        &out_msg_currencies,
        false
    );
    out_msg.set_src(MsgAddressIntOrNone::None);
    let data = out_msg.serialize().unwrap();

    let acc_id = AccountId::from([0x11; 32]);
    let code = compile_code_to_cell(&code).unwrap();

    let mut state_init = StateInit::default();
    state_init.set_code(code);
    state_init.set_data(data);

    let mut begin_currencies = CurrencyCollection::with_grams(2000000000);
    begin_currencies.other.set(&11111111u32, &VarUInteger32::from_two_u128(0, 100).unwrap()).unwrap();

    let mut acc = Account::active_by_init_code_hash(
        MsgAddressInt::with_standart(None, 0, acc_id).unwrap(),
        begin_currencies.clone(),
        10,
        state_init.clone(),
        false
    ).unwrap();

    let mut in_msg_currencies = CurrencyCollection::from_grams(send_value_grams.into());
    if send_value_tokens != 0 {
        in_msg_currencies.other.set(&11111111u32, &VarUInteger32::from_two_u128(0, send_value_tokens as u128).unwrap()).unwrap();
    }

    let msg = create_msg_currency(
        AccountId::from([0x11; 32]),
        &in_msg_currencies,
        false
    );

    let mut currencies_result = CurrencyCollection::from_grams(result_balance_grams.into());
    currencies_result.other.set(&11111111u32, &VarUInteger32::from_two_u128(0, result_balance_tokens).unwrap()).unwrap();

    execute_currencies(&msg, &mut acc, BLOCK_LT + 1, &currencies_result, count_out_msgs).unwrap();
    acc
}

#[test]
fn test_sendrawmsg_currency_messages() {
    execute_sendrawmsg_message(0, 1_400_000_000, 140, 2_222_781_003, 120, 1);
    execute_sendrawmsg_message(1, 1_400_000_000, 140, 2_221_628_003, 120, 1);
    execute_sendrawmsg_message(0, 0, 140, 1_923_382_003, 240, 0);
    execute_sendrawmsg_message(0, 1_400_000_000, 0, 3_322_781_003, 100, 0);

    execute_sendrawmsg_message(128, 1_400_000_000, 140, 0, 0, 1);
    let acc = execute_sendrawmsg_message(128+32, 1_400_000_000, 140, 0, 0, 1);
    assert_eq!(acc.status(), AccountStatus::AccStateNonexist);

    execute_sendrawmsg_message(64, 1_400_000_000, 100, 3322580556, 200, 0);
}

#[test]
fn test_currencies_with_sendmsg_64_flag() {
    let code = "
        ACCEPT
        PUSHCTR C4
        PUSHINT 65
        SENDRAWMSG
    ";

    let mut out_msg_currencies = CurrencyCollection::with_grams(1100000000);
    out_msg_currencies.other.set(&11111111u32, &VarUInteger32::from_two_u128(0, 120).unwrap()).unwrap();

    let mut out_msg = create_msg_currency(
        AccountId::from([0x33; 32]),
        &out_msg_currencies,
        false
    );
    out_msg.set_src(MsgAddressIntOrNone::Some(MsgAddressInt::with_standart(None, 0, AccountId::from([0x11; 32])).unwrap()));
    let data = out_msg.serialize().unwrap();

    let acc_id = AccountId::from([0x11; 32]);
    let code = compile_code_to_cell(code).unwrap();

    let mut state_init = StateInit::default();
    state_init.set_code(code);
    state_init.set_data(data);

    let mut begin_currencies = CurrencyCollection::with_grams(2000000000);
    begin_currencies.other.set(&11111111u32, &VarUInteger32::from_two_u128(0, 150).unwrap()).unwrap();

    let mut acc = Account::active_by_init_code_hash(
        MsgAddressInt::with_standart(None, 0, acc_id).unwrap(),
        begin_currencies.clone(),
        10,
        state_init.clone(),
        false
    ).unwrap();

    let mut in_msg_currencies = CurrencyCollection::with_grams(1000000000);
    in_msg_currencies.other.set(&11111111u32, &VarUInteger32::from_two_u128(0, 100).unwrap()).unwrap();

    let msg = create_msg_currency(
        AccountId::from([0x11; 32]),
        &in_msg_currencies,
        false
    );

    let mut currencies_result = CurrencyCollection::with_grams(815052746);
    currencies_result.other.set(&11111111u32, &VarUInteger32::from_two_u128(0, 30).unwrap()).unwrap();

    let trans = execute_currencies(&msg, &mut acc, BLOCK_LT + 1, &currencies_result, 1).unwrap();

    let mut description = TransactionDescrOrdinary::default();
    let storage_phase_fees = 83185254;
    description.storage_ph = Some(TrStoragePhase::with_params(
        Grams::from(storage_phase_fees),
        None,
        AccStatusChange::Unchanged
    ));
    description.credit_ph = Some(TrCreditPhase::with_params(
        None,
        in_msg_currencies.clone()
    ));

    let mut vm_phase = TrComputePhaseVm::default();
    vm_phase.success = true;
    vm_phase.gas_used = 609.into();
    vm_phase.gas_limit = 1000000.into();
    vm_phase.gas_fees = 609000.into();
    vm_phase.vm_steps = 5;
    description.compute_ph = TrComputePhase::Vm(vm_phase);

    let mut actions = OutActions::default();
    actions.push_back(OutAction::new_send(SENDMSG_PAY_FEE_SEPARATELY + SENDMSG_REMAINING_MSG_BALANCE, out_msg.clone()));
    if let Some(int_header) = out_msg.int_header_mut() {
        int_header.value.add(&in_msg_currencies).unwrap();
        int_header.fwd_fee = 768673.into();
        int_header.created_lt = 2000000002;
        int_header.created_at = 1576526553.into();
    }
    let mut action_ph = TrActionPhase::default();
    action_ph.success = true;
    action_ph.valid = true;
    action_ph.status_change = AccStatusChange::Unchanged;
    action_ph.tot_actions = 1;
    action_ph.spec_actions = 0;
    action_ph.msgs_created = 1;
    action_ph.total_fwd_fees = Some(1153000.into());
    action_ph.total_action_fees = Some(384327.into());
    action_ph.action_list_hash = actions.hash().unwrap();
    action_ph.tot_msg_size = StorageUsedShort::calculate_for_struct(&out_msg).unwrap();
    description.action = Some(action_ph);

    description.credit_first = true;
    description.bounce = None;
    description.aborted = false;
    description.destroyed = false;
    let description = TransactionDescr::Ordinary(description);

    let mut good_trans = Transaction::with_account_and_message(&acc, &msg, BLOCK_LT + 2).unwrap();

    good_trans.add_out_message(&make_common(out_msg)).unwrap();
    good_trans.set_total_fees(CurrencyCollection::with_grams(84178581));
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
}

struct MsgFlag {
    f: u64
}

#[allow(clippy::too_many_arguments)]
fn execute_rawreserve_with_extra_balances_transaction(
    start_balance: u64,
    start_extra_balance: u64,
    reserve: u64,
    extra_reserve: u64,
    r_type: u64,
    msg_flag: MsgFlag,
    result_account_balance: u64,
    result_account_extra_balance: u64,
    count_out_msgs: usize
) {
    let code = format!("
        PUSHINT {}

        NEWC
        PUSHINT {}
        STVARUINT32
        PUSHINT 11111111
        NEWDICT
        PUSHINT 32
        DICTUSETB

        PUSHINT {}
        RAWRESERVEX

        PUSHROOT
        CTOS
        LDREF
        PLDREF
        PUSHINT 0
        SENDRAWMSG
        PUSHINT {}
        SENDRAWMSG
    ", reserve, extra_reserve, r_type, msg_flag.f);
    let data = create_two_messages_data();
    execute_custom_transaction_with_extra_balance(start_balance, start_extra_balance, &code, data, 41_000_000, result_account_balance, result_account_extra_balance, count_out_msgs);
}

#[test]
fn test_send_rawreserve_with_extra_messages() {
    // enought extra balance to reserve
    execute_rawreserve_with_extra_balances_transaction(1_602_586_890, 1_542_586_890, 1_080_012_743, 1_080_012_743, 0, MsgFlag{f: 0}, 1_166_306_139, 1_542_586_890, 2);
    // not enought extra balance to reserve
    execute_rawreserve_with_extra_balances_transaction(1_602_586_890, 1_542_586_890, 1_080_012_743, 1_642_586_890, 0, MsgFlag{f: 0}, 1_316_306_139, 1_542_586_890, 0);
}
