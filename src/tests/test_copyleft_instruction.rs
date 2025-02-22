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

use pretty_assertions::assert_eq;
use ever_block::{
    out_actions::{OutAction, OutActions, SENDMSG_ORDINARY},
    transactions::{
        AccStatusChange, TrActionPhase, TrComputePhase, TrComputePhaseVm, TrCreditPhase,
        TrStoragePhase, Transaction, TransactionDescr,
    },
    ConfigCopyleft, ConfigParam8, ConfigParamEnum, CurrencyCollection, GetRepresentationHash,
    Grams, Serializable, StateInit, StorageUsedShort, TransactionDescrOrdinary,
};
use ever_assembler::compile_code_to_cell;
use ever_block::AccountId;

use super::*;

mod common;
use crate::transaction_executor::tests_copyleft::common::cross_check::disable_cross_check;
use common::*;

pub const COPYLEFT_REWARD_THRESHOLD: u64 = 1_000_000_000;
lazy_static::lazy_static! {
    pub static ref ACCOUNT_ADDRESS: MsgAddressInt = MsgAddressInt::with_standart(None, 0, AccountId::from([0x22; 32])).unwrap();
    pub static ref COPYLEFT_REWARD_ADDRESS: AccountId = AccountId::from([0x11; 32]);
}

fn make_copyleft_config() -> BlockchainConfig {
    let mut config = BLOCKCHAIN_CONFIG.raw_config().clone();

    let mut param8 = ConfigParam8 {
        global_version: config.get_global_version().unwrap()
    };
    param8.global_version.capabilities |= GlobalCapabilities::CapCopyleft as u64;
    config.set_config(ConfigParamEnum::ConfigParam8(param8)).unwrap();

    let mut copyleft_config = ConfigCopyleft::default();
    copyleft_config.copyleft_reward_threshold = COPYLEFT_REWARD_THRESHOLD.into();
    copyleft_config.license_rates.set(&3u8, &10).unwrap();
    copyleft_config.license_rates.set(&7u8, &50).unwrap();
    copyleft_config.license_rates.set(&9u8, &100).unwrap();
    config.set_config(ConfigParamEnum::ConfigParam42(copyleft_config)).unwrap();

    BlockchainConfig::with_config(config.clone()).unwrap()
}

pub fn execute_copyleft(msg: &Message, acc: &mut Account, result_account_balance: impl Into<Grams>, count_out_msgs: usize) -> Result<Transaction> {
    let acc_before = acc.clone();
    disable_cross_check();
    let trans = execute_with_params(make_copyleft_config(), msg, acc, BLOCK_LT + 1);
    check_account_and_transaction(&acc_before, acc, msg, trans.as_ref().ok(), result_account_balance, count_out_msgs);
    acc.update_storage_stat().unwrap();
    trans
}

fn create_msg(value: u64) -> Message {
    create_int_msg_workchain(
        0,
        AccountId::from([0x33; 32]),
        ACCOUNT_ADDRESS.address(),
        value,
        false,
        BLOCK_LT - 2
    )
}

#[test]
fn test_copyleft_instruction() {
    let addr = COPYLEFT_REWARD_ADDRESS.clone();

    let code = "
        PUSHROOT
        CTOS
        PUSHINT 3
        COPYLEFT
    ";
    let start_balance = 100_000_000_000u64;
    let compute_phase_gas_fees = 693000u64;
    let copyleft_reward = compute_phase_gas_fees / 10;
    let msg_income = 1_400_200_000;

    let acc_id = ACCOUNT_ADDRESS.address();
    let code = compile_code_to_cell(code).unwrap();
    let data = addr.serialize().unwrap();

    let mut acc = create_test_account(start_balance, acc_id, code, data);

    let msg = create_msg(msg_income);

    let mut new_acc = Account::active_by_init_code_hash(
        acc.get_addr().unwrap().clone(),
        CurrencyCollection::with_grams(101399379404),
        BLOCK_UT,
        acc.state_init().unwrap().clone(),
        false
    ).unwrap();
    new_acc.update_storage_stat().unwrap();
    new_acc.set_last_tr_time(BLOCK_LT + 2);

    let trans = execute_copyleft(&msg, &mut acc, new_acc.balance().unwrap().grams, 0).unwrap();
    let copyleft = trans.copyleft_reward().clone().unwrap();
    assert_eq!(copyleft.reward.as_u128(), copyleft_reward as u128);
    assert_eq!(&copyleft.address, &*COPYLEFT_REWARD_ADDRESS);

    let mut description = TransactionDescrOrdinary::default();
    let storage_phase_fees = 127596;
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
    vm_phase.vm_steps = 5;
    description.compute_ph = TrComputePhase::Vm(vm_phase);

    let mut actions = OutActions::default();
    actions.push_back(OutAction::CopyLeft{ license: 3, address: addr});
    let mut action_ph = TrActionPhase::default();
    action_ph.success = true;
    action_ph.valid = true;
    action_ph.status_change = AccStatusChange::Unchanged;
    action_ph.tot_actions = 1;
    action_ph.spec_actions = 1;
    action_ph.msgs_created = 0;
    action_ph.total_fwd_fees = None;
    action_ph.total_action_fees = None;
    action_ph.action_list_hash = actions.hash().unwrap();
    description.action = Some(action_ph);

    description.credit_first = true;
    description.bounce = None;
    description.aborted = false;
    description.destroyed = false;
    let description = TransactionDescr::Ordinary(description);

    let mut good_trans = Transaction::with_account_and_message(&new_acc, &msg, BLOCK_LT + 2).unwrap();

    good_trans.set_total_fees(CurrencyCollection::with_grams(gas_fees + storage_phase_fees - copyleft_reward));
    good_trans.orig_status = AccountStatus::AccStateActive;
    good_trans.set_end_status(AccountStatus::AccStateActive);
    good_trans.set_logical_time(BLOCK_LT + 1);
    good_trans.set_now(BLOCK_UT);

    good_trans.write_description(&description).unwrap();

    assert_eq!(acc, new_acc);
    assert_eq!(trans.read_description().unwrap(), description);
    assert_eq!(trans, good_trans);
}

