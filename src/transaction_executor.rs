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


use num_traits::cast::ToPrimitive;
use std::{sync::{atomic::{AtomicU64}, Arc}};
use ton_block::{
    Serializable,
    Account,
    MsgAddressInt, Message,
    Transaction,
};
use ton_types::{Cell, Result};
use ton_vm::{
    smart_contract_info::SmartContractInfo,
    stack::Stack,
};



pub trait StackBuilder {

}

pub trait TransactionExecutor {
    fn execute(
        &self,
        in_msg: Option<&Message>,
        account_root: &mut Cell,
        block_unixtime: u32,
        block_lt: u64,
        last_tr_lt: Arc<AtomicU64>,
        debug: bool
    ) -> Result<Transaction>;
    fn build_contract_info(&self, acc: &Account, acc_address: &MsgAddressInt, block_unixtime: u32, block_lt: u64, tr_lt: u64) -> SmartContractInfo {
        let mut info = SmartContractInfo::with_myself(acc_address.write_to_new_cell().unwrap_or_default().into());
        *info.block_lt_mut() = block_lt;
        *info.trans_lt_mut() = tr_lt;
        *info.unix_time_mut() = block_unixtime;
        if let Some(balance) = acc.get_balance() {
            // info.set_remaining_balance(balance.grams.value().to_u128().unwrap_or_default(), balance.other.clone());
            *info.balance_remaining_grams_mut() = balance.grams.value().to_u128().unwrap_or_default();
            *info.balance_remaining_other_mut() = balance.other_as_hashmap();
        }
        info
    }
    fn build_stack(&self, in_msg: Option<&Message>, account: &Account) -> Stack;
}
