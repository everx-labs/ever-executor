/*
* Copyright 2018-2020 TON DEV SOLUTIONS LTD.
*
* Licensed under the SOFTWARE EVALUATION License (the "License"); you may not use
* this file except in compliance with the License.  You may obtain a copy of the
* License at: https://ton.dev/licenses
*
* Unless required by applicable law or agreed to in writing, software
* distributed under the License is distributed on an "AS IS" BASIS,
* WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
* See the License for the specific TON DEV software governing permissions and
* limitations under the License.
*/

use crate::{
    blockchain_config::BlockchainConfig, 
    transaction_executor::{
        build_ordinary_stack, OrdinaryTransactionExecutor, 
        TransactionDescrOrdinary, TransactionExecutor
    }
};

use std::sync::Arc;
use ton_block::{
    AccountId, GetRepresentationHash, Serializable, StateInit, StorageUsedShort, 
    UInt256, UnixTime32, 
    accounts::{Account, AccountState, AccountStorage, ShardAccount, StorageInfo},
    messages::{
        CommonMsgInfo, CurrencyCollection, ExternalInboundMessageHeader, 
        InternalMessageHeader, Message, MsgAddressInt
    },
    out_actions::{OutAction, OutActions, SENDMSG_ALL_BALANCE, SENDMSG_ORDINARY},
    transactions::{
        AccStatusChange, HashUpdate, Transaction, TransactionDescr, TrActionPhase, 
        TrComputePhase, TrComputePhaseVm, TrStoragePhase
    },
    types::{Grams, VarUInteger7}
};
use ton_types::{BuilderData, Cell, SliceData};
use ton_vm::{
    int, assembler::compile_code, stack::{Stack, StackItem, integer::IntegerData}
};

const BLOCK_LT: u64 = 123;
const ACCOUNT_UT: u32 = 1572169011;
const BLOCK_UT: u32 = 1576526553;

lazy_static::lazy_static! {
    static ref SENDER_ACCOUNT:   AccountId = AccountId::from_raw(vec![0x11; 32], 256);
    static ref RECEIVER_ACCOUNT: AccountId = AccountId::from_raw(vec![0x22; 32], 256);
}

fn create_test_data() -> Cell {
    let (msg1, msg2) = create_two_internal_messages();

    let mut b = BuilderData::with_raw(vec![0x55; 32], 256).unwrap();
    b.append_reference(msg2.write_to_new_cell().unwrap());
    b.append_reference(msg1.write_to_new_cell().unwrap());
    b.into()
}

fn create_two_internal_messages() -> (Message, Message) {
    let msg1 = create_int_msg(
        AccountId::from_raw(vec![0x11; 32], 256),
        AccountId::from_raw(vec![0x33; 32], 256),
        50000000,
        false,
        BLOCK_LT + 2
    );
    let msg2 = create_int_msg(
        AccountId::from_raw(vec![0x11; 32], 256),
        AccountId::from_raw(vec![0x33; 32], 256),
        100000000,
        true,
        BLOCK_LT + 3
    );
    (msg1, msg2)
}

const INTERNAL_FWD_FEE: u64 = 5;
fn create_int_msg(src: AccountId, dest: AccountId, value: u64, bounce: bool, lt: u64) -> Message {
    let balance = CurrencyCollection::with_grams(value);
    let mut hdr = InternalMessageHeader::with_addresses(
        MsgAddressInt::with_standart(None, -1, src).unwrap(),
        MsgAddressInt::with_standart(None, -1, dest).unwrap(),
        balance,
    );
    hdr.bounce = bounce;
    hdr.ihr_disabled = true;
    hdr.ihr_fee = Grams::zero();
    hdr.fwd_fee = Grams(INTERNAL_FWD_FEE.into());
    hdr.created_lt = lt;
    hdr.created_at = UnixTime32::default();
    let msg = Message::with_int_header(hdr);
    //msg.body = Some(SliceData::default());
    msg
}

fn create_test_code() -> Cell {
    let code = "
    ACCEPT
    PUSHROOT
    CTOS
    LDREF
    PLDREF
    PUSHINT 0
    SENDRAWMSG
    PUSHINT 0
    SENDRAWMSG
    ";

    compile_code(code).unwrap().into_cell()
}

