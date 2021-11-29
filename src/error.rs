/*
* Copyright 2018-2020 TON DEV SOLUTIONS LTD.
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

use ton_types::types::ExceptionCode;
use ton_block::ComputeSkipReason;
use ton_vm::stack::StackItem;

#[derive(Debug, failure::Fail, PartialEq)]
pub enum ExecutorError {   
    #[fail(display = "Invalid external message")]
    InvalidExtMessage,
    #[fail(display = "Transaction executor internal error: {}", 0)]
    TrExecutorError(String),
    #[fail(display = "VM Exception, code: {}", 0)]
    TvmExceptionCode(ExceptionCode),
    #[fail(display = "Contract did not accept message, exit code: {}", 0)]
    NoAcceptError(i32, Option<StackItem>),
    #[fail(display = "Cannot pay for importing this external message")]
    NoFundsToImportMsg,
    #[fail(display = "Compute phase skipped while processing exteranl inbound messagewith reason {:?}", 0)]
    ExtMsgComputeSkipped(ComputeSkipReason)
}
