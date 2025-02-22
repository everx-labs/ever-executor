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

use self::common::make_common;

use super::*;

use std::{
    io::{BufRead, BufReader},
    sync::{Arc, atomic::AtomicU64}
};

use pretty_assertions::assert_eq;
use ever_block::{
    Account, AccountStorage, Block, ConfigParams, CurrencyCollection,
    Deserializable, Message, MsgAddressInt, Serializable, StateInit,
    StorageInfo, Transaction,
};
use ever_block::{Cell, Result, read_single_root_boc};

use crate::{
    blockchain_config::BlockchainConfig, 
    OrdinaryTransactionExecutor, TransactionExecutor
};

mod common;

use common::try_replay_transaction;
use crate::transaction_executor::tests_with_real_data::common::custom_config;

fn replay_transaction_by_files(account: &str, account_after: &str, transaction: &str, config: &str) {
    common::replay_transaction_by_files(None, account, account_after, transaction, config)
}

#[ignore]
#[test]
fn test_sample_replay_transaction() {
    let code = "code_boc_file_name_here";
    let data = "data_boc_file_name_here";
    let message = "msg_boc_file_name_here";
    let key_block = "key_block_boc_file_name_here";
    replay_contract_by_files(code, data, message, key_block).unwrap()
}

// account_code and account_data - filenames with bocs of code and data for account
// in_message - filename with boc of message
// key_block - filename with masterchain keyblock
fn replay_contract_by_files(account_code: &str, account_data: &str, in_message: &str, key_block: &str) -> Result<()> {
    let code = read_single_root_boc(std::fs::read(account_code)?)?;
    let data = read_single_root_boc(std::fs::read(account_data)?)?;
    let block = Block::construct_from_file(key_block)?;
    let mc_block_extra = block.read_extra()?.read_custom()?.expect("must be key block");
    let config = mc_block_extra.config().cloned().expect("must be in key block");
    let message = Message::construct_from_file(in_message)?;
    try_replay_contract_as_transaction(code, data, message, config)?;
    Ok(())
}

#[ignore]
#[test]
fn test_sample_replay_many_transactions() {
    let code = "code_boc_file_name_here";
    let data = "data_boc_file_name_here";
    let message = "msg_text_file_name_here";
    let key_block = "key_block_boc_file_name_here";
    many_replay_contract_by_files(code, data, message, key_block).unwrap()
}

// account_code and account_data - filenames with bocs of code and data for account
// in_message - filename with serialized messages as base64
// key_block - filename with masterchain keyblock
fn many_replay_contract_by_files(account_code: &str, account_data: &str, in_message: &str, key_block: &str) -> Result<()> {
    let code = read_single_root_boc(std::fs::read(account_code)?)?; 
    let data = read_single_root_boc(std::fs::read(account_data)?)?;
    let cell = read_single_root_boc(std::fs::read(key_block)?)?;
    let block = Block::construct_from_cell(cell).unwrap();
    let mc_block_extra = block.read_extra()?.read_custom()?.expect("must be key block");
    let config = mc_block_extra.config().cloned().expect("must be in key block");
    let file = std::fs::File::open(in_message)?;
    let mut result = vec![];
    let mut idx = 1;
    for ln in BufReader::new(file).lines() {
        let contents = ln.unwrap();
        println!("Message no. #{} len: {}", idx, contents.len());
        if idx > 0 {
            let message = Message::construct_from_base64(&contents)?;
            let transaction = try_replay_contract_as_transaction(code.clone(), data.clone(), message, config.clone())?;
            let descr = transaction.read_description()?;
            let compute_ph = descr.compute_phase_ref().expect("no compute phase");
            match compute_ph {
                ever_block::TrComputePhase::Vm(vm) => {
                    let steps = vm.vm_steps;
                    let gas = vm.gas_used;
                    result.push((idx, contents.len(), steps, gas));
                }
                _ => panic!("compute skipped")
            }
        }
        idx += 1;
    }
    panic!("{:?}", result)
}

// creates account with code and data and address from message
// then executes with config params
fn try_replay_contract_as_transaction(code: Cell, data: Cell, message: Message, config: ConfigParams) -> Result<Transaction> {
    let at = std::time::SystemTime::now().duration_since(std::time::SystemTime::UNIX_EPOCH).unwrap().as_secs() as u32;
    let lt = 1_000_005;

    let mut state = StateInit::default();
    state.set_code(code);
    state.set_data(data);

    let account_id = message.int_dst_account_id().expect("must be exetrnal inbound message");
    let addr = MsgAddressInt::with_standart(None, 0, account_id)?;
    let balance = CurrencyCollection::with_grams(100_000_000_000);

    let mut account = Account::with_storage(
        &addr,
        &StorageInfo::with_values(at - 100, None),
        &AccountStorage::active_by_init_code_hash(0, balance, state, config.has_capability(GlobalCapabilities::CapInitCodeHash)),
    );
    account.update_storage_stat().unwrap();
    try_replay_transaction(&mut account, Some(message), config, at, lt)
}

