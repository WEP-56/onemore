//! MCP 报文层：JSON-RPC 2.0 构造/解析、双时代 `_meta`、稳定错误码。
//!
//! 规范来源:modelcontextprotocol.io 修订 2026-07-28(modern,无握手、每请求携带
//! `_meta` 元数据)与 2025-11-25 及以前(legacy,`initialize` 握手)。本模块只
//! 负责报文形状,era 探测与回退策略在 `client`。

use serde_json::{json, Map, Value};

/// v1 支持的唯一 modern 协议版本。
pub(crate) const MODERN_PROTOCOL_VERSION: &str = "2026-07-28";
/// legacy 回退时请求的基线版本。
pub(crate) const LEGACY_REQUEST_VERSION: &str = "2025-06-18";
/// legacy 协商中可接受的 server 版本(server 可以回应不同于请求的版本)。
pub(crate) const SUPPORTED_LEGACY_VERSIONS: [&str; 4] =
    ["2024-11-05", "2025-03-26", "2025-06-18", "2025-11-25"];

/// `UnsupportedProtocolVersionError`(2026-07-28 起)。收到它意味着对端是
/// modern server,绝不能回退 initialize。
pub(crate) const ERROR_UNSUPPORTED_PROTOCOL_VERSION: i64 = -32022;
pub(crate) const ERROR_METHOD_NOT_FOUND: i64 = -32601;
pub(crate) const ERROR_INVALID_PARAMS: i64 = -32602;

/// server 的协议时代。按 server 进程探测一次并缓存(规范要求 era 是 server 的
/// 属性,不是单个请求的属性)。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ServerEra {
    /// 2026-07-28 起:无握手,每个请求在 `_meta` 携带协议版本。
    Modern,
    /// 2025-11-25 及以前:`initialize` 握手建立会话级协商。
    Legacy { version: String },
}

impl ServerEra {
    pub(crate) fn label(&self) -> String {
        match self {
            ServerEra::Modern => format!("modern {}", MODERN_PROTOCOL_VERSION),
            ServerEra::Legacy { version } => format!("legacy {}", version),
        }
    }
}

fn client_info() -> Value {
    json!({ "name": "onemore", "version": env!("CARGO_PKG_VERSION") })
}

/// modern 每请求 `_meta`:协议版本、client 身份与空 capabilities(v1 不声明
/// roots/sampling/elicitation,也不开订阅)。
pub(crate) fn modern_meta() -> Value {
    json!({
        "io.modelcontextprotocol/protocolVersion": MODERN_PROTOCOL_VERSION,
        "io.modelcontextprotocol/clientInfo": client_info(),
        "io.modelcontextprotocol/clientCapabilities": {},
    })
}

/// legacy `initialize` 的请求参数。
pub(crate) fn legacy_initialize_params() -> Value {
    json!({
        "protocolVersion": LEGACY_REQUEST_VERSION,
        "capabilities": {},
        "clientInfo": client_info(),
    })
}

/// 构造一条请求。`params` 必须是 object(JSON-RPC 允许省略,MCP 的请求都带)。
pub(crate) fn request(id: u64, method: &str, params: Value) -> Value {
    debug_assert!(params.is_object());
    json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params })
}

