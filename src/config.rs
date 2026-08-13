//! 配置:`config.toml` 的结构与解析。
//!
//! 设计:`[providers.*]` 是一组命名的"接入方案"(profile),
//! `[agent].provider` 选当前用哪个;运行时可用 `/provider 名字` 热切换,
//! 对话历史不丢(得益于统一消息模型)。
//!
//! API key 的解析顺序(见 [`Config::resolve_provider`]):
//! 1. `api_key` 字段直接写明(`""` 表示"该服务无需鉴权",如本地 ollama);
//! 2. `api_key_env` 指定的环境变量;
//! 3. 按接口类型的默认环境变量(ANTHROPIC_API_KEY / OPENAI_API_KEY)。

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use anyhow::{anyhow, bail, Context, Result};
use serde::{Deserialize, Serialize};

use crate::compaction::{CompactionSettings, DEFAULT_KEEP_RECENT_TOKENS, DEFAULT_RESERVE_TOKENS};
use crate::harness::ModelRegistry;
use crate::permission::{PermissionRule, PermissionRules};
use crate::web::{
    WebCapabilityBinding, WebMode, WebSearchBackendKind, WebSearchCredential, WebSearchLocation,
    WebSearchSettings,
};

/// 两类接口。字符串来自 config 的 `api = "..."`。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ApiKind {
    /// Anthropic Messages API
    Messages,
    /// OpenAI Responses API
    Responses,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProviderProfile {
    OpenAiResponses,
    AnthropicMessages,
    DeepSeekResponses,
    DeepSeekMessages,
}

pub const DEFAULT_REASONING_EFFORT: &str = "medium";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "mode", content = "effort", rename_all = "snake_case")]
pub enum ReasoningEffortPolicy {
    Omit,
    Send(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ActiveModelSelection {
    pub provider: String,
    pub model: String,
    pub effort: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderCatalogEntry {
    pub name: String,
    pub default_model: String,
    pub models: Vec<ModelCatalogEntry>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelCatalogEntry {
    pub id: String,
    pub context_window: Option<u64>,
    pub max_tokens: Option<u64>,
    pub efforts: Vec<String>,
    pub default_effort: String,
    pub sends_effort: bool,
}

impl ProviderProfile {
    fn parse(value: Option<&str>, api: ApiKind) -> Result<Self> {
        let profile = match value {
            None => match api {
                ApiKind::Messages => ProviderProfile::AnthropicMessages,
                ApiKind::Responses => ProviderProfile::OpenAiResponses,
            },
            Some("openai") => ProviderProfile::OpenAiResponses,
            Some("anthropic") => ProviderProfile::AnthropicMessages,
            Some("deepseek-responses") => ProviderProfile::DeepSeekResponses,
            Some("deepseek-messages") => ProviderProfile::DeepSeekMessages,
            Some(other) => bail!(
                "未知 provider profile {:?},可选: openai | anthropic | deepseek-responses | deepseek-messages",
                other
            ),
        };
        let valid = matches!(
            (api, profile),
            (ApiKind::Messages, ProviderProfile::AnthropicMessages)
                | (ApiKind::Messages, ProviderProfile::DeepSeekMessages)
                | (ApiKind::Responses, ProviderProfile::OpenAiResponses)
                | (ApiKind::Responses, ProviderProfile::DeepSeekResponses)
        );
        if !valid {
            bail!("provider profile 与 api 类型不匹配");
        }
        Ok(profile)
    }

    fn standard_efforts(self) -> Option<&'static [&'static str]> {
        match self {
            ProviderProfile::OpenAiResponses => {
                Some(&["none", "minimal", "low", "medium", "high", "xhigh", "max"])
            }
            ProviderProfile::AnthropicMessages => Some(&["low", "medium", "high", "xhigh", "max"]),
            ProviderProfile::DeepSeekResponses | ProviderProfile::DeepSeekMessages => None,
        }
    }
}

impl ApiKind {
    fn parse(s: &str) -> Result<ApiKind> {
        match s {
            "messages" => Ok(ApiKind::Messages),
            "responses" => Ok(ApiKind::Responses),
            other => bail!("未知 api 类型 {:?},可选: messages | responses", other),
        }
    }

    fn default_key_env(&self) -> &'static str {
        match self {
            ApiKind::Messages => "ANTHROPIC_API_KEY",
            ApiKind::Responses => "OPENAI_API_KEY",
        }
    }
}

/// 解析完成、可直接构造 Provider 的设置(key 已就位)。
#[derive(Debug, Clone)]
pub struct ProviderSettings {
    pub name: String,
    pub api: ApiKind,
    pub profile: ProviderProfile,
    pub base_url: String,
    /// 空字符串 = 不发鉴权头。
    pub api_key: String,
    pub model: String,
    pub max_tokens: Option<u64>,
    /// 模型上下文窗口(token)。配置后启用上下文预算强制。
    pub context_window: Option<u64>,
    pub selected_effort: String,
    pub reasoning_effort: ReasoningEffortPolicy,
    /// Frozen at provider construction. Hosted tools remain provider-owned;
    /// harness-owned bindings create one matching local tool for the epoch.
    pub web: WebCapabilityBinding,
}

// ---- config.toml 的原始形状(serde 直接映射) ----

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct FileConfig {
    agent: AgentSection,
    #[serde(default)]
    retry: RetrySection,
    #[serde(default)]
    compaction: CompactionSection,
    #[serde(default)]
    permissions: PermissionsSection,
    #[serde(default)]
    web: WebSection,
    #[serde(default)]
    mcp_servers: Vec<RawMcpServerSection>,
    #[serde(default)]
    providers: BTreeMap<String, RawProviderSection>,
}

#[derive(Debug, Deserialize, Clone)]
#[serde(deny_unknown_fields)]
struct RawMcpServerSection {
    name: String,
    command: String,
    #[serde(default)]
    args: Vec<String>,
    #[serde(default)]
    env: BTreeMap<String, String>,
    #[serde(default)]
    cwd: Option<PathBuf>,
    #[serde(default)]
    enabled: Option<bool>,
    #[serde(default)]
    startup_timeout_ms: Option<u64>,
    #[serde(default)]
    call_timeout_ms: Option<u64>,
    #[serde(default)]
    always_ask: Option<bool>,
    #[serde(default)]
    include_tools: Option<Vec<String>>,
    #[serde(default)]
    exclude_tools: Vec<String>,
}

#[derive(Debug, Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct WebSection {
    #[serde(default)]
    mode: Option<String>,
    #[serde(default)]
    external_backends: Vec<String>,
    #[serde(default)]
    backends: BTreeMap<String, RawWebBackendSection>,
    #[serde(default)]
    context_size: Option<String>,
    #[serde(default)]
    allowed_domains: Vec<String>,
    #[serde(default)]
    location: Option<WebLocationSection>,
}

#[derive(Debug, Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct RawWebBackendSection {
    #[serde(default)]
    api_key: Option<String>,
    #[serde(default)]
    api_key_env: Option<String>,
}

