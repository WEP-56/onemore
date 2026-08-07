#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod error;
mod rpc;
mod state;

use std::time::Duration;

use rpc::process::{last_snapshot, shutdown, spawn_rpc, stderr_tail, RpcHandle, StartOptions};
use state::RpcState;
use tauri::{AppHandle, Manager, State};

use error::GuiError;

/// 启动 RPC 子进程并完成 hello（hello 帧通过事件流送达前端）。
#[tauri::command]
async fn rpc_start(
    app: AppHandle,
    state: State<'_, RpcState>,
    options: StartOptions,
) -> Result<serde_json::Value, GuiError> {
    let handle = spawn_rpc(app.clone(), options)?;
    // hello 帧可能在任何时刻到达，这里只负责注册 handle；前端等待 hello 事件
    let mut guard = state.inner.lock().unwrap();
    if let Some(old) = guard.replace(handle) {
        // 理论不可达：前端必须先 stop 才能 start
        drop(tauri::async_runtime::spawn_blocking(move || shutdown(old)));
    }
    Ok(serde_json::json!({ "ok": true }))
}

/// 发送一个 RPC request，等待对应 response（request id 由 backend 分配）。
#[tauri::command]
async fn rpc_request(
    state: State<'_, RpcState>,
    command: String,
    params: Option<serde_json::Value>,
) -> Result<serde_json::Value, GuiError> {
    let handle = state
        .inner
        .lock()
        .unwrap()
        .as_ref()
        .map(|h| (h.tx_frame.clone(), h.pending.clone(), h.id_counter.clone()))
        .ok_or_else(|| GuiError::new("not_connected", "尚未连接到 onemore"))?;
    let (tx_frame, pending, id_counter) = handle;

    let id = format!(
        "req-{}",
        id_counter.fetch_add(1, std::sync::atomic::Ordering::SeqCst)
    );
    let frame = rpc::types::request_frame(&id, &command, params.as_ref());

    tauri::async_runtime::spawn_blocking(move || {
        let (tx, rx) = std::sync::mpsc::channel::<Result<serde_json::Value, GuiError>>();
        pending.lock().unwrap().insert(id.clone(), tx);
        tx_frame
            .send(frame)
            .map_err(|_| GuiError::new("disconnected", "transport 已关闭"))?;
        match rx.recv_timeout(Duration::from_secs(300)) {
            Ok(res) => res,
            Err(_) => {
                pending.lock().unwrap().remove(&id);
                Err(GuiError::new(
                    "request_timeout",
                    format!("request {id} 超时"),
                ))
            }
        }
    })
    .await
    .map_err(|e| GuiError::new("join_error", e.to_string()))?
}

/// 停止 RPC：stdin EOF → 等待安全退出 → 超时强制终止。
#[tauri::command]
async fn rpc_stop(state: State<'_, RpcState>) -> Result<(), GuiError> {
    let handle: RpcHandle = state
        .inner
        .lock()
        .unwrap()
        .take()
        .ok_or_else(|| GuiError::new("not_connected", "尚未连接到 onemore"))?;
    tauri::async_runtime::spawn_blocking(move || shutdown(handle))
        .await
        .map_err(|e| GuiError::new("join_error", e.to_string()))?
}

/// stderr 诊断日志尾部。
#[tauri::command]
async fn rpc_diagnostics_tail(
    state: State<'_, RpcState>,
    limit: usize,
) -> Result<Vec<String>, GuiError> {
    let guard = state.inner.lock().unwrap();
    match guard.as_ref() {
        Some(h) => Ok(stderr_tail(h, limit.max(1))),
        None => Ok(Vec::new()),
    }
}

/// 最近一次权威 snapshot（供前端热重载/刷新后重建画面；backend 仍持有原 transport）。
#[tauri::command]
async fn rpc_snapshot(state: State<'_, RpcState>) -> Result<Option<serde_json::Value>, GuiError> {
    let guard = state.inner.lock().unwrap();
    Ok(guard.as_ref().and_then(last_snapshot))
}

fn main() {
    let app = tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .manage(RpcState::default())
        .invoke_handler(tauri::generate_handler![
            rpc_start,
            rpc_request,
            rpc_stop,
            rpc_diagnostics_tail,
            rpc_snapshot
        ])
        .build(tauri::generate_context!())
        .expect("error while building tauri application");

    app.run(|app_handle, event| {
        // 窗口/应用退出时确保子进程被安全收尾，不留遗留进程
        if let tauri::RunEvent::Exit = event {
            let handle = {
                let state = app_handle.state::<RpcState>();
                let mut guard = state.inner.lock().unwrap();
                guard.take()
            };
            if let Some(handle) = handle {
                let _ = shutdown(handle);
            }
        }
    });
}
