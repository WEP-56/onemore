use serde::Deserialize;

/// 入站帧（stdout JSONL），严格按协议 tag 判别，拒绝未知字段。
#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum InboundFrame {
    Hello {
        version: u32,
        server: serde_json::Value,
        snapshot: serde_json::Value,
    },
    HelloError {
        error: ProtoError,
    },
    Response {
        id: String,
        ok: bool,
        #[serde(default)]
        result: Option<serde_json::Value>,
        #[serde(default)]
        error: Option<ProtoError>,
    },
    Event {
        event: serde_json::Value,
    },
}

#[derive(Debug, Clone, Deserialize)]
pub struct ProtoError {
    pub code: String,
    pub message: String,
}

/// 构造一个 request 帧：`{"type":"request","id":...,"request":{"command":..., ...params}}`。
pub fn request_frame(id: &str, command: &str, params: Option<&serde_json::Value>) -> String {
    let mut request = serde_json::json!({ "command": command });
    if let Some(p) = params {
        if let Some(obj) = p.as_object() {
            for (k, v) in obj {
                request[k] = v.clone();
            }
        } else if !p.is_null() {
            request = p.clone();
        }
    }
    serde_json::to_string(&serde_json::json!({
        "type": "request",
        "id": id,
        "request": request,
    }))
    .expect("request frame serialization cannot fail")
}
