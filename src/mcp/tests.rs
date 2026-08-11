//! MCP 单元测试。
//!
//! 协议与 era 行为用 `std::io::pipe` 内存管道驱动脚本化 server(快、确定、
//! 可编排任意应答序列);进程生命周期(EOF 优雅退出、强杀、stderr 捕获)用
//! 真实 PowerShell 子进程覆盖,仅在 Windows 上运行。

use std::io::{BufRead, BufReader, PipeReader, PipeWriter, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use serde_json::{json, Value};

use super::client::{CallFailure, McpClient};
use super::transport::{Connection, WaitOutcome, MAX_LINE_BYTES};
use super::{decode_base64, import};

const STARTUP: Duration = Duration::from_secs(3);
const CALL: Duration = Duration::from_secs(3);

struct WireEnd {
    incoming: BufReader<PipeReader>,
    outgoing: PipeWriter,
}

fn wire() -> (Connection, WireEnd) {
    let (client_read, server_write) = std::io::pipe().expect("pipe");
    let (server_read, client_write) = std::io::pipe().expect("pipe");
    let conn = Connection::from_streams(Box::new(client_write), Box::new(client_read));
    (
        conn,
        WireEnd {
            incoming: BufReader::new(server_read),
            outgoing: server_write,
        },
    )
}

fn write_json(out: &mut PipeWriter, value: Value) {
    let mut text = value.to_string();
    text.push('\n');
    let _ = out.write_all(text.as_bytes());
    let _ = out.flush();
}

type Handler = Box<dyn FnMut(&Value, &mut PipeWriter) -> bool + Send>;

/// 脚本化 server:逐行读 client 消息,记录后交给 handler;handler 返回 false
/// 模拟 server 崩溃(线程退出,stdout 关闭)。client 关停(stdin EOF)自然结束。
fn serve(wire: WireEnd, mut handler: Handler) -> (Arc<Mutex<Vec<Value>>>, JoinHandle<()>) {
    let seen = Arc::new(Mutex::new(Vec::new()));
    let record = Arc::clone(&seen);
    let handle = std::thread::spawn(move || {
        let mut incoming = wire.incoming;
        let mut outgoing = wire.outgoing;
        let mut line = String::new();
        loop {
            line.clear();
            match incoming.read_line(&mut line) {
                Ok(0) | Err(_) => return,
                Ok(_) => {}
            }
            let Ok(message) = serde_json::from_str::<Value>(line.trim()) else {
                continue;
            };
            record.lock().unwrap().push(message.clone());
            if !handler(&message, &mut outgoing) {
                return;
            }
        }
    });
    (seen, handle)
}

fn wait_until(timeout: Duration, mut check: impl FnMut() -> bool) -> bool {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if check() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    check()
}

fn method_of(message: &Value) -> Option<&str> {
    message.get("method").and_then(Value::as_str)
}

fn discover_result(id: &Value) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "result": {
        "resultType": "complete",
        "supportedVersions": ["2026-07-28"],
        "capabilities": { "tools": {} },
        "_meta": { "io.modelcontextprotocol/serverInfo": { "name": "fixture", "version": "1.0" } },
        "ttlMs": 60000, "cacheScope": "private"
    }})
}

fn tools_list_result(id: &Value) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "result": {
        "resultType": "complete",
        "tools": [
            { "name": "echo", "description": "echo text",
              "inputSchema": { "type": "object", "properties": { "text": { "type": "string" } } } },
            { "name": "shout", "description": "loud", "inputSchema": { "type": "object" } }
        ],
        "ttlMs": 1000, "cacheScope": "private"
    }})
}

/// 完整 modern server;`on_call` 定制 tools/call 行为(None = 不响应)。
fn modern_handler(
    mut on_call: impl FnMut(&Value, &mut PipeWriter) -> bool + Send + 'static,
) -> Handler {
    Box::new(move |message, out| {
        let id = message.get("id").cloned().unwrap_or(Value::Null);
        match method_of(message) {
            Some("server/discover") => write_json(out, discover_result(&id)),
            Some("tools/list") => write_json(out, tools_list_result(&id)),
            Some("tools/call") => return on_call(message, out),
            _ => {}
        }
        true
    })
}