fn create_test_account(amount: u64, address: AccountId, code: Cell, data: Cell) -> Account {
    let mut state = StateInit::default();
    state.set_code(code);
    state.set_data(data);
    Account::with_storage(
        &MsgAddressInt::with_standart(
            None, 
            -1, 
            address
        ).unwrap(),
        &StorageInfo::with_values(
            ACCOUNT_UT,
            None,
        ),
        &AccountStorage {
            last_trans_lt: 0,
            balance: CurrencyCollection::with_grams(amount),
            state: AccountState::with_state(state),
        }
    )
}

fn create_test_external_msg() -> Message {
    let acc_id = AccountId::from_raw(vec![0x11; 32], 256);
    let mut hdr = ExternalInboundMessageHeader::default();
    hdr.dst = MsgAddressInt::with_standart(None, -1, acc_id).unwrap();
    hdr.import_fee = Grams::zero();
    let mut msg = Message::with_ext_in_header(hdr);
    *msg.body_mut() = Some(SliceData::default());
    msg
}

#[test]
fn test_trexecutor_active_acc_with_code1() {
    let used = 1307u64; //gas units
    let storage_fees = 289434515;
    let msg_mine_fee = 3333282;
    let msg_fwd_fee = 10000000;
    let msg_remain_fee = msg_fwd_fee - msg_mine_fee;
    let gas_fees = used * 10000;
    let gas_credit = 10000;

    let config = BlockchainConfig::default();
    let acc_id = AccountId::from_raw(vec![0x11; 32], 256);
    let start_balance = 2000000000u64;
    let acc = create_test_account(start_balance, acc_id.clone(), create_test_code(), create_test_data());
    let old_acc = ShardAccount::with_params(acc, UInt256::from([0;32]), 0).unwrap();
    // balance - (balance of 2 output messages + input msg fee + storage_fee + gas_fee)
    let end_balance = start_balance - (150000000 + msg_fwd_fee + storage_fees + gas_fees); 
    let mut new_acc = create_test_account(end_balance, acc_id.clone(), create_test_code(), create_test_data());
    let msg = create_test_external_msg();
    let block_lt = BLOCK_LT;
    let tr_lt = block_lt + 1;
    new_acc.set_last_tr_time(tr_lt);
    
    let mut executor = OrdinaryTransactionExecutor::new();
    let mut shard_acc = Some(old_acc.clone());
    let trans = executor.execute(
        &config, msg.clone(), &mut shard_acc, BLOCK_UT, block_lt.clone(), true
    ).unwrap();

    let mut good_trans = Transaction::with_account_and_message(&old_acc.read_account().unwrap(), &msg, tr_lt).unwrap();
    good_trans.set_now(BLOCK_UT);
    
    let (mut msg1, mut msg2) = create_two_internal_messages();
    let mut actions = OutActions::default();
    actions.push_back(OutAction::new_send(SENDMSG_ORDINARY, Arc::new(msg1.clone())));
    actions.push_back(OutAction::new_send(SENDMSG_ORDINARY, Arc::new(msg2.clone())));
    let hash = actions.hash().unwrap();
    msg1.get_value_mut().unwrap().grams = Grams::from(50000000 - msg_fwd_fee);
    msg2.get_value_mut().unwrap().grams = Grams::from(100000000 - msg_fwd_fee);
    if let CommonMsgInfo::IntMsgInfo(int_header) = msg1.header_mut() {
        if let CommonMsgInfo::IntMsgInfo(int_header2) = msg2.header_mut() {
            int_header.fwd_fee = msg_remain_fee.into();
            int_header2.fwd_fee = msg_remain_fee.into();
            int_header.created_at = BLOCK_UT.into();
            int_header2.created_at = BLOCK_UT.into();
        }
    }

    good_trans.add_out_message(&msg1).unwrap();
    good_trans.add_out_message(&msg2).unwrap();
    good_trans.set_total_fees((msg_fwd_fee + storage_fees + gas_fees + msg_mine_fee * 2).into());

    let old = Cell::from(old_acc.read_account().unwrap().write_to_new_cell().unwrap());
    let new = Cell::from(new_acc.write_to_new_cell().unwrap());
    good_trans.write_state_update(&HashUpdate::with_hashes(old.repr_hash(), new.repr_hash())).unwrap();
    
    let mut description = TransactionDescrOrdinary::default();
    description.storage_ph = Some(TrStoragePhase::with_params(289434515.into(), None, AccStatusChange::Unchanged));
    description.credit_ph = None;

    let mut vm_phase = TrComputePhaseVm::default();
    vm_phase.success = true;
    vm_phase.msg_state_used = false;
    vm_phase.account_activated = false;
    vm_phase.gas_used = VarUInteger7(used.into());
    vm_phase.gas_limit = VarUInteger7(0);
    vm_phase.gas_credit = Some(gas_credit.into());
    vm_phase.gas_fees = gas_fees.into(); 
    description.compute_ph = TrComputePhase::Vm(vm_phase);

    let mut action_ph = TrActionPhase::default();
    action_ph.success = true;
    action_ph.valid = true;
    action_ph.status_change = AccStatusChange::Unchanged;
    action_ph.tot_actions = 2;
    action_ph.msgs_created = 2;
    action_ph.total_fwd_fees = Some((2 * msg_fwd_fee).into());
    action_ph.total_action_fees = Some((2 * msg_mine_fee).into());
    action_ph.action_list_hash = hash;
    action_ph.tot_msg_size = StorageUsedShort::calculate_for_struct(&msg1).unwrap();
    action_ph.tot_msg_size.append(&msg2.write_to_new_cell().unwrap().into());

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

    // TODO: New fields in StorageInfo were added, so now worck incorrect
    //assert_eq!(shard_acc.unwrap().read_account().unwrap(), new_acc);
    assert_eq!(trans, good_trans);
}

