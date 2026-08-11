//! config.toml 的可视化读写:
//! - get_config_dto:解析现有 config.toml 为结构化 DTO(前端表单用)。
//! - update_config_dto:基于原始文本做增量更新(用 toml_edit 保留注释与未知字段)。
//!
//! 字段结构对齐 onemore CLI 的 FileConfig(见 onemore-cli/src/config.rs)。

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use toml_edit::{value as tv, Array, ArrayOfTables, DocumentMut, InlineTable, Item, Table, Value};

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

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
pub struct WebLocationDto {
    #[serde(default)]
    pub country: Option<String>,
    #[serde(default)]
    pub region: Option<String>,
    #[serde(default)]
    pub city: Option<String>,
    #[serde(default)]
    pub timezone: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
pub struct WebBackendDto {
    pub name: String,
    #[serde(default)]
    pub api_key: Option<String>,
    #[serde(default)]
    pub api_key_env: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
pub struct WebDto {
    #[serde(default)]
    pub mode: Option<String>,
    #[serde(default)]
    pub external_backends: Vec<String>,
    #[serde(default)]
    pub context_size: Option<String>,
    #[serde(default)]
    pub allowed_domains: Vec<String>,
    #[serde(default)]
    pub location: Option<WebLocationDto>,
    #[serde(default)]
    pub backends: Vec<WebBackendDto>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
pub struct McpServerDto {
    pub name: String,
    pub command: String,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub env: BTreeMap<String, String>,
    #[serde(default)]
    pub cwd: Option<String>,
    #[serde(default)]
    pub enabled: Option<bool>,
    #[serde(default)]
    pub startup_timeout_ms: Option<u64>,
    #[serde(default)]
    pub call_timeout_ms: Option<u64>,
    #[serde(default)]
    pub always_ask: Option<bool>,
    #[serde(default)]
    pub include_tools: Option<Vec<String>>,
    #[serde(default)]
    pub exclude_tools: Vec<String>,
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
    pub models: Vec<ModelDto>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConfigDto {
    pub agent: AgentDto,
    pub retry: RetryDto,
    pub compaction: CompactionDto,
    pub permissions: PermissionsDto,
    pub web: WebDto,
    pub mcp_servers: Vec<McpServerDto>,
    pub providers: Vec<ProviderDto>,
}

impl Default for RetryDto {
    fn default() -> Self {
        Self {
            max_attempts: None,
            base_delay_ms: None,
            max_delay_ms: None,
            max_retry_after_ms: None,
        }
    }
}
impl Default for CompactionDto {
    fn default() -> Self {
        Self {
            enabled: None,
            reserve_tokens: None,
            keep_recent_tokens: None,
        }
    }
}
impl Default for PermissionsDto {
    fn default() -> Self {
        Self {
            workspace_read: None,
            workspace_write: None,
            outside_workspace: None,
            commands: None,
        }
    }
}
impl Default for ConfigDto {
    fn default() -> Self {
        Self {
            agent: AgentDto {
                provider: String::new(),
                shell: "auto".into(),
                max_turns: None,
                tool_timeout_secs: None,
                system_prompt: None,
            },
            retry: RetryDto::default(),
            compaction: CompactionDto::default(),
            permissions: PermissionsDto::default(),
            web: WebDto::default(),
            mcp_servers: Vec::new(),
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

fn string_array(item: Option<&Item>) -> Vec<String> {
    item.and_then(Item::as_array)
        .map(|array| {
            array
                .iter()
                .filter_map(|value| value.as_str().map(String::from))
                .collect()
        })
        .unwrap_or_default()
}

fn string_map(item: Option<&Item>) -> BTreeMap<String, String> {
    if let Some(table) = item.and_then(Item::as_inline_table) {
        return table
            .iter()
            .filter_map(|(key, value)| {
                value
                    .as_str()
                    .map(|value| (key.to_string(), value.to_string()))
            })
            .collect();
    }
    item.and_then(Item::as_table)
        .map(|table| {
            table
                .iter()
                .filter_map(|(key, value)| {
                    value
                        .as_str()
                        .map(|value| (key.to_string(), value.to_string()))
                })
                .collect()
        })
        .unwrap_or_default()
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
                                context_window: mt
                                    .get("context_window")
                                    .and_then(Item::as_integer)
                                    .map(|v| v as u64),
                                max_tokens: mt
                                    .get("max_tokens")
                                    .and_then(Item::as_integer)
                                    .map(|v| v as u64),
                                efforts: mt
                                    .get("efforts")
                                    .and_then(Item::as_array)
                                    .map(|arr| {
                                        arr.iter()
                                            .filter_map(|v| v.as_str().map(String::from))
                                            .collect()
                                    })
                                    .unwrap_or_default(),
                                default_effort: mt
                                    .get("default_effort")
                                    .and_then(Item::as_str)
                                    .map(String::from),
                            });
                        }
                    }
                }
                let legacy_model = pt.get("model").and_then(Item::as_str).map(String::from);
                if models.is_empty() {
                    if let Some(model) = legacy_model.as_ref() {
                        models.push(ModelDto {
                            name: model.clone(),
                            context_window: pt
                                .get("context_window")
                                .and_then(Item::as_integer)
                                .map(|v| v as u64),
                            max_tokens: pt
                                .get("max_tokens")
                                .and_then(Item::as_integer)
                                .map(|v| v as u64),
                            efforts: Vec::new(),
                            default_effort: None,
                        });
                    }
                }
                providers.push(ProviderDto {
                    name: name.to_string(),
                    api: pt
                        .get("api")
                        .and_then(Item::as_str)
                        .unwrap_or_default()
                        .to_string(),
                    profile: pt.get("profile").and_then(Item::as_str).map(String::from),
                    base_url: pt
                        .get("base_url")
                        .and_then(Item::as_str)
                        .unwrap_or_default()
                        .to_string(),
                    api_key_env: pt
                        .get("api_key_env")
                        .and_then(Item::as_str)
                        .map(String::from),
                    api_key: pt.get("api_key").and_then(Item::as_str).map(String::from),
                    default_model: pt
                        .get("default_model")
                        .and_then(Item::as_str)
                        .map(String::from)
                        .or(legacy_model),
                    models,
                });
            }
        }
    }

    let web_table = doc.get("web").and_then(Item::as_table);
    let location = web_table
        .and_then(|web| web.get("location"))
        .and_then(Item::as_table)
        .map(|table| WebLocationDto {
            country: table
                .get("country")
                .and_then(Item::as_str)
                .map(String::from),
            region: table.get("region").and_then(Item::as_str).map(String::from),
            city: table.get("city").and_then(Item::as_str).map(String::from),
            timezone: table
                .get("timezone")
                .and_then(Item::as_str)
                .map(String::from),
        });
    let mut web_backends = Vec::new();
    if let Some(backends) = web_table
        .and_then(|web| web.get("backends"))
        .and_then(Item::as_table)
    {
        for (name, item) in backends.iter() {
            if let Some(table) = item.as_table() {
                web_backends.push(WebBackendDto {
                    name: name.to_string(),
                    api_key: table
                        .get("api_key")
                        .and_then(Item::as_str)
                        .map(String::from),
                    api_key_env: table
                        .get("api_key_env")
                        .and_then(Item::as_str)
                        .map(String::from),
                });
            }
        }
    }
    let web = WebDto {
        mode: web_table
            .and_then(|table| table.get("mode"))
            .and_then(Item::as_str)
            .map(String::from),
        external_backends: web_table
            .map(|table| string_array(table.get("external_backends")))
            .unwrap_or_default(),
        context_size: web_table
            .and_then(|table| table.get("context_size"))
            .and_then(Item::as_str)
            .map(String::from),
        allowed_domains: web_table
            .map(|table| string_array(table.get("allowed_domains")))
            .unwrap_or_default(),
        location,
        backends: web_backends,
    };

    let mcp_servers = doc
        .get("mcp_servers")
        .and_then(Item::as_array_of_tables)
        .map(|servers| {
            servers
                .iter()
                .map(|table| McpServerDto {
                    name: table
                        .get("name")
                        .and_then(Item::as_str)
                        .unwrap_or_default()
                        .to_string(),
                    command: table
                        .get("command")
                        .and_then(Item::as_str)
                        .unwrap_or_default()
                        .to_string(),
                    args: string_array(table.get("args")),
                    env: string_map(table.get("env")),
                    cwd: table.get("cwd").and_then(Item::as_str).map(String::from),
                    enabled: table.get("enabled").and_then(Item::as_bool),
                    startup_timeout_ms: table
                        .get("startup_timeout_ms")
                        .and_then(Item::as_integer)
                        .map(|value| value as u64),
                    call_timeout_ms: table
                        .get("call_timeout_ms")
                        .and_then(Item::as_integer)
                        .map(|value| value as u64),
                    always_ask: table.get("always_ask").and_then(Item::as_bool),
                    include_tools: table
                        .get("include_tools")
                        .map(|item| string_array(Some(item))),
                    exclude_tools: string_array(table.get("exclude_tools")),
                })
                .collect()
        })
        .unwrap_or_default();

    ConfigDto {
        agent: AgentDto {
            provider: get_str(doc, "agent", "provider")
                .unwrap_or_default()
                .to_string(),
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
        web,
        mcp_servers,
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
    set_opt_int(
        &mut doc,
        "agent",
        "max_turns",
        dto.agent.max_turns.map(|v| v as i64),
    );
    set_opt_int(
        &mut doc,
        "agent",
        "tool_timeout_secs",
        dto.agent.tool_timeout_secs.map(|v| v as i64),
    );
    set_opt_str(
        &mut doc,
        "agent",
        "system_prompt",
        dto.agent.system_prompt.as_deref(),
    );

    // ── retry ──
    ensure_table(&mut doc, "retry");
    set_opt_int(
        &mut doc,
        "retry",
        "max_attempts",
        dto.retry.max_attempts.map(|v| v as i64),
    );
    set_opt_int(
        &mut doc,
        "retry",
        "base_delay_ms",
        dto.retry.base_delay_ms.map(|v| v as i64),
    );
    set_opt_int(
        &mut doc,
        "retry",
        "max_delay_ms",
        dto.retry.max_delay_ms.map(|v| v as i64),
    );
    set_opt_int(
        &mut doc,
        "retry",
        "max_retry_after_ms",
        dto.retry.max_retry_after_ms.map(|v| v as i64),
    );

    // ── compaction ──
    ensure_table(&mut doc, "compaction");
    set_opt_bool(&mut doc, "compaction", "enabled", dto.compaction.enabled);
    set_opt_int(
        &mut doc,
        "compaction",
        "reserve_tokens",
        dto.compaction.reserve_tokens.map(|v| v as i64),
    );
    set_opt_int(
        &mut doc,
        "compaction",
        "keep_recent_tokens",
        dto.compaction.keep_recent_tokens.map(|v| v as i64),
    );

    // ── permissions ──
    ensure_table(&mut doc, "permissions");
    set_opt_str(
        &mut doc,
        "permissions",
        "workspace_read",
        dto.permissions.workspace_read.as_deref(),
    );
    set_opt_str(
        &mut doc,
        "permissions",
        "workspace_write",
        dto.permissions.workspace_write.as_deref(),
    );
    set_opt_str(
        &mut doc,
        "permissions",
        "outside_workspace",
        dto.permissions.outside_workspace.as_deref(),
    );
    set_opt_str(
        &mut doc,
        "permissions",
        "commands",
        dto.permissions.commands.as_deref(),
    );

    update_web(&mut doc, &dto.web);
    update_mcp_servers(&mut doc, &dto.mcp_servers);

    // ── providers(upsert + 删除多余) ──
    let mut wanted: BTreeMap<String, &ProviderDto> = BTreeMap::new();
    for p in &dto.providers {
        wanted.insert(p.name.clone(), p);
    }
    let had_providers = doc.get("providers").is_some();
    ensure_table(&mut doc, "providers");
    if let Some(Item::Table(pt)) = doc.get_mut("providers") {
        // 删除 DTO 中不存在的 provider
        let stale: Vec<String> = pt
            .iter()
            .filter(|(k, _)| !wanted.contains_key(*k))
            .map(|(k, _)| k.to_string())
            .collect();
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
                // 可视化编辑器统一写新格式。旧格式字段与 default_model/models 不能共存。
                remove_legacy_provider_model_fields(ptable);

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
                                set_opt_int_in_table(
                                    ms,
                                    "context_window",
                                    m.context_window.map(|v| v as i64),
                                );
                                set_opt_int_in_table(
                                    ms,
                                    "max_tokens",
                                    m.max_tokens.map(|v| v as i64),
                                );
                                if m.efforts.is_empty() {
                                    ms.remove("efforts");
                                } else {
                                    let arr = toml_edit::Array::from_iter(
                                        m.efforts
                                            .iter()
                                            .map(|e| toml_edit::Value::from(e.as_str())),
                                    );
                                    ms.insert("efforts", Item::Value(toml_edit::Value::Array(arr)));
                                }
                                set_opt_str_in_table(
                                    ms,
                                    "default_effort",
                                    m.default_effort.as_deref(),
                                );
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
        std::fs::create_dir_all(parent)
            .map_err(|e| GuiError::new("write_config", e.to_string()))?;
    }
    std::fs::write(&path, doc.to_string())
        .map_err(|e| GuiError::new("write_config", e.to_string()))?;
    Ok(())
}

fn ensure_table(doc: &mut DocumentMut, key: &str) {
    if !doc.contains_key(key) {
        doc.insert(key, Item::Table(Table::new()));
    }
}

fn update_web(doc: &mut DocumentMut, dto: &WebDto) {
    ensure_table(doc, "web");
    let Some(web) = doc.get_mut("web").and_then(Item::as_table_mut) else {
        return;
    };
    set_opt_str_in_table(web, "mode", dto.mode.as_deref());
    set_string_array_in_table(web, "external_backends", &dto.external_backends);
    set_opt_str_in_table(web, "context_size", dto.context_size.as_deref());
    set_string_array_in_table(web, "allowed_domains", &dto.allowed_domains);

    match dto.location.as_ref() {
        Some(location) => {
            if !web.contains_key("location") {
                web.insert("location", Item::Table(Table::new()));
            }
            if let Some(table) = web.get_mut("location").and_then(Item::as_table_mut) {
                set_opt_str_in_table(table, "country", location.country.as_deref());
                set_opt_str_in_table(table, "region", location.region.as_deref());
                set_opt_str_in_table(table, "city", location.city.as_deref());
                set_opt_str_in_table(table, "timezone", location.timezone.as_deref());
            }
        }
        None => {
            web.remove("location");
        }
    }

    if !web.contains_key("backends") {
        web.insert("backends", Item::Table(Table::new()));
    }
    let Some(backends) = web.get_mut("backends").and_then(Item::as_table_mut) else {
        return;
    };
    let wanted: BTreeMap<&str, &WebBackendDto> = dto
        .backends
        .iter()
        .map(|backend| (backend.name.as_str(), backend))
        .collect();
    let stale: Vec<String> = backends
        .iter()
        .filter(|(name, _)| !wanted.contains_key(*name))
        .map(|(name, _)| name.to_string())
        .collect();
    for name in stale {
        backends.remove(&name);
    }
    for (name, backend) in wanted {
        if !backends.contains_key(name) {
            backends.insert(name, Item::Table(Table::new()));
        }
        if let Some(table) = backends.get_mut(name).and_then(Item::as_table_mut) {
            let api_key = backend.api_key.as_deref().filter(|value| !value.is_empty());
            let api_key_env = if api_key.is_some() {
                None
            } else {
                backend.api_key_env.as_deref()
            };
            set_opt_str_in_table(table, "api_key", api_key);
            set_opt_str_in_table(table, "api_key_env", api_key_env);
        }
    }
}

fn update_mcp_servers(doc: &mut DocumentMut, servers: &[McpServerDto]) {
    if servers.is_empty() {
        doc.remove("mcp_servers");
        return;
    }
    if !doc.contains_key("mcp_servers") {
        doc.insert("mcp_servers", Item::ArrayOfTables(ArrayOfTables::new()));
    }
    let Some(array) = doc
        .get_mut("mcp_servers")
        .and_then(Item::as_array_of_tables_mut)
    else {
        return;
    };
    let wanted: BTreeMap<&str, &McpServerDto> = servers
        .iter()
        .map(|server| (server.name.as_str(), server))
        .collect();
    for index in (0..array.len()).rev() {
        let name = array
            .get(index)
            .and_then(|table| table.get("name"))
            .and_then(Item::as_str)
            .unwrap_or_default();
        if !wanted.contains_key(name) {
            array.remove(index);
        }
    }
    for server in servers {
        let existing_index = array
            .iter()
            .position(|table| table.get("name").and_then(Item::as_str) == Some(&server.name));
        let index = match existing_index {
            Some(index) => index,
            None => {
                array.push(Table::new());
                array.len() - 1
            }
        };
        let table = array.get_mut(index).expect("new MCP table must exist");
        set_str_in_table(table, "name", &server.name);
        set_str_in_table(table, "command", &server.command);
        set_string_array_in_table(table, "args", &server.args);
        if server.env.is_empty() {
            table.remove("env");
        } else if let Some(env) = table.get_mut("env").and_then(Item::as_table_mut) {
            let stale: Vec<String> = env
                .iter()
                .filter(|(key, _)| !server.env.contains_key(*key))
                .map(|(key, _)| key.to_string())
                .collect();
            for key in stale {
                env.remove(&key);
            }
            for (key, value) in &server.env {
                set_str_in_table(env, key, value);
            }
        } else {
            let mut env = InlineTable::new();
            for (key, value) in &server.env {
                env.insert(key, Value::from(value.as_str()));
            }
            table.insert("env", Item::Value(Value::InlineTable(env)));
        }
        set_opt_str_in_table(table, "cwd", server.cwd.as_deref());
        set_opt_bool_in_table(table, "enabled", server.enabled);
        set_opt_int_in_table(
            table,
            "startup_timeout_ms",
            server.startup_timeout_ms.map(|value| value as i64),
        );
        set_opt_int_in_table(
            table,
            "call_timeout_ms",
            server.call_timeout_ms.map(|value| value as i64),
        );
        set_opt_bool_in_table(table, "always_ask", server.always_ask);
        set_optional_string_array_in_table(table, "include_tools", server.include_tools.as_deref());
        set_string_array_in_table(table, "exclude_tools", &server.exclude_tools);
    }
}

fn set_string_array_in_table(table: &mut Table, key: &str, values: &[String]) {
    if values.is_empty() {
        table.remove(key);
    } else {
        let array = Array::from_iter(values.iter().map(|value| Value::from(value.as_str())));
        table.insert(key, Item::Value(Value::Array(array)));
    }
}

fn set_optional_string_array_in_table(table: &mut Table, key: &str, values: Option<&[String]>) {
    match values {
        Some(values) => {
            let array = Array::from_iter(values.iter().map(|value| Value::from(value.as_str())));
            table.insert(key, Item::Value(Value::Array(array)));
        }
        None => {
            table.remove(key);
        }
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

fn remove_legacy_provider_model_fields(table: &mut Table) {
    table.remove("model");
    table.remove("max_tokens");
    table.remove("context_window");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn legacy_provider_is_exposed_as_a_model_entry() {
        let doc = r#"
[agent]
provider = "legacy"

[providers.legacy]
api = "responses"
base_url = "https://example.com/v1"
model = "legacy-model"
context_window = 32000
max_tokens = 4096
"#
        .parse::<DocumentMut>()
        .unwrap();

        let dto = parse_doc(&doc);
        let provider = &dto.providers[0];
        assert_eq!(provider.default_model.as_deref(), Some("legacy-model"));
        assert_eq!(provider.models.len(), 1);
        assert_eq!(provider.models[0].name, "legacy-model");
        assert_eq!(provider.models[0].context_window, Some(32000));
        assert_eq!(provider.models[0].max_tokens, Some(4096));
    }

    #[test]
    fn new_format_removes_all_legacy_provider_model_fields() {
        let mut doc = r#"
model = "legacy-model"
context_window = 32000
max_tokens = 4096
default_model = "new-model"
"#
        .parse::<DocumentMut>()
        .unwrap();
        let table = doc.as_table_mut();

        remove_legacy_provider_model_fields(table);
        assert!(!table.contains_key("model"));
        assert!(!table.contains_key("context_window"));
        assert!(!table.contains_key("max_tokens"));
        assert_eq!(
            table.get("default_model").and_then(Item::as_str),
            Some("new-model")
        );
    }

    #[test]
    fn web_and_mcp_sections_round_trip_through_visual_dto() {
        let mut doc = r#"
# keep this comment
[web]
mode = "external"
external_backends = ["tavily"]
context_size = "high"
allowed_domains = ["example.com"]

[web.location]
country = "CN"
timezone = "Asia/Shanghai"

[web.backends.tavily]
api_key_env = "TAVILY_API_KEY"

[[mcp_servers]]
name = "playwright"
command = "cmd"
args = ["/c", "npx"]
env = { DEBUG = "1" }
enabled = true
always_ask = true
include_tools = ["browser_navigate"]
exclude_tools = []
"#
        .parse::<DocumentMut>()
        .unwrap();

        let mut dto = parse_doc(&doc);
        assert_eq!(dto.web.mode.as_deref(), Some("external"));
        assert_eq!(
            dto.web.location.as_ref().unwrap().country.as_deref(),
            Some("CN")
        );
        assert_eq!(dto.web.backends[0].name, "tavily");
        assert_eq!(
            dto.mcp_servers[0].env.get("DEBUG").map(String::as_str),
            Some("1")
        );
        assert_eq!(
            dto.mcp_servers[0].include_tools.as_ref().unwrap()[0],
            "browser_navigate"
        );

        dto.web.backends[0].api_key = Some("secret".into());
        dto.mcp_servers[0].call_timeout_ms = Some(90_000);
        update_web(&mut doc, &dto.web);
        update_mcp_servers(&mut doc, &dto.mcp_servers);

        let rendered = doc.to_string();
        assert!(rendered.contains("# keep this comment"));
        let reparsed = parse_doc(&doc);
        assert_eq!(reparsed.web.backends[0].api_key.as_deref(), Some("secret"));
        assert_eq!(reparsed.web.backends[0].api_key_env, None);
        assert_eq!(reparsed.mcp_servers[0].call_timeout_ms, Some(90_000));
    }
}
