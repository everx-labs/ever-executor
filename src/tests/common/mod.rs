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

#![cfg(test)]
#![allow(dead_code)]
#![allow(clippy::duplicate_mod)]
#![allow(clippy::field_reassign_with_default)]

pub(crate) mod cross_check;

use super::*;

use std::sync::{atomic::AtomicU64, Arc};

use crate::{OrdinaryTransactionExecutor, TickTockTransactionExecutor};
use pretty_assertions::assert_eq;
use ever_block::{
    accounts::{Account, AccountStorage, StorageInfo},
    config_params::GlobalCapabilities,
    messages::{
        CommonMsgInfo, ExternalInboundMessageHeader, InternalMessageHeader, Message, MsgAddressInt,
    },
    transactions::{
        TrComputePhase, Transaction, TransactionDescr
    },
    AddSub, ConfigParam8, ConfigParamEnum, ConfigParams, CurrencyCollection, Deserializable, Grams, 
    Serializable, StateInit, TrBouncePhase, TransactionDescrOrdinary, TransactionTickTock, 
    UnixTime32, VarUInteger32, AccStatusChange, CommonMessage,
};
use ever_assembler::compile_code_to_cell;
use ever_block::{AccountId, BuilderData, Cell, HashmapE, SliceData, UInt256, Result};
use crate::{BlockchainConfig, ExecuteParams, TransactionExecutor};

pub const BLOCK_LT: u64 = 2_000_000_000;
pub const PREV_BLOCK_LT: u64 = 1_998_000_000;
pub const ACCOUNT_UT: u32 = 1572169011;
pub const BLOCK_UT:   u32 = 1576526553;
pub const MSG1_BALANCE: u64 = 50_000_000;
pub const MSG2_BALANCE: u64 = 100_000_000;
pub const MSG_FWD_FEE: u64 = 10_000_000;
pub const MSG_MINE_FEE: u64 = 3_333_282;

lazy_static::lazy_static! {
    pub static ref SENDER_ACCOUNT:   AccountId = AccountId::from([0x11; 32]);
    pub static ref RECEIVER_ACCOUNT: AccountId = AccountId::from([0x22; 32]);
    pub static ref BLOCKCHAIN_CONFIG: BlockchainConfig = default_config();
}

pub fn make_common(std_msg: Message) -> CommonMessage {
    CommonMessage::Std(std_msg)
}

pub fn read_config() -> Result<ConfigParams> {
    ever_block::Deserializable::construct_from_file("real_ever_boc/default_config.boc")
}

pub fn custom_config(version: Option<u32>, capabilities: Option<u64>) -> BlockchainConfig {
    let mut config = read_config().unwrap();
    let mut param8 = ConfigParam8 {
        global_version: config.get_global_version().unwrap()
    };
    if let Some(version) = version {
        param8.global_version.version = version
    }
    if let Some(capabilities) = capabilities {
        param8.global_version.capabilities |= capabilities
    }
    config.set_config(ConfigParamEnum::ConfigParam8(param8)).unwrap();
    BlockchainConfig::with_config(config).unwrap()
}

pub fn default_config() -> BlockchainConfig {
    BlockchainConfig::with_config(read_config().unwrap()).unwrap()
}

pub fn execute_params(
    last_tr_lt: Arc<AtomicU64>,
    state_libs: HashmapE,
    seed_block: UInt256,
    block_version: u32,
) -> ExecuteParams {
    ExecuteParams {
        state_libs,
        block_unixtime: BLOCK_UT,
        block_lt: BLOCK_LT,
        last_tr_lt,
        seed_block,
        debug: true,
        block_version,
        ..ExecuteParams::default()
    }
}

pub fn execute_params_none(last_tr_lt: Arc<AtomicU64>) -> ExecuteParams {
    execute_params(last_tr_lt, HashmapE::default(), UInt256::default(), 0)
}

pub fn execute_params_block_version(last_tr_lt: Arc<AtomicU64>, block_version: u32) -> ExecuteParams {
    execute_params(last_tr_lt, HashmapE::default(), UInt256::default(), block_version)
}

pub fn create_two_internal_messages() -> (Message, Message) {
    let msg1 = create_int_msg(
        AccountId::from([0x11; 32]),
        AccountId::from([0x33; 32]),
        MSG1_BALANCE,
        false,
        BLOCK_LT + 2
    );
    let msg2 = create_int_msg(
        AccountId::from([0x11; 32]),
        AccountId::from([0x33; 32]),
        MSG2_BALANCE,
        true,
        BLOCK_LT + 3
    );
    (msg1, msg2)
}

