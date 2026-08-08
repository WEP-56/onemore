//! 工作区管理:持久化工作区路径列表 + 分组,供左栏展示和切换。
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
    #[serde(default)]
    pub group_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkspaceGroup {
    pub id: String,
    pub name: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct WorkspaceList {
    pub workspaces: Vec<WorkspaceEntry>,
    #[serde(default)]
    pub groups: Vec<WorkspaceGroup>,
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

/// 添加或更新工作区(已存在则更新 label + last_used)。
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
            group_id: None,
        });
    }

    save_workspaces(&list)?;
    Ok(list)
}

/// 删除工作区(仅从列表移除,不删文件)。
pub fn remove_workspace(path: &str) -> Result<WorkspaceList, GuiError> {
    let mut list = load_workspaces()?;
    list.workspaces.retain(|w| w.path != path);
    save_workspaces(&list)?;
    Ok(list)
}

/// 重命名工作区(仅改 label)。
pub fn rename_workspace(path: &str, label: &str) -> Result<WorkspaceList, GuiError> {
    let mut list = load_workspaces()?;
    if let Some(entry) = list.workspaces.iter_mut().find(|w| w.path == path) {
        entry.label = label.to_string();
    }
    save_workspaces(&list)?;
    Ok(list)
}

/// 新建分组。
pub fn create_group(name: &str) -> Result<WorkspaceList, GuiError> {
    let mut list = load_workspaces()?;
    let id = format!(
        "g-{}",
        uuid::Uuid::new_v4().simple()
    );
    list.groups.push(WorkspaceGroup { id, name: name.to_string() });
    save_workspaces(&list)?;
    Ok(list)
}

/// 重命名分组。
pub fn rename_group(id: &str, name: &str) -> Result<WorkspaceList, GuiError> {
    let mut list = load_workspaces()?;
    if let Some(group) = list.groups.iter_mut().find(|g| g.id == id) {
        group.name = name.to_string();
    }
    save_workspaces(&list)?;
    Ok(list)
}

/// 删除分组(组内工作区回到未分组)。
pub fn delete_group(id: &str) -> Result<WorkspaceList, GuiError> {
    let mut list = load_workspaces()?;
    list.groups.retain(|g| g.id != id);
    for w in list.workspaces.iter_mut() {
        if w.group_id.as_deref() == Some(id) {
            w.group_id = None;
        }
    }
    save_workspaces(&list)?;
    Ok(list)
}

/// 分配/移动工作区到分组(group_id 传空字符串表示取消分组)。
pub fn assign_group(path: &str, group_id: &str) -> Result<WorkspaceList, GuiError> {
    let mut list = load_workspaces()?;
    if let Some(entry) = list.workspaces.iter_mut().find(|w| w.path == path) {
        entry.group_id = if group_id.is_empty() { None } else { Some(group_id.to_string()) };
    }
    save_workspaces(&list)?;
    Ok(list)
}