#[test]
fn test_copyleft_in_masterchain() {
    let addr = COPYLEFT_REWARD_ADDRESS.clone();

    let code = "
        PUSHROOT
        CTOS
        PUSHINT 3
        COPYLEFT
    ";
    let start_balance = 100_000_000_000u64;
    let compute_phase_gas_fees = 1930000u64;
    let msg_income = 1_400_200_000;

    let acc_id = ACCOUNT_ADDRESS.address();
    let code = compile_code_to_cell(code).unwrap();
    let data = addr.serialize().unwrap();

    let mut acc = create_test_account(start_balance, acc_id, code, data);

    let msg = create_int_msg_workchain(
        -1,
        AccountId::from([0x33; 32]),
        ACCOUNT_ADDRESS.address(),
        msg_income,
        false,
        BLOCK_LT - 2
    );

    let mut new_acc = Account::active_by_init_code_hash(
        acc.get_addr().unwrap().clone(),
        CurrencyCollection::with_grams(101270674127),
        BLOCK_UT,
        acc.state_init().unwrap().clone(),
        false
    ).unwrap();
    new_acc.update_storage_stat().unwrap();
    new_acc.set_last_tr_time(BLOCK_LT + 2);

    let trans = execute_copyleft(&msg, &mut acc, new_acc.balance().unwrap().grams, 0).unwrap();
    let copyleft = trans.copyleft_reward();
    assert!(copyleft.is_none());

    let mut description = TransactionDescrOrdinary::default();
    let storage_phase_fees = 127595873;
    description.storage_ph = Some(TrStoragePhase::with_params(
        Grams::from(storage_phase_fees),
        None,
        AccStatusChange::Unchanged
    ));
    description.credit_ph = Some(TrCreditPhase::with_params(
        None,
        CurrencyCollection::with_grams(msg_income)
    ));

    let gas_used = (compute_phase_gas_fees / 10000) as u32;
    let gas_fees = compute_phase_gas_fees;
    let mut vm_phase = TrComputePhaseVm::default();
    vm_phase.success = true;
    vm_phase.gas_used = gas_used.into();
    vm_phase.gas_limit = 140020u32.into();
    vm_phase.gas_fees = gas_fees.into();
    vm_phase.exit_code = 0;
    vm_phase.vm_steps = 5;
    description.compute_ph = TrComputePhase::Vm(vm_phase);

    let actions = OutActions::default();
    let mut action_ph = TrActionPhase::default();
    action_ph.success = true;
    action_ph.valid = true;
    action_ph.status_change = AccStatusChange::Unchanged;
    action_ph.tot_actions = 0;
    action_ph.spec_actions = 0;
    action_ph.msgs_created = 0;
    action_ph.total_fwd_fees = None;
    action_ph.total_action_fees = None;
    action_ph.action_list_hash = actions.hash().unwrap();
    description.action = Some(action_ph);

    description.credit_first = true;
    description.bounce = None;
    description.aborted = false;
    description.destroyed = false;
    let description = TransactionDescr::Ordinary(description);

    let mut good_trans = Transaction::with_account_and_message(&new_acc, &msg, BLOCK_LT + 2).unwrap();

    good_trans.set_total_fees(CurrencyCollection::with_grams(gas_fees + storage_phase_fees));
    good_trans.orig_status = AccountStatus::AccStateActive;
    good_trans.set_end_status(AccountStatus::AccStateActive);
    good_trans.set_logical_time(BLOCK_LT + 1);
    good_trans.set_now(BLOCK_UT);

    good_trans.write_description(&description).unwrap();

    assert_eq!(acc, new_acc);
    assert_eq!(trans.read_description().unwrap(), description);
    assert_eq!(trans, good_trans);
}