pub fn create_two_messages_data() -> Cell {
    let (msg1, msg2) = create_two_internal_messages();

    let mut b = BuilderData::with_raw(vec![0x55; 32], 256).unwrap();
    b.checked_append_reference(msg2.serialize().unwrap()).unwrap();
    b.checked_append_reference(msg1.serialize().unwrap()).unwrap();
    b.into_cell().unwrap()
}

pub fn create_two_messages_data_2(src: AccountId, w_id: i8) -> Cell {
    let (mut msg1, mut msg2) = create_two_internal_messages();
    msg1.set_src_address(MsgAddressInt::with_standart(None, w_id, src.clone()).unwrap());
    msg2.set_src_address(MsgAddressInt::with_standart(None, w_id, src).unwrap());

    let mut b = BuilderData::with_raw(vec![0x55; 32], 256).unwrap();
    b.checked_append_reference(msg2.serialize().unwrap()).unwrap();
    b.checked_append_reference(msg1.serialize().unwrap()).unwrap();
    b.into_cell().unwrap()
}

pub fn create_int_msg_workchain(w_id: i8, src: AccountId, dest: AccountId, value: impl Into<Grams>, bounce: bool, lt: u64) -> Message {
    let mut hdr = InternalMessageHeader::with_addresses(
        MsgAddressInt::with_standart(None, w_id, src).unwrap(),
        MsgAddressInt::with_standart(None, w_id, dest).unwrap(),
        CurrencyCollection::from_grams(value.into()),
    );
    hdr.bounce = bounce;
    hdr.ihr_disabled = true;
    hdr.created_lt = lt;
    hdr.created_at = UnixTime32::default();
    Message::with_int_header(hdr)
}

pub fn create_int_msg(src: AccountId, dest: AccountId, value: impl Into<Grams>, bounce: bool, lt: u64) -> Message {
    create_int_msg_workchain(-1, src, dest, value, bounce, lt)
}

pub fn create_send_two_messages_code() -> Cell {
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
    ").unwrap()
}

pub fn create_test_account_workchain(amount: impl Into<Grams>, w_id: i8, address: AccountId, code: Cell, data: Cell) -> Account {
    let mut account = Account::with_storage(
        &MsgAddressInt::with_standart(
            None,
            w_id,
            address
        ).unwrap(),
        &StorageInfo::with_values(
            ACCOUNT_UT,
            None,
        ),
        &AccountStorage::active_by_init_code_hash(0, CurrencyCollection::from_grams(amount.into()), StateInit::default(), false),
    );
    account.set_code(code);
    account.set_data(data);
    account.update_storage_stat().unwrap();
    account
}

pub fn create_test_account(amount: impl Into<Grams>, address: AccountId, code: Cell, data: Cell) -> Account {
    create_test_account_workchain(amount, -1, address, code, data)
}

pub fn create_test_external_msg() -> Message {
    let acc_id = AccountId::from([0x11; 32]);
    let hdr = ExternalInboundMessageHeader::new(
        Default::default(),
        MsgAddressInt::with_standart(None, -1, acc_id).unwrap()
    );
    Message::with_ext_in_header_and_body(hdr, SliceData::default())
}

