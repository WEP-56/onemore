#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod config;
mod error;
mod local;
mod rpc;
mod session;
mod state;
mod workspace;

use std::time::Duration;

use rpc::process::{last_snapshot, shutdown, spawn_rpc, stderr_tail, RpcHandle, StartOptions};
use state::RpcState;
use tauri::{AppHandle, Manager, State};

use error::GuiError;

// ── RPC commands ──

#[tauri::command]
async fn rpc_start(
    app: AppHandle,
    state: State<'_, RpcState>,
    options: StartOptions,
) -> Result<serde_json::Value, GuiError> {
    let handle = spawn_rpc(app.clone(), options)?;
    let mut guard = state.inner.lock().unwrap();
    if let Some(old) = guard.replace(handle) {
        drop(tauri::async_runtime::spawn_blocking(move || shutdown(old)));
    }
    Ok(serde_json::json!({ "ok": true }))
}

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
                Err(GuiError::new("request_timeout", format!("request {id} 超时")))
            }
        }
    })
    .await
    .map_err(|e| GuiError::new("join_error", e.to_string()))?
}

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

#[tauri::command]
async fn rpc_snapshot(state: State<'_, RpcState>) -> Result<Option<serde_json::Value>, GuiError> {
    let guard = state.inner.lock().unwrap();
    Ok(guard.as_ref().and_then(last_snapshot))
}

// ── Config commands ──

#[tauri::command]
async fn read_config() -> Result<String, GuiError> {
    tauri::async_runtime::spawn_blocking(|| config::read_config())
        .await
        .map_err(|e| GuiError::new("join_error", e.to_string()))?
}

#[tauri::command]
async fn write_config(content: String) -> Result<(), GuiError> {
    tauri::async_runtime::spawn_blocking(move || config::write_config(&content))
        .await
        .map_err(|e| GuiError::new("join_error", e.to_string()))?
}

// ── Workspace commands ──

#[tauri::command]
async fn list_workspaces() -> Result<workspace::WorkspaceList, GuiError> {
    tauri::async_runtime::spawn_blocking(|| workspace::load_workspaces())
        .await
        .map_err(|e| GuiError::new("join_error", e.to_string()))?
}

#[tauri::command]
async fn add_workspace(path: String) -> Result<workspace::WorkspaceList, GuiError> {
    tauri::async_runtime::spawn_blocking(move || workspace::add_workspace(&path))
        .await
        .map_err(|e| GuiError::new("join_error", e.to_string()))?
}

#[tauri::command]
async fn remove_workspace(path: String) -> Result<workspace::WorkspaceList, GuiError> {
    tauri::async_runtime::spawn_blocking(move || workspace::remove_workspace(&path))
        .await
        .map_err(|e| GuiError::new("join_error", e.to_string()))?
}

// ── Session commands ──

#[tauri::command]
async fn list_all_sessions() -> Result<Vec<session::SessionEntry>, GuiError> {
    tauri::async_runtime::spawn_blocking(|| session::list_all_sessions())
        .await
        .map_err(|e| GuiError::new("join_error", e.to_string()))?
}

// ── Git & file tree commands ──

#[tauri::command]
async fn get_git_status(workspace: String) -> Result<local::GitStatus, GuiError> {
    tauri::async_runtime::spawn_blocking(move || local::git_status(&workspace))
        .await
        .map_err(|e| GuiError::new("join_error", e.to_string()))?
}

#[tauri::command]
async fn get_file_tree(
    workspace: String,
    max_depth: Option<usize>,
) -> Result<Vec<local::FileTreeNode>, GuiError> {
    let depth = max_depth.unwrap_or(3);
    tauri::async_runtime::spawn_blocking(move || local::file_tree(&workspace, depth))
        .await
        .map_err(|e| GuiError::new("join_error", e.to_string()))?
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
            rpc_snapshot,
            read_config,
            write_config,
            list_workspaces,
            add_workspace,
            remove_workspace,
            list_all_sessions,
            get_git_status,
            get_file_tree
        ])
        .build(tauri::generate_context!())
        .expect("error while building tauri application");

    app.run(|app_handle, event| {
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
