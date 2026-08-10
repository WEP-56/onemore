use serde::{Deserialize, Serialize};

use crate::sdk::{
    ApprovalDecisionView, ModelMetadata, ServerInfo, SessionEvent, SessionSnapshot,
    SessionSummaryView,
};

#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub(crate) enum ClientMessage {
    Hello { version: u32 },
    Request { id: String, request: RequestCommand },
}

#[derive(Debug, Deserialize)]
#[serde(tag = "command", rename_all = "snake_case", deny_unknown_fields)]
pub(crate) enum RequestCommand {
    Prompt {
        text: String,
    },
    Steer {
        text: String,
    },
    FollowUp {
        text: String,
    },
    Abort,
    Compact,
    SetModel {
        provider: String,
        model: String,
        effort: String,
    },
    ClearConversation,
    ListSessions {
        #[serde(default)]
        all: bool,
    },
    LoadSession {
        session_id: String,
    },
    ListModels,
    GetSnapshot,
    ApprovalResponse {
        request_id: String,
        decision: ApprovalDecisionView,
    },
    Shutdown,
}

#[derive(Debug, Serialize)]
pub(crate) struct HelloResponse {
    #[serde(rename = "type")]
    kind: &'static str,
    version: u32,
    server: ServerInfo,
    snapshot: SessionSnapshot,
}

impl HelloResponse {
    pub(crate) fn new(server: ServerInfo, snapshot: SessionSnapshot) -> Self {
        HelloResponse {
            kind: "hello",
            version: crate::sdk::PROTOCOL_VERSION,
            server,
            snapshot,
        }
    }
}

#[derive(Debug, Serialize)]
pub(crate) struct HelloError {
    #[serde(rename = "type")]
    kind: &'static str,
    error: RpcError,
}

impl HelloError {
    pub(crate) fn new(code: impl Into<String>, message: impl Into<String>) -> Self {
        HelloError {
            kind: "hello_error",
            error: RpcError::new(code, message),
        }
    }
}

#[derive(Debug, Serialize)]
pub(crate) struct SuccessResponse {
    #[serde(rename = "type")]
    kind: &'static str,
    id: String,
    ok: bool,
    result: ResponseResult,
}

impl SuccessResponse {
    pub(crate) fn new(id: String, result: ResponseResult) -> Self {
        SuccessResponse {
            kind: "response",
            id,
            ok: true,
            result,
        }
    }
}

#[derive(Debug, Serialize)]
pub(crate) struct ErrorResponse {
    #[serde(rename = "type")]
    kind: &'static str,
    id: String,
    ok: bool,
    error: RpcError,
}

impl ErrorResponse {
    pub(crate) fn new(id: String, code: impl Into<String>, message: impl Into<String>) -> Self {
        ErrorResponse {
            kind: "response",
            id,
            ok: false,
            error: RpcError::new(code, message),
        }
    }
}

#[derive(Debug, Serialize)]
pub(crate) struct ProtocolErrorEnvelope {
    #[serde(rename = "type")]
    kind: &'static str,
    error: RpcError,
}

impl ProtocolErrorEnvelope {
    pub(crate) fn new(code: impl Into<String>, message: impl Into<String>) -> Self {
        ProtocolErrorEnvelope {
            kind: "protocol_error",
            error: RpcError::new(code, message),
        }
    }
}

#[derive(Debug, Serialize)]
pub(crate) struct EventEnvelope {
    #[serde(rename = "type")]
    kind: &'static str,
    event: SessionEvent,
}

impl EventEnvelope {
    pub(crate) fn new(event: SessionEvent) -> Self {
        EventEnvelope {
            kind: "event",
            event,
        }
    }
}

#[derive(Debug, Serialize)]
#[serde(tag = "command", rename_all = "snake_case")]
pub(crate) enum ResponseResult {
    Prompt { command_id: String },
    Steer { command_id: String },
    FollowUp { command_id: String },
    Abort { command_id: String },
    Compact { command_id: String },
    SetModel { command_id: String },
    ClearConversation { command_id: String },
    ListSessions { sessions: Vec<SessionSummaryView> },
    LoadSession { command_id: String },
    ListModels { models: Vec<ModelMetadata> },
    GetSnapshot { snapshot: Box<SessionSnapshot> },
    ApprovalResponse,
    Shutdown { command_id: String },
}

#[derive(Debug, Serialize)]
struct RpcError {
    code: String,
    message: String,
}

impl RpcError {
    fn new(code: impl Into<String>, message: impl Into<String>) -> Self {
        RpcError {
            code: code.into(),
            message: message.into(),
        }
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn commands_reject_unknown_fields() {
        let error = serde_json::from_value::<ClientMessage>(json!({
            "type": "request",
            "id": "req-1",
            "request": {"command": "prompt", "text": "hi", "extra": true}
        }))
        .unwrap_err();
        assert!(error.to_string().contains("unknown field"));
    }

    #[test]
    fn envelopes_reject_coerced_versions_and_unknown_commands() {
        assert!(serde_json::from_value::<ClientMessage>(json!({
            "type": "hello",
            "version": "2"
        }))
        .is_err());
        assert!(serde_json::from_value::<ClientMessage>(json!({
            "type": "request",
            "id": "req-1",
            "request": {"command": "bash", "command_line": "echo no"}
        }))
        .is_err());
    }

    #[test]
    fn list_sessions_scope_defaults_to_current_workspace() {
        let current = serde_json::from_value::<ClientMessage>(json!({
            "type": "request",
            "id": "req-current",
            "request": {"command": "list_sessions"}
        }))
        .unwrap();
        assert!(matches!(
            current,
            ClientMessage::Request {
                request: RequestCommand::ListSessions { all: false },
                ..
            }
        ));

        let all = serde_json::from_value::<ClientMessage>(json!({
            "type": "request",
            "id": "req-all",
            "request": {"command": "list_sessions", "all": true}
        }))
        .unwrap();
        assert!(matches!(
            all,
            ClientMessage::Request {
                request: RequestCommand::ListSessions { all: true },
                ..
            }
        ));
    }

    #[test]
    fn session_list_response_includes_workspace() {
        let value = serde_json::to_value(ResponseResult::ListSessions {
            sessions: vec![SessionSummaryView {
                id: "session".into(),
                title: "title".into(),
                workspace: "E:\\project".into(),
                message_count: 3,
                updated_at: 4,
            }],
        })
        .unwrap();
        assert_eq!(value["sessions"][0]["workspace"], "E:\\project");
    }
}