#[test]
fn test_copyleft_uninit_account() {
    let addr = COPYLEFT_REWARD_ADDRESS.clone();

    let code = "
        PUSHROOT
        CTOS
        PUSHINT 3
        COPYLEFT
    ";
    let compute_phase_gas_fees = 693000u64;
    let copyleft_reward = compute_phase_gas_fees / 10;
    let msg_income = 1_400_200_000;

    let code = compile_code_to_cell(code).unwrap();
    let data = addr.serialize().unwrap();
    let mut state_init = StateInit::default();
    state_init.set_code(code);
    state_init.set_data(data);

    let mut acc = Account::default();

    let mut msg = create_int_msg_workchain(
        0,
        AccountId::from([0x33; 32]),
        AccountId::from(state_init.hash().unwrap()),
        msg_income,
        false,
        BLOCK_LT - 2
    );
    msg.set_state_init(state_init.clone());

    let mut new_acc = Account::active_by_init_code_hash(
        MsgAddressInt::with_standart(None, 0, AccountId::from(state_init.hash().unwrap())).unwrap(),
        CurrencyCollection::with_grams(1399507000),
        BLOCK_UT,
        state_init.clone(),
        false
    ).unwrap();
    new_acc.update_storage_stat().unwrap();
    new_acc.set_last_tr_time(BLOCK_LT + 2);

    let trans = execute_copyleft(&msg, &mut acc, new_acc.balance().unwrap().grams, 0).unwrap();
    let copyleft = trans.copyleft_reward().clone().unwrap();
    assert_eq!(copyleft.reward.as_u128(), copyleft_reward as u128);
    assert_eq!(&copyleft.address, &*COPYLEFT_REWARD_ADDRESS);

    let mut description = TransactionDescrOrdinary::default();
    let storage_phase_fees = 0;
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
    vm_phase.vm_steps = 5;
    description.compute_ph = TrComputePhase::Vm(vm_phase);

    let mut actions = OutActions::default();
    actions.push_back(OutAction::CopyLeft{ license: 3, address: addr});
    let mut action_ph = TrActionPhase::default();
    action_ph.success = true;
    action_ph.valid = true;
    action_ph.status_change = AccStatusChange::Unchanged;
    action_ph.tot_actions = 1;
    action_ph.spec_actions = 1;
    action_ph.msgs_created = 0;
    action_ph.total_fwd_fees = None;
    action_ph.total_action_fees = None;
    action_ph.action_list_hash = actions.hash().unwrap();
    description.action = Some(action_ph);

    description.credit_first = true;
    description.bounce = None;
    description.aborted = false;
    description.destroyed = false;
    let description = TransactionDescr::Ordinary(description);

    let mut good_trans = Transaction::with_account_and_message(&new_acc, &msg, BLOCK_LT + 2).unwrap();

    good_trans.set_total_fees(CurrencyCollection::with_grams(gas_fees + storage_phase_fees - copyleft_reward));
    good_trans.orig_status = AccountStatus::AccStateNonexist;
    good_trans.set_end_status(AccountStatus::AccStateActive);
    good_trans.set_logical_time(BLOCK_LT + 1);
    good_trans.set_now(BLOCK_UT);

    good_trans.write_description(&description).unwrap();

    assert_eq!(acc, new_acc);
    assert_eq!(trans.read_description().unwrap(), description);
    assert_eq!(trans, good_trans);
}

#[test]
fn test_copyleft_not_found_config() {
    let addr = COPYLEFT_REWARD_ADDRESS.clone();

    let code = "
        PUSHROOT
        CTOS
        PUSHINT 9
        COPYLEFT
    ";
    let start_balance = 100_000_000_000u64;
    let msg_income = 1_400_200_000;

    let acc_id = ACCOUNT_ADDRESS.address();
    let code = compile_code_to_cell(code).unwrap();
    let data = addr.serialize().unwrap();

    let mut acc = create_test_account(start_balance, acc_id, code, data);

    let msg = create_msg(msg_income);

    disable_cross_check();
    let mut config = BLOCKCHAIN_CONFIG.raw_config().clone();
    let mut param8 = ConfigParam8 {
        global_version: config.get_global_version().unwrap()
    };
    param8.global_version.capabilities |= GlobalCapabilities::CapCopyleft as u64;
    config.set_config(ConfigParamEnum::ConfigParam8(param8)).unwrap();
    let config = BlockchainConfig::with_config(config.clone()).unwrap();
    let err = execute_with_params(config, &msg, &mut acc, BLOCK_LT + 1).expect_err("Expect error");
    if let ExecutorError::TrExecutorError(e) = err.downcast::<ExecutorError>().unwrap() {
        assert!(e.contains("no config 42 (copyleft)"));
    } else {
        unreachable!()
    }
}