#[test]
fn test_real_transaction() {
    replay_transaction_by_files(
        "real_ever_boc/simple_account_old.boc",
        "real_ever_boc/simple_account_new.boc",
        "real_ever_boc/simple_transaction.boc",
        "real_ever_boc/config.boc"
    )
}

#[test]
fn test_real_deploy_transaction() {
    replay_transaction_by_files(
        "real_ever_boc/deploy_account_old.boc",
        "real_ever_boc/deploy_account_new.boc",
        "real_ever_boc/deploy_transaction.boc",
        "real_ever_boc/config.boc"
    )
}

#[test]
fn test_init_account_transaction() {
    replay_transaction_by_files(
        "real_ever_boc/init_account_old.boc",
        "real_ever_boc/init_account_new.boc",
        "real_ever_boc/init_transaction.boc",
        "real_ever_boc/config.boc"
    )
}

#[test]
fn test_check_execute_bounced_message() {
    replay_transaction_by_files(
        "real_ever_boc/bounce_msg_account_old.boc",
        "real_ever_boc/bounce_msg_account_new.boc",
        "real_ever_boc/bounce_msg_transaction.boc",
        "real_ever_boc/config.boc"
    )
}

#[test]
fn test_check_execute_out_message_with_body_in_ref() {
    replay_transaction_by_files(
        "real_ever_boc/msg_body_ref_account_old.boc",
        "real_ever_boc/msg_body_ref_account_new.boc",
        "real_ever_boc/msg_body_ref_transaction.boc",
        "real_ever_boc/config.boc"
    )
}

#[test]
fn test_check_execute_uninit_account() {
    replay_transaction_by_files(
        "real_ever_boc/uninit_account_old.boc",
        "real_ever_boc/uninit_account_new.boc",
        "real_ever_boc/uninit_account_transaction.boc",
        "real_ever_boc/config.boc"
    )
}

#[test]
fn test_check_send_remainig_msg_balance() {
    replay_transaction_by_files(
        "real_ever_boc/send_remainig_msg_balance_account_old.boc",
        "real_ever_boc/send_remainig_msg_balance_account_new.boc",
        "real_ever_boc/send_remainig_msg_balance_transaction.boc",
        "real_ever_boc/config.boc"
    )
}

#[test]
fn test_check_out_of_gas_transaction() {
    replay_transaction_by_files(
        "real_ever_boc/out_of_gas_account_old.boc",
        "real_ever_boc/out_of_gas_account_new.boc",
        "real_ever_boc/out_of_gas_transaction.boc",
        "real_ever_boc/config.boc"
    )
}

#[test]
fn test_check_wrong_skip_reason() {
    replay_transaction_by_files(
        "real_ever_boc/wrong_skip_reason_account_old.boc",
        "real_ever_boc/wrong_skip_reason_account_new.boc",
        "real_ever_boc/wrong_skip_reason_transaction.boc",
        "real_ever_boc/config.boc"
    )
}

#[test]
fn test_check_wrong_compute_phase() {
    replay_transaction_by_files(
        "real_ever_boc/wrong_compute_phase_account_old.boc",
        "real_ever_boc/wrong_compute_phase_account_new.boc",
        "real_ever_boc/wrong_compute_phase_transaction.boc",
        "real_ever_boc/config.boc"
    )
}

#[test]
fn test_check_nofunds_to_send_message_without_error() {
    replay_transaction_by_files(
        "real_ever_boc/nofunds_without_error_account_old.boc",
        "real_ever_boc/nofunds_without_error_account_new.boc",
        "real_ever_boc/nofunds_without_error_transaction.boc",
        "real_ever_boc/config.boc"
    )
}

#[test]
fn test_bounce_message_to_new_account() {
    replay_transaction_by_files(
        "real_ever_boc/bounce_message_to_new_account_account_old.boc",
        "real_ever_boc/bounce_message_to_new_account_account_new.boc",
        "real_ever_boc/bounce_message_to_new_account_transaction.boc",
        "real_ever_boc/config.boc"
    )
}

#[test]
fn test_out_of_gas_in_cmd() {
    replay_transaction_by_files(
        "real_ever_boc/bounce_message_to_new_account_account_old.boc",
        "real_ever_boc/bounce_message_to_new_account_account_new.boc",
        "real_ever_boc/bounce_message_to_new_account_transaction.boc",
        "real_ever_boc/config.boc"
    )
}

