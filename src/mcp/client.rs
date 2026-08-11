//! MCP client：era 探测与缓存、legacy 握手、tools/list 分页、tools/call 与取消。
//!
//! era 探测遵循规范 stdio binding 的回退规则:先发 `server/discover`,返回
//! `DiscoverResult` 或 `UnsupportedProtocolVersionError` 都证明对端是 modern
//! server(后者绝不回退);其他任何错误或探测超时才回退 `initialize` 握手。
//! era 是 server 进程的属性,探测一次后缓存整个连接生命周期。

use std::sync::atomic::AtomicBool;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use serde_json::{Map, Value};

use super::protocol::{self, ServerEra};
use super::transport::{Connection, WaitOutcome};
use crate::config::McpServerConfig;

/// era 探测最多等待的时长;为 legacy 握手保留至少一半启动预算。
const PROBE_TIMEOUT_CAP: Duration = Duration::from_secs(10);
const MAX_LIST_PAGES: usize = 16;
const MAX_RAW_TOOLS: usize = 1024;

/// server 声明的一个工具,未经导入卫生处理的原始形状。
#[derive(Debug, Clone)]
pub(crate) struct RawTool {
    pub name: String,
    pub title: Option<String>,
    pub description: Option<String>,
    /// 规范要求为 JSON Schema object;非 object 由导入层拒绝。
    pub input_schema: Option<Value>,
}

/// 一次 tools/call 的成功结果(isError 也算协议层成功,由上层转为工具错误)。
#[derive(Debug, Default)]
pub(crate) struct CallOutcome {
    /// text 内容块按序以空行拼接。
    pub text: String,
    /// image 内容块(base64 未解码,由代理层落盘)。
    pub images: Vec<ImageBlock>,
    /// 被丢弃内容块的说明(audio/resource 等)。
    pub dropped: Vec<String>,
    pub structured: Option<Value>,
    /// server 报告的工具执行错误(规范:应交给模型自纠)。
    pub is_error: bool,
}

#[derive(Debug)]
pub(crate) struct ImageBlock {
    pub mime: String,
    pub data_base64: String,
}

/// tools/call 的失败分类,由工具代理层映射为 `ToolError`。
#[derive(Debug)]
pub(crate) enum CallFailure {
    /// 取消标志被置位;cancelled 通知已发出。
    Aborted,
    /// 超过该 server 的 call_timeout;cancelled 通知已发出。
    Timeout { limit: Duration },
    /// 连接已故障或写入失败;恢复需要 /reload。
    ServerUnavailable { reason: String },
    /// server 返回 JSON-RPC 协议错误。
    Rpc { code: i64, message: String },
    /// 结果形状不受支持(input_required 等)。
    Protocol { message: String },
}

/// 连接就绪的 server:era 已探测,tools 已列出。
pub(crate) struct McpClient {
    pub name: String,
    conn: Connection,
    era: ServerEra,
    call_timeout: Duration,
    /// v1 每 server 同时只有一个在途请求(工具为 Sequential,此锁兜底)。
    request_gate: Mutex<()>,
    pub server_label: Option<String>,
}

pub(crate) struct ConnectResult {
    pub client: McpClient,
    pub tools: Vec<RawTool>,
    pub warnings: Vec<String>,
    /// server 提供的 LLM 使用说明。v1 不注入 system prompt,仅在 /mcp 展示。
    pub instructions: Option<String>,
}

impl std::fmt::Debug for McpClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("McpClient")
            .field("name", &self.name)
            .field("era", &self.era)
            .finish_non_exhaustive()
    }
}

impl std::fmt::Debug for ConnectResult {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ConnectResult")
            .field("client", &self.client)
            .field("tools", &self.tools.len())
            .field("warnings", &self.warnings)
            .finish()
    }
}

impl McpClient {
    /// spawn 子进程并完成 era 探测与 tools/list。任何失败返回带 stderr 线索的
    /// 描述,由 host 降级为启动 notice。
    pub(crate) fn connect(config: &McpServerConfig) -> Result<ConnectResult, String> {
        let conn = Connection::spawn(
            &config.command,
            &config.args,
            &config.env,
            config.cwd.as_deref(),
        )
        .map_err(|error| format!("启动 {} 失败: {error}", config.command))?;
        Self::handshake(
            config.name.clone(),
            conn,
            config.startup_timeout,
            config.call_timeout,
        )
    }

