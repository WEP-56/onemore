//! 工作区管理：持久化工作区路径列表，供左栏展示和切换。
//! 存储在 roaming/onemore/gui-workspaces.json。

use std::fs;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::config::onemore_data_dir;
use crate::error::GuiError;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkspaceEntry {
    pub path: String,
    pub label: String,
    pub last_used: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct WorkspaceList {
    pub workspaces: Vec<WorkspaceEntry>,
}

fn store_path() -> Result<PathBuf, GuiError> {
    Ok(onemore_data_dir()?.join("gui-workspaces.json"))
}

pub fn load_workspaces() -> Result<WorkspaceList, GuiError> {
    let path = store_path()?;
    if !path.exists() {
        return Ok(WorkspaceList::default());
    }
    let text =
        fs::read_to_string(&path).map_err(|e| GuiError::new("load_workspaces", e.to_string()))?;
    if text.trim().is_empty() {
        return Ok(WorkspaceList::default());
    }
    serde_json::from_str(&text).map_err(|e| GuiError::new("load_workspaces", e.to_string()))
}

pub fn save_workspaces(list: &WorkspaceList) -> Result<(), GuiError> {
    let path = store_path()?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| GuiError::new("save_workspaces", e.to_string()))?;
    }
    let text =
        serde_json::to_string_pretty(list).map_err(|e| GuiError::new("save_workspaces", e.to_string()))?;
    fs::write(&path, text).map_err(|e| GuiError::new("save_workspaces", e.to_string()))
}

/// 添加或更新工作区（已存在则更新 label + last_used）。
pub fn add_workspace(path: &str) -> Result<WorkspaceList, GuiError> {
    let mut list = load_workspaces()?;
    let canonical = dunce::canonicalize(path)
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_else(|_| path.to_string());

    let label = std::path::Path::new(&canonical)
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| canonical.clone());

    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);

    if let Some(entry) = list.workspaces.iter_mut().find(|w| w.path == canonical) {
        entry.label = label;
        entry.last_used = now;
    } else {
        list.workspaces.push(WorkspaceEntry {
            path: canonical,
            label,
            last_used: now,
        });
    }

    save_workspaces(&list)?;
    Ok(list)
}

/// 删除工作区（仅从列表移除，不删文件）。
pub fn remove_workspace(path: &str) -> Result<WorkspaceList, GuiError> {
    let mut list = load_workspaces()?;
    list.workspaces.retain(|w| w.path != path);
    save_workspaces(&list)?;
    Ok(list)
}