#[test]
fn test_copyleft_too_big_percent() {
    let addr = COPYLEFT_REWARD_ADDRESS.clone();

    let code = "
        PUSHROOT
        CTOS
        PUSHINT 9
        COPYLEFT
    ";
    let start_balance = 100_000_000_000u64;
    let msg_income = 1_400_200_000;

    let acc_id = ACCOUNT_ADDRESS.address();
    let code = compile_code_to_cell(code).unwrap();
    let data = addr.serialize().unwrap();

    let mut acc = create_test_account(start_balance, acc_id, code, data);

    let msg = create_msg(msg_income);

    disable_cross_check();
    let err = execute_with_params(make_copyleft_config(), &msg, &mut acc, BLOCK_LT + 1).expect_err("Expect error");
    if let ExecutorError::TrExecutorError(e) = err.downcast::<ExecutorError>().unwrap() {
        assert!(e.contains("copyleft percent"));
    } else {
        unreachable!()
    }
}

#[test]
fn test_copyleft_not_found_license() {
    let addr = COPYLEFT_REWARD_ADDRESS.clone();

    let code = "
        PUSHROOT
        CTOS
        PUSHINT 1
        COPYLEFT
    ";
    let start_balance = 100_000_000_000u64;
    let msg_income = 1_400_200_000;

    let acc_id = ACCOUNT_ADDRESS.address();
    let code = compile_code_to_cell(code).unwrap();
    let data = addr.serialize().unwrap();

    let mut acc = create_test_account(start_balance, acc_id, code, data);

    let msg = create_msg(msg_income);

    let mut new_acc = Account::active_by_init_code_hash(
        acc.get_addr().unwrap().clone(),
        CurrencyCollection::with_grams(101399379404),
        1576526553,
        acc.state_init().unwrap().clone(),
        false
    ).unwrap();
    new_acc.update_storage_stat().unwrap();
    new_acc.set_last_tr_time(BLOCK_LT + 2);

    let trans = execute_copyleft(&msg, &mut acc, new_acc.balance().unwrap().grams, 0).unwrap();
    let copyleft = trans.copyleft_reward();
    assert!(copyleft.is_none());

    let mut description = TransactionDescrOrdinary::default();
    let storage_phase_fees = 127596u64;
    description.storage_ph = Some(TrStoragePhase::with_params(
        Grams::from(storage_phase_fees),
        None,
        AccStatusChange::Unchanged
    ));
    description.credit_ph = Some(TrCreditPhase::with_params(
        None,
        CurrencyCollection::with_grams(msg_income)
    ));

    let gas_used = 693u32;
    let gas_fees = gas_used as u64 * 1000;
    let mut vm_phase = TrComputePhaseVm::default();
    vm_phase.success = true;
    vm_phase.gas_used = gas_used.into();
    vm_phase.gas_limit = 1000000u32.into();
    vm_phase.gas_fees = gas_fees.into();
    vm_phase.vm_steps = 5;
    description.compute_ph = TrComputePhase::Vm(vm_phase);

    let mut action_ph = TrActionPhase::default();
    let mut actions = OutActions::default();
    actions.push_back(OutAction::CopyLeft{ license: 1, address: addr});
    action_ph.success = false;
    action_ph.valid = false;
    action_ph.status_change = AccStatusChange::Unchanged;
    action_ph.tot_actions = 1;
    action_ph.spec_actions = 1;
    action_ph.msgs_created = 0;
    action_ph.total_fwd_fees = None;
    action_ph.total_action_fees = None;
    action_ph.result_code = RESULT_CODE_NOT_FOUND_LICENSE;
    action_ph.result_arg = Some(0);
    action_ph.action_list_hash = actions.hash().unwrap();
    description.action = Some(action_ph);

    description.credit_first = true;
    description.bounce = None;
    description.aborted = true;
    description.destroyed = false;
    let description = TransactionDescr::Ordinary(description);

    let mut good_trans = Transaction::with_account_and_message(&new_acc, &msg, BLOCK_LT + 2).unwrap();

    good_trans.set_total_fees(CurrencyCollection::with_grams(gas_fees + storage_phase_fees));
    good_trans.orig_status = AccountStatus::AccStateActive;
    good_trans.set_end_status(AccountStatus::AccStateActive);
    good_trans.set_logical_time(BLOCK_LT + 1);
    good_trans.set_now(BLOCK_UT);

    good_trans.write_description(&description).unwrap();

    assert_eq!(acc, new_acc);
    assert_eq!(trans.read_description().unwrap(), description);
    assert_eq!(trans, good_trans);
}