#[derive(Debug, Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct WebLocationSection {
    #[serde(default)]
    country: Option<String>,
    #[serde(default)]
    region: Option<String>,
    #[serde(default)]
    city: Option<String>,
    #[serde(default)]
    timezone: Option<String>,
}

#[derive(Debug, Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct RetrySection {
    #[serde(default)]
    max_attempts: Option<u32>,
    #[serde(default)]
    base_delay_ms: Option<u64>,
    #[serde(default)]
    max_delay_ms: Option<u64>,
    #[serde(default)]
    max_retry_after_ms: Option<u64>,
}

#[derive(Debug, Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct CompactionSection {
    #[serde(default)]
    enabled: Option<bool>,
    #[serde(default)]
    reserve_tokens: Option<u64>,
    #[serde(default)]
    keep_recent_tokens: Option<u64>,
}

#[derive(Debug, Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct PermissionsSection {
    #[serde(default)]
    workspace_read: Option<String>,
    #[serde(default)]
    workspace_write: Option<String>,
    #[serde(default)]
    outside_workspace: Option<String>,
    #[serde(default)]
    commands: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct AgentSection {
    provider: String,
    #[serde(default)]
    shell: Option<String>,
    #[serde(default)]
    system_prompt: Option<String>,
    #[serde(default)]
    max_turns: Option<u32>,
    #[serde(default)]
    tool_timeout_secs: Option<u64>,
}

#[derive(Debug, Deserialize, Clone)]
#[serde(deny_unknown_fields)]
struct RawProviderSection {
    api: String,
    #[serde(default)]
    profile: Option<String>,
    base_url: String,
    model: Option<String>,
    #[serde(default)]
    default_model: Option<String>,
    #[serde(default)]
    models: BTreeMap<String, RawModelSection>,
    #[serde(default)]
    api_key: Option<String>,
    #[serde(default)]
    api_key_env: Option<String>,
    #[serde(default)]
    max_tokens: Option<u64>,
    #[serde(default)]
    context_window: Option<u64>,
}

#[derive(Debug, Deserialize, Clone)]
#[serde(deny_unknown_fields)]
struct RawModelSection {
    context_window: u64,
    #[serde(default)]
    max_tokens: Option<u64>,
    #[serde(default)]
    efforts: Option<Vec<String>>,
    #[serde(default)]
    default_effort: Option<String>,
}

#[derive(Debug, Clone)]
struct ProviderSection {
    api: ApiKind,
    profile: ProviderProfile,
    base_url: String,
    default_model: String,
    models: BTreeMap<String, ModelSection>,
    api_key: Option<String>,
    api_key_env: Option<String>,
}

#[derive(Debug, Clone)]
struct WebBackendAuth {
    api_key: Option<String>,
    api_key_env: Option<String>,
}

#[derive(Debug, Clone)]
struct ModelSection {
    context_window: Option<u64>,
    max_tokens: Option<u64>,
    reasoning: ModelReasoning,
}

#[derive(Debug, Clone)]
enum ModelReasoning {
    Omit,
    Send {
        efforts: Vec<String>,
        default_effort: String,
    },
}

impl ModelReasoning {
    fn efforts(&self) -> Vec<String> {
        match self {
            ModelReasoning::Omit => vec![DEFAULT_REASONING_EFFORT.to_string()],
            ModelReasoning::Send { efforts, .. } => efforts.clone(),
        }
    }

    fn default_effort(&self) -> &str {
        match self {
            ModelReasoning::Omit => DEFAULT_REASONING_EFFORT,
            ModelReasoning::Send { default_effort, .. } => default_effort,
        }
    }

    fn resolve(&self, effort: &str) -> Result<ReasoningEffortPolicy> {
        match self {
            ModelReasoning::Omit if effort == DEFAULT_REASONING_EFFORT => {
                Ok(ReasoningEffortPolicy::Omit)
            }
            ModelReasoning::Omit => bail!(
                "该模型未配置可发送的 reasoning effort,只能使用 {}",
                DEFAULT_REASONING_EFFORT
            ),
            ModelReasoning::Send { efforts, .. } if efforts.iter().any(|item| item == effort) => {
                Ok(ReasoningEffortPolicy::Send(effort.to_string()))
            }
            ModelReasoning::Send { efforts, .. } => bail!(
                "未知 reasoning effort {:?},可选: {}",
                effort,
                efforts.join(", ")
            ),
        }
    }
}

