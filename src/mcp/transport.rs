//! MCP stdio transport：行框架读写、reader 线程与 id 路由、stderr 环形缓冲、
//! 关停强杀。
//!
//! 框架规则(规范 stdio binding):换行分隔的 JSON-RPC,UTF-8,单条消息不含内嵌
//! 换行;stdout 上只允许合法 MCP 消息,stderr 是 server 日志。实现要点:
//! - 写端在互斥锁内,reader 线程也用它回复 legacy server 的 ping/未知请求;
//! - reader 线程按数字 id 把响应投递到 pending 槽位,调用线程用条件变量 +
//!   有界超时片等待,因此取消不依赖可中断 I/O(杀进程即可解除 reader 阻塞);
//! - 单行超上限、stdout 噪声行过多、读写失败都把连接置为不可逆故障,
//!   所有在等与后续的请求立即得到稳定错误;
//! - 关停顺序:关 stdin → 有界等待退出 → Job Object 强杀整树 → join 线程。
//!
//! 通用性:核心只依赖 `Read`/`Write`,测试用 `std::io::pipe` 内存管道驱动
//! 脚本化 server,进程生命周期单独用真实子进程覆盖。

use std::collections::{HashMap, VecDeque};
use std::io::{BufRead, BufReader, Read, Write};
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use serde_json::Value;

use super::protocol::{self, RpcError, ServerMessage};
use crate::process::{kill_tree, ProcessJob};

/// 单条消息(一行)的字节上限;超限视为协议故障,防止敌意 server 撑爆内存。
pub(crate) const MAX_LINE_BYTES: usize = 8 * 1024 * 1024;
/// stdout 上非 MCP 消息行的容忍上限(npm banner 等);超过后判定协议故障。
const MAX_NOISE_LINES: usize = 20;
const STDERR_TAIL_LINES: usize = 100;
const STDERR_LINE_MAX_BYTES: usize = 2048;
/// 关 stdin 后等待 server 自行退出的宽限期,逾期强杀进程树。
const SHUTDOWN_GRACE: Duration = Duration::from_millis(2000);
/// 等待响应时的取消检查粒度。
const WAIT_SLICE: Duration = Duration::from_millis(25);

/// 等待一次响应的终局。
#[derive(Debug)]
pub(crate) enum WaitOutcome {
    Done(Result<Value, RpcError>),
    /// 取消标志被置位;调用方负责发送 `notifications/cancelled`。
    Cancelled,
    /// 到达调用方给出的 deadline;调用方负责发送 `notifications/cancelled`。
    TimedOut,
    /// 连接已故障(server 退出、协议违规、写失败),原因供诊断。
    Failed(String),
}

struct ConnState {
    /// 在途请求。`None` = 等待中,`Some` = 已到达待取走;被放弃的 id 直接移除,
    /// 迟到响应因查不到槽位而被丢弃。
    pending: HashMap<u64, Option<Result<Value, RpcError>>>,
    /// 一旦置位不可恢复;后续所有请求与等待立即失败。
    failed: Option<String>,
    noise_lines: usize,
    /// legacy server 自发的 `notifications/tools/list_changed`;上层轮询后转 notice。
    list_changed: bool,
}

struct Shared {
    state: Mutex<ConnState>,
    wake: Condvar,
}

impl Shared {
    fn fail(&self, reason: impl Into<String>) {
        let mut state = self.state.lock().unwrap();
        if state.failed.is_none() {
            state.failed = Some(reason.into());
        }
        drop(state);
        self.wake.notify_all();
    }
}

type SharedWriter = Arc<Mutex<Option<Box<dyn Write + Send>>>>;

struct Teardown {
    process: Option<(Child, Option<ProcessJob>)>,
    reader: Option<JoinHandle<()>>,
    stderr: Option<JoinHandle<()>>,
}

/// 一条 stdio MCP 连接。所有方法 `&self`:关停通过内部一次性取出实现,
/// 允许被多个 `Arc` 持有者共享(工具代理与 host)。
pub(crate) struct Connection {
    shared: Arc<Shared>,
    writer: SharedWriter,
    stderr_tail: Arc<Mutex<VecDeque<String>>>,
    next_id: AtomicU64,
    teardown: Mutex<Teardown>,
    closed: AtomicBool,
}