pub fn check_account_and_transaction_balances(
    acc_before: &Account,
    acc_after: &Account,
    msg: &Message,
    trans: Option<&Transaction>,
) {
    if trans.is_none() {
        // no checks needed
        return;
    }

    let trans = trans.unwrap();

    let mut left = acc_before.balance().cloned().unwrap_or_default();
    if let Some(value) = msg.get_value() {
        left.add(value).unwrap();
    }
    left.grams += msg.int_header().map_or(0, |header| header.ihr_fee.as_u128());

    let mut right = acc_after.balance().cloned().unwrap_or_default();
    right.add(trans.total_fees()).unwrap();
    let copyleft = trans.copyleft_reward().clone();
    let copyleft_reward_after = copyleft.map_or(0.into(), |copyleft| copyleft.reward);
    right.grams.add(&copyleft_reward_after).unwrap();
    trans.iterate_out_msgs(|out_msg| {
        if let Some(header) = out_msg.get_std().unwrap().int_header() {
            right.add(header.value())?;
            right.grams.add(header.fwd_fee())?;
            right.grams.add(&header.ihr_fee)?;
        }
        Ok(true)
    }).unwrap();
    assert_eq!(left, right);

    // check fees
    let descr = trans.read_description().unwrap();
    if let TransactionDescr::Ordinary(descr) = descr {
        let mut total_fee = trans.total_fees().clone();

        let mut fees = CurrencyCollection::default();
        fees.grams += descr.storage_ph.as_ref().map_or(0, |st| st.storage_fees_collected.as_u128());
        if let Some(storage) = descr.storage_ph.as_ref() {
            if storage.status_change == AccStatusChange::Deleted {
                if descr.credit_first {
                    fees.add(&msg.get_value().cloned().unwrap_or_default()).unwrap();
                    fees.grams -= msg.get_value().cloned().unwrap_or_default().grams;
                }
                fees.add(&acc_before.balance().cloned().unwrap_or_default()).unwrap();
                fees.grams -= acc_before.balance().map_or(0, |cc| cc.grams.as_u128());
            }
        }
        if let Some(cr) = descr.credit_ph.as_ref() {
            if let Some(g) = cr.due_fees_collected.as_ref() {
                fees.grams += *g
            }
        }
        if let TrComputePhase::Vm(cp) = &descr.compute_ph {
            fees.grams += cp.gas_fees
        }
        if let Some(ap) = descr.action.as_ref() {
            if ap.success {
                fees.grams += ap.total_action_fees()
            }
        }
        if let Some(TrBouncePhase::Ok(bp)) = descr.bounce.as_ref() {
            fees.grams += bp.msg_fees
        }
        let is_special = BLOCKCHAIN_CONFIG.is_special_account(&msg.dst().unwrap()).unwrap();
        let is_ext_msg = matches!(msg.header(), CommonMsgInfo::ExtInMsgInfo(_));
        if is_ext_msg && !is_special {
            let in_msg_cell = msg.serialize().unwrap();
            let in_fwd_fee = BLOCKCHAIN_CONFIG.calc_fwd_fee(msg.is_masterchain(), &in_msg_cell).unwrap();
            fees.grams += in_fwd_fee
        }

        total_fee.grams.add(&copyleft_reward_after).unwrap();

        assert_eq!(fees, total_fee);
    }

    // check messages fees
    let mut fwd_fees = Grams::zero();
    trans.iterate_out_msgs(|out_msg| {
        if let Ok(std_msg) = out_msg.get_std() {
            if let Some(header) = std_msg.int_header() {
            fwd_fees.add(&header.fwd_fee)?;
            fwd_fees.add(&header.ihr_fee)?;
            }
        }
        Ok(true)
    }).unwrap();

    let descr = trans.read_description().unwrap();
    if let TransactionDescr::Ordinary(descr) = descr {
        let mut trans_fwd_fee = Grams::zero();
        if let Some(ap) = descr.action.as_ref() {
            if ap.success {
                trans_fwd_fee += ap.total_fwd_fees() - ap.total_action_fees()
            }
        }
        if descr.bounce.is_some() {
            if let TrBouncePhase::Ok(bp) = &descr.bounce.as_ref().unwrap() {
                trans_fwd_fee += bp.fwd_fees;
            }
        }
        assert_eq!(fwd_fees, trans_fwd_fee);
    }

    // check logical time
    if !acc_after.is_none() {
        assert_eq!(
            trans.logical_time() + trans.msg_count() as u64 + 1,
            acc_after.last_tr_time().unwrap()
        );
    }

    // other checks
    assert_eq!(trans.orig_status, acc_before.status());
    assert_eq!(trans.end_status, acc_after.status());
}

pub fn check_account_and_transaction(
    acc_before: &Account,
    acc_after: &Account,
    msg: &Message,
    trans: Option<&Transaction>,
    result_account_balance: impl Into<Grams>,
    count_out_msgs: usize,
) {
    if let Some(trans) = trans {
        assert_eq!(
            (trans.out_msgs.len().unwrap(), acc_after.balance().cloned().unwrap_or_default()),
            (count_out_msgs, CurrencyCollection::from_grams(result_account_balance.into()))
        );
    }
    check_account_and_transaction_balances(acc_before, acc_after, msg, trans);
}

pub fn execute_with_block_version(
    config: BlockchainConfig, 
    msg: &Message, 
    acc: &mut Account, 
    tr_lt: u64, 
    block_version: u32
) -> Result<Transaction> {
    let lt = Arc::new(AtomicU64::new(tr_lt));
    let executor = OrdinaryTransactionExecutor::new(config.clone());
    let acc_before = acc.clone();
    let trans= executor.execute_with_params(Some(&CommonMessage::Std(msg.clone())), acc, execute_params_block_version(lt, block_version));
    if cfg!(feature = "cross_check") {
        cross_check::cross_check(
            &config,
            &acc_before,
            acc,
            Some(msg),
            trans.as_ref().ok(),
            &execute_params_block_version(Arc::new(AtomicU64::new(tr_lt)), block_version), 
            0
        );
    }
    trans
}