/// 一个 stdio MCP server 的启动与治理配置(`[[mcp_servers]]`)。
/// server 是不受信的外部进程;这里只描述怎么启动与怎么收紧,放宽不存在。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct McpServerConfig {
    /// 工具前缀(`mcp__{name}__{tool}`),须匹配 `^[a-z0-9][a-z0-9_-]{0,31}$`。
    pub name: String,
    /// 可执行文件,不经 shell 解析。Windows 上 npm 系 server 写
    /// `command = "cmd"`、`args = ["/c", "npx", ...]`。
    pub command: String,
    pub args: Vec<String>,
    /// 叠加在继承环境之上;注意环境中的敏感变量对 server 进程可见。
    pub env: Vec<(String, String)>,
    pub cwd: Option<PathBuf>,
    pub enabled: bool,
    /// spawn、era 探测与 tools/list 的总预算。npx 首次运行会下载包,留足余量。
    pub startup_timeout: std::time::Duration,
    /// 单次 tools/call 预算,超时后发送 cancelled 通知并返回超时错误。
    pub call_timeout: std::time::Duration,
    /// 只可收紧:true 时该 server 全部工具逐次审批,不提供 session 授权。
    pub always_ask: bool,
    /// 按 server 侧原始工具名精确过滤;None 表示不过滤。
    pub include_tools: Option<Vec<String>>,
    pub exclude_tools: Vec<String>,
}

const DEFAULT_MCP_STARTUP_TIMEOUT_MS: u64 = 30_000;
const DEFAULT_MCP_CALL_TIMEOUT_MS: u64 = 60_000;

fn normalize_mcp_server(
    raw: RawMcpServerSection,
    seen: &mut std::collections::HashSet<String>,
) -> Result<McpServerConfig> {
    let name = raw.name.trim().to_string();
    let head_valid = name
        .chars()
        .next()
        .is_some_and(|c| c.is_ascii_lowercase() || c.is_ascii_digit());
    let body_valid = name
        .chars()
        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_' || c == '-');
    if name.is_empty() || name.len() > 32 || !head_valid || !body_valid {
        bail!(
            "[[mcp_servers]].name {:?} 无效:须匹配 ^[a-z0-9][a-z0-9_-]{{0,31}}$",
            raw.name
        );
    }
    if !seen.insert(name.clone()) {
        bail!("[[mcp_servers]] 存在重复的 name {:?}", name);
    }
    if raw.command.trim().is_empty() {
        bail!("[[mcp_servers]] {} 的 command 不能为空", name);
    }
    let startup_timeout_ms = raw
        .startup_timeout_ms
        .unwrap_or(DEFAULT_MCP_STARTUP_TIMEOUT_MS);
    let call_timeout_ms = raw.call_timeout_ms.unwrap_or(DEFAULT_MCP_CALL_TIMEOUT_MS);
    if startup_timeout_ms == 0 || call_timeout_ms == 0 {
        bail!(
            "[[mcp_servers]] {} 的 startup_timeout_ms/call_timeout_ms 必须大于 0",
            name
        );
    }
    for key in raw.env.keys() {
        if key.trim().is_empty() || key.chars().any(char::is_control) {
            bail!("[[mcp_servers]] {} 的 env 含空名或控制字符", name);
        }
    }
    Ok(McpServerConfig {
        name,
        command: raw.command.trim().to_string(),
        args: raw.args,
        env: raw.env.into_iter().collect(),
        cwd: raw.cwd,
        enabled: raw.enabled.unwrap_or(true),
        startup_timeout: std::time::Duration::from_millis(startup_timeout_ms),
        call_timeout: std::time::Duration::from_millis(call_timeout_ms),
        always_ask: raw.always_ask.unwrap_or(false),
        include_tools: raw.include_tools,
        exclude_tools: raw.exclude_tools,
    })
}

/// 校验过的配置。
#[derive(Debug)]
pub struct Config {
    pub(crate) source_path: Option<PathBuf>,
    pub active_provider: String,
    /// auto | gitbash | powershell | cmd
    pub shell: String,
    pub system_prompt: Option<String>,
    /// 一轮对话里最多连续调用模型的次数(失控保护)。
    pub max_turns: u32,
    pub retry_policy: crate::agent_loop::RetryPolicy,
    /// 单个工具调用的执行超时(None = 不限制;run_command 另有自己的超时)。
    pub tool_timeout: Option<std::time::Duration>,
    pub compaction: CompactionSettings,
    pub permission_rules: PermissionRules,
    /// stdio MCP server 列表;空表示完全不装配 MCP 能力。
    pub mcp_servers: Vec<McpServerConfig>,
    web_mode: WebMode,
    web_search: WebSearchSettings,
    external_web_backends: Vec<WebSearchBackendKind>,
    external_web_backend_auth: BTreeMap<WebSearchBackendKind, WebBackendAuth>,
    providers: BTreeMap<String, ProviderSection>,
}