fn create_transfer_ext_msg(src: AccountId, dest: AccountId, value: u64) -> Message {
    let mut hdr = ExternalInboundMessageHeader::default();
    hdr.dst = MsgAddressInt::with_standart(None, -1, src.clone()).unwrap();
    hdr.import_fee = Grams::zero();
    let mut msg = Message::with_ext_in_header(hdr);
    *msg.body_mut() = Some(create_transfer_int_header(src, dest, value).write_to_new_cell().unwrap().into());
    msg
}

fn create_transfer_int_header(src: AccountId, dest: AccountId, value: u64) -> InternalMessageHeader {
    let msg = create_int_msg(src, dest, value, true, 0);
    match msg.withdraw_header() {
        CommonMsgInfo::IntMsgInfo(int_hdr) => int_hdr,
        _ => panic!("must be internal message header"),
    }
}

fn create_wallet_data() -> Cell {
    //test public key
    BuilderData::with_raw(vec![0x00; 32], 256).unwrap().into()
}

fn create_wallet_code() -> Cell {
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
    compile_code(code).unwrap().into_cell()
}

#[test]
fn test_light_wallet_contract() {
    let contract_code = create_wallet_code();
    let contract_data = create_wallet_data();
    let acc1_id = AccountId::from_raw(vec![0x11; 32], 256);
    let acc2_id = AccountId::from_raw(vec![0x22; 32], 256);

    let gas_used1 = 1387;
    let gas_fee1 = gas_used1 * 10000;
    let gas_fee2 = 1000000; // flat_gas_price
    let start_balance1 = 1000000000;
    let start_balance2 = 500000000;
    let fwd_fee = 10000000;
    let storage_fee1 = 138234403;
    let storage_fee2 = 138234435;
    
    let acc1 = create_test_account(start_balance1.clone(), acc1_id.clone(), contract_code.clone(), contract_data.clone());
    let mut shard_acc1 = Some(ShardAccount::with_params(acc1.clone(), UInt256::from([0;32]), 0).unwrap());
    let acc2 = create_test_account(start_balance2, acc2_id.clone(), contract_code.clone(), contract_data.clone());
    let mut shard_acc2 = Some(ShardAccount::with_params(acc2, UInt256::from([0;32]), 0).unwrap());

    let config = BlockchainConfig::default();

    let transfer = 100000000;
    let in_msg = create_transfer_ext_msg(acc1_id, acc2_id, transfer);
    let block_ut = BLOCK_UT;
    let block_lt = 1u64;

    let mut executor = OrdinaryTransactionExecutor::new();
    let trans = executor.execute(
        &config, in_msg, &mut shard_acc1, block_ut.clone(), block_lt.clone(), true
    ).unwrap();
    let msg = trans.get_out_msg(0).unwrap().unwrap();
    println!("{:?}", msg);
    //new acc.balance = acc.balance - in_fwd_fee - transfer_value - storage_fees - gas_fee
    //transfer_value is reduced by fwd_fees:
    //new transfer_value = transfer_value - msg.fwd.fee
    let newbalance1 = start_balance1 - fwd_fee - transfer - storage_fee1 - gas_fee1;
    assert_eq!(shard_acc1.clone().unwrap().read_account().unwrap().get_balance().unwrap().clone(), CurrencyCollection::with_grams(newbalance1));
    assert_ne!(shard_acc1.clone().unwrap().last_trans_lt(), 0);
    assert_ne!(shard_acc1.unwrap().last_trans_hash(), &UInt256::from([0;32]));

    let mut executor = OrdinaryTransactionExecutor::new();
    let _trans = executor.execute(&config, msg, &mut shard_acc2, block_ut + 1, block_lt + 1, true).unwrap();

    //new acc.balance = acc.balance + transfer_value - fwd_fee - storage_fee - gas_fee
    let newbalance2 = start_balance2 + transfer - fwd_fee - storage_fee2 - gas_fee2;
    assert_eq!(shard_acc2.clone().unwrap().read_account().unwrap().get_balance().unwrap().clone(), CurrencyCollection::with_grams(newbalance2));
    assert_ne!(shard_acc2.clone().unwrap().last_trans_lt(), 0);
    assert_ne!(shard_acc2.unwrap().last_trans_hash(), &UInt256::from([0;32]));

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

    compile_code(&code).unwrap().into_cell()
}

