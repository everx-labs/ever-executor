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

use super::*;

mod common;
use common::*;
use std::sync::{atomic::AtomicU64, Arc};
use rand::prelude::Rng;
use rand::prelude::StdRng;
use ever_block::{
    accounts::Account,
    CurrencyCollection, ExternalInboundMessageHeader, Grams,
    InternalMessageHeader, MsgAddressInt, Serializable, StateInit, UnixTime32,
};
use ever_assembler::compile_code_to_cell;
use ever_block::{AccountId, SliceData, UInt256, BuilderData};
use std::panic;

fn gen_bool(rng: &mut StdRng) -> bool {
    rng.gen_bool(0.5)
}

fn gen_simple_int(rng: &mut StdRng, n: u64) -> u64 {
    rng.gen_range(0..n)
}

fn gen_int(rng: &mut StdRng, max_digits: u8) -> u64 {
    assert!(max_digits <= 19);
    let count_digits = rng.gen_range(0..max_digits + 1) as u32;
    if count_digits == max_digits as u32 {
        0
    } else {
        rng.gen_range(u64::pow(10, count_digits)..10 * u64::pow(10, count_digits))
    }
}

fn gen_states(state_num: u8, accept: bool, ihr_disabled: bool, src: &AccountId, w_id: i8) -> StateInit {
    match state_num {
        0 => StateInit::default(),
        1 => {
            let code = compile_code_to_cell(&(if accept { "ACCEPT " } else { "" }.to_string() + "
                NOP
            ")).unwrap();
            let mut state_correct = StateInit::default();
            state_correct.set_code(code);
            state_correct
        },
        2 => {
            let code = compile_code_to_cell(&(if accept { "ACCEPT " } else { "" }.to_string() + "
                PUSHROOT
                SENDRAWMSG
            ")).unwrap();
            let mut state_incorrect = StateInit::default();
            state_incorrect.set_code(code);
            state_incorrect
        },
        3 => {
            let code = compile_code_to_cell(&(if accept { "ACCEPT " } else { "" }.to_string() + "
                PUSHROOT
                PUSHINT 128
                SENDRAWMSG
            ")).unwrap();
            let mut state_send_msg = StateInit::default();
            state_send_msg.set_code(code);
            let mut out_msg = create_int_msg(src.clone(), [0; 32].into(), 100, false, 0);
            if let CommonMsgInfo::IntMsgInfo(header) = out_msg.header_mut() {
                header.ihr_disabled = ihr_disabled;
            } else {
                unreachable!()
            }
            out_msg.set_src_address(MsgAddressInt::with_standart(None, w_id, src.clone()).unwrap());
            state_send_msg.set_data(out_msg.serialize().unwrap());
            state_send_msg
        },
        4 => {
            let code = compile_code_to_cell(&(if accept { "ACCEPT " } else { "" }.to_string() + "
                PUSHROOT
                PUSHINT 0
                SENDRAWMSG
                PUSHINT 1
                SENDRAWMSG
            ")).unwrap();
            let mut state_send_two_msg = StateInit::default();
            state_send_two_msg.set_code(code);
            state_send_two_msg.set_data(create_two_messages_data_2(src.clone(), w_id));
            state_send_two_msg
        },
        5 => {
            let code = compile_code_to_cell(&(if accept { "ACCEPT " } else { "" }.to_string() + "
                PUSHINT 1000000000
                PUSHINT 0
                RAWRESERVE
                PUSHROOT
                CTOS
                LDREF
                PLDREF
                PUSHINT 0
                SENDRAWMSG
                PUSHINT 1
                SENDRAWMSG
            ")).unwrap();
            let mut state_rawreserve = StateInit::default();
            state_rawreserve.set_code(code);
            state_rawreserve.set_data(create_two_messages_data_2(src.clone(), w_id));
            state_rawreserve
        }
        _ => panic!("Incorrect value")
    }
}

fn gen_state(rng: &mut StdRng, src: &AccountId, w_id: i8) -> StateInit {
    gen_states(gen_simple_int(rng, 6) as u8, gen_bool(rng), gen_bool(rng), src, w_id)
}

#[test]
fn test_rand_1() {
    let mut rng = rand::SeedableRng::seed_from_u64(234235253453);

    let ordinary_acc_id = AccountId::from([0x11; 32]);
    let special_acc_id = AccountId::from([0x66u8; 32]);

    let blockchain_config = BLOCKCHAIN_CONFIG.to_owned();
    assert!(
        blockchain_config.is_special_account(
            &MsgAddressInt::with_standart(None, -1, special_acc_id.clone()).unwrap()
        ).unwrap()
    );

    for _iteration in 0..60_000 {
        let is_special = gen_bool(&mut rng);
        let acc_id = if !is_special {&ordinary_acc_id} else {&special_acc_id};
        let workchain_id = -(gen_simple_int(&mut rng, 2) as i8);

        let mut state_init: Option<StateInit> = None;
        let mut acc = match gen_simple_int(&mut rng, 4) {
            0 => {
                state_init = Some(gen_state(&mut rng, acc_id, workchain_id));
                Account::active_by_init_code_hash(
                    MsgAddressInt::with_standart(None, workchain_id, acc_id.clone()).unwrap(),
                    CurrencyCollection::with_grams(gen_int(&mut rng, 19)),
                    BLOCK_UT - std::cmp::min(gen_int(&mut rng, 11) as u32, BLOCK_UT),
                    state_init.clone().unwrap(),
                    gen_bool(&mut rng)
                ).unwrap()
            }
            1 => {
                Account::default()
            }
            2 => {
                Account::uninit(
                    MsgAddressInt::with_standart(None, workchain_id, acc_id.clone()).unwrap(),
                    BLOCK_LT,
                    BLOCK_UT - std::cmp::min(gen_int(&mut rng, 11) as u32, BLOCK_UT),
                    CurrencyCollection::with_grams(gen_int(&mut rng, 19))
                )
            }
            3 => {
                state_init = Some(gen_state(&mut rng, acc_id, workchain_id));
                let state_hash = state_init.clone().unwrap().serialize().unwrap().repr_hash();
                let due = Grams::from(gen_int(&mut rng, 19));
                Account::frozen(
                    MsgAddressInt::with_standart(None, workchain_id, acc_id.clone()).unwrap(),
                    BLOCK_LT,
                    BLOCK_UT - std::cmp::min(gen_int(&mut rng, 11) as u32, BLOCK_UT),
                    if gen_bool(&mut rng) {state_hash} else {UInt256::default()},
                    if due.is_zero() {None} else {Some(due)},
                    CurrencyCollection::with_grams(gen_int(&mut rng, 19))
                )
            }
            _ => panic!("Incorrect value")
        };

        let mut msg = if gen_bool(&mut rng) {
            let dst = MsgAddressInt::with_standart(None, workchain_id, acc_id.clone()).unwrap();
            let hdr = ExternalInboundMessageHeader::new(Default::default(), dst);
            Message::with_ext_in_header_and_body(hdr, SliceData::default())
        } else {
            let workchain_id_sender = -(gen_simple_int(&mut rng, 2) as i8);
            let mut hdr = InternalMessageHeader::with_addresses(
                MsgAddressInt::with_standart(None, workchain_id_sender, AccountId::from([0x33; 32])).unwrap(),
                MsgAddressInt::with_standart(None, workchain_id, acc_id.clone()).unwrap(),
                CurrencyCollection::with_grams(gen_int(&mut rng, 19)),
            );
            hdr.bounce = gen_bool(&mut rng);
            hdr.ihr_disabled = gen_bool(&mut rng);
            hdr.created_lt = PREV_BLOCK_LT;
            hdr.created_at = UnixTime32::default();
            hdr.fwd_fee = Grams::from(gen_int(&mut rng, 10));
            hdr.ihr_fee = Grams::from(gen_int(&mut rng, 10));
            Message::with_int_header(hdr)
        };

        if gen_bool(&mut rng) {
            msg.set_state_init(gen_state(&mut rng, acc_id, workchain_id).clone());
        } else if let Some(state_init) = state_init {
            msg.set_state_init(state_init);
        }

        let tr_lt = BLOCK_LT + 1;
        let lt = Arc::new(AtomicU64::new(tr_lt));

        let acc_copy = acc.clone();
        let executor = OrdinaryTransactionExecutor::new(blockchain_config.clone());
        let trans = executor.execute_with_params(Some(&make_common(msg.clone())), &mut acc, execute_params_none(lt));
        if cfg!(feature = "cross_check") {
            let mut skip_cross_check = false;

            // init_code_hash is not supported in cpp node
            if acc_copy.init_code_hash().is_some() {
                skip_cross_check = true;
            }

            if !skip_cross_check {
                cross_check::cross_check(
                    &blockchain_config, 
                    &acc_copy, 
                    &acc, 
                    Some(&msg), 
                    trans.as_ref().ok(), 
                    &execute_params_none(Arc::new(AtomicU64::new(tr_lt))), 
                    _iteration
                )
            }
        }
        if let Ok(trans) = &trans {
            assert_ne!(trans.read_description().unwrap().action_phase_ref().map_or(0, |ap| ap.result_code), 35);
        }

        check_account_and_transaction_balances(&acc_copy, &acc, &msg, trans.as_ref().ok());
    }
}

fn gen_reserve_code(value: u64, t: u8) -> String {
    format!(
        "PUSHINT {}
        PUSHINT {}
        RAWRESERVE
        ",
        value, t
    )
}

fn gen_message_code(t: u8) -> String {
    format!(
        "PUSHINT {}
        SENDRAWMSG
        ",
        t
    )
}

fn gen_messages_code(rng: &mut StdRng, count_msgs: u8) -> String {
    let mut str = "".to_string();
    for _i in 0..count_msgs {
        let mut flags= 0;
        flags += match gen_simple_int(rng, 3) {
            0 => 0,
            1 => 64,
            2 => 128,
            _ => panic!("Incorrect value")
        };
        flags += if gen_bool(rng) {1} else {0};
        flags += if gen_bool(rng) {2} else {0};
        flags += if gen_bool(rng) {32} else {0};

        flags += if gen_simple_int(rng, 50) == 0 {21} else {0};

        str += &*(gen_message_code(flags));
    };
    str
}

fn gen_state_reserve_and_sendmsgs(rng: &mut StdRng, src: &AccountId, w_id: i8) -> StateInit {
    let mut code = "".to_string();

    if gen_bool(rng) {
        code += "ACCEPT
        "
    }

    code += "PUSHROOT
    ";

    let msgs_before = gen_simple_int(rng, 10) as u8;
    let msgs_after = gen_simple_int(rng, 10) as u8;
    let msgs_all = msgs_after + msgs_before;
    if msgs_all > 0 {
        code += &*"CTOS
            LDREF
            PLDREF
            ".repeat((msgs_all - 1) as usize);
        code += "
            CTOS
            PLDREF
        ";
    }

    code += &*gen_messages_code(rng, msgs_before);

    if gen_simple_int(rng, 10) != 0 {
        let reserve_flags = gen_simple_int(rng, 17) as u8;
        code += &*gen_reserve_code(gen_int(rng, 19), reserve_flags);
    }

    code += &*gen_messages_code(rng, msgs_after);

    let mut b = BuilderData::with_raw(vec![0x55; 32], 256).unwrap();
    let mut fst = true;
    for _i in 0..msgs_all {
        let cell_b = b.into_cell().unwrap();
        b = BuilderData::with_raw(vec![0x55; 32], 256).unwrap();

        let mut msg1 = create_int_msg(
            AccountId::from([0x11; 32]),
            AccountId::from([0x33; 32]),
            gen_int(rng, 19),
            false,
            BLOCK_LT + 2
        );
        msg1.set_src_address(MsgAddressInt::with_standart(None, w_id, src.clone()).unwrap());

        b.checked_append_reference(msg1.serialize().unwrap()).unwrap();
        if !fst {
            b.checked_append_reference(cell_b).unwrap();
        }
        fst = false;
    }

    let mut state_rawreserve = StateInit::default();
    state_rawreserve.set_code(compile_code_to_cell(&code).unwrap());
    state_rawreserve.set_data(b.into_cell().unwrap());
    state_rawreserve
}

#[test]
fn test_rand_reverse_and_messages() {
    let mut rng = rand::SeedableRng::seed_from_u64(234235253453);

    let ordinary_acc_id = AccountId::from([0x11; 32]);
    let special_acc_id = AccountId::from([0x66u8; 32]);

    let blockchain_config = BLOCKCHAIN_CONFIG.to_owned();
    assert!(
        blockchain_config.is_special_account(
            &MsgAddressInt::with_standart(None, -1, special_acc_id.clone()).unwrap()
        ).unwrap()
    );

    for _iteration in 0..50_000 {
        let is_special = gen_bool(&mut rng);
        let acc_id = if !is_special {&ordinary_acc_id} else {&special_acc_id};
        let workchain_id = -(gen_simple_int(&mut rng, 2) as i8);

        let mut acc = Account::active_by_init_code_hash(
            MsgAddressInt::with_standart(None, workchain_id, acc_id.clone()).unwrap(),
            CurrencyCollection::with_grams(gen_int(&mut rng, 19)),
            BLOCK_UT,
            gen_state_reserve_and_sendmsgs(&mut rng, acc_id, workchain_id),
            false
        ).unwrap();

        let msg = if gen_bool(&mut rng) {
            let hdr = ExternalInboundMessageHeader::new(
                Default::default(),
                MsgAddressInt::with_standart(None, workchain_id, acc_id.clone()).unwrap(),
            );
            Message::with_ext_in_header_and_body(hdr, SliceData::default())
        } else {
            let mut hdr = InternalMessageHeader::with_addresses(
                MsgAddressInt::with_standart(None, workchain_id, AccountId::from([0x33; 32])).unwrap(),
                MsgAddressInt::with_standart(None, workchain_id, acc_id.clone()).unwrap(),
                CurrencyCollection::with_grams(gen_int(&mut rng, 19)),
            );
            hdr.bounce = gen_bool(&mut rng);
            hdr.ihr_disabled = true;
            hdr.created_lt = PREV_BLOCK_LT;
            hdr.created_at = UnixTime32::default();
            Message::with_int_header(hdr)
        };

        let tr_lt = BLOCK_LT + 1;
        let lt = Arc::new(AtomicU64::new(tr_lt));

        let acc_copy = acc.clone();
        let executor = OrdinaryTransactionExecutor::new(blockchain_config.clone());
        let trans = executor.execute_with_params(Some(&make_common(msg.clone())), &mut acc, execute_params_none(lt));
        if cfg!(feature = "cross_check") {
            cross_check::cross_check(
                &blockchain_config, 
                &acc_copy, 
                &acc, 
                Some(&msg), 
                trans.as_ref().ok(), 
                &execute_params_none(Arc::new(AtomicU64::new(tr_lt))), 
                _iteration
            )
        }
        if let Ok(trans) = &trans {
            assert_ne!(trans.read_description().unwrap().action_phase_ref().map_or(0, |ap| ap.result_code), 35);
        }

        check_account_and_transaction_balances(&acc_copy, &acc, &msg, trans.as_ref().ok());
    }
}
