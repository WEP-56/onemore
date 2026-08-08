//! config.toml 的可视化读写:
//! - get_config_dto:解析现有 config.toml 为结构化 DTO(前端表单用)。
//! - update_config_dto:基于原始文本做增量更新(用 toml_edit 保留注释与未知字段)。
//!
//! 字段结构对齐 onemore CLI 的 FileConfig(见 onemore-cli/src/config.rs)。

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use toml_edit::{value as tv, DocumentMut, Item, Table};

use crate::config::{config_path, read_config};
use crate::error::GuiError;

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct AgentDto {
    pub provider: String,
    pub shell: String,
    #[serde(default)]
    pub max_turns: Option<u32>,
    #[serde(default)]
    pub tool_timeout_secs: Option<u64>,
    #[serde(default)]
    pub system_prompt: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RetryDto {
    #[serde(default)]
    pub max_attempts: Option<u32>,
    #[serde(default)]
    pub base_delay_ms: Option<u64>,
    #[serde(default)]
    pub max_delay_ms: Option<u64>,
    #[serde(default)]
    pub max_retry_after_ms: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompactionDto {
    #[serde(default)]
    pub enabled: Option<bool>,
    #[serde(default)]
    pub reserve_tokens: Option<u64>,
    #[serde(default)]
    pub keep_recent_tokens: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PermissionsDto {
    #[serde(default)]
    pub workspace_read: Option<String>,
    #[serde(default)]
    pub workspace_write: Option<String>,
    #[serde(default)]
    pub outside_workspace: Option<String>,
    #[serde(default)]
    pub commands: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelDto {
    pub name: String,
    #[serde(default)]
    pub context_window: Option<u64>,
    #[serde(default)]
    pub max_tokens: Option<u64>,
    #[serde(default)]
    pub efforts: Vec<String>,
    #[serde(default)]
    pub default_effort: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderDto {
    pub name: String,
    pub api: String,
    #[serde(default)]
    pub profile: Option<String>,
    pub base_url: String,
    #[serde(default)]
    pub api_key_env: Option<String>,
    #[serde(default)]
    pub api_key: Option<String>,
    #[serde(default)]
    pub default_model: Option<String>,
    #[serde(default)]
    pub max_tokens: Option<u64>,
    #[serde(default)]
    pub context_window: Option<u64>,
    #[serde(default)]
    pub models: Vec<ModelDto>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConfigDto {
    pub agent: AgentDto,
    pub retry: RetryDto,
    pub compaction: CompactionDto,
    pub permissions: PermissionsDto,
    pub providers: Vec<ProviderDto>,
}

impl Default for RetryDto {
    fn default() -> Self {
        Self { max_attempts: None, base_delay_ms: None, max_delay_ms: None, max_retry_after_ms: None }
    }
}
impl Default for CompactionDto {
    fn default() -> Self {
        Self { enabled: None, reserve_tokens: None, keep_recent_tokens: None }
    }
}
impl Default for PermissionsDto {
    fn default() -> Self {
        Self { workspace_read: None, workspace_write: None, outside_workspace: None, commands: None }
    }
}
impl Default for ConfigDto {
    fn default() -> Self {
        Self {
            agent: AgentDto { provider: String::new(), shell: "auto".into(), max_turns: None, tool_timeout_secs: None, system_prompt: None },
            retry: RetryDto::default(),
            compaction: CompactionDto::default(),
            permissions: PermissionsDto::default(),
            providers: Vec::new(),
        }
    }
}

/// 解析当前 config.toml 为 DTO;文件不存在时返回默认 DTO。
pub fn get_config_dto() -> Result<ConfigDto, GuiError> {
    let raw = read_config()?;
    if raw.trim().is_empty() {
        return Ok(ConfigDto::default());
    }
    let doc = raw
        .parse::<DocumentMut>()
        .map_err(|e| GuiError::new("parse_config", format!("解析 config.toml 失败: {e}")))?;
    Ok(parse_doc(&doc))
}

fn get_str<'a>(doc: &'a DocumentMut, table: &str, key: &str) -> Option<&'a str> {
    doc.get(table)?.get(key)?.as_str()
}
fn get_u64(doc: &DocumentMut, table: &str, key: &str) -> Option<u64> {
    doc.get(table)?.get(key)?.as_integer().map(|v| v as u64)
}
fn get_bool(doc: &DocumentMut, table: &str, key: &str) -> Option<bool> {
    doc.get(table)?.get(key)?.as_bool()
}

fn parse_doc(doc: &DocumentMut) -> ConfigDto {
    let mut providers = Vec::new();
    if let Some(providers_table) = doc.get("providers").and_then(Item::as_table) {
        for (name, item) in providers_table.iter() {
            if let Some(pt) = item.as_table() {
                let mut models = Vec::new();
                if let Some(models_table) = pt.get("models").and_then(Item::as_table) {
                    for (mname, mitem) in models_table.iter() {
                        if let Some(mt) = mitem.as_table() {
                            models.push(ModelDto {
                                name: mname.to_string(),
                                context_window: mt.get("context_window").and_then(Item::as_integer).map(|v| v as u64),
                                max_tokens: mt.get("max_tokens").and_then(Item::as_integer).map(|v| v as u64),
                                efforts: mt
                                    .get("efforts")
                                    .and_then(Item::as_array)
                                    .map(|arr| arr.iter().filter_map(|v| v.as_str().map(String::from)).collect())
                                    .unwrap_or_default(),
                                default_effort: mt.get("default_effort").and_then(Item::as_str).map(String::from),
                            });
                        }
                    }
                }
                providers.push(ProviderDto {
                    name: name.to_string(),
                    api: pt.get("api").and_then(Item::as_str).unwrap_or_default().to_string(),
                    profile: pt.get("profile").and_then(Item::as_str).map(String::from),
                    base_url: pt.get("base_url").and_then(Item::as_str).unwrap_or_default().to_string(),
                    api_key_env: pt.get("api_key_env").and_then(Item::as_str).map(String::from),
                    api_key: pt.get("api_key").and_then(Item::as_str).map(String::from),
                    default_model: pt.get("default_model").and_then(Item::as_str).map(String::from),
                    max_tokens: pt.get("max_tokens").and_then(Item::as_integer).map(|v| v as u64),
                    context_window: pt.get("context_window").and_then(Item::as_integer).map(|v| v as u64),
                    models,
                });
            }
        }
    }

    ConfigDto {
        agent: AgentDto {
            provider: get_str(doc, "agent", "provider").unwrap_or_default().to_string(),
            shell: get_str(doc, "agent", "shell").unwrap_or("auto").to_string(),
            max_turns: get_u64(doc, "agent", "max_turns").map(|v| v as u32),
            tool_timeout_secs: get_u64(doc, "agent", "tool_timeout_secs"),
            system_prompt: get_str(doc, "agent", "system_prompt").map(String::from),
        },
        retry: RetryDto {
            max_attempts: get_u64(doc, "retry", "max_attempts").map(|v| v as u32),
            base_delay_ms: get_u64(doc, "retry", "base_delay_ms"),
            max_delay_ms: get_u64(doc, "retry", "max_delay_ms"),
            max_retry_after_ms: get_u64(doc, "retry", "max_retry_after_ms"),
        },
        compaction: CompactionDto {
            enabled: get_bool(doc, "compaction", "enabled"),
            reserve_tokens: get_u64(doc, "compaction", "reserve_tokens"),
            keep_recent_tokens: get_u64(doc, "compaction", "keep_recent_tokens"),
        },
        permissions: PermissionsDto {
            workspace_read: get_str(doc, "permissions", "workspace_read").map(String::from),
            workspace_write: get_str(doc, "permissions", "workspace_write").map(String::from),
            outside_workspace: get_str(doc, "permissions", "outside_workspace").map(String::from),
            commands: get_str(doc, "permissions", "commands").map(String::from),
        },
        providers,
    }
}

/// 基于原始文本增量更新配置(保留注释与未知字段),写入磁盘。
pub fn update_config_dto(dto: &ConfigDto) -> Result<(), GuiError> {
    let raw = read_config()?;
    let mut doc = if raw.trim().is_empty() {
        DocumentMut::new()
    } else {
        raw.parse::<DocumentMut>()
            .map_err(|e| GuiError::new("parse_config", format!("解析 config.toml 失败: {e}")))?
    };

    // ── agent ──
    ensure_table(&mut doc, "agent");
    set_str(&mut doc, "agent", "provider", &dto.agent.provider);
    set_str(&mut doc, "agent", "shell", &dto.agent.shell);
    set_opt_int(&mut doc, "agent", "max_turns", dto.agent.max_turns.map(|v| v as i64));
    set_opt_int(&mut doc, "agent", "tool_timeout_secs", dto.agent.tool_timeout_secs.map(|v| v as i64));
    set_opt_str(&mut doc, "agent", "system_prompt", dto.agent.system_prompt.as_deref());

    // ── retry ──
    ensure_table(&mut doc, "retry");
    set_opt_int(&mut doc, "retry", "max_attempts", dto.retry.max_attempts.map(|v| v as i64));
    set_opt_int(&mut doc, "retry", "base_delay_ms", dto.retry.base_delay_ms.map(|v| v as i64));
    set_opt_int(&mut doc, "retry", "max_delay_ms", dto.retry.max_delay_ms.map(|v| v as i64));
    set_opt_int(&mut doc, "retry", "max_retry_after_ms", dto.retry.max_retry_after_ms.map(|v| v as i64));

    // ── compaction ──
    ensure_table(&mut doc, "compaction");
    set_opt_bool(&mut doc, "compaction", "enabled", dto.compaction.enabled);
    set_opt_int(&mut doc, "compaction", "reserve_tokens", dto.compaction.reserve_tokens.map(|v| v as i64));
    set_opt_int(&mut doc, "compaction", "keep_recent_tokens", dto.compaction.keep_recent_tokens.map(|v| v as i64));

    // ── permissions ──
    ensure_table(&mut doc, "permissions");
    set_opt_str(&mut doc, "permissions", "workspace_read", dto.permissions.workspace_read.as_deref());
    set_opt_str(&mut doc, "permissions", "workspace_write", dto.permissions.workspace_write.as_deref());
    set_opt_str(&mut doc, "permissions", "outside_workspace", dto.permissions.outside_workspace.as_deref());
    set_opt_str(&mut doc, "permissions", "commands", dto.permissions.commands.as_deref());

    // ── providers(upsert + 删除多余) ──
    let mut wanted: BTreeMap<String, &ProviderDto> = BTreeMap::new();
    for p in &dto.providers {
        wanted.insert(p.name.clone(), p);
    }
    let had_providers = doc.get("providers").is_some();
    ensure_table(&mut doc, "providers");
    if let Some(Item::Table(pt)) = doc.get_mut("providers") {
        // 删除 DTO 中不存在的 provider
        let stale: Vec<String> = pt.iter().filter(|(k, _)| !wanted.contains_key(*k)).map(|(k, _)| k.to_string()).collect();
        for k in stale {
            pt.remove(&k);
        }
        // upsert 每个 provider
        for (name, p) in &wanted {
            if !pt.contains_key(name) {
                pt.insert(name, Item::Table(Table::new()));
            }
            if let Some(Item::Table(ptable)) = pt.get_mut(name) {
                set_str_in_table(ptable, "api", &p.api);
                set_opt_str_in_table(ptable, "profile", p.profile.as_deref());
                set_str_in_table(ptable, "base_url", &p.base_url);
                set_opt_str_in_table(ptable, "api_key_env", p.api_key_env.as_deref());
                set_opt_str_in_table(ptable, "api_key", p.api_key.as_deref());
                set_opt_str_in_table(ptable, "default_model", p.default_model.as_deref());
                set_opt_int_in_table(ptable, "max_tokens", p.max_tokens.map(|v| v as i64));
                set_opt_int_in_table(ptable, "context_window", p.context_window.map(|v| v as i64));

                // models 子表
                let mut wanted_models: BTreeMap<String, &ModelDto> = BTreeMap::new();
                for m in &p.models {
                    wanted_models.insert(m.name.clone(), m);
                }
                if wanted_models.is_empty() {
                    ptable.remove("models");
                } else {
                    if !ptable.contains_key("models") {
                        ptable.insert("models", Item::Table(Table::new()));
                    }
                    if let Some(Item::Table(mt)) = ptable.get_mut("models") {
                        let stale_models: Vec<String> = mt
                            .iter()
                            .filter(|(k, _)| !wanted_models.contains_key(*k))
                            .map(|(k, _)| k.to_string())
                            .collect();
                        for k in stale_models {
                            mt.remove(&k);
                        }
                        for (mname, m) in &wanted_models {
                            if !mt.contains_key(mname) {
                                mt.insert(mname, Item::Table(Table::new()));
                            }
                            if let Some(Item::Table(ms)) = mt.get_mut(mname) {
                                set_opt_int_in_table(ms, "context_window", m.context_window.map(|v| v as i64));
                                set_opt_int_in_table(ms, "max_tokens", m.max_tokens.map(|v| v as i64));
                                if m.efforts.is_empty() {
                                    ms.remove("efforts");
                                } else {
                                    let arr = toml_edit::Array::from_iter(m.efforts.iter().map(|e| toml_edit::Value::from(e.as_str())));
                                    ms.insert("efforts", Item::Value(toml_edit::Value::Array(arr)));
                                }
                                set_opt_str_in_table(ms, "default_effort", m.default_effort.as_deref());
                            }
                        }
                    }
                }
            }
        }
        if !had_providers && wanted.is_empty() {
            // 空 providers 表也保留,供用户后续填写
        }
    }

    let path = config_path()?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| GuiError::new("write_config", e.to_string()))?;
    }
    std::fs::write(&path, doc.to_string()).map_err(|e| GuiError::new("write_config", e.to_string()))?;
    Ok(())
}

fn ensure_table(doc: &mut DocumentMut, key: &str) {
    if !doc.contains_key(key) {
        doc.insert(key, Item::Table(Table::new()));
    }
}

fn set_str(doc: &mut DocumentMut, table: &str, key: &str, value: &str) {
    if let Some(Item::Table(t)) = doc.get_mut(table) {
        set_str_in_table(t, key, value);
    }
}

fn set_opt_str(doc: &mut DocumentMut, table: &str, key: &str, value: Option<&str>) {
    if let Some(Item::Table(t)) = doc.get_mut(table) {
        set_opt_str_in_table(t, key, value);
    }
}

fn set_opt_int(doc: &mut DocumentMut, table: &str, key: &str, value: Option<i64>) {
    if let Some(Item::Table(t)) = doc.get_mut(table) {
        set_opt_int_in_table(t, key, value);
    }
}

fn set_opt_bool(doc: &mut DocumentMut, table: &str, key: &str, value: Option<bool>) {
    if let Some(Item::Table(t)) = doc.get_mut(table) {
        set_opt_bool_in_table(t, key, value);
    }
}

fn set_str_in_table(t: &mut Table, key: &str, value: &str) {
    if value.is_empty() {
        t.remove(key);
    } else {
        t.insert(key, tv(value));
    }
}

fn set_opt_str_in_table(t: &mut Table, key: &str, value: Option<&str>) {
    match value {
        Some(v) if !v.is_empty() => {
            t.insert(key, tv(v));
        }
        _ => {
            t.remove(key);
        }
    }
}

fn set_opt_int_in_table(t: &mut Table, key: &str, value: Option<i64>) {
    match value {
        Some(v) => {
            t.insert(key, tv(v));
        }
        None => {
            t.remove(key);
        }
    }
}

fn set_opt_bool_in_table(t: &mut Table, key: &str, value: Option<bool>) {
    match value {
        Some(v) => {
            t.insert(key, tv(v));
        }
        None => {
            t.remove(key);
        }
    }
}
