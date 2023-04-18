/*
 * Copyright (C) 2022 - 2023 EverX. All Rights Reserved.
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

use criterion::{criterion_group, criterion_main, Criterion};
use serde::{Serialize, Deserialize};
use ton_block::{Deserializable, Account, Transaction, ConfigParams, Serializable, TransactionDescr};
use ton_executor::{BlockchainConfig, ExecuteParams, OrdinaryTransactionExecutor, TickTockTransactionExecutor, TransactionExecutor};
use ton_types::{error, fail, Result, UInt256, Cell, Status};
use std::sync::{Arc, atomic::AtomicU64};

#[path = "../src/tests/common/mod.rs"]
mod common;
use common::replay_transaction_by_files;

fn bench_real_transaction(c: &mut Criterion) {
    replay_transaction_by_files(Some((c, "real_transaction")),
        "real_ton_boc/simple_account_old.boc",
        "real_ton_boc/simple_account_new.boc",
        "real_ton_boc/simple_transaction.boc",
        "real_ton_boc/config.boc"
    )
}

fn bench_real_deploy_transaction(c: &mut Criterion) {
    replay_transaction_by_files(Some((c, "real_deploy_transaction")),
        "real_ton_boc/deploy_account_old.boc",
        "real_ton_boc/deploy_account_new.boc",
        "real_ton_boc/deploy_transaction.boc",
        "real_ton_boc/config.boc"
    )
}

fn bench_init_account_transaction(c: &mut Criterion) {
    replay_transaction_by_files(Some((c, "init_account_transaction")),
        "real_ton_boc/init_account_old.boc",
        "real_ton_boc/init_account_new.boc",
        "real_ton_boc/init_transaction.boc",
        "real_ton_boc/config.boc"
    )
}

fn bench_check_execute_bounced_message(c: &mut Criterion) {
    replay_transaction_by_files(Some((c, "check_execute_bounced_message")),
        "real_ton_boc/bounce_msg_account_old.boc",
        "real_ton_boc/bounce_msg_account_new.boc",
        "real_ton_boc/bounce_msg_transaction.boc",
        "real_ton_boc/config.boc"
    )
}

fn bench_check_execute_out_message_with_body_in_ref(c: &mut Criterion) {
    replay_transaction_by_files(Some((c, "check_execute_out_message_with_body_in_ref")),
        "real_ton_boc/msg_body_ref_account_old.boc",
        "real_ton_boc/msg_body_ref_account_new.boc",
        "real_ton_boc/msg_body_ref_transaction.boc",
        "real_ton_boc/config.boc"
    )
}

fn bench_check_execute_uninit_account(c: &mut Criterion) {
    replay_transaction_by_files(Some((c, "check_execute_uninit_account")),
        "real_ton_boc/uninit_account_old.boc",
        "real_ton_boc/uninit_account_new.boc",
        "real_ton_boc/uninit_account_transaction.boc",
        "real_ton_boc/config.boc"
    )
}

fn bench_check_send_remainig_msg_balance(c: &mut Criterion) {
    replay_transaction_by_files(Some((c, "check_send_remainig_msg_balance")),
        "real_ton_boc/send_remainig_msg_balance_account_old.boc",
        "real_ton_boc/send_remainig_msg_balance_account_new.boc",
        "real_ton_boc/send_remainig_msg_balance_transaction.boc",
        "real_ton_boc/config.boc"
    )
}

fn bench_check_out_of_gas_transaction(c: &mut Criterion) {
    replay_transaction_by_files(Some((c, "check_out_of_gas_transaction")),
        "real_ton_boc/out_of_gas_account_old.boc",
        "real_ton_boc/out_of_gas_account_new.boc",
        "real_ton_boc/out_of_gas_transaction.boc",
        "real_ton_boc/config.boc"
    )
}

fn bench_check_wrong_skip_reason(c: &mut Criterion) {
    replay_transaction_by_files(Some((c, "check_wrong_skip_reason")),
        "real_ton_boc/wrong_skip_reason_account_old.boc",
        "real_ton_boc/wrong_skip_reason_account_new.boc",
        "real_ton_boc/wrong_skip_reason_transaction.boc",
        "real_ton_boc/config.boc"
    )
}

fn bench_check_wrong_compute_phase(c: &mut Criterion) {
    replay_transaction_by_files(Some((c, "check_wrong_compute_phase")),
        "real_ton_boc/wrong_compute_phase_account_old.boc",
        "real_ton_boc/wrong_compute_phase_account_new.boc",
        "real_ton_boc/wrong_compute_phase_transaction.boc",
        "real_ton_boc/config.boc"
    )
}

fn bench_check_nofunds_to_send_message_without_error(c: &mut Criterion) {
    replay_transaction_by_files(Some((c, "check_nofunds_to_send_message_without_error")),
        "real_ton_boc/nofunds_without_error_account_old.boc",
        "real_ton_boc/nofunds_without_error_account_new.boc",
        "real_ton_boc/nofunds_without_error_transaction.boc",
        "real_ton_boc/config.boc"
    )
}

fn bench_bounce_message_to_new_account(c: &mut Criterion) {
    replay_transaction_by_files(Some((c, "bounce_message_to_new_account")),
        "real_ton_boc/bounce_message_to_new_account_account_old.boc",
        "real_ton_boc/bounce_message_to_new_account_account_new.boc",
        "real_ton_boc/bounce_message_to_new_account_transaction.boc",
        "real_ton_boc/config.boc"
    )
}

fn bench_out_of_gas_in_cmd(c: &mut Criterion) {
    replay_transaction_by_files(Some((c, "out_of_gas_in_cmd")),
        "real_ton_boc/bounce_message_to_new_account_account_old.boc",
        "real_ton_boc/bounce_message_to_new_account_account_new.boc",
        "real_ton_boc/bounce_message_to_new_account_transaction.boc",
        "real_ton_boc/config.boc"
    )
}

fn bench_freeze_account(c: &mut Criterion) {
    replay_transaction_by_files(Some((c, "freeze_account")),
        "real_ton_boc/freeze_account_old.boc",
        "real_ton_boc/freeze_account_new.boc",
        "real_ton_boc/freeze_transaction.boc",
        "real_ton_boc/config.boc"
    )
}

fn bench_send_to_frozen_account(c: &mut Criterion) {
    replay_transaction_by_files(Some((c, "send_to_frozen_account")),
        "real_ton_boc/send_to_frozen_account_old.boc",
        "real_ton_boc/send_to_frozen_account_new.boc",
        "real_ton_boc/send_to_frozen_transaction.boc",
        "real_ton_boc/config.boc"
    )
}

fn bench_unfreeze_account(c: &mut Criterion) {
    replay_transaction_by_files(Some((c, "unfreeze_account")),
        "real_ton_boc/unfreeze_account_old.boc",
        "real_ton_boc/unfreeze_account_new.boc",
        "real_ton_boc/unfreeze_transaction.boc",
        "real_ton_boc/config.boc"
    )
}

fn bench_bounce_to_empty_account(c: &mut Criterion) {
    replay_transaction_by_files(Some((c, "bounce_to_empty_account")),
        "real_ton_boc/bounce_to_empty_account_old.boc",
        "real_ton_boc/bounce_to_empty_account_new.boc",
        "real_ton_boc/bounce_to_empty_transaction.boc",
        "real_ton_boc/config.boc"
    )
}

fn bench_bounce_to_low_balance_account(c: &mut Criterion) {
    replay_transaction_by_files(Some((c, "bounce_to_low_balance_account")),
        "real_ton_boc/bounce_to_low_balance_account_old.boc",
        "real_ton_boc/bounce_to_low_balance_account_new.boc",
        "real_ton_boc/bounce_to_low_balance_transaction.boc",
        "real_ton_boc/config.boc"
    )
}

fn bench_depool_balance_check(c: &mut Criterion) {
    replay_transaction_by_files(Some((c, "depool_balance_check")),
        "real_ton_boc/depool_balance_check_account_old.boc",
        "real_ton_boc/depool_balance_check_account_new.boc",
        "real_ton_boc/depool_balance_check_transaction.boc",
        "real_ton_boc/config.boc"
    )
}

fn bench_no_balance_to_send_transaction(c: &mut Criterion) {
    replay_transaction_by_files(Some((c, "no_balance_to_send_transaction")),
        "real_ton_boc/no_balance_to_send_account_old.boc",
        "real_ton_boc/no_balance_to_send_account_new.boc",
        "real_ton_boc/no_balance_to_send_transaction.boc",
        "real_ton_boc/config.boc"
    )
}

fn bench_int_message_to_elector_transaction(c: &mut Criterion) {
    replay_transaction_by_files(Some((c, "int_message_to_elector_transaction")),
        "real_ton_boc/int_message_to_elector_account_old.boc",
        "real_ton_boc/int_message_to_elector_account_new.boc",
        "real_ton_boc/int_message_to_elector_transaction.boc",
        "real_ton_boc/config.boc"
    )
}

fn bench_int_message_to_elector2_transaction(c: &mut Criterion) {
    replay_transaction_by_files(Some((c, "int_message_to_elector2_transaction")),
        "real_ton_boc/int_message_to_elector2_account_old.boc",
        "real_ton_boc/int_message_to_elector2_account_new.boc",
        "real_ton_boc/int_message_to_elector2_transaction.boc",
        "real_ton_boc/config.boc"
    )
}

fn bench_ihr_message(c: &mut Criterion) {
    replay_transaction_by_files(Some((c, "ihr_message")),
        "real_ton_boc/ihr_message_account_old.boc",
        "real_ton_boc/ihr_message_account_new.boc",
        "real_ton_boc/ihr_message_transaction.boc",
        "real_ton_boc/config.boc"
    )
}

fn bench_tick_tock_message(c: &mut Criterion) {
    replay_transaction_by_files(Some((c, "tick_tock_message")),
        "real_ton_boc/tick_tock_acc_old.boc",
        "real_ton_boc/tick_tock_acc_new.boc",
        "real_ton_boc/tick_tock_tx.boc",
        "real_ton_boc/config.boc"
    )
}

fn bench_count_steps_vm(c: &mut Criterion) {
    replay_transaction_by_files(Some((c, "count_steps_vm")),
        "real_ton_boc/count_steps_acc_old.boc",
        "real_ton_boc/count_steps_acc_new.boc",
        "real_ton_boc/count_steps_tx.boc",
        "real_ton_boc/config.boc"
    )
}

fn bench_not_aborted_accepted_transaction(c: &mut Criterion) {
    replay_transaction_by_files(Some((c, "not_aborted_accepted_transaction")),
        "real_ton_boc/not_abort_accept_account_account_old.boc",
        "real_ton_boc/not_abort_accept_account_account_new.boc",
        "real_ton_boc/not_abort_accept_account_transaction.boc",
        "real_ton_boc/config.boc"
    )
}

fn bench_ihr_fee_output_msg(c: &mut Criterion) {
    replay_transaction_by_files(Some((c, "ihr_fee_output_msg")),
        "real_ton_boc/ihr_fee_account_old.boc",
        "real_ton_boc/ihr_fee_account_new.boc",
        "real_ton_boc/ihr_fee_transaction.boc",
        "real_ton_boc/config.boc"
    )
}

#[derive(Serialize, Deserialize)]
struct BlockDescr {
    id: String,
    config_boc: String,
    accounts: Vec<BlockAccountDescr>,
}

#[derive(Serialize, Deserialize)]
struct BlockAccountDescr {
    account_boc: String,
    transactions: Vec<String>,
}

#[derive(Clone)]
struct BlockData {
    config: BlockchainConfig,
    accounts: Vec<BlockAccountData>,
}

#[derive(Clone)]
struct BlockAccountData {
    account_cell: Cell,
    transactions: Vec<Transaction>,
}

fn load_blockchain_config(config_account: &Account) -> Result<BlockchainConfig> {
    let config_cell = config_account
        .get_data().ok_or_else(|| error!("config account data loading error"))?
        .reference(0).ok();
    let config_params = ConfigParams::with_address_and_params(
        UInt256::with_array([0x55; 32]), config_cell);
    BlockchainConfig::with_config(config_params)
}

fn load_block(block_filename: &str) -> Result<BlockData> {
    let block_file = std::fs::File::open(block_filename)?;
    let block: BlockDescr = serde_json::from_reader(std::io::BufReader::new(block_file))?;
    let config_account = Account::construct_from_base64(&block.config_boc)?;
    let config = load_blockchain_config(&config_account)?;
    let mut accounts = Vec::new();
    for acc in block.accounts {
        let account = Account::construct_from_base64(&acc.account_boc)?;
        let account_cell = account.serialize()?;
        let mut transactions = Vec::new();
        for txn in acc.transactions {
            let tr = Transaction::construct_from_base64(&txn)?;
            transactions.push(tr);
        }
        accounts.push(BlockAccountData { account_cell, transactions });
    }
    Ok(BlockData { config, accounts })
}

fn replay_block(data: BlockData) -> Status {
    for acc in data.accounts {
        let mut account = acc.account_cell;
        for tr in acc.transactions {
            let executor: Box<dyn TransactionExecutor> =
                match tr.read_description()? {
                    TransactionDescr::TickTock(desc) => {
                        Box::new(TickTockTransactionExecutor::new(data.config.clone(), desc.tt))
                    }
                    TransactionDescr::Ordinary(_) => {
                        Box::new(OrdinaryTransactionExecutor::new(data.config.clone()))
                    }
                    _ => fail!("unknown transaction type")
                };
            executor.execute_with_libs_and_params(
                tr.read_in_msg()?.as_ref(),
                &mut account,
                ExecuteParams {
                    block_unixtime: tr.now(),
                    block_lt: tr.logical_time(),
                    last_tr_lt: Arc::new(AtomicU64::new(tr.logical_time())),
                    ..Default::default()
                }
            )?;
            if account.repr_hash() != tr.read_state_update()?.new_hash {
                fail!("new hash mismatch");
            }
        }
    }
    Ok(())
}

// block 9278c99e55994a1636d4343b651c09beceb684cdb3a3a173f2c844feeef541ba
// downloaded from os.ton.dev on 2022-03-24
fn bench_block_0(c: &mut Criterion) {
    let block = load_block("benches/block-0.json").unwrap();
    c.bench_function("block-0", |b| b.iter(|| {
        replay_block(block.clone()).expect("replay failed")
    }));
}

criterion_group!(benches,
    bench_real_transaction,
    bench_real_deploy_transaction,
    bench_init_account_transaction,
    bench_check_execute_bounced_message,
    bench_check_execute_out_message_with_body_in_ref,
    bench_check_execute_uninit_account,
    bench_check_send_remainig_msg_balance,
    bench_check_out_of_gas_transaction,
    bench_check_wrong_skip_reason,
    bench_check_wrong_compute_phase,
    bench_check_nofunds_to_send_message_without_error,
    bench_bounce_message_to_new_account,
    bench_out_of_gas_in_cmd,
    bench_freeze_account,
    bench_send_to_frozen_account,
    bench_unfreeze_account,
    bench_bounce_to_empty_account,
    bench_bounce_to_low_balance_account,
    bench_depool_balance_check,
    bench_no_balance_to_send_transaction,
    bench_int_message_to_elector_transaction,
    bench_int_message_to_elector2_transaction,
    bench_ihr_message,
    bench_tick_tock_message,
    bench_count_steps_vm,
    bench_not_aborted_accepted_transaction,
    bench_ihr_fee_output_msg,
    bench_block_0,
);
criterion_main!(benches);