    /// 对已建立的连接执行 era 探测、(必要时)legacy 握手与 tools/list。
    /// 与 [`McpClient::connect`] 分离以便测试用内存管道驱动。
    pub(crate) fn handshake(
        name: String,
        conn: Connection,
        startup_timeout: Duration,
        call_timeout: Duration,
    ) -> Result<ConnectResult, String> {
        let deadline = Instant::now() + startup_timeout;
        let (era, server_label, instructions) = match detect_era(&conn, deadline) {
            Ok(detected) => detected,
            Err(error) => {
                let hint = stderr_hint(&conn);
                conn.shutdown();
                return Err(format!("{error}{hint}"));
            }
        };
        let (tools, warnings) = match list_tools(&conn, &era, deadline) {
            Ok(listed) => listed,
            Err(error) => {
                let hint = stderr_hint(&conn);
                conn.shutdown();
                return Err(format!("tools/list 失败: {error}{hint}"));
            }
        };
        Ok(ConnectResult {
            client: McpClient {
                name,
                conn,
                era,
                call_timeout,
                request_gate: Mutex::new(()),
                server_label,
            },
            tools,
            warnings,
            instructions,
        })
    }

    pub(crate) fn era_label(&self) -> String {
        self.era.label()
    }

    pub(crate) fn failure(&self) -> Option<String> {
        self.conn.failure()
    }

    pub(crate) fn take_list_changed(&self) -> bool {
        self.conn.take_list_changed()
    }

    pub(crate) fn stderr_tail(&self) -> Vec<String> {
        self.conn.stderr_tail()
    }

    pub(crate) fn shutdown(&self) {
        self.conn.shutdown();
    }

    /// 不经握手的 stub(era 视为 modern,连接立即 EOF)。仅供代理层的
    /// 形状/映射测试构造 `McpTool`,不可用于真实调用。
    #[cfg(test)]
    pub(crate) fn test_stub(name: &str) -> McpClient {
        let (client_read, _server_write) = std::io::pipe().expect("pipe");
        let (_server_read, client_write) = std::io::pipe().expect("pipe");
        let conn = Connection::from_streams(Box::new(client_write), Box::new(client_read));
        McpClient {
            name: name.into(),
            conn,
            era: ServerEra::Modern,
            call_timeout: Duration::from_secs(1),
            request_gate: Mutex::new(()),
            server_label: None,
        }
    }

    /// 调用远端工具。取消与超时都会发出 `notifications/cancelled` 并立即返回;
    /// server 若无视取消继续执行,其迟到响应会被 transport 丢弃(进程隔离,
    /// 不影响本地状态)。
    pub(crate) fn call_tool(
        &self,
        tool_name: &str,
        arguments: &Value,
        cancel: &AtomicBool,
    ) -> Result<CallOutcome, CallFailure> {
        let _gate = self.request_gate.lock().unwrap();
        if let Some(reason) = self.conn.failure() {
            return Err(CallFailure::ServerUnavailable { reason });
        }
        let mut params = Map::new();
        params.insert("name".into(), Value::String(tool_name.to_string()));
        params.insert("arguments".into(), arguments.clone());
        self.stamp_meta(&mut params);
        let id = self.conn.allocate_id();
        self.conn
            .send_request(id, "tools/call", Value::Object(params))
            .map_err(|reason| CallFailure::ServerUnavailable { reason })?;
        let deadline = Instant::now() + self.call_timeout;
        match self.conn.wait_response(id, deadline, Some(cancel)) {
            WaitOutcome::Done(Ok(result)) => parse_call_result(&result),
            WaitOutcome::Done(Err(rpc)) => Err(CallFailure::Rpc {
                code: rpc.code,
                message: rpc.message,
            }),
            WaitOutcome::Cancelled => {
                self.notify_cancelled(id, "用户取消");
                Err(CallFailure::Aborted)
            }
            WaitOutcome::TimedOut => {
                self.notify_cancelled(id, "client 超时");
                Err(CallFailure::Timeout {
                    limit: self.call_timeout,
                })
            }
            WaitOutcome::Failed(reason) => Err(CallFailure::ServerUnavailable { reason }),
        }
    }

    fn stamp_meta(&self, params: &mut Map<String, Value>) {
        if matches!(self.era, ServerEra::Modern) {
            params.insert("_meta".into(), protocol::modern_meta());
        }
    }

    fn notify_cancelled(&self, request_id: u64, reason: &str) {
        let mut params = Map::new();
        params.insert("requestId".into(), Value::from(request_id));
        params.insert("reason".into(), Value::String(reason.to_string()));
        self.stamp_meta(&mut params);
        let _ = self
            .conn
            .send_notification("notifications/cancelled", Value::Object(params));
    }
}

