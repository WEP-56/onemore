//! 读写 onemore 的 config.toml（位于 roaming data dir / onemore / config.toml）。
//! GUI 不做校验，只做读写透传：读取时返回原始文本，写入时整文件覆盖。

use std::fs;
use std::path::PathBuf;

use crate::error::GuiError;

pub const ONEMORE_HOME_ENV: &str = "ONEMORE_HOME";

/// 返回 onemore 数据目录（roaming/onemore）。
pub fn onemore_data_dir() -> Result<PathBuf, GuiError> {
    if let Some(home) = std::env::var_os(ONEMORE_HOME_ENV).filter(|value| !value.is_empty()) {
        return Ok(PathBuf::from(home));
    }

    let data_dir = dirs::data_dir()
        .ok_or_else(|| GuiError::new("no_data_dir", "无法确定系统数据目录"))?;
    let user_profile = std::env::var_os("USERPROFILE").map(PathBuf::from);
    Ok(resolve_data_dir(&data_dir, user_profile.as_deref()).join("onemore"))
}

fn resolve_data_dir(data_dir: &std::path::Path, user_profile: Option<&std::path::Path>) -> PathBuf {
    #[cfg(windows)]
    {
        let normalized = data_dir.to_string_lossy().replace('/', "\\").to_ascii_lowercase();
        let is_packaged_redirect = normalized.contains("\\appdata\\local\\packages\\")
            && normalized.contains("\\localcache\\roaming");
        if is_packaged_redirect {
            if let Some(profile) = user_profile {
                return profile.join("AppData").join("Roaming");
            }
        }
    }

    let _ = user_profile;
    data_dir.to_path_buf()
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn packaged_app_redirect_uses_real_user_roaming_directory() {
        let redirected = std::path::Path::new(
            r"C:\Users\test\AppData\Local\Packages\Host.App\LocalCache\Roaming",
        );
        let profile = std::path::Path::new(r"C:\Users\test");

        #[cfg(windows)]
        assert_eq!(
            resolve_data_dir(redirected, Some(profile)),
            std::path::Path::new(r"C:\Users\test\AppData\Roaming"),
        );
        #[cfg(not(windows))]
        assert_eq!(resolve_data_dir(redirected, Some(profile)), redirected);
    }

    #[test]
    fn normal_roaming_directory_is_unchanged() {
        let roaming = std::path::Path::new(r"C:\Users\test\AppData\Roaming");
        let profile = std::path::Path::new(r"C:\Users\test");
        assert_eq!(resolve_data_dir(roaming, Some(profile)), roaming);
    }
}