fn echo_call(message: &Value, out: &mut PipeWriter) -> bool {
    let id = message.get("id").cloned().unwrap_or(Value::Null);
    let text = message["params"]["arguments"]["text"]
        .as_str()
        .unwrap_or("?");
    write_json(
        out,
        json!({ "jsonrpc": "2.0", "id": id, "result": {
            "resultType": "complete",
            "content": [{ "type": "text", "text": format!("echo: {text}") }],
            "isError": false
        }}),
    );
    true
}

fn legacy_handler() -> Handler {
    Box::new(|message, out| {
        let id = message.get("id").cloned().unwrap_or(Value::Null);
        match method_of(message) {
            Some("server/discover") => write_json(
                out,
                json!({ "jsonrpc": "2.0", "id": id, "error": {
                    "code": -32601, "message": "Method not found" } }),
            ),
            Some("initialize") => write_json(
                out,
                json!({ "jsonrpc": "2.0", "id": id, "result": {
                    "protocolVersion": "2025-06-18",
                    "capabilities": { "tools": {} },
                    "serverInfo": { "name": "old-fixture", "version": "0.9" }
                }}),
            ),
            Some("tools/list") => write_json(
                out,
                json!({ "jsonrpc": "2.0", "id": id, "result": {
                    "tools": [{ "name": "legacy_echo", "description": "echo",
                        "inputSchema": { "type": "object" } }]
                }}),
            ),
            Some("tools/call") => return echo_call(message, out),
            _ => {}
        }
        true
    })
}

fn handshake(conn: Connection) -> Result<super::client::ConnectResult, String> {
    McpClient::handshake("srv".into(), conn, STARTUP, CALL)
}

#[test]
fn modern_server_handshake_lists_and_calls() {
    let (conn, wire) = wire();
    let (seen, _server) = serve(wire, modern_handler(echo_call));

    let connected = handshake(conn).expect("modern handshake");
    assert_eq!(connected.client.era_label(), "modern 2026-07-28");
    assert_eq!(connected.tools.len(), 2);
    assert_eq!(
        connected.client.server_label.as_deref(),
        Some("fixture 1.0")
    );

    let cancel = AtomicBool::new(false);
    let outcome = connected
        .client
        .call_tool("echo", &json!({ "text": "hi" }), &cancel)
        .expect("call");
    assert_eq!(outcome.text, "echo: hi");
    assert!(!outcome.is_error);

    let seen = seen.lock().unwrap();
    let discover = seen
        .iter()
        .find(|message| method_of(message) == Some("server/discover"))
        .expect("discover sent");
    assert_eq!(
        discover["params"]["_meta"]["io.modelcontextprotocol/protocolVersion"],
        "2026-07-28"
    );
    let call = seen
        .iter()
        .find(|message| method_of(message) == Some("tools/call"))
        .expect("call sent");
    assert!(
        call["params"]["_meta"].is_object(),
        "modern 请求必须携带 _meta"
    );
    assert!(
        !seen.iter().any(|m| method_of(m) == Some("initialize")),
        "modern 通路不得发送 initialize"
    );
}

#[test]
fn legacy_fallback_via_error_then_initialize() {
    let (conn, wire) = wire();
    let (seen, _server) = serve(wire, legacy_handler());

    let connected = handshake(conn).expect("legacy handshake");
    assert_eq!(connected.client.era_label(), "legacy 2025-06-18");
    assert_eq!(connected.tools.len(), 1);
    assert_eq!(
        connected.client.server_label.as_deref(),
        Some("old-fixture 0.9")
    );

    let cancel = AtomicBool::new(false);
    let outcome = connected
        .client
        .call_tool("legacy_echo", &json!({ "text": "x" }), &cancel)
        .expect("call");
    assert_eq!(outcome.text, "echo: x");

    let seen = seen.lock().unwrap();
    assert!(seen
        .iter()
        .any(|m| method_of(m) == Some("notifications/initialized")));
    let call = seen
        .iter()
        .find(|message| method_of(message) == Some("tools/call"))
        .expect("call sent");
    assert!(
        call["params"].get("_meta").is_none(),
        "legacy 请求不携带 modern _meta"
    );
}