pub fn execute_with_params(config: BlockchainConfig, msg: &Message, acc: &mut Account, tr_lt: u64) -> Result<Transaction> {
    execute_with_block_version(config, msg, acc, tr_lt, 0)
} 

pub fn execute_with_params_c(
    config: BlockchainConfig,
    msg: &Message,
    acc: &mut Account,
    tr_lt: u64,
    result_account_balance: impl Into<Grams>,
    count_out_msgs: usize
) -> Result<Transaction> {
    let acc_before = acc.clone();
    let trans = execute_with_params(config, msg, acc, tr_lt);
    check_account_and_transaction(&acc_before, acc, msg, trans.as_ref().ok(), result_account_balance, count_out_msgs);
    trans
}

pub fn execute(msg: &Message, acc: &mut Account, tr_lt: u64) -> Result<Transaction> {
    let acc_before = acc.clone();
    let trans = execute_with_params(BLOCKCHAIN_CONFIG.to_owned(), msg, acc, tr_lt);
    check_account_and_transaction_balances(&acc_before, acc, msg, None);
    trans
}

pub fn execute_c(msg: &Message, acc: &mut Account, tr_lt: u64, result_account_balance: impl Into<Grams>, count_out_msgs: usize) -> Result<Transaction> {
    let acc_before = acc.clone();
    let trans = execute_with_params(BLOCKCHAIN_CONFIG.to_owned(), msg, acc, tr_lt);
    check_account_and_transaction(&acc_before, acc, msg, trans.as_ref().ok(), result_account_balance, count_out_msgs);
    trans
}

pub fn execute_custom_transaction(
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

    let trans = execute_c(&msg, &mut acc, BLOCK_LT + 1, result_account_balance, count_out_msgs).unwrap();
    (acc, trans)
}

#[allow(clippy::too_many_arguments)]
pub fn execute_custom_transaction_with_extra_balance(
    start_balance: u64,
    start_extra_balance: u64,
    code: &str,
    data: Cell,
    msg_balance: u64,
    result_account_balance: u64,
    result_account_extra_balance: u64,
    count_out_msgs: usize,
) {
    let acc_id = AccountId::from([0x11; 32]);
    let code = compile_code_to_cell(code).unwrap();
    let mut acc = create_test_account(start_balance, acc_id.clone(), code, data);
    let mut acc_balance = acc.balance().unwrap().clone();
    acc_balance.other.set(&11111111u32, &VarUInteger32::from_two_u128(0, start_extra_balance.into()).unwrap()).unwrap();
    acc.set_balance(acc_balance);
    let msg = create_int_msg(
        AccountId::from([0x33; 32]),
        acc_id,
        msg_balance,
        false,
        BLOCK_LT - 2
    );

    let tr_lt = BLOCK_LT + 1;
    let trans = execute_with_params(BLOCKCHAIN_CONFIG.to_owned(), &msg, &mut acc, tr_lt).unwrap();

    assert_eq!(trans.out_msgs.len().unwrap(), count_out_msgs);
    let mut answer = CurrencyCollection::with_grams(result_account_balance);
    answer.other.set(&11111111u32, &VarUInteger32::from_two_u128(0, result_account_extra_balance.into()).unwrap()).unwrap();
    assert_eq!(acc.balance().unwrap_or(&CurrencyCollection::default()), &answer);
}

pub fn get_tr_descr(tr: &Transaction) -> TransactionDescrOrdinary {
    if let TransactionDescr::Ordinary(descr) = tr.read_description().unwrap() {
        descr
    } else {
        panic!("Not found description")
    }
}

