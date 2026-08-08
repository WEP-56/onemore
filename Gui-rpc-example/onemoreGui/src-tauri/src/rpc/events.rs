use serde::Serialize;

pub const EVENT_NAME: &str = "onemore://rpc-event";

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum RpcEvent {
    Hello {
        server: serde_json::Value,
        snapshot: serde_json::Value,
    },
    Event { event: serde_json::Value },
    Stderr { line: String },
    ProcessExit { code: Option<i32> },
    TransportError { code: String, message: String },
}