#[test]
fn legacy_fallback_via_probe_timeout() {
    let (conn, wire) = wire();
    // 无视 discover(不响应),其余按 legacy 处理:探测超时 → 回退 initialize。
    let mut inner = legacy_handler();
    let handler: Handler = Box::new(move |message, out| {
        if method_of(message) == Some("server/discover") {
            return true;
        }
        inner(message, out)
    });
    let (_seen, _server) = serve(wire, handler);

    let connected = handshake(conn).expect("probe 超时后应回退 legacy");
    assert_eq!(connected.client.era_label(), "legacy 2025-06-18");
}

#[test]
fn unsupported_version_error_never_falls_back_to_initialize() {
    let (conn, wire) = wire();
    let handler: Handler = Box::new(|message, out| {
        let id = message.get("id").cloned().unwrap_or(Value::Null);
        if method_of(message) == Some("server/discover") {
            write_json(
                out,
                json!({ "jsonrpc": "2.0", "id": id, "error": {
                    "code": -32022, "message": "Unsupported protocol version",
                    "data": { "supported": ["2027-01-01"], "requested": "2026-07-28" }
                }}),
            );
        }
        true
    });
    let (seen, _server) = serve(wire, handler);

    let error = handshake(conn).expect_err("版本不兼容应失败");
    assert!(error.contains("协议版本不兼容"), "{error}");
    assert!(error.contains("2027-01-01"), "{error}");
    assert!(
        !seen
            .lock()
            .unwrap()
            .iter()
            .any(|m| method_of(m) == Some("initialize")),
        "-32022 证明对端是 modern server,不得回退 initialize"
    );
}

#[test]
fn incompatible_modern_supported_versions_fail_clearly() {
    let (conn, wire) = wire();
    let handler: Handler = Box::new(|message, out| {
        let id = message.get("id").cloned().unwrap_or(Value::Null);
        if method_of(message) == Some("server/discover") {
            write_json(
                out,
                json!({ "jsonrpc": "2.0", "id": id, "result": {
                    "resultType": "complete", "supportedVersions": ["2099-01-01"],
                    "capabilities": {}, "ttlMs": 0, "cacheScope": "private"
                }}),
            );
        }
        true
    });
    let (_seen, _server) = serve(wire, handler);
    let error = handshake(conn).expect_err("不兼容版本应失败");
    assert!(error.contains("2099-01-01"), "{error}");
}

#[test]
fn cancellation_returns_promptly_and_sends_cancelled_notification() {
    let (conn, wire) = wire();
    // tools/call 永不响应,模拟慢工具。
    let (seen, _server) = serve(wire, modern_handler(|_, _| true));

    let connected = handshake(conn).expect("handshake");
    let cancel = Arc::new(AtomicBool::new(false));
    let flag = Arc::clone(&cancel);
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(100));
        flag.store(true, Ordering::Relaxed);
    });
    let started = Instant::now();
    let failure = connected
        .client
        .call_tool("echo", &json!({}), &cancel)
        .expect_err("应被取消");
    assert!(matches!(failure, CallFailure::Aborted), "{failure:?}");
    assert!(
        started.elapsed() < Duration::from_secs(1),
        "取消应在有界时间内返回: {:?}",
        started.elapsed()
    );

    assert!(
        wait_until(Duration::from_secs(2), || {
            let seen = seen.lock().unwrap();
            let call_id = seen
                .iter()
                .find(|m| method_of(m) == Some("tools/call"))
                .and_then(|m| m.get("id").cloned());
            seen.iter().any(|m| {
                method_of(m) == Some("notifications/cancelled")
                    && m["params"].get("requestId") == call_id.as_ref()
            })
        }),
        "server 应收到与调用 id 配对的 cancelled 通知"
    );
}