fn create_test_transfer_account(amount: u64, mode: u8) -> Account {
    create_test_transfer_account_with_ending(amount, mode, "")
}

fn create_test_transfer_account_with_ending(amount: u64, mode: u8, ending: &str) -> Account {
    let acc_id = AccountId::from_raw(vec![0x11; 32], 256);

    let mut state = StateInit::default();
    state.set_code(test_transfer_code(mode, ending));
    Account::with_storage(
        &MsgAddressInt::with_standart(
            None, 
            -1, 
            acc_id
        ).unwrap(),
        &StorageInfo::with_values(
            ACCOUNT_UT,
            None,
        ),
        &AccountStorage {
            last_trans_lt: 0,
            balance: CurrencyCollection::with_grams(amount),
            state: AccountState::with_state(state),
        }
    )
}

fn create_test_external_msg_with_int(transfer_value: u64) -> Message {
    let acc_id = SENDER_ACCOUNT.clone();
    let mut hdr = ExternalInboundMessageHeader::default();
    hdr.dst = MsgAddressInt::with_standart(None, -1, acc_id.clone()).unwrap();
    hdr.import_fee = Grams::zero();
    let mut msg = Message::with_ext_in_header(hdr);

    let int_msg = create_int_msg(
        acc_id.clone(),
        RECEIVER_ACCOUNT.clone(),
        transfer_value,
        false,
        BLOCK_LT + 2
    );
    *msg.body_mut() = Some(int_msg.write_to_new_cell().unwrap().into());

    msg
}