#[test]
fn test_copyleft_many() {
    let addr = COPYLEFT_REWARD_ADDRESS.clone();

    let code = "
        PUSHROOT
        CTOS
        PUSHINT 7
        COPYLEFT

        PUSHROOT
        CTOS
        PUSHINT 7
        COPYLEFT

        PUSHROOT
        CTOS
        PUSHINT 3
        COPYLEFT
    ";
    let start_balance = 100_000_000_000u64;
    let compute_phase_gas_fees = 825000u64;
    let msg_income = 1_400_200_000;

    let acc_id = ACCOUNT_ADDRESS.address();
    let code = compile_code_to_cell(code).unwrap();
    let data = addr.serialize().unwrap();

    let mut acc = create_test_account(start_balance, acc_id, code, data);

    let msg = create_msg(msg_income);

    let mut new_acc = Account::active_by_init_code_hash(
        acc.get_addr().unwrap().clone(),
        CurrencyCollection::with_grams(101399241021),
        BLOCK_UT,
        acc.state_init().unwrap().clone(),
        false
    ).unwrap();
    new_acc.update_storage_stat().unwrap();
    new_acc.set_last_tr_time(BLOCK_LT + 2);

    let trans = execute_copyleft(&msg, &mut acc, new_acc.balance().unwrap().grams, 0).unwrap();
    let copyleft = trans.copyleft_reward();
    assert!(copyleft.is_none());

    let mut description = TransactionDescrOrdinary::default();
    let storage_phase_fees = 133979;
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
    vm_phase.success = false;
    vm_phase.gas_used = gas_used.into();
    vm_phase.gas_limit = 1000000u32.into();
    vm_phase.gas_fees = gas_fees.into();
    vm_phase.vm_steps = 8;
    vm_phase.exit_code = 14;
    description.compute_ph = TrComputePhase::Vm(vm_phase);
    description.action = None;

    description.credit_first = true;
    description.bounce = None;
    description.aborted = true;
    description.destroyed = false;
    let description = TransactionDescr::Ordinary(description);

    let mut good_trans = Transaction::with_account_and_message(&new_acc, &msg, BLOCK_LT + 2).unwrap();

    good_trans.set_total_fees(CurrencyCollection::with_grams(gas_fees + storage_phase_fees));
    good_trans.orig_status = AccountStatus::AccStateActive;
    good_trans.set_end_status(AccountStatus::AccStateActive);
    good_trans.set_logical_time(BLOCK_LT + 1);
    good_trans.set_now(BLOCK_UT);

    good_trans.write_description(&description).unwrap();

    assert_eq!(acc, new_acc);
    assert_eq!(trans.read_description().unwrap(), description);
    assert_eq!(trans, good_trans);
}

