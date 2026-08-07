//! 前端看到的 GUI 事件流（封闭 DTO）：协议数据原样透传，transport 错误/进程退出/
//! stderr 诊断使用 GUI 自己的形状，不伪装成 SessionEvent。

use serde::Serialize;

pub const EVENT_NAME: &str = "onemore://rpc-event";

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum RpcEvent {
    /// hello 成功：server info + 初始 snapshot（协议数据）
    Hello {
        server: serde_json::Value,
        snapshot: serde_json::Value,
    },
    /// 协议事件帧（session_snapshot / progress / command_finished / settled）
    Event { event: serde_json::Value },
    /// stderr 诊断行
    Stderr { line: String },
    /// 子进程退出（正常或异常）
    ProcessExit { code: Option<i32> },
    /// transport 级错误（读取失败、非法帧、重复 response、版本不匹配等）
    TransportError { code: String, message: String },
}
