use serde::Serialize;

pub const EVENT_NAME: &str = "onemore://rpc-event";

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum RpcEvent {
    Hello {
        connection_id: String,
        server: serde_json::Value,
        snapshot: serde_json::Value,
    },
    Event {
        connection_id: String,
        event: serde_json::Value,
    },
    Stderr {
        connection_id: String,
        line: String,
    },
    ProcessExit {
        connection_id: String,
        code: Option<i32>,
    },
    TransportError {
        connection_id: String,
        code: String,
        message: String,
    },
}