#[test]
fn test_trexecutor_active_acc_with_code2() {
    let start_balance = 2000000000;
    let gas_used = 1170;
    let gas_fees = gas_used * 10000;
    let transfer = 50000000;
    let storage_fee = 78924597;
    let msg_mine_fee = 3333282;
    let msg_fwd_fee = 10000000;
    let msg_remain_fee = msg_fwd_fee - msg_mine_fee;

    let acc = create_test_transfer_account(start_balance.clone(), SENDMSG_ORDINARY);
    let old_acc = ShardAccount::with_params(acc.clone(), UInt256::from([0;32]), 0).unwrap();
    let config = BlockchainConfig::default();
    let mut new_acc = create_test_transfer_account(
        start_balance - (msg_fwd_fee + transfer + storage_fee + gas_fees), 0);
    let msg = create_test_external_msg_with_int(transfer);
    let block_lt = BLOCK_LT;
    let tr_lt = block_lt + 1;
    new_acc.set_last_tr_time(tr_lt);
    
    let mut executor = OrdinaryTransactionExecutor::new();
    let mut shard_acc = Some(old_acc.clone());
    let trans = executor.execute(
        &config, msg.clone(), &mut shard_acc, BLOCK_UT, block_lt.clone(), true
    ).unwrap();
    //println!("{:#?}", trans.read_description().unwrap());
    
    let mut good_trans = Transaction::with_account_and_message(&old_acc.read_account().unwrap(), &msg, tr_lt).unwrap();
    good_trans.set_now(BLOCK_UT);
    
    let msg1 = create_int_msg(
        AccountId::from_raw(vec![0x11; 32], 256),
        AccountId::from_raw(vec![0x22; 32], 256),
        transfer,
        false,
        BLOCK_LT + 2
    );
    let mut msg1_new_value = create_int_msg(
        AccountId::from_raw(vec![0x11; 32], 256),
        AccountId::from_raw(vec![0x22; 32], 256),
        transfer - msg_fwd_fee,
        false,
        BLOCK_LT + 2
    );
    if let CommonMsgInfo::IntMsgInfo(int_header) = msg1_new_value.header_mut() {
        int_header.fwd_fee = msg_remain_fee.into();
        int_header.created_at = BLOCK_UT.into();
    }

    good_trans.add_out_message(&msg1_new_value.clone()).unwrap();   
    good_trans.set_total_fees((msg_fwd_fee + storage_fee + gas_fees + msg_mine_fee).into());

    let old = Cell::from(old_acc.read_account().unwrap().write_to_new_cell().unwrap());
    let new = Cell::from(new_acc.write_to_new_cell().unwrap());
    good_trans.write_state_update(&HashUpdate::with_hashes(old.repr_hash(), new.repr_hash())).unwrap();
    
    let mut description = TransactionDescrOrdinary::default();
    description.storage_ph = Some(TrStoragePhase::with_params(storage_fee.into(), None, AccStatusChange::Unchanged));
    description.credit_ph = None;

    let mut vm_phase = TrComputePhaseVm::default();
    vm_phase.success = true;
    vm_phase.msg_state_used = false;
    vm_phase.account_activated = false;
    vm_phase.gas_used = VarUInteger7(gas_used.into());
    vm_phase.gas_limit = VarUInteger7(0);
    vm_phase.gas_credit = Some(10000.into());
    vm_phase.gas_fees = gas_fees.into();
    description.compute_ph = TrComputePhase::Vm(vm_phase);

    let mut action_ph = TrActionPhase::default();
    action_ph.success = true;
    action_ph.valid = true;
    action_ph.status_change = AccStatusChange::Unchanged;
    action_ph.tot_actions = 1;
    action_ph.msgs_created = 1;
    action_ph.total_fwd_fees = Some(msg_fwd_fee.into());
    action_ph.total_action_fees = Some(msg_mine_fee.into());
    action_ph.tot_msg_size = StorageUsedShort::calculate_for_struct(&msg1_new_value).unwrap();

    let mut actions = OutActions::default();
    actions.push_back(OutAction::new_send(SENDMSG_ORDINARY, Arc::new(msg1)));
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

    // TODO: New fields in StorageInfo were added, so now worck incorrect
    //assert_eq!(shard_acc.unwrap().read_account().unwrap(), new_acc);
    assert_eq!(trans, good_trans);
}

#[test]
fn test_trexecutor_active_acc_credit_first_false() {
    let start_balance = 1000000000;
    let acc = create_test_transfer_account(start_balance.clone(), SENDMSG_ORDINARY);

    let mut shard_acc = Some(ShardAccount::with_params(acc, UInt256::from([0;32]), 0).unwrap());
    let block_ut = BLOCK_UT;
    let msg = create_int_msg(
        AccountId::from_raw(vec![0x33; 32], 256),
        AccountId::from_raw(vec![0x11; 32], 256),
        50000000,
        true,
        0,
    );

    let config = BlockchainConfig::default();
    let mut executor = OrdinaryTransactionExecutor::new();
    let trans = executor.execute(&config, msg, &mut shard_acc, block_ut, 1, false).unwrap();
    assert_eq!(trans.read_description().unwrap().is_credit_first().unwrap(), false);
}

