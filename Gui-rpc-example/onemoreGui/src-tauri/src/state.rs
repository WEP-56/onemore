use std::collections::HashMap;
use std::sync::Mutex;

use crate::rpc::process::RpcHandle;

/// 全局 RPC 状态：每个桌面任务持有独立 RPC 子进程，切换任务不会结束后台运行。
#[derive(Default)]
pub struct RpcState {
    pub inner: Mutex<HashMap<String, RpcHandle>>,
}