fn stderr_hint(conn: &Connection) -> String {
    let tail = conn.stderr_tail();
    match tail.last() {
        Some(line) => format!("(stderr: {})", crate::util::ellipsis(line, 160)),
        None => String::new(),
    }
}

/// 双时代探测。返回 era、server 标签与 instructions。
fn detect_era(
    conn: &Connection,
    startup_deadline: Instant,
) -> Result<(ServerEra, Option<String>, Option<String>), String> {
    // 探测最多用掉一半剩余预算(且不超过硬上限),legacy 回退握手始终有余量。
    let remaining = startup_deadline.saturating_duration_since(Instant::now());
    let probe_deadline = Instant::now() + (remaining / 2).min(PROBE_TIMEOUT_CAP);
    let id = conn.allocate_id();
    let params = protocol::object(vec![("_meta", protocol::modern_meta())]);
    conn.send_request(id, "server/discover", params)
        .map_err(|reason| format!("server 不可用: {reason}"))?;
    match conn.wait_response(id, probe_deadline, None) {
        WaitOutcome::Done(Ok(result)) => {
            // DiscoverResult:对端是 modern server。
            let supported: Vec<String> = result
                .get("supportedVersions")
                .and_then(Value::as_array)
                .map(|versions| {
                    versions
                        .iter()
                        .filter_map(Value::as_str)
                        .map(str::to_string)
                        .collect()
                })
                .unwrap_or_default();
            if !supported
                .iter()
                .any(|version| version == protocol::MODERN_PROTOCOL_VERSION)
            {
                return Err(format!(
                    "协议版本不兼容: server 支持 [{}],onemore 支持 {}",
                    supported.join(", "),
                    protocol::MODERN_PROTOCOL_VERSION
                ));
            }
            let label = protocol::server_info_label(&result);
            let instructions = result
                .get("instructions")
                .and_then(Value::as_str)
                .map(str::to_string);
            Ok((ServerEra::Modern, label, instructions))
        }
        WaitOutcome::Done(Err(rpc)) if rpc.code == protocol::ERROR_UNSUPPORTED_PROTOCOL_VERSION => {
            // 认识 modern 错误的就是 modern server,规范禁止回退 initialize。
            let supported: Vec<String> = rpc
                .data
                .as_ref()
                .and_then(|data| data.get("supported"))
                .and_then(Value::as_array)
                .map(|versions| {
                    versions
                        .iter()
                        .filter_map(Value::as_str)
                        .map(str::to_string)
                        .collect()
                })
                .unwrap_or_default();
            if supported
                .iter()
                .any(|version| version == protocol::MODERN_PROTOCOL_VERSION)
            {
                return Ok((ServerEra::Modern, None, None));
            }
            Err(format!(
                "协议版本不兼容: server 支持 [{}],onemore 支持 {}",
                supported.join(", "),
                protocol::MODERN_PROTOCOL_VERSION
            ))
        }
        // 其他错误(常见 -32601/-32602)或探测超时 → legacy server,回退握手。
        WaitOutcome::Done(Err(_)) | WaitOutcome::TimedOut => {
            legacy_initialize(conn, startup_deadline)
        }
        WaitOutcome::Cancelled => unreachable!("探测不携带取消标志"),
        WaitOutcome::Failed(reason) => Err(format!("server 不可用: {reason}")),
    }
}

fn legacy_initialize(
    conn: &Connection,
    deadline: Instant,
) -> Result<(ServerEra, Option<String>, Option<String>), String> {
    let id = conn.allocate_id();
    conn.send_request(id, "initialize", protocol::legacy_initialize_params())
        .map_err(|reason| format!("server 不可用: {reason}"))?;
    match conn.wait_response(id, deadline, None) {
        WaitOutcome::Done(Ok(result)) => {
            let version = result
                .get("protocolVersion")
                .and_then(Value::as_str)
                .ok_or("initialize 响应缺少 protocolVersion")?;
            if !protocol::SUPPORTED_LEGACY_VERSIONS.contains(&version) {
                return Err(format!(
                    "协议版本不兼容: server 协商出 {},onemore 支持 [{}]",
                    version,
                    protocol::SUPPORTED_LEGACY_VERSIONS.join(", ")
                ));
            }
            conn.send_notification("notifications/initialized", Value::Null)
                .map_err(|reason| format!("server 不可用: {reason}"))?;
            let label = protocol::server_info_label(&result);
            let instructions = result
                .get("instructions")
                .and_then(Value::as_str)
                .map(str::to_string);
            Ok((
                ServerEra::Legacy {
                    version: version.to_string(),
                },
                label,
                instructions,
            ))
        }
        WaitOutcome::Done(Err(rpc)) => Err(format!(
            "initialize 被拒绝({}): {};server 可能只支持更新的协议版本",
            rpc.code, rpc.message
        )),
        WaitOutcome::TimedOut => Err("启动超时: era 探测与 initialize 均无响应".into()),
        WaitOutcome::Cancelled => unreachable!("握手不携带取消标志"),
        WaitOutcome::Failed(reason) => Err(format!("server 不可用: {reason}")),
    }
}