impl Config {
    pub fn load(path: &Path) -> Result<Config> {
        let text = std::fs::read_to_string(path)
            .with_context(|| format!("读取配置 {} 失败", path.display()))?;
        let raw: FileConfig =
            toml::from_str(&text).with_context(|| format!("解析配置 {} 失败", path.display()))?;
        if raw.providers.is_empty() {
            bail!("配置里没有任何 [providers.*]");
        }
        if !raw.providers.contains_key(&raw.agent.provider) {
            bail!(
                "[agent].provider = {:?} 不存在,可选: {}",
                raw.agent.provider,
                raw.providers.keys().cloned().collect::<Vec<_>>().join(", ")
            );
        }
        let compaction = CompactionSettings {
            enabled: raw.compaction.enabled.unwrap_or(true),
            reserve_tokens: raw
                .compaction
                .reserve_tokens
                .unwrap_or(DEFAULT_RESERVE_TOKENS),
            keep_recent_tokens: raw
                .compaction
                .keep_recent_tokens
                .unwrap_or(DEFAULT_KEEP_RECENT_TOKENS),
        };
        compaction.validate()?;
        let providers = raw
            .providers
            .into_iter()
            .map(|(name, provider)| {
                normalize_provider(&name, provider).map(|provider| (name, provider))
            })
            .collect::<Result<BTreeMap<_, _>>>()?;
        let defaults = PermissionRules::default();
        let permission_rules = PermissionRules {
            workspace_read: parse_permission_rule(
                raw.permissions.workspace_read.as_deref(),
                defaults.workspace_read,
                "workspace_read",
            )?,
            workspace_write: parse_permission_rule(
                raw.permissions.workspace_write.as_deref(),
                defaults.workspace_write,
                "workspace_write",
            )?,
            outside_workspace: parse_permission_rule(
                raw.permissions.outside_workspace.as_deref(),
                defaults.outside_workspace,
                "outside_workspace",
            )?,
            opaque_side_effect: parse_permission_rule(
                raw.permissions.commands.as_deref(),
                defaults.opaque_side_effect,
                "commands",
            )?,
        };
        let shell = raw.agent.shell.unwrap_or_else(|| "auto".into());
        if !matches!(shell.as_str(), "auto" | "gitbash" | "powershell" | "cmd") {
            bail!(
                "[agent].shell = {:?} 无效,可选: auto | gitbash | powershell | cmd",
                shell
            );
        }
        let max_turns = raw.agent.max_turns.unwrap_or(200);
        if max_turns == 0 {
            bail!("[agent].max_turns 必须大于 0");
        }
        let retry_defaults = crate::agent_loop::RetryPolicy::default();
        let retry_policy = crate::agent_loop::RetryPolicy {
            max_attempts: raw
                .retry
                .max_attempts
                .unwrap_or(retry_defaults.max_attempts),
            base_delay: std::time::Duration::from_millis(
                raw.retry
                    .base_delay_ms
                    .unwrap_or(retry_defaults.base_delay.as_millis() as u64),
            ),
            max_delay: std::time::Duration::from_millis(
                raw.retry
                    .max_delay_ms
                    .unwrap_or(retry_defaults.max_delay.as_millis() as u64),
            ),
            max_retry_after: std::time::Duration::from_millis(
                raw.retry
                    .max_retry_after_ms
                    .unwrap_or(retry_defaults.max_retry_after.as_millis() as u64),
            ),
            jitter_seed: retry_defaults.jitter_seed,
        };
        validate_retry_policy(retry_policy)?;
        let WebSection {
            mode,
            external_backends,
            backends,
            context_size,
            allowed_domains,
            location,
        } = raw.web;
        let web_mode = mode.as_deref().unwrap_or("auto").trim();
        let web_mode = WebMode::parse(&web_mode).map_err(|error| anyhow!("[web].mode {error}"))?;
        let mut external_web_backends = Vec::new();
        for backend in external_backends {
            let backend = backend.trim().to_ascii_lowercase();
            if backend.is_empty() || backend.chars().count() > 64 {
                bail!("[web].external_backends contains an empty or oversized backend name");
            }
            if !backend.chars().all(|character| {
                character.is_ascii_lowercase() || character.is_ascii_digit() || character == '-'
            }) {
                bail!(
                    "[web].external_backends contains invalid backend name {:?}",
                    backend
                );
            }
            let kind = WebSearchBackendKind::parse(&backend).map_err(|error| {
                anyhow!(
                    "[web].external_backends contains unsupported backend {:?}; {}",
                    backend,
                    error
                )
            })?;
            if external_web_backends.contains(&kind) {
                bail!(
                    "[web].external_backends contains duplicate backend {:?}",
                    backend
                );
            }
            external_web_backends.push(kind);
        }
        let mut external_web_backend_auth = BTreeMap::new();
        for (backend, auth) in backends {
            let backend_name = backend.trim().to_ascii_lowercase();
            if backend_name.is_empty() || backend_name.chars().count() > 64 {
                bail!("[web.backends] contains an empty or oversized backend name");
            }
            if !backend_name.chars().all(|character| {
                character.is_ascii_lowercase() || character.is_ascii_digit() || character == '-'
            }) {
                bail!(
                    "[web.backends] contains invalid backend name {:?}",
                    backend_name
                );
            }
            let kind = WebSearchBackendKind::parse(&backend_name).map_err(|error| {
                anyhow!(
                    "[web.backends] contains unsupported backend {:?}; {}",
                    backend_name,
                    error
                )
            })?;
            if auth.api_key.is_some() && auth.api_key_env.is_some() {
                bail!(
                    "[web.backends.{}] 只能配置 api_key 或 api_key_env 其中一个",
                    backend_name
                );
            }
            let api_key_env = auth
                .api_key_env
                .map(|value| {
                    let value = value.trim().to_string();
                    if value.is_empty()
                        || value.chars().count() > 128
                        || value.chars().any(char::is_control)
                    {
                        bail!(
                            "[web.backends.{}].api_key_env is empty, oversized, or contains control characters",
                            backend_name
                        );
                    }
                    Ok(value)
                })
                .transpose()?;
            if external_web_backend_auth
                .insert(
                    kind,
                    WebBackendAuth {
                        api_key: auth.api_key,
                        api_key_env,
                    },
                )
                .is_some()
            {
                bail!(
                    "[web.backends] contains duplicate backend {:?}",
                    backend_name
                );
            }
        }
        let web_search = WebSearchSettings::new(
            context_size.as_deref(),
            allowed_domains,
            location.map(|location| WebSearchLocation {
                country: location.country,
                region: location.region,
                city: location.city,
                timezone: location.timezone,
            }),
        )
        .map_err(|error| anyhow!("[web] {error}"))?;
        let mut seen_mcp_names = std::collections::HashSet::new();
        let mcp_servers = raw
            .mcp_servers
            .into_iter()
            .map(|server| normalize_mcp_server(server, &mut seen_mcp_names))
            .collect::<Result<Vec<_>>>()?;
        Ok(Config {
            source_path: Some(path.to_path_buf()),
            active_provider: raw.agent.provider,
            shell,
            system_prompt: raw.agent.system_prompt,
            max_turns,
            retry_policy,
            tool_timeout: raw
                .agent
                .tool_timeout_secs
                .filter(|secs| *secs > 0)
                .map(std::time::Duration::from_secs),
            compaction,
            permission_rules,
            mcp_servers,
            web_mode,
            web_search,
            external_web_backends,
            external_web_backend_auth,
            providers,
        })
    }

