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
    messages::CommonMsgInfo,
    out_actions::{OutAction, OutActions, RESERVE_EXACTLY, SENDMSG_ALL_BALANCE, SENDMSG_ORDINARY},
    transactions::{
        AccStatusChange, TrActionPhase, TrComputePhase, TrComputePhaseVm, TrCreditPhase,
        TrStoragePhase, Transaction, TransactionDescr,
    },
    CurrencyCollection, GetRepresentationHash, Grams, Serializable, StorageUsedShort,
};
use ever_assembler::compile_code_to_cell;
use ever_block::AccountId;

#[test]
fn test_trexecutor_active_acc_with_rawreserve_and_sendmsg() {
    let used = 1864u32; //gas units
    let storage_fees = 293158000;
    let msg_mine_fee = MSG_MINE_FEE;
    let msg_fwd_fee = MSG_FWD_FEE;
    let msg_remain_fee = msg_fwd_fee - msg_mine_fee;
    let gas_fees = used as u64 * 10000;
    let reserve = 133000000;

    let acc_id = AccountId::from([0x11; 32]);
    let start_balance = 310000000u64;
    let msg_income = 1230000000u64;
    let code = compile_code_to_cell(&format!("
        PUSHINT {}
        PUSHINT 0
        RAWRESERVE
        PUSHROOT
        CTOS
        LDREF
        PLDREF
        PUSHINT 0
        SENDRAWMSG
        PUSHINT 128
        SENDRAWMSG
    ", reserve)).unwrap();
    let mut acc = create_test_account(start_balance, acc_id.clone(), code.clone(), create_two_messages_data());
    let end_balance = reserve;
    let mut new_acc = create_test_account(end_balance, acc_id, code, create_two_messages_data());
    let msg = create_int_msg(
        AccountId::from([0x22; 32]),
        AccountId::from([0x11; 32]),
        msg_income,
        true,
        BLOCK_LT - 1_000_000 + 1,
    );
    let tr_lt = BLOCK_LT + 1;
    new_acc.set_last_tr_time(tr_lt + 3);
    new_acc.set_last_paid(BLOCK_UT);
    let trans = execute_c(&msg, &mut acc, tr_lt, new_acc.balance().unwrap().grams, 2).unwrap();
    acc.update_storage_stat().unwrap();

    assert_eq!(acc, new_acc);
    let mut good_trans = Transaction::with_account_and_message(&acc, &msg, tr_lt).unwrap();
    good_trans.set_now(BLOCK_UT);

    let (mut msg1, mut msg2) = create_two_internal_messages();
    let mut actions = OutActions::default();
    actions.push_back(OutAction::new_reserve(RESERVE_EXACTLY, CurrencyCollection::with_grams(reserve)));
    actions.push_back(OutAction::new_send(SENDMSG_ORDINARY, msg1.clone()));
    actions.push_back(OutAction::new_send(SENDMSG_ALL_BALANCE, msg2.clone()));
    if let CommonMsgInfo::IntMsgInfo(int_header) = msg1.header_mut() {
        if let CommonMsgInfo::IntMsgInfo(int_header2) = msg2.header_mut() {
            int_header.value.grams = Grams::from(MSG1_BALANCE - msg_fwd_fee);
            int_header2.value.grams = Grams::from(start_balance + msg_income - reserve - MSG1_BALANCE - msg_fwd_fee - gas_fees - storage_fees);
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
    vm_phase.gas_limit = 123000u32.into();
    vm_phase.gas_credit = None;
    vm_phase.gas_fees = gas_fees.into();
    vm_phase.vm_steps = 12;
    description.compute_ph = TrComputePhase::Vm(vm_phase);

    let mut action_ph = TrActionPhase::default();
    action_ph.success = true;
    action_ph.valid = true;
    action_ph.status_change = AccStatusChange::Unchanged;
    action_ph.tot_actions = 3;
    action_ph.spec_actions = 1;
    action_ph.msgs_created = 2;
    action_ph.add_fwd_fees((2 * msg_fwd_fee).into());
    action_ph.add_action_fees((2 * msg_mine_fee).into());
    action_ph.action_list_hash = actions.hash().unwrap();
    action_ph.tot_msg_size = StorageUsedShort::calculate_for_struct(&msg1).unwrap();
    action_ph.tot_msg_size.append(&msg2.serialize().unwrap());

    description.action = Some(action_ph);
    description.credit_first = false;
    description.bounce = None;
    description.aborted = false;
    description.destroyed = false;
    good_trans.write_description(&TransactionDescr::Ordinary(description)).unwrap();

    assert_eq!(
        trans.read_description().unwrap().compute_phase_ref().unwrap().clone().get_vmphase_mut().unwrap().gas_used,
        good_trans.read_description().unwrap().compute_phase_ref().unwrap().clone().get_vmphase_mut().unwrap().gas_used);

    assert_eq!(trans.read_description().unwrap(), good_trans.read_description().unwrap());

    trans.out_msgs.scan_diff(&good_trans.out_msgs, |key: ever_block::U15, msg1, msg2| {
        assert_eq!(msg1, msg2, "for key {}", key.0);
        Ok(true)
    }).unwrap();
    assert_eq!(trans, good_trans);
}

struct MsgFlag {
    f: u64
}

fn execute_rawreserve_transaction(start_balance: u64, reserve: u64, r_type: u64, msg_flag: MsgFlag, result_account_balance: u64, count_out_msgs: usize) {
    let code = &format!("
        PUSHINT {}
        PUSHINT {}
        RAWRESERVE
        PUSHROOT
        CTOS
        LDREF
        PLDREF
        PUSHINT 0
        SENDRAWMSG
        PUSHINT {}
        SENDRAWMSG
    ", reserve, r_type, msg_flag.f);
    let data = create_two_messages_data();
    execute_custom_transaction(start_balance, code, data, 41_000_000, false, result_account_balance, count_out_msgs);
}

#[test]
fn test_send_rawreserve_messages() {
    execute_rawreserve_transaction(1_502_586_890, 1_080_012_743, 0, MsgFlag{f: 0}, 1_083_012_743, 2);
    execute_rawreserve_transaction(1_502_586_890, 1_083_012_743, 0, MsgFlag{f: 0}, 1_083_012_743, 2);
    execute_rawreserve_transaction(1_502_586_890, 1_084_012_743, 0, MsgFlag{f: 0}, 1_233_012_743, 0);
    execute_rawreserve_transaction(1_502_586_890, 1_000_000_000, 0, MsgFlag{f: 128}, 1_000_000_000, 2);
    execute_rawreserve_transaction(1_502_586_890, 1_084_012_743, 0, MsgFlag{f: 128}, 1_084_012_743, 2);
    execute_rawreserve_transaction(1_502_586_890, 1_000_000_000, 0, MsgFlag{f: 64}, 1_059_960_816, 2);
    execute_rawreserve_transaction(1_502_586_890, 1_104_012_743, 0, MsgFlag{f: 64}, 1_232_400_816, 0);

    execute_rawreserve_transaction(1_502_586_890, 1_084_012_743, 0, MsgFlag{f: 2}, 1_183_012_743, 1);
    execute_rawreserve_transaction(1_502_586_890, 1_104_012_743, 0, MsgFlag{f: 64+2}, 1_182_400_816, 1);

    execute_rawreserve_transaction(530_000_000 - 40_000_000, 200_000_000, 1, MsgFlag{f: 0}, 70_425_853, 2);
    execute_rawreserve_transaction(530_000_000 - 40_000_000, 200_000_000, 0, MsgFlag{f: 0}, 220_425_853, 0);

    execute_rawreserve_transaction(630_000_000 - 40_000_000, 100_000_000, 2, MsgFlag{f: 0}, 170_425_853, 2);
    execute_rawreserve_transaction(630_000_000 - 40_000_000, 400_000_000, 2, MsgFlag{f: 0}, 320_425_853, 0);
    execute_rawreserve_transaction(630_000_000 - 40_000_000, 100_000_000, 0, MsgFlag{f: 0}, 170_425_853, 2);

    execute_rawreserve_transaction(1_502_586_890, 3_084_012_743, 13, MsgFlag{f: 0}, 1_232_400_816, 0);

    execute_rawreserve_transaction(630_000_000, 300_000_000, 12, MsgFlag{f: 0}, 209_813_926, 2);
    execute_rawreserve_transaction(630_000_000, 300_000_000, 0, MsgFlag{f: 0}, 360_425_853, 0);
}