fn list_tools(
    conn: &Connection,
    era: &ServerEra,
    deadline: Instant,
) -> Result<(Vec<RawTool>, Vec<String>), String> {
    let mut tools = Vec::new();
    let mut warnings = Vec::new();
    let mut cursor: Option<String> = None;
    for _page in 0..MAX_LIST_PAGES {
        let mut params = Map::new();
        if let Some(cursor) = &cursor {
            params.insert("cursor".into(), Value::String(cursor.clone()));
        }
        if matches!(era, ServerEra::Modern) {
            params.insert("_meta".into(), protocol::modern_meta());
        }
        let id = conn.allocate_id();
        conn.send_request(id, "tools/list", Value::Object(params))?;
        let result = match conn.wait_response(id, deadline, None) {
            WaitOutcome::Done(Ok(result)) => result,
            WaitOutcome::Done(Err(rpc)) => {
                return Err(format!("server 返回错误({}): {}", rpc.code, rpc.message))
            }
            WaitOutcome::TimedOut => return Err("等待响应超时".into()),
            WaitOutcome::Cancelled => unreachable!("tools/list 不携带取消标志"),
            WaitOutcome::Failed(reason) => return Err(reason),
        };
        let Some(items) = result.get("tools").and_then(Value::as_array) else {
            return Err("结果缺少 tools 数组".into());
        };
        for item in items {
            match parse_raw_tool(item) {
                Ok(tool) => tools.push(tool),
                Err(reason) => warnings.push(format!("忽略一条非法工具声明: {reason}")),
            }
        }
        if tools.len() > MAX_RAW_TOOLS {
            warnings.push(format!(
                "server 声明的工具超过 {} 个,已停止继续拉取",
                MAX_RAW_TOOLS
            ));
            tools.truncate(MAX_RAW_TOOLS);
            break;
        }
        cursor = result
            .get("nextCursor")
            .and_then(Value::as_str)
            .map(str::to_string);
        if cursor.is_none() {
            break;
        }
    }
    Ok((tools, warnings))
}

fn parse_raw_tool(item: &Value) -> Result<RawTool, String> {
    let Some(name) = item.get("name").and_then(Value::as_str) else {
        return Err("缺少 name".into());
    };
    Ok(RawTool {
        name: name.to_string(),
        title: item
            .get("title")
            .and_then(Value::as_str)
            .map(str::to_string),
        description: item
            .get("description")
            .and_then(Value::as_str)
            .map(str::to_string),
        input_schema: item.get("inputSchema").cloned(),
    })
}

fn parse_call_result(result: &Value) -> Result<CallOutcome, CallFailure> {
    if protocol::result_type(result) == "input_required" {
        return Err(CallFailure::Protocol {
            message: "server 请求交互式输入(MRTR),onemore 未支持该模式".into(),
        });
    }
    let mut outcome = CallOutcome {
        is_error: result
            .get("isError")
            .and_then(Value::as_bool)
            .unwrap_or(false),
        structured: result.get("structuredContent").cloned(),
        ..CallOutcome::default()
    };
    let mut texts: Vec<String> = Vec::new();
    if let Some(blocks) = result.get("content").and_then(Value::as_array) {
        for block in blocks {
            let kind = block.get("type").and_then(Value::as_str).unwrap_or("?");
            match kind {
                "text" => {
                    if let Some(text) = block.get("text").and_then(Value::as_str) {
                        texts.push(text.to_string());
                    }
                }
                "image" => {
                    let data = block.get("data").and_then(Value::as_str);
                    let mime = block.get("mimeType").and_then(Value::as_str);
                    match (data, mime) {
                        (Some(data), Some(mime)) => outcome.images.push(ImageBlock {
                            mime: mime.to_string(),
                            data_base64: data.to_string(),
                        }),
                        _ => outcome.dropped.push("image 块缺少 data/mimeType".into()),
                    }
                }
                other => outcome
                    .dropped
                    .push(format!("{} 内容块(v1 不支持,已丢弃)", other)),
            }
        }
    }
    outcome.text = texts.join("\n\n");
    Ok(outcome)
}