    pub fn provider_names(&self) -> Vec<String> {
        self.providers.keys().cloned().collect()
    }

    pub fn provider_catalog(&self) -> Vec<ProviderCatalogEntry> {
        self.providers
            .iter()
            .map(|(name, provider)| ProviderCatalogEntry {
                name: name.clone(),
                default_model: provider.default_model.clone(),
                models: provider
                    .models
                    .iter()
                    .map(|(id, model)| ModelCatalogEntry {
                        id: id.clone(),
                        context_window: model.context_window,
                        max_tokens: model.max_tokens,
                        efforts: model.reasoning.efforts(),
                        default_effort: model.reasoning.default_effort().to_string(),
                        sends_effort: matches!(model.reasoning, ModelReasoning::Send { .. }),
                    })
                    .collect(),
            })
            .collect()
    }

    pub fn default_selection(&self, provider: &str) -> Result<ActiveModelSelection> {
        let section = self.provider(provider)?;
        let model = section
            .models
            .get(&section.default_model)
            .expect("normalized provider must contain its default model");
        Ok(ActiveModelSelection {
            provider: provider.to_string(),
            model: section.default_model.clone(),
            effort: model.reasoning.default_effort().to_string(),
        })
    }

    pub fn model_default_effort(&self, provider: &str, model: &str) -> Result<&str> {
        let section = self.provider(provider)?;
        section
            .models
            .get(model)
            .map(|model| model.reasoning.default_effort())
            .ok_or_else(|| anyhow!("provider {:?} 没有模型 {:?}", provider, model))
    }

    /// 把某个 provider 的默认模型解析成可用设置(含 key 查找)。
    pub fn resolve_provider(&self, name: &str) -> Result<ProviderSettings> {
        let selection = self.default_selection(name)?;
        self.resolve_selection(&selection)
    }

    pub fn resolve_selection(&self, selection: &ActiveModelSelection) -> Result<ProviderSettings> {
        let sec = self.provider(&selection.provider)?;
        let model = sec.models.get(&selection.model).ok_or_else(|| {
            anyhow!(
                "provider {:?} 没有模型 {:?},可选: {}",
                selection.provider,
                selection.model,
                sec.models.keys().cloned().collect::<Vec<_>>().join(", ")
            )
        })?;
        let reasoning_effort = model.reasoning.resolve(&selection.effort)?;
        let api_key = match &sec.api_key {
            Some(k) => k.clone(), // 允许 ""(无鉴权)
            None => {
                let env_name = sec
                    .api_key_env
                    .clone()
                    .unwrap_or_else(|| sec.api.default_key_env().to_string());
                std::env::var(&env_name).map_err(|_| {
                    anyhow!(
                        "找不到 API key:请设置环境变量 {},或在 config.toml 的 \
                         [providers.{}] 里写 api_key = \"...\"(本地无鉴权服务写 api_key = \"\")",
                        env_name,
                        selection.provider
                    )
                })?
            }
        };
        let external_backend = self.resolve_external_web_backend(sec.profile)?;
        Ok(ProviderSettings {
            name: selection.provider.clone(),
            api: sec.api,
            profile: sec.profile,
            base_url: sec.base_url.trim_end_matches('/').to_string(),
            api_key,
            model: selection.model.clone(),
            max_tokens: model.max_tokens,
            context_window: model.context_window,
            selected_effort: selection.effort.clone(),
            reasoning_effort,
            web: WebCapabilityBinding::resolve(
                self.web_mode,
                sec.profile,
                self.web_search.clone(),
                external_backend,
            ),
        })
    }

    fn resolve_external_web_backend(
        &self,
        profile: ProviderProfile,
    ) -> Result<Option<(WebSearchBackendKind, WebSearchCredential)>> {
        let needs_external = match self.web_mode {
            WebMode::External => true,
            WebMode::Auto => profile != ProviderProfile::OpenAiResponses,
            WebMode::Native | WebMode::Disabled => false,
        };
        if !needs_external {
            return Ok(None);
        }
        for backend in &self.external_web_backends {
            if let Some(credential) = self.resolve_web_backend_credential(*backend)? {
                return Ok(Some((*backend, credential)));
            }
        }
        if self.web_mode == WebMode::External {
            if self.external_web_backends.is_empty() {
                bail!(
                    "[web].mode = \"external\" requires at least one supported external_backends entry"
                );
            }
            bail!(
                "没有找到可用的外部 Web 搜索密钥；可在 [web.backends.<name>] 写 api_key，或设置这些环境变量: {}",
                self.external_web_backends
                    .iter()
                    .map(|backend| backend.api_key_env())
                    .collect::<Vec<_>>()
                    .join(", ")
            );
        }
        Ok(None)
    }

    fn resolve_web_backend_credential(
        &self,
        backend: WebSearchBackendKind,
    ) -> Result<Option<WebSearchCredential>> {
        let auth = self.external_web_backend_auth.get(&backend);
        if let Some(api_key) = auth.and_then(|auth| auth.api_key.as_deref()) {
            if api_key.trim().is_empty() {
                return Ok(None);
            }
            return WebSearchCredential::new(api_key.to_string())
                .map(Some)
                .map_err(|error| {
                    anyhow!(
                        "[web.backends.{}].api_key 无效: {}",
                        backend_name(backend),
                        error
                    )
                });
        }
        let env_name = auth
            .and_then(|auth| auth.api_key_env.as_deref())
            .unwrap_or_else(|| backend.api_key_env());
        let Some(value) = std::env::var(env_name)
            .ok()
            .filter(|value| !value.trim().is_empty())
        else {
            return Ok(None);
        };
        WebSearchCredential::new(value)
            .map(Some)
            .map_err(|error| anyhow!("环境变量 {} 中的 Web 搜索密钥无效: {}", env_name, error))
    }