impl Connection {
    /// spawn 子进程并接管标准流。不经 shell 解析;Windows 上 npm 系 server
    /// 需要 config 写成 `command = "cmd", args = ["/c", "npx", ...]`。
    pub(crate) fn spawn(
        command: &str,
        args: &[String],
        env: &[(String, String)],
        cwd: Option<&Path>,
    ) -> std::io::Result<Connection> {
        let mut cmd = Command::new(command);
        cmd.args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        for (key, value) in env {
            cmd.env(key, value);
        }
        if let Some(cwd) = cwd {
            cmd.current_dir(cwd);
        }
        let mut child = cmd.spawn()?;
        let job = ProcessJob::attach(&child).ok();
        let stdin = child.stdin.take().expect("stdin was piped");
        let stdout = child.stdout.take().expect("stdout was piped");
        let stderr = child.stderr.take().expect("stderr was piped");
        Ok(Self::assemble(
            Box::new(stdin),
            Box::new(stdout),
            Some(Box::new(stderr)),
            Some((child, job)),
        ))
    }

    /// 从任意字节流组装连接(测试用内存管道;生产路径经 [`Connection::spawn`])。
    #[cfg(test)]
    pub(crate) fn from_streams(
        writer: Box<dyn Write + Send>,
        reader: Box<dyn Read + Send>,
    ) -> Connection {
        Self::assemble(writer, reader, None, None)
    }

    fn assemble(
        writer: Box<dyn Write + Send>,
        reader: Box<dyn Read + Send>,
        stderr: Option<Box<dyn Read + Send>>,
        process: Option<(Child, Option<ProcessJob>)>,
    ) -> Connection {
        let shared = Arc::new(Shared {
            state: Mutex::new(ConnState {
                pending: HashMap::new(),
                failed: None,
                noise_lines: 0,
                list_changed: false,
            }),
            wake: Condvar::new(),
        });
        let writer: SharedWriter = Arc::new(Mutex::new(Some(writer)));
        let stderr_tail = Arc::new(Mutex::new(VecDeque::new()));

        let reader_handle = {
            let shared = Arc::clone(&shared);
            let writer = Arc::clone(&writer);
            std::thread::spawn(move || run_reader(reader, shared, writer))
        };
        let stderr_handle = stderr.map(|stderr| {
            let tail = Arc::clone(&stderr_tail);
            std::thread::spawn(move || run_stderr(stderr, tail))
        });

        Connection {
            shared,
            writer,
            stderr_tail,
            next_id: AtomicU64::new(1),
            teardown: Mutex::new(Teardown {
                process,
                reader: Some(reader_handle),
                stderr: stderr_handle,
            }),
            closed: AtomicBool::new(false),
        }
    }

    pub(crate) fn allocate_id(&self) -> u64 {
        self.next_id.fetch_add(1, Ordering::Relaxed)
    }

    /// 注册 pending 槽位并写出请求。写失败会把连接置为故障。
    pub(crate) fn send_request(&self, id: u64, method: &str, params: Value) -> Result<(), String> {
        {
            let mut state = self.shared.state.lock().unwrap();
            if let Some(reason) = &state.failed {
                return Err(reason.clone());
            }
            state.pending.insert(id, None);
        }
        let message = protocol::request(id, method, params);
        if let Err(error) = write_line(&self.writer, &message) {
            self.shared.state.lock().unwrap().pending.remove(&id);
            self.shared.fail(format!("写入 server 失败: {error}"));
            return Err(error);
        }
        Ok(())
    }

    /// 等待响应:条件变量 + 有界超时片,每片检查取消标志、deadline 与连接故障。
    /// 除 `Done` 外的所有出路都会移除槽位,使迟到响应被 reader 丢弃。
    pub(crate) fn wait_response(
        &self,
        id: u64,
        deadline: Instant,
        cancel: Option<&AtomicBool>,
    ) -> WaitOutcome {
        let mut state = self.shared.state.lock().unwrap();
        loop {
            if matches!(state.pending.get(&id), Some(Some(_))) {
                let result = state
                    .pending
                    .remove(&id)
                    .flatten()
                    .expect("slot checked as done");
                return WaitOutcome::Done(result);
            }
            if let Some(reason) = &state.failed {
                let reason = reason.clone();
                state.pending.remove(&id);
                return WaitOutcome::Failed(reason);
            }
            if cancel.is_some_and(|flag| flag.load(Ordering::Relaxed)) {
                state.pending.remove(&id);
                return WaitOutcome::Cancelled;
            }
            if Instant::now() >= deadline {
                state.pending.remove(&id);
                return WaitOutcome::TimedOut;
            }
            let (next, _) = self.shared.wake.wait_timeout(state, WAIT_SLICE).unwrap();
            state = next;
        }
    }