#[test]
fn test_trexecutor_active_acc_with_zero_balance() {
    let start_balance = 0;
    let acc = create_test_transfer_account(start_balance.clone(), SENDMSG_ORDINARY);
    let transfer = 1000000000;
    let storage_fee = 76796891;
    let gas_fee = 1000000; // flat_gas_price

    let mut shard_acc = Some(ShardAccount::with_params(acc, UInt256::from([0;32]), 0).unwrap());
    let block_ut = BLOCK_UT;
    let msg = create_int_msg(
        AccountId::from_raw(vec![0x33; 32], 256),
        AccountId::from_raw(vec![0x11; 32], 256),
        transfer,
        false,
        0,
    );

    let config = BlockchainConfig::default();
    let mut executor = OrdinaryTransactionExecutor::new();
    let trans = executor.execute(&config, msg, &mut shard_acc, block_ut, 1, false).unwrap();
    assert_eq!(trans.read_description().unwrap().is_aborted(), false);
    let vm_phase_success = trans.read_description().unwrap().compute_phase_ref().unwrap().clone().get_vmphase_mut().unwrap().success;
    assert_eq!(vm_phase_success, true);
    assert_eq!(
        shard_acc.unwrap().read_account().unwrap().get_balance().unwrap(),
        &CurrencyCollection::with_grams(transfer - storage_fee - gas_fee));
}

//contract send all its balance to another account using special mode in SENDRAWMSG.
//contract balance must equal to zero after transaction.
fn active_acc_send_all_balance(ending: &str) {
    let start_balance = 10_000_000_000; //10 grams
    let acc = create_test_transfer_account_with_ending(start_balance.clone(), SENDMSG_ALL_BALANCE, ending);

    let mut shard_acc = Some(ShardAccount::with_params(acc, UInt256::from(SENDER_ACCOUNT.get_bytestring(0)), 0).unwrap());
    let block_ut = BLOCK_UT;
    let msg = create_test_external_msg_with_int(start_balance);

    let config = BlockchainConfig::default();
    let mut executor = OrdinaryTransactionExecutor::new();
    let trans = executor.execute(&config, msg, &mut shard_acc, block_ut, 1, false).unwrap();
    assert_eq!(trans.read_description().unwrap().is_aborted(), false);
    let vm_phase_success = trans.read_description().unwrap().compute_phase_ref().unwrap().clone().get_vmphase_mut().unwrap().success;
    assert_eq!(vm_phase_success, true);
    assert_eq!(shard_acc.unwrap().read_account().unwrap().get_balance().unwrap(), &CurrencyCollection::with_grams(0));
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
fn test_build_ordinary_stack() {
    let acc_balance = 10_000_000;
    let msg_balance = 15_000;
    let acc_id = AccountId::from_raw(vec![0x22; 32], 256);
    let msg_int = create_int_msg(
        AccountId::from_raw(vec![0x11; 32], 256),
        acc_id.clone(),
        msg_balance,
        false,
        0,
    );
    let acc = create_test_account(acc_balance, acc_id.clone(), create_wallet_code(), create_wallet_data());

    let test_stack1 = build_ordinary_stack(&msg_int, &acc);

    let body_slice1: SliceData = msg_int.clone().body().unwrap_or_default();

    //stack for internal msg
    let mut etalon_stack1 = Stack::new();
    etalon_stack1
        .push(int!(acc_balance))
        .push(int!(msg_balance))
        .push(StackItem::Cell(msg_int.write_to_new_cell().unwrap().into()))
        .push(StackItem::Slice(body_slice1))
        .push(int!(0));

    assert_eq!(test_stack1, etalon_stack1);

    let msg_ext = create_test_external_msg();
    let test_stack2 = build_ordinary_stack(&msg_ext, &acc);

    let body_slice2: SliceData = msg_ext.clone().body().unwrap_or_default();

    //stack for external msg
    let mut etalon_stack2 = Stack::new();
    etalon_stack2
        .push(int!(acc_balance))
        .push(int!(0))
        .push(StackItem::Cell(msg_ext.write_to_new_cell().unwrap().into()))
        .push(StackItem::Slice(body_slice2))
        .push(int!(-1));

    assert_eq!(test_stack2, etalon_stack2);
}