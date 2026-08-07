use std::sync::Mutex;

use crate::rpc::process::RpcHandle;

/// 全局 RPC 状态：一个 app window 至多一个 RPC 子进程。
#[derive(Default)]
pub struct RpcState {
    pub inner: Mutex<Option<RpcHandle>>,
}