#[test]
fn test_freeze_account() {
    replay_transaction_by_files(
        "real_ever_boc/freeze_account_old.boc",
        "real_ever_boc/freeze_account_new.boc",
        "real_ever_boc/freeze_transaction.boc",
        "real_ever_boc/config.boc"
    )
}

#[test]
fn test_send_to_frozen_account() {
    replay_transaction_by_files(
        "real_ever_boc/send_to_frozen_account_old.boc",
        "real_ever_boc/send_to_frozen_account_new.boc",
        "real_ever_boc/send_to_frozen_transaction.boc",
        "real_ever_boc/config.boc"
    )
}

#[test]
fn test_unfreeze_account() {
    replay_transaction_by_files(
        "real_ever_boc/unfreeze_account_old.boc",
        "real_ever_boc/unfreeze_account_new.boc",
        "real_ever_boc/unfreeze_transaction.boc",
        "real_ever_boc/config.boc"
    )
}

#[test]
fn test_bounce_to_empty_account() {
    replay_transaction_by_files(
        "real_ever_boc/bounce_to_empty_account_old.boc",
        "real_ever_boc/bounce_to_empty_account_new.boc",
        "real_ever_boc/bounce_to_empty_transaction.boc",
        "real_ever_boc/config.boc"
    )
}

#[test]
fn test_bounce_to_low_balance_account() {
    replay_transaction_by_files(
        "real_ever_boc/bounce_to_low_balance_account_old.boc",
        "real_ever_boc/bounce_to_low_balance_account_new.boc",
        "real_ever_boc/bounce_to_low_balance_transaction.boc",
        "real_ever_boc/config.boc"
    )
}

#[test]
fn test_depool_balance_check() {
    replay_transaction_by_files(
        "real_ever_boc/depool_balance_check_account_old.boc",
        "real_ever_boc/depool_balance_check_account_new.boc",
        "real_ever_boc/depool_balance_check_transaction.boc",
        "real_ever_boc/config.boc"
    )
}

#[test]
fn test_no_balance_to_send_transaction() {
    replay_transaction_by_files(
        "real_ever_boc/no_balance_to_send_account_old.boc",
        "real_ever_boc/no_balance_to_send_account_new.boc",
        "real_ever_boc/no_balance_to_send_transaction.boc",
        "real_ever_boc/config.boc"
    )
}

#[test]
fn test_int_message_to_elector_transaction() {
    replay_transaction_by_files(
        "real_ever_boc/int_message_to_elector_account_old.boc",
        "real_ever_boc/int_message_to_elector_account_new.boc",
        "real_ever_boc/int_message_to_elector_transaction.boc",
        "real_ever_boc/config.boc"
    )
}

#[test]
fn test_int_message_to_elector2_transaction() {
    replay_transaction_by_files(
        "real_ever_boc/int_message_to_elector2_account_old.boc",
        "real_ever_boc/int_message_to_elector2_account_new.boc",
        "real_ever_boc/int_message_to_elector2_transaction.boc",
        "real_ever_boc/config.boc"
    )
}

#[test]
fn test_ihr_message() {
    replay_transaction_by_files(
        "real_ever_boc/ihr_message_account_old.boc",
        "real_ever_boc/ihr_message_account_new.boc",
        "real_ever_boc/ihr_message_transaction.boc",
        "real_ever_boc/config.boc"
    )
}

#[test]
fn test_tick_tock_message() {
    replay_transaction_by_files(
        "real_ever_boc/tick_tock_acc_old.boc",
        "real_ever_boc/tick_tock_acc_new.boc",
        "real_ever_boc/tick_tock_tx.boc",
        "real_ever_boc/config.boc"
    )
}

#[test]
fn test_count_steps_vm() {
    replay_transaction_by_files(
        "real_ever_boc/count_steps_acc_old.boc",
        "real_ever_boc/count_steps_acc_new.boc",
        "real_ever_boc/count_steps_tx.boc",
        "real_ever_boc/config.boc"
    )
}

#[test]
fn test_not_aborted_accepted_transaction() {
    replay_transaction_by_files(
        "real_ever_boc/not_abort_accept_account_account_old.boc",
        "real_ever_boc/not_abort_accept_account_account_new.boc",
        "real_ever_boc/not_abort_accept_account_transaction.boc",
        "real_ever_boc/config.boc"
    )
}

