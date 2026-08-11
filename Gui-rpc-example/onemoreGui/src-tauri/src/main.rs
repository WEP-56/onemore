#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod config;
mod config_edit;
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
    connection_id: String,
    options: StartOptions,
) -> Result<serde_json::Value, GuiError> {
    if state.inner.lock().unwrap().contains_key(&connection_id) {
        return Err(GuiError::new(
            "connection_exists",
            format!("RPC connection {connection_id} 已存在"),
        ));
    }
    let handle = spawn_rpc(app.clone(), connection_id.clone(), options)?;
    let mut guard = state.inner.lock().unwrap();
    if let std::collections::hash_map::Entry::Vacant(entry) = guard.entry(connection_id.clone()) {
        entry.insert(handle);
    } else {
        drop(guard);
        drop(tauri::async_runtime::spawn_blocking(move || {
            shutdown(handle)
        }));
        return Err(GuiError::new("connection_exists", "RPC connection 已存在"));
    }
    Ok(serde_json::json!({ "ok": true, "connection_id": connection_id }))
}

#[tauri::command]
async fn rpc_request(
    state: State<'_, RpcState>,
    connection_id: String,
    command: String,
    params: Option<serde_json::Value>,
) -> Result<serde_json::Value, GuiError> {
    let handle = state
        .inner
        .lock()
        .unwrap()
        .get(&connection_id)
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

#[tauri::command]
async fn rpc_stop(state: State<'_, RpcState>, connection_id: String) -> Result<(), GuiError> {
    let handle: RpcHandle = state
        .inner
        .lock()
        .unwrap()
        .remove(&connection_id)
        .ok_or_else(|| GuiError::new("not_connected", "尚未连接到 onemore"))?;
    tauri::async_runtime::spawn_blocking(move || shutdown(handle))
        .await
        .map_err(|e| GuiError::new("join_error", e.to_string()))?
}

#[tauri::command]
async fn rpc_diagnostics_tail(
    state: State<'_, RpcState>,
    connection_id: String,
    limit: usize,
) -> Result<Vec<String>, GuiError> {
    let guard = state.inner.lock().unwrap();
    match guard.get(&connection_id) {
        Some(h) => Ok(stderr_tail(h, limit.max(1))),
        None => Ok(Vec::new()),
    }
}

#[tauri::command]
async fn rpc_snapshot(
    state: State<'_, RpcState>,
    connection_id: String,
) -> Result<Option<serde_json::Value>, GuiError> {
    let guard = state.inner.lock().unwrap();
    Ok(guard.get(&connection_id).and_then(last_snapshot))
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

#[tauri::command]
async fn get_config_dto() -> Result<config_edit::ConfigDto, GuiError> {
    tauri::async_runtime::spawn_blocking(config_edit::get_config_dto)
        .await
        .map_err(|e| GuiError::new("join_error", e.to_string()))?
}

#[tauri::command]
async fn update_config_dto(dto: config_edit::ConfigDto) -> Result<(), GuiError> {
    tauri::async_runtime::spawn_blocking(move || config_edit::update_config_dto(&dto))
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

#[tauri::command]
async fn rename_workspace(
    path: String,
    label: String,
) -> Result<workspace::WorkspaceList, GuiError> {
    tauri::async_runtime::spawn_blocking(move || workspace::rename_workspace(&path, &label))
        .await
        .map_err(|e| GuiError::new("join_error", e.to_string()))?
}

#[tauri::command]
async fn create_group(name: String) -> Result<workspace::WorkspaceList, GuiError> {
    tauri::async_runtime::spawn_blocking(move || workspace::create_group(&name))
        .await
        .map_err(|e| GuiError::new("join_error", e.to_string()))?
}

#[tauri::command]
async fn rename_group(id: String, name: String) -> Result<workspace::WorkspaceList, GuiError> {
    tauri::async_runtime::spawn_blocking(move || workspace::rename_group(&id, &name))
        .await
        .map_err(|e| GuiError::new("join_error", e.to_string()))?
}

#[tauri::command]
async fn delete_group(id: String) -> Result<workspace::WorkspaceList, GuiError> {
    tauri::async_runtime::spawn_blocking(move || workspace::delete_group(&id))
        .await
        .map_err(|e| GuiError::new("join_error", e.to_string()))?
}

#[tauri::command]
async fn assign_group(
    path: String,
    group_id: String,
) -> Result<workspace::WorkspaceList, GuiError> {
    tauri::async_runtime::spawn_blocking(move || workspace::assign_group(&path, &group_id))
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

#[tauri::command]
async fn rename_session(session_id: String, title: String) -> Result<(), GuiError> {
    tauri::async_runtime::spawn_blocking(move || session::rename_session(&session_id, &title))
        .await
        .map_err(|e| GuiError::new("join_error", e.to_string()))?
}

#[tauri::command]
async fn delete_session(session_id: String) -> Result<(), GuiError> {
    tauri::async_runtime::spawn_blocking(move || session::delete_session(&session_id))
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
            get_config_dto,
            update_config_dto,
            list_workspaces,
            add_workspace,
            remove_workspace,
            rename_workspace,
            create_group,
            rename_group,
            delete_group,
            assign_group,
            list_all_sessions,
            rename_session,
            delete_session,
            get_git_status,
            get_file_tree
        ])
        .build(tauri::generate_context!())
        .expect("error while building tauri application");

    app.run(|app_handle, event| {
        if let tauri::RunEvent::Exit = event {
            let handles = {
                let state = app_handle.state::<RpcState>();
                let mut guard = state.inner.lock().unwrap();
                guard.drain().map(|(_, handle)| handle).collect::<Vec<_>>()
            };
            for handle in handles {
                let _ = shutdown(handle);
            }
        }
    });
}