#[test]
fn call_timeout_sends_cancelled_notification() {
    let (conn, wire) = wire();
    let (seen, _server) = serve(wire, modern_handler(|_, _| true));
    let connected = McpClient::handshake("srv".into(), conn, STARTUP, Duration::from_millis(200))
        .expect("handshake");

    let cancel = AtomicBool::new(false);
    let started = Instant::now();
    let failure = connected
        .client
        .call_tool("echo", &json!({}), &cancel)
        .expect_err("应超时");
    assert!(
        matches!(failure, CallFailure::Timeout { .. }),
        "{failure:?}"
    );
    assert!(started.elapsed() < Duration::from_secs(2));
    assert!(wait_until(Duration::from_secs(2), || {
        seen.lock()
            .unwrap()
            .iter()
            .any(|m| method_of(m) == Some("notifications/cancelled"))
    }));
}

#[test]
fn transport_routes_out_of_order_responses() {
    let (conn, wire) = wire();
    let handler: Handler = Box::new(|message, out| {
        // 收到第二条请求后按相反顺序响应两条。
        if message.get("id").and_then(Value::as_u64) == Some(2) {
            write_json(
                out,
                json!({ "jsonrpc": "2.0", "id": 2, "result": { "tag": "b" } }),
            );
            write_json(
                out,
                json!({ "jsonrpc": "2.0", "id": 1, "result": { "tag": "a" } }),
            );
        }
        true
    });
    let (_seen, _server) = serve(wire, handler);

    let first = conn.allocate_id();
    let second = conn.allocate_id();
    conn.send_request(first, "x/a", json!({})).unwrap();
    conn.send_request(second, "x/b", json!({})).unwrap();
    let deadline = Instant::now() + Duration::from_secs(2);
    let WaitOutcome::Done(Ok(a)) = conn.wait_response(first, deadline, None) else {
        panic!("first 应有结果");
    };
    let WaitOutcome::Done(Ok(b)) = conn.wait_response(second, deadline, None) else {
        panic!("second 应有结果");
    };
    assert_eq!(a["tag"], "a");
    assert_eq!(b["tag"], "b");
    conn.shutdown();
}

#[test]
fn noise_lines_are_tolerated_within_limit() {
    let (conn, wire) = wire();
    let handler: Handler = Box::new(|message, out| {
        let id = message.get("id").cloned().unwrap_or(Value::Null);
        if method_of(message) == Some("server/discover") {
            for i in 0..5 {
                let _ = out.write_all(format!("npm WARN deprecated pkg{i}\n").as_bytes());
            }
            let _ = out.flush();
            write_json(out, discover_result(&id));
        } else if method_of(message) == Some("tools/list") {
            write_json(out, tools_list_result(&id));
        }
        true
    });
    let (_seen, _server) = serve(wire, handler);
    let connected = handshake(conn).expect("有限噪声行不应中断连接");
    assert_eq!(connected.tools.len(), 2);
}

#[test]
fn noise_overflow_fails_the_connection() {
    let (conn, wire) = wire();
    let handler: Handler = Box::new(|message, out| {
        if method_of(message) == Some("server/discover") {
            for i in 0..30 {
                let _ = out.write_all(format!("garbage line {i}\n").as_bytes());
            }
            let _ = out.flush();
        }
        true
    });
    let (_seen, _server) = serve(wire, handler);
    let error = handshake(conn).expect_err("噪声超限应判故障");
    assert!(error.contains("非 MCP 消息行"), "{error}");
}