    pub(crate) fn send_notification(&self, method: &str, params: Value) -> Result<(), String> {
        let message = protocol::notification(method, params);
        write_line(&self.writer, &message).map_err(|error| {
            self.shared.fail(format!("写入 server 失败: {error}"));
            error
        })
    }

    /// 连接是否已不可用;`Some(原因)` 供工具错误与 `/mcp` 展示。
    pub(crate) fn failure(&self) -> Option<String> {
        self.shared.state.lock().unwrap().failed.clone()
    }

    /// 取走并清除 list_changed 标志(server 可能再次发送,届时再提醒一次)。
    pub(crate) fn take_list_changed(&self) -> bool {
        std::mem::take(&mut self.shared.state.lock().unwrap().list_changed)
    }

    pub(crate) fn stderr_tail(&self) -> Vec<String> {
        self.stderr_tail.lock().unwrap().iter().cloned().collect()
    }

    /// 幂等关停:关 stdin → 有界等待 → 强杀进程树 → join 读线程。
    /// 进程消亡使管道关闭,reader 的阻塞读自然解除,因此 join 是有界的。
    pub(crate) fn shutdown(&self) {
        if self.closed.swap(true, Ordering::SeqCst) {
            return;
        }
        *self.writer.lock().unwrap() = None;
        let mut teardown = self.teardown.lock().unwrap();
        if let Some((mut child, job)) = teardown.process.take() {
            let waited = Instant::now();
            loop {
                match child.try_wait() {
                    Ok(Some(_)) => break,
                    Ok(None) if waited.elapsed() >= SHUTDOWN_GRACE => {
                        kill_tree(&mut child, job.as_ref());
                        let _ = child.wait();
                        break;
                    }
                    Ok(None) => std::thread::sleep(Duration::from_millis(20)),
                    Err(_) => {
                        kill_tree(&mut child, job.as_ref());
                        let _ = child.wait();
                        break;
                    }
                }
            }
        }
        if let Some(handle) = teardown.reader.take() {
            let _ = handle.join();
        }
        if let Some(handle) = teardown.stderr.take() {
            let _ = handle.join();
        }
        drop(teardown);
        self.shared.fail("连接已关闭");
    }
}

impl Drop for Connection {
    fn drop(&mut self) {
        self.shutdown();
    }
}

fn write_line(writer: &SharedWriter, message: &Value) -> Result<(), String> {
    let mut text = serde_json::to_string(message).map_err(|error| error.to_string())?;
    // serde_json 对字符串内换行做转义,单行框架天然成立。
    debug_assert!(!text.contains('\n'));
    text.push('\n');
    let mut guard = writer.lock().unwrap();
    let Some(writer) = guard.as_mut() else {
        return Err("连接已关闭".into());
    };
    writer
        .write_all(text.as_bytes())
        .and_then(|_| writer.flush())
        .map_err(|error| error.to_string())
}