    pub fn validate_selection(&self, selection: &ActiveModelSelection) -> Result<()> {
        let sec = self.provider(&selection.provider)?;
        let model = sec.models.get(&selection.model).ok_or_else(|| {
            anyhow!(
                "provider {:?} 没有模型 {:?}",
                selection.provider,
                selection.model
            )
        })?;
        model.reasoning.resolve(&selection.effort)?;
        Ok(())
    }

    fn provider(&self, name: &str) -> Result<&ProviderSection> {
        self.providers.get(name).ok_or_else(|| {
            anyhow!(
                "provider {:?} 不存在,可选: {}",
                name,
                self.provider_names().join(", ")
            )
        })
    }
}

fn backend_name(backend: WebSearchBackendKind) -> &'static str {
    match backend {
        WebSearchBackendKind::Tavily => "tavily",
        WebSearchBackendKind::Brave => "brave",
        WebSearchBackendKind::Exa => "exa",
        WebSearchBackendKind::Serper => "serper",
    }
}

impl ModelRegistry for Config {
    fn initial_selection(&self) -> Result<ActiveModelSelection> {
        Config::default_selection(self, &self.active_provider)
    }

    fn default_selection(&self, provider: &str) -> Result<ActiveModelSelection> {
        Config::default_selection(self, provider)
    }

    fn resolve_selection(&self, selection: &ActiveModelSelection) -> Result<ProviderSettings> {
        Config::resolve_selection(self, selection)
    }

    fn validate_selection(&self, selection: &ActiveModelSelection) -> Result<()> {
        Config::validate_selection(self, selection)
    }

    fn model_default_effort(&self, provider: &str, model: &str) -> Result<String> {
        Config::model_default_effort(self, provider, model).map(str::to_string)
    }

    fn provider_catalog(&self) -> Vec<ProviderCatalogEntry> {
        Config::provider_catalog(self)
    }
}

fn normalize_provider(name: &str, raw: RawProviderSection) -> Result<ProviderSection> {
    if name.trim().is_empty() {
        bail!("provider 名称不能为空");
    }
    let base_url = raw.base_url.trim().to_string();
    if !base_url.starts_with("http://") && !base_url.starts_with("https://") {
        bail!(
            "[providers.{}].base_url 必须是 http:// 或 https:// URL",
            name
        );
    }
    if raw.api_key.is_some() && raw.api_key_env.is_some() {
        bail!(
            "[providers.{}] 只能配置 api_key 或 api_key_env 其中一个",
            name
        );
    }
    if raw
        .api_key_env
        .as_deref()
        .is_some_and(|name| name.trim().is_empty())
    {
        bail!("[providers.{}].api_key_env 不能为空", name);
    }
    let api = ApiKind::parse(&raw.api).with_context(|| format!("[providers.{}].api 无效", name))?;
    let profile = ProviderProfile::parse(raw.profile.as_deref(), api)
        .with_context(|| format!("[providers.{}].profile 无效", name))?;
    let uses_new_format = raw.default_model.is_some() || !raw.models.is_empty();
    let uses_legacy_format =
        raw.model.is_some() || raw.max_tokens.is_some() || raw.context_window.is_some();
    if uses_new_format && uses_legacy_format {
        bail!(
            "[providers.{}] 不能混用旧 model/max_tokens/context_window 与新 default_model/models",
            name
        );
    }

    let (default_model, models) = if uses_new_format {
        let default_model = raw
            .default_model
            .ok_or_else(|| anyhow!("[providers.{}] 使用 models 时必须配置 default_model", name))?;
        if raw.models.is_empty() {
            bail!("[providers.{}].models 不能为空", name);
        }
        let models = raw
            .models
            .into_iter()
            .map(|(model_id, model)| {
                if model_id.trim().is_empty() {
                    bail!("[providers.{}].models 含空模型 ID", name);
                }
                normalize_model(name, &model_id, profile, model).map(|model| (model_id, model))
            })
            .collect::<Result<BTreeMap<_, _>>>()?;
        if !models.contains_key(&default_model) {
            bail!(
                "[providers.{}].default_model {:?} 不在 models 中",
                name,
                default_model
            );
        }
        (default_model, models)
    } else {
        let model = raw
            .model
            .ok_or_else(|| anyhow!("[providers.{}] 缺少 model 或 models", name))?;
        validate_model_limits(name, &model, raw.context_window, raw.max_tokens)?;
        let mut models = BTreeMap::new();
        models.insert(
            model.clone(),
            ModelSection {
                context_window: raw.context_window,
                max_tokens: raw.max_tokens,
                reasoning: normalize_model_reasoning(name, &model, profile, None, None)?,
            },
        );
        (model, models)
    };

    Ok(ProviderSection {
        api,
        profile,
        base_url,
        default_model,
        models,
        api_key: raw.api_key,
        api_key_env: raw.api_key_env.map(|name| name.trim().to_string()),
    })
}

fn normalize_model(
    provider_name: &str,
    model_id: &str,
    profile: ProviderProfile,
    raw: RawModelSection,
) -> Result<ModelSection> {
    let context_window = Some(raw.context_window);
    validate_model_limits(provider_name, model_id, context_window, raw.max_tokens)?;
    let reasoning = normalize_model_reasoning(
        provider_name,
        model_id,
        profile,
        raw.efforts,
        raw.default_effort,
    )?;
    Ok(ModelSection {
        context_window,
        max_tokens: raw.max_tokens,
        reasoning,
    })
}

