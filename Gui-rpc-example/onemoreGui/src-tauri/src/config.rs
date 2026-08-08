//! 读写 onemore 的 config.toml（位于 roaming data dir / onemore / config.toml）。
//! GUI 不做校验，只做读写透传：读取时返回原始文本，写入时整文件覆盖。

use std::fs;
use std::path::PathBuf;

use crate::error::GuiError;

/// 返回 onemore 数据目录（roaming/onemore）。
pub fn onemore_data_dir() -> Result<PathBuf, GuiError> {
    let base = dirs::data_dir()
        .ok_or_else(|| GuiError::new("no_data_dir", "无法确定系统数据目录"))?;
    Ok(base.join("onemore"))
}

/// config.toml 的完整路径。
pub fn config_path() -> Result<PathBuf, GuiError> {
    Ok(onemore_data_dir()?.join("config.toml"))
}

/// sessions 目录路径。
pub fn sessions_dir() -> Result<PathBuf, GuiError> {
    Ok(onemore_data_dir()?.join("sessions"))
}

/// workspaces 目录路径（onemore 原生 workspace prefs，暂未使用）。
#[allow(dead_code)]
pub fn workspaces_dir() -> Result<PathBuf, GuiError> {
    Ok(onemore_data_dir()?.join("workspaces"))
}

/// 读取 config.toml 原文。
pub fn read_config() -> Result<String, GuiError> {
    let path = config_path()?;
    if !path.exists() {
        return Ok(String::new());
    }
    fs::read_to_string(&path).map_err(|e| GuiError::new("read_config", e.to_string()))
}

/// 整文件覆盖写入 config.toml。
pub fn write_config(content: &str) -> Result<(), GuiError> {
    let path = config_path()?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| GuiError::new("write_config", e.to_string()))?;
    }
    fs::write(&path, content).map_err(|e| GuiError::new("write_config", e.to_string()))
}