fn run_reader(reader: Box<dyn Read + Send>, shared: Arc<Shared>, writer: SharedWriter) {
    let mut reader = BufReader::new(reader);
    let mut line = Vec::new();
    loop {
        line.clear();
        match read_line_bounded(&mut reader, &mut line, MAX_LINE_BYTES) {
            Ok(ReadLine::Eof) => {
                shared.fail("server 关闭了输出流");
                return;
            }
            Ok(ReadLine::Line) => {}
            Err(ReadLineError::TooLong) => {
                shared.fail(format!("单条消息超过 {} 字节上限", MAX_LINE_BYTES));
                return;
            }
            Err(ReadLineError::Io(error)) => {
                shared.fail(format!("读取 server 输出失败: {error}"));
                return;
            }
        }
        let text = String::from_utf8_lossy(&line);
        let text = text.trim();
        if text.is_empty() {
            continue;
        }
        match protocol::parse_server_message(text) {
            Ok(ServerMessage::Response { id, result }) => {
                let mut state = shared.state.lock().unwrap();
                if let Some(slot) = state.pending.get_mut(&id) {
                    *slot = Some(result);
                    drop(state);
                    shared.wake.notify_all();
                }
                // 查不到槽位 = 已放弃(取消/超时)的迟到响应,按预期丢弃。
            }
            Ok(ServerMessage::Notification { method }) => {
                if method == "notifications/tools/list_changed" {
                    shared.state.lock().unwrap().list_changed = true;
                }
                // 其余通知(progress、message 等)v1 不消费,一律忽略。
            }
            Ok(ServerMessage::Request { id, method }) => {
                // 仅 legacy server 会发起请求。必须回复而不是沉默,否则 server
                // 会挂在等待上:ping 回空结果,其余按 JSON-RPC 回 method-not-found。
                let reply = if method == "ping" {
                    serde_json::json!({ "jsonrpc": "2.0", "id": id, "result": {} })
                } else {
                    serde_json::json!({
                        "jsonrpc": "2.0",
                        "id": id,
                        "error": {
                            "code": protocol::ERROR_METHOD_NOT_FOUND,
                            "message": format!("onemore 不支持 server 发起的 {method}"),
                        }
                    })
                };
                let _ = write_line(&writer, &reply);
            }
            Err(_) => {
                let mut state = shared.state.lock().unwrap();
                state.noise_lines += 1;
                let overflow = state.noise_lines > MAX_NOISE_LINES;
                drop(state);
                if overflow {
                    shared.fail("stdout 出现过多非 MCP 消息行");
                    return;
                }
            }
        }
    }
}

fn run_stderr(reader: Box<dyn Read + Send>, tail: Arc<Mutex<VecDeque<String>>>) {
    let mut reader = BufReader::new(reader);
    let mut line = Vec::new();
    loop {
        line.clear();
        match read_line_bounded(&mut reader, &mut line, STDERR_LINE_MAX_BYTES) {
            Ok(ReadLine::Eof) => return,
            Ok(ReadLine::Line) => {
                let text = String::from_utf8_lossy(&line);
                push_stderr_line(&tail, crate::util::sanitize(text.trim_end()));
            }
            Err(ReadLineError::TooLong) => {
                push_stderr_line(&tail, "(超长 stderr 行已截断)".to_string());
                if skip_to_newline(&mut reader).is_err() {
                    return;
                }
            }
            Err(ReadLineError::Io(_)) => return,
        }
    }
}

fn push_stderr_line(tail: &Mutex<VecDeque<String>>, line: String) {
    if line.is_empty() {
        return;
    }
    let mut tail = tail.lock().unwrap();
    if tail.len() >= STDERR_TAIL_LINES {
        tail.pop_front();
    }
    tail.push_back(line);
}

enum ReadLine {
    Line,
    Eof,
}

enum ReadLineError {
    TooLong,
    Io(std::io::Error),
}

/// 读一行(不含换行符)进 `buf`,超过 `cap` 报 `TooLong`。EOF 前的最后一段
/// 允许没有换行符,按一行处理;`buf` 为空时的 EOF 是干净结束。
fn read_line_bounded(
    reader: &mut impl BufRead,
    buf: &mut Vec<u8>,
    cap: usize,
) -> Result<ReadLine, ReadLineError> {
    loop {
        let available = match reader.fill_buf() {
            Ok(available) => available,
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(error) => return Err(ReadLineError::Io(error)),
        };
        if available.is_empty() {
            return if buf.is_empty() {
                Ok(ReadLine::Eof)
            } else {
                Ok(ReadLine::Line)
            };
        }
        match available.iter().position(|&byte| byte == b'\n') {
            Some(position) => {
                if buf.len() + position > cap {
                    return Err(ReadLineError::TooLong);
                }
                buf.extend_from_slice(&available[..position]);
                reader.consume(position + 1);
                return Ok(ReadLine::Line);
            }
            None => {
                let length = available.len();
                if buf.len() + length > cap {
                    return Err(ReadLineError::TooLong);
                }
                buf.extend_from_slice(available);
                reader.consume(length);
            }
        }
    }
}

/// 丢弃当前行剩余字节直到换行符(用于超长 stderr 行之后的重新同步)。
fn skip_to_newline(reader: &mut impl BufRead) -> std::io::Result<()> {
    loop {
        let available = reader.fill_buf()?;
        if available.is_empty() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::UnexpectedEof,
                "eof",
            ));
        }
        match available.iter().position(|&byte| byte == b'\n') {
            Some(position) => {
                reader.consume(position + 1);
                return Ok(());
            }
            None => {
                let length = available.len();
                reader.consume(length);
            }
        }
    }
}