/// 构造一条通知。`params` 为 `Null` 时整体省略(legacy `notifications/initialized`)。
pub(crate) fn notification(method: &str, params: Value) -> Value {
    if params.is_null() {
        json!({ "jsonrpc": "2.0", "method": method })
    } else {
        json!({ "jsonrpc": "2.0", "method": method, "params": params })
    }
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct RpcError {
    pub code: i64,
    pub message: String,
    pub data: Option<Value>,
}

/// 一条来自 server stdout 的消息,已按方向分类。
#[derive(Debug)]
pub(crate) enum ServerMessage {
    /// 对 client 请求的响应,按数字 id 关联。
    Response {
        id: u64,
        result: Result<Value, RpcError>,
    },
    /// server 通知(list_changed、progress 等)。
    Notification { method: String },
    /// server 发起的请求。仅 legacy server 会出现(ping/sampling 等),
    /// modern 规范禁止 server→client 请求。id 原样保留用于回复。
    Request { id: Value, method: String },
}

/// 解析一行 stdout。返回 Err 表示这不是合法 MCP 消息(噪声行)。
pub(crate) fn parse_server_message(line: &str) -> Result<ServerMessage, String> {
    let value: Value = serde_json::from_str(line).map_err(|error| error.to_string())?;
    let Value::Object(map) = value else {
        return Err("消息不是 JSON object".into());
    };
    let method = map.get("method").and_then(Value::as_str);
    let id = map.get("id");
    match (method, id) {
        (Some(method), Some(id)) => Ok(ServerMessage::Request {
            id: id.clone(),
            method: method.to_string(),
        }),
        (Some(method), None) => Ok(ServerMessage::Notification {
            method: method.to_string(),
        }),
        (None, Some(id)) => {
            let Some(id) = id.as_u64() else {
                // client 只用 u64 id;其他形状对应不到任何 pending 请求。
                return Err("响应 id 不是本 client 发出的形状".into());
            };
            if let Some(error) = map.get("error") {
                let code = error.get("code").and_then(Value::as_i64).unwrap_or(0);
                let message = error
                    .get("message")
                    .and_then(Value::as_str)
                    .unwrap_or("(server 未提供错误信息)")
                    .to_string();
                let data = error.get("data").cloned();
                Ok(ServerMessage::Response {
                    id,
                    result: Err(RpcError {
                        code,
                        message,
                        data,
                    }),
                })
            } else if let Some(result) = map.get("result") {
                Ok(ServerMessage::Response {
                    id,
                    result: Ok(result.clone()),
                })
            } else {
                Err("响应缺少 result 与 error".into())
            }
        }
        (None, None) => Err("消息既无 method 也无 id".into()),
    }
}

/// 从结果对象读取 `resultType`。2026-07-28 起为必填;更早协议的结果缺省该字段,
/// 规范要求按 `"complete"` 处理。
pub(crate) fn result_type(result: &Value) -> &str {
    result
        .get("resultType")
        .and_then(Value::as_str)
        .unwrap_or("complete")
}

/// 从 modern 结果 `_meta` 或 legacy initialize 结果提取 server 身份标签(仅展示,
/// 规范明确不得据此做行为或安全决策)。
pub(crate) fn server_info_label(result: &Value) -> Option<String> {
    let info = result
        .get("_meta")
        .and_then(|meta| meta.get("io.modelcontextprotocol/serverInfo"))
        .or_else(|| result.get("serverInfo"))?;
    let name = info.get("name").and_then(Value::as_str)?;
    let version = info.get("version").and_then(Value::as_str).unwrap_or("");
    Some(format!("{} {}", name, version).trim().to_string())
}

/// 提取一个 object 参数容器,便于逐字段追加。
pub(crate) fn object(entries: Vec<(&str, Value)>) -> Value {
    let mut map = Map::new();
    for (key, value) in entries {
        map.insert(key.to_string(), value);
    }
    Value::Object(map)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_response_notification_and_request() {
        let response = parse_server_message(r#"{"jsonrpc":"2.0","id":3,"result":{"ok":true}}"#);
        assert!(matches!(
            response,
            Ok(ServerMessage::Response {
                id: 3,
                result: Ok(_)
            })
        ));

        let error = parse_server_message(
            r#"{"jsonrpc":"2.0","id":4,"error":{"code":-32602,"message":"bad"}}"#,
        );
        let Ok(ServerMessage::Response {
            id: 4,
            result: Err(rpc),
        }) = error
        else {
            panic!("应解析为错误响应: {error:?}");
        };
        assert_eq!(rpc.code, ERROR_INVALID_PARAMS);
        assert_eq!(rpc.message, "bad");

        let notification = parse_server_message(
            r#"{"jsonrpc":"2.0","method":"notifications/tools/list_changed"}"#,
        );
        assert!(matches!(
            notification,
            Ok(ServerMessage::Notification { method }) if method == "notifications/tools/list_changed"
        ));

        let request = parse_server_message(r#"{"jsonrpc":"2.0","id":"srv-1","method":"ping"}"#);
        assert!(matches!(
            request,
            Ok(ServerMessage::Request { method, .. }) if method == "ping"
        ));
    }

    #[test]
    fn noise_lines_are_rejected() {
        assert!(parse_server_message("npm WARN deprecated").is_err());
        assert!(parse_server_message("[]").is_err());
        assert!(parse_server_message(r#"{"jsonrpc":"2.0"}"#).is_err());
        // 字符串 id 不是本 client 发出的形状 → 噪声(不会中断连接,只计数)。
        assert!(parse_server_message(r#"{"jsonrpc":"2.0","id":"x","result":{}}"#).is_err());
    }

    #[test]
    fn result_type_defaults_to_complete_for_legacy_results() {
        assert_eq!(result_type(&json!({"content": []})), "complete");
        assert_eq!(
            result_type(&json!({"resultType": "input_required"})),
            "input_required"
        );
    }

    #[test]
    fn server_info_label_reads_both_eras() {
        let modern = json!({
            "_meta": { "io.modelcontextprotocol/serverInfo": { "name": "srv", "version": "1.2" } }
        });
        assert_eq!(server_info_label(&modern).as_deref(), Some("srv 1.2"));
        let legacy = json!({ "serverInfo": { "name": "old", "version": "0.9" } });
        assert_eq!(server_info_label(&legacy).as_deref(), Some("old 0.9"));
        assert_eq!(server_info_label(&json!({})), None);
    }
}