fn normalize_model_reasoning(
    provider_name: &str,
    model_id: &str,
    profile: ProviderProfile,
    configured_efforts: Option<Vec<String>>,
    configured_default: Option<String>,
) -> Result<ModelReasoning> {
    let Some(standard_efforts) = profile.standard_efforts() else {
        if configured_efforts.is_some() || configured_default.is_some() {
            bail!(
                "[providers.{}.models.{:?}] profile {:?} 不支持配置 efforts/default_effort",
                provider_name,
                model_id,
                profile
            );
        }
        return Ok(ModelReasoning::Omit);
    };

    let mut efforts = match configured_efforts {
        Some(efforts) => efforts,
        None => standard_efforts
            .iter()
            .map(|effort| (*effort).to_string())
            .collect(),
    };
    if efforts.is_empty() {
        if configured_default.is_some() {
            bail!(
                "[providers.{}.models.{:?}] efforts=[] 时不能配置 default_effort",
                provider_name,
                model_id
            );
        }
        return Ok(ModelReasoning::Omit);
    }
    for effort in &mut efforts {
        *effort = effort.trim().to_string();
        if effort.is_empty() || effort.chars().count() > 64 {
            bail!(
                "[providers.{}.models.{:?}].efforts 含空值或超过 64 字符",
                provider_name,
                model_id
            );
        }
    }
    let mut seen = std::collections::HashSet::new();
    for effort in &efforts {
        if !seen.insert(effort.as_str()) {
            bail!(
                "[providers.{}.models.{:?}].efforts 重复: {:?}",
                provider_name,
                model_id,
                effort
            );
        }
    }

    let default_effort = match configured_default {
        Some(default) => {
            let default = default.trim().to_string();
            if default.is_empty() || default.chars().count() > 64 {
                bail!(
                    "[providers.{}.models.{:?}].default_effort 不能为空或超过 64 字符",
                    provider_name,
                    model_id
                );
            }
            default
        }
        None => efforts
            .iter()
            .find(|effort| effort.as_str() == DEFAULT_REASONING_EFFORT)
            .unwrap_or(&efforts[0])
            .clone(),
    };
    if !efforts.iter().any(|effort| effort == &default_effort) {
        bail!(
            "[providers.{}.models.{:?}].default_effort {:?} 不在 efforts 中",
            provider_name,
            model_id,
            default_effort
        );
    }
    Ok(ModelReasoning::Send {
        efforts,
        default_effort,
    })
}

fn validate_model_limits(
    provider_name: &str,
    model_id: &str,
    context_window: Option<u64>,
    max_tokens: Option<u64>,
) -> Result<()> {
    if context_window == Some(0) {
        bail!(
            "[providers.{}.models.{:?}].context_window 必须大于 0",
            provider_name,
            model_id
        );
    }
    if max_tokens == Some(0) {
        bail!(
            "[providers.{}.models.{:?}].max_tokens 必须大于 0",
            provider_name,
            model_id
        );
    }
    if let (Some(window), Some(max)) = (context_window, max_tokens) {
        if max > window {
            bail!(
                "[providers.{}.models.{:?}].max_tokens 不能大于 context_window",
                provider_name,
                model_id
            );
        }
    }
    Ok(())
}

fn parse_permission_rule(
    value: Option<&str>,
    default: PermissionRule,
    field: &str,
) -> Result<PermissionRule> {
    value
        .map(PermissionRule::parse)
        .transpose()
        .map_err(|error| anyhow!("[permissions].{}: {}", field, error))
        .map(|rule| rule.unwrap_or(default))
}

fn validate_retry_policy(policy: crate::agent_loop::RetryPolicy) -> Result<()> {
    if policy.max_attempts == 0 {
        bail!("[retry].max_attempts 必须大于 0");
    }
    if policy.base_delay.is_zero() {
        bail!("[retry].base_delay_ms 必须大于 0");
    }
    if policy.max_delay < policy.base_delay {
        bail!("[retry].max_delay_ms 不能小于 base_delay_ms");
    }
    if policy.max_retry_after.is_zero() {
        bail!("[retry].max_retry_after_ms 必须大于 0");
    }
    Ok(())
}

/// 首次运行时写出的模板(同时也是 config.example.toml 的内容)。
pub const EXAMPLE_CONFIG: &str = r#"# Onemore 全局配置文件(Windows 默认 %APPDATA%/onemore/config.toml)
# [agent].provider 决定当前用哪个 [providers.*];TUI 里可用 /provider 名字 热切换。

[agent]
provider = "anthropic"
# run_command 用的 shell:auto | gitbash | powershell | cmd
# auto = 找到 Git Bash 就用它(模型对 bash 语法最熟),否则退回 PowerShell
shell = "auto"
# 一轮对话里最多连续调用模型的次数(防止失控空转)
max_turns = 200
# 可选：单个工具调用超时秒数；省略或 0 表示不限制。
# tool_timeout_secs = 300
# 想完全接管系统提示就取消下面的注释:
# system_prompt = "You are ..."

# 模型请求级重试。max_attempts 包含首次请求；只有尚未产生流事件时才会自动重放。
[retry]
max_attempts = 8
base_delay_ms = 1000
max_delay_ms = 10000
max_retry_after_ms = 60000

# 自动压缩在正常输入预算前预留一段余量，并原样保留最近消息。
[compaction]
enabled = true
reserve_tokens = 16384
keep_recent_tokens = 20000

# 权限规则:allow | ask | deny。hard deny(设备路径、无法安全解析的路径)不受这里覆盖。
[permissions]
workspace_read = "allow"
workspace_write = "allow"
outside_workspace = "ask"
commands = "ask"