#[test]
fn test_copyleft_with_sendmsg_64() {
    let addr = COPYLEFT_REWARD_ADDRESS.clone();

    let code = "
        PUSHROOT
        CTOS
        LDREF
        LDREF
        SWAP
        PUSHINT 64
        SENDRAWMSG
        SWAP
        PUSHINT 64
        SENDRAWMSG

        PUSHINT 3
        COPYLEFT
    ";
    let start_balance = 100_000_000_000u64;
    let compute_phase_gas_fees = 1869000u64;
    let copyleft_reward = compute_phase_gas_fees / 10;
    let msg_income = 1_400_200_000;

    let acc_id = ACCOUNT_ADDRESS.address();
    let code = compile_code_to_cell(code).unwrap();

    let (mut msg1, mut msg2) = create_two_internal_messages();
    msg1.set_src_address(MsgAddressInt::with_standart(None, 0, acc_id.clone()).unwrap());
    msg2.set_src_address(MsgAddressInt::with_standart(None, 0, acc_id.clone()).unwrap());
    let mut b = addr.write_to_new_cell().unwrap();
    b.checked_append_reference(msg2.serialize().unwrap()).unwrap();
    b.checked_append_reference(msg1.serialize().unwrap()).unwrap();
    let data = b.into_cell().unwrap();

    let mut acc = create_test_account_workchain(start_balance, 0, acc_id, code, data);

    let msg = create_msg(msg_income);

    let mut new_acc = Account::active_by_init_code_hash(
        acc.get_addr().unwrap().clone(),
        CurrencyCollection::with_grams(99851577969),
        BLOCK_UT,
        acc.state_init().unwrap().clone(),
        false
    ).unwrap();
    new_acc.update_storage_stat().unwrap();
    new_acc.set_last_tr_time(BLOCK_LT + 4);

    let trans = execute_copyleft(&msg, &mut acc, new_acc.balance().unwrap().grams, 2).unwrap();
    let copyleft = trans.copyleft_reward().clone().unwrap();
    assert_eq!(copyleft.reward.as_u128(), copyleft_reward as u128);
    assert_eq!(&copyleft.address, &*COPYLEFT_REWARD_ADDRESS);

    let mut description = TransactionDescrOrdinary::default();
    let storage_phase_fees = 291031;
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
    vm_phase.vm_steps = 13;
    description.compute_ph = TrComputePhase::Vm(vm_phase);

    let msg_mine_fee = MSG_MINE_FEE;
    let msg_fwd_fee = MSG_FWD_FEE;
    let msg_remain_fee = msg_fwd_fee - msg_mine_fee;
    let mut actions = OutActions::default();
    actions.push_back(OutAction::new_send(SENDMSG_REMAINING_MSG_BALANCE, msg1.clone()));
    actions.push_back(OutAction::new_send(SENDMSG_REMAINING_MSG_BALANCE, msg2.clone()));
    if let Some(int_header) = msg1.int_header_mut() {
        if let Some(int_header2) = msg2.int_header_mut() {
            int_header.value.grams = Grams::from(MSG1_BALANCE + msg_income - compute_phase_gas_fees - msg_fwd_fee);
            int_header2.value.grams = Grams::from(MSG2_BALANCE - compute_phase_gas_fees - msg_fwd_fee);
            int_header.fwd_fee = msg_remain_fee.into();
            int_header2.fwd_fee = msg_remain_fee.into();
            int_header.created_at = BLOCK_UT.into();
            int_header2.created_at = BLOCK_UT.into();
        }
    }
    actions.push_back(OutAction::CopyLeft{ license: 3, address: addr});
    let mut action_ph = TrActionPhase::default();
    action_ph.success = true;
    action_ph.valid = true;
    action_ph.status_change = AccStatusChange::Unchanged;
    action_ph.tot_actions = 3;
    action_ph.spec_actions = 1;
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
    good_trans.set_total_fees(CurrencyCollection::with_grams(gas_fees + storage_phase_fees + msg_mine_fee * 2 - copyleft_reward));
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
fn test_copyleft_even_action_failed() {
    let addr = COPYLEFT_REWARD_ADDRESS.clone();

    let code = "
        PUSHROOT
        CTOS
        LDREF
        LDREF
        SWAP
        PUSHINT 64
        SENDRAWMSG
        SWAP
        PUSHINT 64
        SENDRAWMSG

        PUSHINT 3
        COPYLEFT
    ";
    let start_balance = 100_000_000_000u64;
    let compute_phase_gas_fees = 1869000u64;
    let copyleft_reward = compute_phase_gas_fees / 10;
    let msg_income = 1_400_200_000;

    let acc_id = ACCOUNT_ADDRESS.address();
    let code = compile_code_to_cell(code).unwrap();

    let (msg1, msg2) = create_two_internal_messages();
    let mut b = addr.write_to_new_cell().unwrap();
    b.checked_append_reference(msg2.serialize().unwrap()).unwrap();
    b.checked_append_reference(msg1.serialize().unwrap()).unwrap();
    let data = b.into_cell().unwrap();

    let mut acc = create_test_account(start_balance, acc_id, code, data);

    let msg = create_msg(msg_income);

    let mut new_acc = Account::active_by_init_code_hash(
        acc.get_addr().unwrap().clone(),
        CurrencyCollection::with_grams(101398039969),
        BLOCK_UT,
        acc.state_init().unwrap().clone(),
        false
    ).unwrap();
    new_acc.update_storage_stat().unwrap();
    new_acc.set_last_tr_time(BLOCK_LT + 2);

    let trans = execute_copyleft(&msg, &mut acc, new_acc.balance().unwrap().grams, 0).unwrap();
    let copyleft = trans.copyleft_reward().clone().unwrap();
    assert_eq!(copyleft.reward.as_u128(), copyleft_reward as u128);
    assert_eq!(&copyleft.address, &*COPYLEFT_REWARD_ADDRESS);

    let mut description = TransactionDescrOrdinary::default();
    let storage_phase_fees = 291031;
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
    vm_phase.vm_steps = 13;
    description.compute_ph = TrComputePhase::Vm(vm_phase);

    let mut actions = OutActions::default();
    actions.push_back(OutAction::new_send(SENDMSG_REMAINING_MSG_BALANCE, msg1));
    actions.push_back(OutAction::new_send(SENDMSG_REMAINING_MSG_BALANCE, msg2));
    actions.push_back(OutAction::CopyLeft{ license: 3, address: addr});
    let mut action_ph = TrActionPhase::default();
    action_ph.success = false;
    action_ph.valid = true;
    action_ph.status_change = AccStatusChange::Unchanged;
    action_ph.tot_actions = 3;
    action_ph.spec_actions = 1;
    action_ph.msgs_created = 0;
    action_ph.total_fwd_fees = None;
    action_ph.total_action_fees = None;
    action_ph.result_code = 35;
    action_ph.action_list_hash = actions.hash().unwrap();
    description.action = Some(action_ph);

    description.credit_first = true;
    description.bounce = None;
    description.aborted = true;
    description.destroyed = false;
    let description = TransactionDescr::Ordinary(description);

    let mut good_trans = Transaction::with_account_and_message(&new_acc, &msg, BLOCK_LT + 2).unwrap();

    good_trans.set_total_fees(CurrencyCollection::with_grams(gas_fees + storage_phase_fees - copyleft_reward));
    good_trans.orig_status = AccountStatus::AccStateActive;
    good_trans.set_end_status(AccountStatus::AccStateActive);
    good_trans.set_logical_time(BLOCK_LT + 1);
    good_trans.set_now(BLOCK_UT);

    good_trans.write_description(&description).unwrap();

    assert_eq!(acc, new_acc);
    assert_eq!(trans.read_description().unwrap(), description);
    assert_eq!(trans, good_trans);
}

#[test]
fn test_copyleft_delete2() {
    let addr = COPYLEFT_REWARD_ADDRESS.clone();

    let code = "
        PUSHROOT
        CTOS
        LDREF
        LDREF
        SWAP
        PUSHINT 0
        SENDRAWMSG
        SWAP
        PUSHINT 160
        SENDRAWMSG

        PUSHINT 3
        COPYLEFT
    ";
    let start_balance = 100_000_000_000u64;
    let compute_phase_gas_fees = 1869000u64;
    let copyleft_reward = compute_phase_gas_fees / 10;
    let msg_income = 1_400_200_000;

    let acc_id = ACCOUNT_ADDRESS.address();
    let code = compile_code_to_cell(code).unwrap();

    let (mut msg1, mut msg2) = create_two_internal_messages();
    msg1.set_src_address(MsgAddressInt::with_standart(None, 0, acc_id.clone()).unwrap());
    msg2.set_src_address(MsgAddressInt::with_standart(None, 0, acc_id.clone()).unwrap());
    let mut b = addr.write_to_new_cell().unwrap();
    b.checked_append_reference(msg2.serialize().unwrap()).unwrap();
    b.checked_append_reference(msg1.serialize().unwrap()).unwrap();
    let data = b.into_cell().unwrap();

    let mut acc = create_test_account_workchain(start_balance, 0, acc_id, code, data);

    let msg = create_msg(msg_income);

    let new_acc = Account::default();

    let trans = execute_copyleft(&msg, &mut acc, 0, 2).unwrap();
    let copyleft = trans.copyleft_reward().clone().unwrap();
    assert_eq!(copyleft.reward.as_u128(), copyleft_reward as u128);
    assert_eq!(&copyleft.address, &*COPYLEFT_REWARD_ADDRESS);

    let mut description = TransactionDescrOrdinary::default();
    let storage_phase_fees = 291031;
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
    vm_phase.gas_limit = 1000000.into();
    vm_phase.gas_fees = gas_fees.into();
    vm_phase.vm_steps = 13;
    description.compute_ph = TrComputePhase::Vm(vm_phase);

    let msg_mine_fee = MSG_MINE_FEE;
    let msg_fwd_fee = MSG_FWD_FEE;
    let msg_remain_fee = msg_fwd_fee - msg_mine_fee;
    let mut actions = OutActions::default();
    actions.push_back(OutAction::new_send(SENDMSG_ORDINARY, msg1.clone()));
    actions.push_back(OutAction::new_send(SENDMSG_ALL_BALANCE + SENDMSG_DELETE_IF_EMPTY, msg2.clone()));
    if let Some(int_header) = msg1.int_header_mut() {
        if let Some(int_header2) = msg2.int_header_mut() {
            int_header.value.grams = Grams::from(MSG1_BALANCE - msg_fwd_fee);
            int_header2.value.grams = Grams::from(101338039969u64);
            int_header.fwd_fee = msg_remain_fee.into();
            int_header2.fwd_fee = msg_remain_fee.into();
            int_header.created_at = BLOCK_UT.into();
            int_header2.created_at = BLOCK_UT.into();
        }
    }
    actions.push_back(OutAction::CopyLeft{ license: 3, address: addr});
    let mut action_ph = TrActionPhase::default();
    action_ph.success = true;
    action_ph.valid = true;
    action_ph.status_change = AccStatusChange::Deleted;
    action_ph.tot_actions = 3;
    action_ph.spec_actions = 1;
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
    description.destroyed = true;
    let description = TransactionDescr::Ordinary(description);

    let mut good_trans = Transaction::with_account_and_message(&new_acc, &msg, BLOCK_LT + 2).unwrap();

    good_trans.add_out_message(&make_common(msg1)).unwrap();
    good_trans.add_out_message(&make_common(msg2)).unwrap();
    good_trans.set_total_fees(CurrencyCollection::with_grams(gas_fees + storage_phase_fees + msg_mine_fee * 2 - copyleft_reward));
    good_trans.orig_status = AccountStatus::AccStateActive;
    good_trans.set_end_status(AccountStatus::AccStateNonexist);
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
fn test_copyleft_without_capability() {
    let addr = COPYLEFT_REWARD_ADDRESS.clone();

    let code = "
        PUSHROOT
        CTOS
        PUSHINT 3
        COPYLEFT
    ";
    let start_balance = 100_000_000_000u64;
    let compute_phase_gas_fees = 212000u64;
    let msg_income = 1_400_200_000;

    let acc_id = ACCOUNT_ADDRESS.address();
    let code = compile_code_to_cell(code).unwrap();
    let data = addr.serialize().unwrap();

    let mut acc = create_test_account(start_balance, acc_id, code, data);

    let msg = create_msg(msg_income);

    let mut new_acc = Account::active_by_init_code_hash(
        acc.get_addr().unwrap().clone(),
        CurrencyCollection::with_grams(101399860404),
        BLOCK_UT,
        acc.state_init().unwrap().clone(),
        false
    ).unwrap();
    new_acc.update_storage_stat().unwrap();
    new_acc.set_last_tr_time(BLOCK_LT + 2);

    let acc_before = acc.clone();
    disable_cross_check();
    let mut config = BLOCKCHAIN_CONFIG.raw_config().clone();
    let mut copyleft_config = ConfigCopyleft::default();
    copyleft_config.copyleft_reward_threshold = COPYLEFT_REWARD_THRESHOLD.into();
    copyleft_config.license_rates.set(&3u8, &10).unwrap();
    config.set_config(ConfigParamEnum::ConfigParam42(copyleft_config)).unwrap();
    let config = BlockchainConfig::with_config(config.clone()).unwrap();
    let trans = execute_with_params(config, &msg, &mut acc, BLOCK_LT + 1).unwrap();
    let copyleft = trans.copyleft_reward();
    assert!(copyleft.is_none());
    check_account_and_transaction(&acc_before, &acc, &msg, Some(&trans), new_acc.balance().unwrap().grams, 0);
    acc.update_storage_stat().unwrap();

    let mut description = TransactionDescrOrdinary::default();
    let storage_phase_fees = 127596;
    description.storage_ph = Some(TrStoragePhase::with_params(
        Grams::from(storage_phase_fees),
        None,
        AccStatusChange::Unchanged
    ));
    description.credit_ph = Some(TrCreditPhase::with_params(
        None,
        CurrencyCollection::with_grams(msg_income)
    ));

    let gas_used = compute_phase_gas_fees as u32 / 1000;
    let gas_fees = compute_phase_gas_fees;
    let mut vm_phase = TrComputePhaseVm::default();
    vm_phase.success = false;
    vm_phase.gas_used = gas_used.into();
    vm_phase.gas_limit = 1000000u32.into();
    vm_phase.gas_fees = gas_fees.into();
    vm_phase.vm_steps = 4;
    vm_phase.exit_code = 6;
    description.compute_ph = TrComputePhase::Vm(vm_phase);
    description.action = None;

    description.credit_first = true;
    description.bounce = None;
    description.aborted = true;
    description.destroyed = false;
    let description = TransactionDescr::Ordinary(description);

    let mut good_trans = Transaction::with_account_and_message(&new_acc, &msg, BLOCK_LT + 2).unwrap();

    good_trans.set_total_fees(CurrencyCollection::with_grams(gas_fees + storage_phase_fees));
    good_trans.orig_status = AccountStatus::AccStateActive;
    good_trans.set_end_status(AccountStatus::AccStateActive);
    good_trans.set_logical_time(BLOCK_LT + 1);
    good_trans.set_now(BLOCK_UT);

    good_trans.write_description(&description).unwrap();

    assert_eq!(acc, new_acc);
    assert_eq!(trans.read_description().unwrap(), description);
    assert_eq!(trans, good_trans);
}