#[test]
fn oversized_line_fails_the_connection() {
    let (client_read, mut server_write) = std::io::pipe().expect("pipe");
    let (_server_read, client_write) = std::io::pipe().expect("pipe");
    let conn = Connection::from_streams(Box::new(client_write), Box::new(client_read));

    let writer = std::thread::spawn(move || {
        let chunk = vec![b'a'; 64 * 1024];
        let mut written = 0usize;
        while written <= MAX_LINE_BYTES + chunk.len() {
            if server_write.write_all(&chunk).is_err() {
                return;
            }
            written += chunk.len();
        }
        let _ = server_write.write_all(b"\n");
    });

    let id = conn.allocate_id();
    conn.send_request(id, "x/a", json!({})).unwrap();
    let outcome = conn.wait_response(id, Instant::now() + Duration::from_secs(10), None);
    let WaitOutcome::Failed(reason) = outcome else {
        panic!("超长行应判故障: {outcome:?}");
    };
    assert!(reason.contains("字节上限"), "{reason}");
    conn.shutdown();
    let _ = writer.join();
}

#[test]
fn crash_mid_call_fails_current_and_subsequent_calls() {
    let (conn, wire) = wire();
    // tools/call 时直接退出(stdout 关闭),模拟 server 崩溃。
    let (_seen, _server) = serve(wire, modern_handler(|_, _| false));
    let connected = handshake(conn).expect("handshake");

    let cancel = AtomicBool::new(false);
    let failure = connected
        .client
        .call_tool("echo", &json!({}), &cancel)
        .expect_err("崩溃应报错");
    assert!(
        matches!(failure, CallFailure::ServerUnavailable { .. }),
        "{failure:?}"
    );

    let started = Instant::now();
    let failure = connected
        .client
        .call_tool("echo", &json!({}), &cancel)
        .expect_err("后续调用应立即失败");
    assert!(
        matches!(failure, CallFailure::ServerUnavailable { .. }),
        "{failure:?}"
    );
    assert!(started.elapsed() < Duration::from_millis(500));
    assert!(connected.client.failure().is_some());
}

#[test]
fn call_results_map_is_error_input_required_and_content_blocks() {
    let (conn, wire) = wire();
    let handler = modern_handler(|message, out| {
        let id = message.get("id").cloned().unwrap_or(Value::Null);
        let name = message["params"]["name"].as_str().unwrap_or("");
        let result = match name {
            "fail" => json!({
                "resultType": "complete",
                "content": [{ "type": "text", "text": "boom" }],
                "isError": true
            }),
            "mrtr" => json!({
                "resultType": "input_required",
                "inputRequests": []
            }),
            _ => json!({
                "resultType": "complete",
                "content": [
                    { "type": "text", "text": "第一段" },
                    { "type": "image", "data": "aGk=", "mimeType": "image/png" },
                    { "type": "audio", "data": "xx", "mimeType": "audio/wav" },
                    { "type": "text", "text": "第二段" }
                ],
                "structuredContent": { "count": 2 },
                "isError": false
            }),
        };
        write_json(out, json!({ "jsonrpc": "2.0", "id": id, "result": result }));
        true
    });
    let (_seen, _server) = serve(wire, handler);
    let connected = handshake(conn).expect("handshake");
    let cancel = AtomicBool::new(false);

    let outcome = connected
        .client
        .call_tool("rich", &json!({}), &cancel)
        .expect("rich 调用");
    assert_eq!(outcome.text, "第一段\n\n第二段");
    assert_eq!(outcome.images.len(), 1);
    assert_eq!(outcome.images[0].mime, "image/png");
    assert_eq!(outcome.dropped.len(), 1, "{:?}", outcome.dropped);
    assert_eq!(outcome.structured, Some(json!({ "count": 2 })));

    let outcome = connected
        .client
        .call_tool("fail", &json!({}), &cancel)
        .expect("isError 属于协议层成功");
    assert!(outcome.is_error);
    assert_eq!(outcome.text, "boom");

    let failure = connected
        .client
        .call_tool("mrtr", &json!({}), &cancel)
        .expect_err("input_required 不受支持");
    assert!(
        matches!(failure, CallFailure::Protocol { .. }),
        "{failure:?}"
    );
}