# ---- Web 搜索 ----
# mode 可选值：auto | native | external | disabled
# auto：OpenAI Responses 使用服务商原生搜索；其他模型按 external_backends 的顺序，
#       选择第一个能解析出非空密钥的外部搜索厂商（直接 api_key、自定义 api_key_env
#       或厂商标准环境变量均可）。
# native：只使用模型服务商的原生搜索；当前仅支持 OpenAI Responses，不回退到外部厂商。
# external：强制使用外部搜索厂商；未配置厂商或找不到可用密钥时，配置加载会失败。
# disabled：完全关闭 Web 搜索。
# 选中的实现会保持不变，直到执行 /reload 或切换 provider/model。
[web]
mode = "auto"
# 外部搜索厂商的优先级从左到右；固定支持 tavily、brave、exa、serper。
# 未配置厂商密钥时，依次尝试标准环境变量：TAVILY_API_KEY、BRAVE_SEARCH_API_KEY、EXA_API_KEY、SERPER_API_KEY。
# 每次只会绑定一个厂商，不会在搜索失败后自动切换到下一个厂商。
# external_backends = ["tavily", "brave", "exa", "serper"]
# 可选：搜索结果上下文量。low / medium / high 分别最多返回 3 / 5 / 10 条结果；
# OpenAI 原生搜索会把它作为 search_context_size 提示。省略时使用服务商或 Onemore 默认值。
# context_size = "medium"
# 可选：只允许返回这些域名及其子域名，最多 100 个；填写纯域名，不要带协议或路径。
# allowed_domains = ["developers.openai.com", "platform.openai.com"]
# 可选：搜索位置提示。该信息来自本地配置，模型不能在工具参数中修改。
# [web.location]
# country = "US"
# region = "California"
# city = "San Francisco"
# timezone = "America/Los_Angeles"
# 每个厂商都支持直接写 api_key，或指定 api_key_env 从环境变量读取；两者只能选一个。
# 两者都省略时，默认读取该厂商的标准环境变量。下面的四段都是可选示例，请按需取消注释。
# [web.backends.tavily]
# api_key = "tvly-..."
# [web.backends.brave]
# api_key_env = "BRAVE_SEARCH_API_KEY"
# [web.backends.exa]
# api_key = "..."
# [web.backends.serper]
# api_key = "..."
# 如果使用环境变量，请删除对应 api_key，改为 api_key_env = "SERPER_API_KEY"

# ---- MCP servers(stdio)----
# 外部 MCP server 以子进程接入，工具以 mcp__{name}__ 前缀注册；详见
# docs/planning/mcp-client-plan.md。server 属于不受信第三方：工具调用默认逐次
# 审批(可在会话内按工具授权)，server 声明的 annotations 不会放宽权限。
# [[mcp_servers]]
# name = "playwright"               # 必填：工具前缀，须匹配 ^[a-z0-9][a-z0-9_-]{0,31}$
# command = "cmd"                   # 必填：可执行文件，不经 shell 解析
# args = ["/c", "npx", "-y", "@playwright/mcp@latest"]
# # Windows 上 npm 系 server 需经 cmd /c 启动；npx 首次运行会下载包，
# # 启动超时请留足或预先全局安装。
# # env = { KEY = "value" }         # 叠加在继承环境上；环境中的敏感变量对 server 可见
# # cwd = "E:/somewhere"            # 缺省继承 Onemore 工作目录
# # enabled = true
# # startup_timeout_ms = 30000      # spawn + 协议探测 + tools/list 的总预算
# # call_timeout_ms = 60000         # 单次工具调用预算，超时会通知 server 取消
# # always_ask = true               # 只可收紧：该 server 全部工具逐次审批
# # include_tools = ["browser_navigate", "browser_click"]  # 按 server 侧原始名精确过滤
# # exclude_tools = []

# ---- Anthropic Messages API ----
[providers.anthropic]
api = "messages"
profile = "anthropic"
base_url = "https://api.anthropic.com"
api_key_env = "ANTHROPIC_API_KEY"
default_model = "claude-sonnet-5"
# api_key = "sk-ant-..."           # 不想用环境变量就直接写(请保护好此文件)

# 模型 ID 含 `.` 时，表名必须加引号，例如 [providers.anthropic.models."claude-5.1-sonnet"]。
[providers.anthropic.models."claude-sonnet-5"]
context_window = 200000
max_tokens = 32000
# 省略 efforts：按 profile="anthropic" 使用标准列表
# low | medium | high | xhigh | max，默认 medium。

[providers.anthropic.models."claude-opus-5"]
context_window = 200000
max_tokens = 32000
# 模型不支持 effort 时显式写空数组；TUI 只显示 medium，请求不发送 effort。
efforts = []

# ---- OpenAI Responses API(OpenAI 当前主推)----
# profile 决定请求字段和流事件语义，不要求模型名称必须是 OpenAI 品牌；
# 接受 OpenAI Responses reasoning 字段的兼容网关也应使用 profile="openai"。
[providers.openai]
api = "responses"
profile = "openai"
base_url = "https://api.openai.com/v1"
api_key_env = "OPENAI_API_KEY"
default_model = "gpt-5"

[providers.openai.models."gpt-5"]
context_window = 400000
max_tokens = 128000
# 省略 efforts：按 profile="openai" 使用标准列表
# none | minimal | low | medium | high | xhigh | max，默认 medium。

[providers.openai.models."gpt-5-pro"]
context_window = 400000
max_tokens = 128000
# 非空数组完整覆盖 profile 标准列表；不要求包含 medium。
efforts = ["low", "high", "max"]
# 可选；省略时优先选 medium，不存在 medium 则选数组第一项。
default_effort = "high"

# ---- DeepSeek Responses API ----
[providers.deepseek]
api = "responses"
profile = "deepseek-responses"
base_url = "https://api.deepseek.com"
api_key_env = "DEEPSEEK_API_KEY"
default_model = "deepseek-v4-flash"

[providers.deepseek.models."deepseek-v4-flash"]
context_window = 131072
max_tokens = 8192
# deepseek-* profile 不定义标准 effort，也不允许配置 efforts；
# TUI 默认 medium，请求不发送 effort 字段。
"#;

#[cfg(test)]
mod tests;
