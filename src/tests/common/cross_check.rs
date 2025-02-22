/*
* Copyright 2018-2020 EVERX DEV SOLUTIONS LTD.
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

use super::*;
use std::sync::Mutex;
use lazy_static::lazy_static;
use pretty_assertions::assert_eq;
use std::collections::HashSet;
use std::thread::ThreadId;
use ever_block::{
    accounts::Account, messages::Message, transactions::Transaction, Deserializable, Serializable,
};
use ever_block::read_single_root_boc;
use std::thread;
use core::sync::atomic::Ordering;
use crate::BlockchainConfig;
use crate::ExecuteParams;

lazy_static! {
    static ref DISABLED_TESTS: Mutex<HashSet<ThreadId>> = Mutex::new(HashSet::<ThreadId>::default());
    static ref DISABLED_TESTS_SCOPE: Mutex<HashSet<ThreadId>> = Mutex::new(HashSet::<ThreadId>::default());
}

pub(crate) fn disable_cross_check() {
    let thread_id = thread::current().id();
    DISABLED_TESTS.lock().unwrap().insert(thread_id);
}

pub struct DisableCrossCheck {
    thread_id: ThreadId
}

impl DisableCrossCheck {
    pub fn new() -> DisableCrossCheck {
        let thread_id = thread::current().id();
        DISABLED_TESTS_SCOPE.lock().unwrap().insert(thread_id);
        DisableCrossCheck { thread_id }
    }
}

impl Drop for DisableCrossCheck {
    fn drop(&mut self) {
        DISABLED_TESTS.lock().unwrap().remove(&self.thread_id);
    }
}

fn load_cells(data: &[u8]) -> Cell {
    read_single_root_boc(data).unwrap()
}

pub(crate) fn cross_check(config: &BlockchainConfig, acc_before: &Account, acc_after: &Account, msg: Option<&Message>, transaction: Option<&Transaction>, params: &ExecuteParams, _it: u32) {
    let thread_id = thread::current().id();

    if DISABLED_TESTS.lock().unwrap().remove(&thread_id) || DISABLED_TESTS_SCOPE.lock().unwrap().contains(&thread_id) {
        return;
    }

    let lib_name = "libtransaction-replayer-lib-dyn.so";
    let lib = libloading::Library::new(lib_name).unwrap();

    let acc_data = acc_before.write_to_bytes().unwrap();
    let cfg_data = config.raw_config().write_to_bytes().unwrap();

    let mut res_acc_size: i32 = 500000;
    let mut res_tx_size: i32 = 500000;
    let mut res_acc = vec![0u8; res_acc_size as usize];
    let mut res_tx = vec![0u8; res_tx_size as usize];

    if let Some(msg) = msg {
        let msg_data = msg.write_to_bytes().unwrap();

        unsafe {
            type RunBoc<'a> = libloading::Symbol<'a,
                unsafe extern "C" fn(*const u8, i32, *const u8, i32, *const u8, i32, u64, i32, u64, *mut u8, &mut i32, *mut u8, &mut i32) -> bool
            >;
            let run_boc: RunBoc = lib.get(b"replay_ordinary_transaction_ext").unwrap();

            let res: bool = run_boc(
                acc_data.as_ptr(), acc_data.len() as i32,
                msg_data.as_ptr(), msg_data.len() as i32,
                cfg_data.as_ptr(), cfg_data.len() as i32,
                params.last_tr_lt.load(Ordering::Relaxed), params.block_unixtime as i32, params.block_lt,
                res_acc.as_mut_ptr(), &mut res_acc_size, res_tx.as_mut_ptr(), &mut res_tx_size
            );
            assert!(res, "check preallocated size for output data");
        }
    } else {
        let tick = if let TransactionDescr::TickTock(descr) = transaction.unwrap().read_description().unwrap() {
            descr.tt.is_tick()
        } else {
            unreachable!();
        };
        unsafe {
            type RunBoc<'a> = libloading::Symbol<'a,
                unsafe extern "C" fn(*const u8, i32, *const u8, i32, u64, i32, u64, bool, *mut u8, &mut i32, *mut u8, &mut i32) -> bool
            >;
            let run_boc: RunBoc = lib.get(b"replay_ticktock_transaction_ext").unwrap();

            let res: bool = run_boc(
                acc_data.as_ptr(), acc_data.len() as i32,
                cfg_data.as_ptr(), cfg_data.len() as i32,
                params.last_tr_lt.load(Ordering::Relaxed), params.block_unixtime as i32, params.block_lt, tick,
                res_acc.as_mut_ptr(), &mut res_acc_size, res_tx.as_mut_ptr(), &mut res_tx_size
            );
            assert!(res, "check preallocated size for output data");
        }
    }

    if res_tx_size != 0 {
        let acc_file_cells = load_cells(&res_acc[0..res_acc_size as usize]);
        let tx_file_cells = load_cells(&res_tx[0..res_tx_size as usize]);

        let mut acc_file = Account::construct_from_cell(acc_file_cells.clone()).unwrap();
        let tx_file = Transaction::construct_from_cell(tx_file_cells.clone()).unwrap();

        let mut acc_after_copy = acc_after.clone();
        acc_after_copy.update_storage_stat().unwrap();
        acc_file.update_storage_stat().unwrap();
        assert!(transaction.is_some());
        let mut transaction = transaction.unwrap().clone();
        transaction.write_state_update(&tx_file.read_state_update().unwrap()).unwrap();

        /*if acc_after_copy != acc_file {
            println!("Iteration {}", _it);
        }*/
        assert_eq!(transaction.read_description().unwrap(), tx_file.read_description().unwrap());
        assert_eq!(acc_after_copy, acc_file);
        transaction.out_msgs.scan_diff(&tx_file.out_msgs, |key: ever_block::U15, msg1, msg2| {
            assert_eq!(msg1, msg2, "for key {}", key.0);
            Ok(true)
        }).unwrap();
        assert_eq!(transaction, tx_file);

        assert_eq!(acc_after_copy.serialize().unwrap(), acc_file_cells);
        assert_eq!(transaction.serialize().unwrap(), tx_file_cells);
    } else {
        assert!(transaction.is_none());
    }
}