pub fn replay_transaction_by_files(c: Option<(&mut criterion::Criterion, &str)>, account: &str, account_after: &str, transaction: &str, config: &str) {
    let mut account = Account::construct_from_file(account).unwrap();
    let transaction = Transaction::construct_from_file(transaction).unwrap();
    let message = transaction.read_in_msg().unwrap();
    let account_after = Account::construct_from_file(account_after).unwrap();
    let config = ConfigParams::construct_from_file(config).unwrap();

    let mut left = account.balance().cloned().unwrap_or_default().grams;
    if let Some(msg) = message.as_ref() {
        let msg = msg.get_std().unwrap();
        if let Some(value) = msg.get_value() {
            left.add(&value.grams).unwrap();
        }
        // here can be fault in future in IHR will work
        if let Some(header) = msg.int_header() {
            left.add(&header.ihr_fee).unwrap(); // ihr is disabled, so we return ihr_fee
        }
    }

    let at = transaction.now();
    let lt = transaction.logical_time();
    let old_hash = account.serialize().unwrap().repr_hash();

    if let Some((c, name)) = c {
        c.bench_function(name, |b| b.iter(|| {
            let m = message.as_ref().map(|m| m.get_std().unwrap()).map(|m|m.clone());
            try_replay_transaction(&mut account.clone(), m, config.clone(), at, lt).unwrap();
        }));
    }
    let m = message.as_ref().map(|m| m.get_std().unwrap()).map(|m|m.clone());
    let mut our_transaction = try_replay_transaction(&mut account, m, config.clone(), at, lt).unwrap();

    let mut right = account.balance().cloned().unwrap_or_default().grams;
    right.add(&our_transaction.total_fees().grams).unwrap();
    our_transaction.iterate_out_msgs(|out_msg| {
        if let Some(header) = out_msg.get_std().unwrap().int_header() {
            right.add(&header.value().grams)?;
            right.add(header.fwd_fee())?;
            right.add(&header.ihr_fee)?;
        }
        Ok(true)
    }).unwrap();
    assert_eq!(left, right);

    our_transaction.set_prev_trans_hash(transaction.prev_trans_hash().clone());
    our_transaction.set_prev_trans_lt(transaction.prev_trans_lt());
    // our_transaction.write_to_file("real_ever_boc/transaction_my.boc").unwrap();

    let max = std::cmp::max(transaction.msg_count(), our_transaction.msg_count());
    for i in 0..max {
        assert_eq!(our_transaction.get_out_msg(i).ok(), transaction.get_out_msg(i).ok());
    }
    assert_eq!(our_transaction.read_description().unwrap(), transaction.read_description().unwrap());
    let hash_update = transaction.read_state_update().unwrap();
    assert_eq!(hash_update.old_hash, old_hash);

    our_transaction.write_state_update(&hash_update).unwrap();
    our_transaction.set_prev_trans_hash(transaction.prev_trans_hash().clone());
    our_transaction.set_prev_trans_lt(transaction.prev_trans_lt());

    // determine correct mechanism of storage calculation
    if config.has_capability(GlobalCapabilities::CapFastStorageStat) {
        account.update_storage_stat_fast().unwrap();
    } else {
        account.update_storage_stat().unwrap();
    }
    let new_hash = account.serialize().unwrap().repr_hash();
    if hash_update.new_hash == new_hash {
        assert_eq!(our_transaction, transaction);
        assert_eq!(account, account_after);
        return
    }
    // the new account can be unrechable in the blockchain - try to save new one
    // it will be correct because of hash cheking in state update of transaction
    // account.write_to_file("real_ever_boc/account_new.boc").unwrap();
    assert_eq!(account, account_after);
}

pub fn try_replay_transaction(
    account: &mut Account,
    message: Option<Message>,
    config_params: ConfigParams,
    at: u32,
    tx_lt: u64
) -> Result<Transaction> {
    let config = BlockchainConfig::with_config(config_params).unwrap();
    let executor: Box<dyn TransactionExecutor> = if message.is_none() {
        let tt = account.get_tick_tock().unwrap();
        let tt = match (tt.tick, tt.tock) {
            (true, false) => TransactionTickTock::Tick,
            (false, true) => TransactionTickTock::Tock,
            (_tick, _tock) => panic!("must be tick or tock")
        };
        Box::new(TickTockTransactionExecutor::new(config.clone(), tt))
    } else {
        Box::new(OrdinaryTransactionExecutor::new(config.clone()))
    };

    let block_lt = tx_lt - tx_lt % 1_000_000;
    let lt = Arc::new(AtomicU64::new(tx_lt));
    let params = ExecuteParams {
        block_unixtime: at,
        block_lt,
        last_tr_lt: lt,
        debug: true,
        ..ExecuteParams::default()
    };
    let acc_before = account.clone();
    let cmn_msg = message.map(make_common);
    let tx = executor.execute_with_params(cmn_msg.as_ref(), account, params);
    if cfg!(feature = "cross_check") {
        let params = ExecuteParams {
            block_unixtime: at,
            block_lt,
            last_tr_lt: Arc::new(AtomicU64::new(tx_lt)),
            debug: true,
            ..ExecuteParams::default()
        };

        common::cross_check::cross_check(&config, &acc_before, account, cmn_msg.as_ref().map(|m| m.get_std().unwrap()), tx.as_ref().ok(), &params, 0);
    }
    tx
}