#[test]
fn list_changed_notification_sets_flag() {
    let (conn, wire) = wire();
    let handler = modern_handler(|message, out| {
        let id = message.get("id").cloned().unwrap_or(Value::Null);
        write_json(
            out,
            json!({ "jsonrpc": "2.0", "method": "notifications/tools/list_changed" }),
        );
        write_json(
            out,
            json!({ "jsonrpc": "2.0", "id": id, "result": {
                "resultType": "complete", "content": [], "isError": false } }),
        );
        true
    });
    let (_seen, _server) = serve(wire, handler);
    let connected = handshake(conn).expect("handshake");
    let cancel = AtomicBool::new(false);
    let _ = connected.client.call_tool("echo", &json!({}), &cancel);
    assert!(wait_until(Duration::from_secs(2), || {
        connected.client.take_list_changed()
    }));
}

#[test]
fn legacy_server_initiated_requests_get_replies() {
    let (conn, wire) = wire();
    let mut inner = legacy_handler();
    let handler: Handler = Box::new(move |message, out| {
        let keep = inner(message, out);
        if method_of(message) == Some("notifications/initialized") {
            // 握手完成后 server 立即发起 ping 与一个 client 不支持的请求。
            write_json(
                out,
                json!({ "jsonrpc": "2.0", "id": "srv-ping", "method": "ping" }),
            );
            write_json(
                out,
                json!({ "jsonrpc": "2.0", "id": "srv-roots", "method": "roots/list" }),
            );
        }
        keep
    });
    let (seen, _server) = serve(wire, handler);
    let _connected = handshake(conn).expect("handshake");

    assert!(
        wait_until(Duration::from_secs(2), || {
            let seen = seen.lock().unwrap();
            let ping_ok = seen
                .iter()
                .any(|m| m.get("id") == Some(&json!("srv-ping")) && m.get("result").is_some());
            let roots_rejected = seen.iter().any(|m| {
                m.get("id") == Some(&json!("srv-roots"))
                    && m["error"]["code"].as_i64() == Some(-32601)
            });
            ping_ok && roots_rejected
        }),
        "ping 应得到空结果,未知请求应得到 -32601: {:?}",
        seen.lock().unwrap()
    );
}

#[test]
fn decode_base64_roundtrips_and_rejects_garbage() {
    assert_eq!(decode_base64("aGk=").unwrap(), b"hi");
    assert_eq!(decode_base64("aGVsbG8sIHdvcmxk").unwrap(), b"hello, world");
    assert_eq!(decode_base64("aGVs\nbG8=").unwrap(), b"hello");
    assert_eq!(decode_base64("").unwrap(), Vec::<u8>::new());
    assert!(decode_base64("a").is_err());
    assert!(decode_base64("@@@@").is_err());
    assert!(decode_base64("aa=a").is_err());
}

#[test]
fn import_module_is_linked() {
    // import 自己的测试在 import.rs;这里只固定模块导出形状。
    let report = import::import_tools("srv", Vec::new(), None, &[], &mut |_| false);
    assert!(report.tools.is_empty());
}

// ---- 真实子进程的生命周期测试(仅 Windows;fixture 用 PowerShell 脚本) ----

#[cfg(windows)]
mod process_lifecycle {
    use super::*;
    use std::path::PathBuf;