#[test]
fn test_ihr_fee_output_msg() {
    if !cfg!(feature = "ihr_disabled") {
        replay_transaction_by_files(
            "real_ever_boc/ihr_fee_account_old.boc",
            "real_ever_boc/ihr_fee_account_new.boc",
            "real_ever_boc/ihr_fee_transaction.boc",
            "real_ever_boc/config.boc"
        )
    }
}

#[ignore = "test for replay transaction by files"]
#[test]
fn test_replay_transaction_by_files() {
    log4rs::init_file("src/tests/log_cfg.yml", Default::default()).ok();
    let mut account = Account::construct_from_file("real_ever_boc/account_old.boc").unwrap();
    let message = Message::construct_from_file("real_ever_boc/message.boc").unwrap();
    let key_block = Block::construct_from_file("real_ever_boc/config.boc").unwrap();
    let config_params = key_block.read_extra().unwrap().read_custom().unwrap().unwrap().config().unwrap().clone();
    let at = match message.int_header() {
        Some(hdr) => hdr.created_at.as_u32(),
        None => std::time::SystemTime::now().duration_since(std::time::SystemTime::UNIX_EPOCH).unwrap().as_secs() as u32
    };
    let lt = account.last_tr_time().unwrap() + 100;

    let config = BlockchainConfig::with_config(config_params).unwrap();
    let executor = OrdinaryTransactionExecutor::new(config);

    let block_lt = lt - lt % 1_000_000;
    let lt = Arc::new(AtomicU64::new(lt));

    let params = ExecuteParams {
        state_libs: HashmapE::with_bit_len(32),
        block_unixtime: at,
        block_lt,
        last_tr_lt: lt,
        seed_block: UInt256::default(),
        debug: true,
        ..ExecuteParams::default()
    };
    let tr = executor.execute_with_params(Some(&make_common(message)), &mut account, params).unwrap();
    tr.write_to_file("real_ever_boc/transaction_my.boc").unwrap();
    account.write_to_file("real_ever_boc/account_new_my.boc").unwrap();
}

#[test]
fn test_reserve_value_message() {
    let mut account = Account::construct_from_file("real_ever_boc/reserve_value_from_account.boc").unwrap();
    let message = Message::construct_from_file("real_ever_boc/reserve_value_message.boc").unwrap();
    let config = ConfigParams::construct_from_file("real_ever_boc/config.boc").unwrap();
    let (at, lt) = message.at_and_lt().unwrap();

    let our_transaction = try_replay_transaction(&mut account, Some(message), config, at, lt).unwrap();
    assert_eq!(our_transaction.out_msgs.len().unwrap(), 1);
}

#[test]
fn test_revert_action_phase() {
    let mut account = Account::construct_from_file("real_ever_boc/revert_action_account.boc").unwrap();
    let transaction = Transaction::construct_from_file("real_ever_boc/revert_action_transaction.boc").unwrap();
    let message = transaction.read_in_msg().unwrap().unwrap();
    let config = ConfigParams::construct_from_file("real_ever_boc/config.boc").unwrap();

    let lt: u64 = 241181000001; 
    let at: u32 = 1626694478;

    let mut answer = account.clone();
    if let CommonMessage::Std(message) = message {
        try_replay_transaction(&mut account, Some(message), config, at, lt).unwrap();
    }
    answer.set_data(account.get_data().unwrap());
    answer.set_balance(account.get_balance().unwrap().clone());
    answer.set_last_tr_time(account.last_tr_time().unwrap_or(0));
    assert_eq!(answer, account);
}

#[test]
fn test_deploy_contract_with_lib() {
    let account = Account::construct_from_file("real_ever_boc/account_with_deploy_contract_with_lib_function.boc").unwrap();
    let message = Message::construct_from_file("real_ever_boc/msg_with_deploy_contract_with_lib_function.boc").unwrap();
    let config = ConfigParams::construct_from_file("real_ever_boc/config.boc").unwrap();

    let lt: u64 = account.last_tr_time().unwrap() + 1;
    let at: u32 = 1668059542 - 10;

    let mut answer = account;
    let tx= try_replay_transaction(&mut answer, Some(message), config, at, lt).unwrap();

    let deploy_message = tx.get_out_msg(0).unwrap().unwrap();
    let mut new_account = Account::default();

    let lt: u64 = 1;
    let at: u32 = 1668059542 - 9;

    let config = custom_config(None, Some(GlobalCapabilities::CapSetLibCode as u64));
    let config = config.raw_config().clone();

    if let CommonMessage::Std(msg) = deploy_message {
        try_replay_transaction(&mut new_account, Some(msg), config, at, lt).unwrap();
    }
    assert_eq!(0, new_account.libraries().len().unwrap());
    assert_eq!(AccountStatus::AccStateUninit, new_account.status());
}
