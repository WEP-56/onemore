//! Onemore `--rpc` 子进程的生命周期管理：不经 shell 启动、单 writer、
//! stdout JSONL reader、stderr 诊断、pending response 关联、安全 shutdown。

use std::collections::{HashMap, VecDeque};
use std::io::{BufRead, BufReader};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::AtomicU64;
use std::sync::{mpsc, Arc, Mutex};
use std::thread;
use std::time::Duration;

use serde::Deserialize;
use tauri::{AppHandle, Emitter};

use crate::error::GuiError;

use super::events::{RpcEvent, EVENT_NAME};
use super::reader::JsonlReader;
use super::types::{InboundFrame, ProtoError};
use super::writer;

const MAX_FRAME_BYTES: usize = 8 * 1024 * 1024;
const MAX_STDERR_LINES: usize = 400;
const SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(10);

#[derive(Debug, Clone, Deserialize)]
pub struct StartOptions {
    pub executable: String,
    #[serde(default)]
    pub config: Option<String>,
    pub workspace: String,
}

type PendingMap = Arc<Mutex<HashMap<String, mpsc::Sender<Result<serde_json::Value, GuiError>>>>>;

pub struct RpcHandle {
    pub tx_frame: mpsc::Sender<String>,
    pub pending: PendingMap,
    pub child: Arc<Mutex<Option<Child>>>,
    pub done_rx: mpsc::Receiver<()>,
    pub stderr_log: Arc<Mutex<VecDeque<String>>>,
    pub id_counter: Arc<AtomicU64>,
    pub last_snapshot: Arc<Mutex<Option<serde_json::Value>>>,
}

pub fn spawn_rpc(app: AppHandle, options: StartOptions) -> Result<RpcHandle, GuiError> {
    let mut child = spawn_with_fallback(&options)?;

    let stdin = child
        .stdin
        .take()
        .ok_or_else(|| GuiError::new("spawn_failed", "无法接管子进程 stdin"))?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| GuiError::new("spawn_failed", "无法接管子进程 stdout"))?;
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| GuiError::new("spawn_failed", "无法接管子进程 stderr"))?;

    let child = Arc::new(Mutex::new(Some(child)));
    let pending: PendingMap = Arc::new(Mutex::new(HashMap::new()));

    let (tx_frame, rx_frame) = mpsc::channel::<String>();
    writer::spawn_writer(app.clone(), stdin, rx_frame);

    let (done_tx, done_rx) = mpsc::channel::<()>();
    let last_snapshot: Arc<Mutex<Option<serde_json::Value>>> = Arc::new(Mutex::new(None));
    thread::spawn({
        let app = app.clone();
        let child = child.clone();
        let pending = pending.clone();
        let last_snapshot = last_snapshot.clone();
        move || reader_loop(app, stdout, child, pending, last_snapshot, done_tx)
    });

    let stderr_log = Arc::new(Mutex::new(VecDeque::new()));
    thread::spawn({
        let app = app.clone();
        let stderr_log = stderr_log.clone();
        move || stderr_loop(app, stderr, stderr_log)
    });

    tx_frame
        .send("{\"type\":\"hello\",\"version\":1}".to_string())
        .map_err(|_| GuiError::new("io_error", "writer 已退出，无法发送 hello"))?;

    Ok(RpcHandle {
        tx_frame,
        pending,
        child,
        done_rx,
        stderr_log,
        id_counter: Arc::new(AtomicU64::new(0)),
        last_snapshot,
    })
}

pub fn last_snapshot(handle: &RpcHandle) -> Option<serde_json::Value> {
    handle.last_snapshot.lock().unwrap().clone()
}

fn build_command(executable: &str, config: Option<&str>) -> Result<Command, GuiError> {
    let lower = executable.to_ascii_lowercase();
    let is_script = lower.ends_with(".cmd") || lower.ends_with(".bat");
    let mut cmd = if is_script {
        let mut c = Command::new("cmd");
        c.arg("/c").arg(executable);
        c
    } else {
        Command::new(executable)
    };
    cmd.arg("--rpc");
    if let Some(cfg) = config {
        cmd.arg("--config").arg(cfg);
    }
    Ok(cmd)
}

fn spawn_with_fallback(options: &StartOptions) -> Result<Child, GuiError> {
    let first = {
        let mut cmd = build_command(&options.executable, options.config.as_deref())?;
        cmd.current_dir(&options.workspace)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        cmd.spawn()
    };
    match first {
        Ok(child) => Ok(child),
        Err(first_err) => match resolve_script(&options.executable) {
            Some(script) => {
                let mut c = Command::new("cmd");
                c.arg("/c").arg(&script).arg("--rpc");
                if let Some(cfg) = options.config.as_deref() {
                    c.arg("--config").arg(cfg);
                }
                c.current_dir(&options.workspace)
                    .stdin(Stdio::piped())
                    .stdout(Stdio::piped())
                    .stderr(Stdio::piped());
                c.spawn().map_err(|e| {
                    GuiError::new(
                        "spawn_failed",
                        format!(
                            "无法启动 {}（尝试 {} 也失败）: {e}",
                            options.executable,
                            script.display()
                        ),
                    )
                })
            }
            None => Err(GuiError::new(
                "spawn_failed",
                format!("无法启动 {}: {first_err}", options.executable),
            )),
        },
    }
}

fn resolve_script(executable: &str) -> Option<std::path::PathBuf> {
    if executable.contains(['\\', '/']) || executable.contains('.') {
        return None;
    }
    let path_var = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&path_var) {
        for ext in ["cmd", "bat"] {
            let p = dir.join(format!("{executable}.{ext}"));
            if p.is_file() {
                return Some(p);
            }
        }
    }
    None
}