    fn fixture(script: &str) -> (PathBuf, Vec<String>) {
        static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "onemore-mcp-fixture-{}-{}.ps1",
            std::process::id(),
            n
        ));
        let mut bytes = vec![0xEF, 0xBB, 0xBF];
        bytes.extend_from_slice(script.as_bytes());
        std::fs::write(&path, bytes).expect("write fixture");
        let args = vec![
            "-NoProfile".to_string(),
            "-NonInteractive".to_string(),
            "-ExecutionPolicy".to_string(),
            "Bypass".to_string(),
            "-File".to_string(),
            path.display().to_string(),
        ];
        (path, args)
    }

    #[test]
    fn shutdown_after_stdin_eof_is_graceful() {
        let (path, args) = fixture(
            "while ($true) { $line = [Console]::In.ReadLine(); if ($null -eq $line) { exit 0 } }",
        );
        let conn = Connection::spawn("powershell.exe", &args, &[], None).expect("spawn");
        // 等进程真正跑起来再关,覆盖"读循环中收到 EOF"的路径。
        std::thread::sleep(Duration::from_millis(500));
        let started = Instant::now();
        conn.shutdown();
        assert!(
            started.elapsed() < Duration::from_secs(10),
            "关停应有界: {:?}",
            started.elapsed()
        );
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn shutdown_force_kills_a_hung_process() {
        // 该进程不读 stdin,EOF 对它无效,只能强杀。
        let (path, args) = fixture("while ($true) { Start-Sleep -Seconds 60 }");
        let conn = Connection::spawn("powershell.exe", &args, &[], None).expect("spawn");
        std::thread::sleep(Duration::from_millis(500));
        let started = Instant::now();
        conn.shutdown();
        let elapsed = started.elapsed();
        assert!(elapsed < Duration::from_secs(15), "强杀应有界: {elapsed:?}");
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn stderr_is_captured_into_the_tail_buffer() {
        let (path, args) = fixture(
            "[Console]::Error.WriteLine('boot diagnostics 123'); \
             while ($true) { $line = [Console]::In.ReadLine(); if ($null -eq $line) { exit 0 } }",
        );
        let conn = Connection::spawn("powershell.exe", &args, &[], None).expect("spawn");
        assert!(
            wait_until(Duration::from_secs(10), || {
                conn.stderr_tail()
                    .iter()
                    .any(|line| line.contains("boot diagnostics 123"))
            }),
            "stderr 应进入环形缓冲: {:?}",
            conn.stderr_tail()
        );
        conn.shutdown();
        let _ = std::fs::remove_file(path);
    }
}

// ---- 真实 @playwright/mcp 的端到端验收(需要本机全局安装,故 ignore) ----
// 运行: cargo test --lib mcp:: -- --ignored --nocapture

#[cfg(windows)]
mod playwright_e2e {
    use super::*;
    use crate::config::McpServerConfig;

    fn playwright_config() -> McpServerConfig {
        McpServerConfig {
            name: "playwright".into(),
            command: "cmd".into(),
            args: vec!["/c".into(), "playwright-mcp".into(), "--headless".into()],
            env: Vec::new(),
            cwd: None,
            enabled: true,
            startup_timeout: Duration::from_secs(60),
            call_timeout: Duration::from_secs(60),
            always_ask: false,
            include_tools: None,
            exclude_tools: Vec::new(),
        }
    }

    #[test]
    #[ignore = "需要全局安装 @playwright/mcp(npm install -g @playwright/mcp)与可用浏览器"]
    fn playwright_end_to_end_navigate_snapshot_shutdown() {
        let connected =
            super::super::client::McpClient::connect(&playwright_config()).expect("connect");
        eprintln!("era = {}", connected.client.era_label());
        eprintln!("server = {:?}", connected.client.server_label);
        let names: Vec<&str> = connected
            .tools
            .iter()
            .map(|tool| tool.name.as_str())
            .collect();
        eprintln!("tools({}): {:?}", names.len(), names);
        assert!(
            names.iter().any(|name| name.contains("navigate")),
            "{names:?}"
        );

        let cancel = AtomicBool::new(false);
        let navigate = connected
            .client
            .call_tool(
                "browser_navigate",
                &json!({ "url": "about:blank" }),
                &cancel,
            )
            .expect("navigate 调用应到达 server");
        eprintln!(
            "navigate: is_error={} text={}",
            navigate.is_error,
            crate::util::ellipsis(&navigate.text, 300)
        );
        assert!(!navigate.is_error, "{}", navigate.text);

        let snapshot = connected
            .client
            .call_tool("browser_snapshot", &json!({}), &cancel)
            .expect("snapshot 调用应到达 server");
        eprintln!(
            "snapshot: is_error={} chars={}",
            snapshot.is_error,
            snapshot.text.chars().count()
        );
        assert!(!snapshot.is_error, "{}", snapshot.text);
        assert!(!snapshot.text.trim().is_empty());

        connected.client.shutdown();
    }
}