pub fn shutdown(handle: RpcHandle) -> Result<(), GuiError> {
    drop(handle.tx_frame);
    match handle.done_rx.recv_timeout(SHUTDOWN_TIMEOUT) {
        Ok(()) => Ok(()),
        Err(_) => {
            let mut guard = handle.child.lock().unwrap();
            if let Some(c) = guard.as_mut() {
                let _ = c.kill();
                let _ = c.wait();
            }
            Err(GuiError::new(
                "shutdown_timeout",
                "onemore 未在超时内退出，已强制终止",
            ))
        }
    }
}

fn reader_loop(
    app: AppHandle,
    stdout: std::process::ChildStdout,
    child: Arc<Mutex<Option<Child>>>,
    pending: PendingMap,
    last_snapshot: Arc<Mutex<Option<serde_json::Value>>>,
    done_tx: mpsc::Sender<()>,
) {
    let mut reader = JsonlReader::new(BufReader::new(stdout), MAX_FRAME_BYTES);
    let mut fatal: Option<(String, String)> = None;
    loop {
        match reader.next_frame() {
            Ok(Some(value)) => {
                if let Err(e) = handle_frame(&app, value, &pending, &last_snapshot) {
                    fatal = Some(e);
                    break;
                }
            }
            Ok(None) => break,
            Err(e) => {
                fatal = Some(("read_error".into(), e.to_string()));
                break;
            }
        }
    }
    if let Some((code, message)) = fatal {
        let _ = app.emit(EVENT_NAME, RpcEvent::TransportError { code, message });
    }
    let code = wait_child(&child);
    let _ = app.emit(EVENT_NAME, RpcEvent::ProcessExit { code });
    fail_all_pending(&pending);
    let _ = done_tx.send(());
}

fn handle_frame(
    app: &AppHandle,
    value: serde_json::Value,
    pending: &PendingMap,
    last_snapshot: &Arc<Mutex<Option<serde_json::Value>>>,
) -> Result<(), (String, String)> {
    let frame: InboundFrame =
        serde_json::from_value(value).map_err(|e| ("invalid_frame".into(), e.to_string()))?;
    match frame {
        InboundFrame::Hello {
            version,
            server,
            snapshot,
        } => {
            if version != 1 {
                return Err((
                    "version_mismatch".into(),
                    format!("unsupported protocol version {version}"),
                ));
            }
            *last_snapshot.lock().unwrap() = Some(snapshot.clone());
            let _ = app.emit(EVENT_NAME, RpcEvent::Hello { server, snapshot });
        }
        InboundFrame::HelloError { error } => {
            return Err((error.code, error.message));
        }
        InboundFrame::Response {
            id,
            ok,
            result,
            error,
        } => resolve_response(pending, id, ok, result, error)?,
        InboundFrame::Event { event } => {
            if event.get("type").and_then(|t| t.as_str()) == Some("session_snapshot") {
                if let Some(snapshot) = event.get("snapshot").cloned() {
                    *last_snapshot.lock().unwrap() = Some(snapshot);
                }
            }
            let _ = app.emit(EVENT_NAME, RpcEvent::Event { event });
        }
    }
    Ok(())
}

fn resolve_response(
    pending: &PendingMap,
    id: String,
    ok: bool,
    result: Option<serde_json::Value>,
    error: Option<ProtoError>,
) -> Result<(), (String, String)> {
    let mut guard = pending.lock().unwrap();
    match guard.remove(&id) {
        Some(tx) => {
            let res = if ok {
                Ok(result.unwrap_or(serde_json::Value::Null))
            } else {
                Err(GuiError::new(
                    error
                        .as_ref()
                        .map(|e| e.code.clone())
                        .unwrap_or_else(|| "error".into()),
                    error
                        .as_ref()
                        .map(|e| e.message.clone())
                        .unwrap_or_default(),
                ))
            };
            let _ = tx.send(res);
            Ok(())
        }
        None => Err((
            "duplicate_or_unknown_response".into(),
            format!("no pending request for id {id}"),
        )),
    }
}

fn wait_child(child: &Arc<Mutex<Option<Child>>>) -> Option<i32> {
    let mut guard = child.lock().unwrap();
    match guard.as_mut() {
        Some(c) => c.wait().ok().and_then(|s| s.code()),
        None => None,
    }
}

fn fail_all_pending(pending: &PendingMap) {
    let mut guard = pending.lock().unwrap();
    for (_, tx) in guard.drain() {
        let _ = tx.send(Err(GuiError::new(
            "disconnected",
            "transport 已关闭，请求未完成",
        )));
    }
}

fn stderr_loop(
    app: AppHandle,
    stderr: std::process::ChildStderr,
    log: Arc<Mutex<VecDeque<String>>>,
) {
    let reader = BufReader::new(stderr);
    for line in reader.lines() {
        let line = line.unwrap_or_else(|_| "[invalid utf-8 stderr line]".into());
        {
            let mut g = log.lock().unwrap();
            g.push_back(line.clone());
            while g.len() > MAX_STDERR_LINES {
                g.pop_front();
            }
        }
        let _ = app.emit(EVENT_NAME, RpcEvent::Stderr { line });
    }
}

pub fn stderr_tail(handle: &RpcHandle, limit: usize) -> Vec<String> {
    let g = handle.stderr_log.lock().unwrap();
    g.iter().rev().take(limit).rev().cloned().collect()
}
