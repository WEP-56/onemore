use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use crate::compaction::CompactionSettings;
use crate::config::{Config, McpServerConfig, ProviderSettings};
use crate::context::instructions::Instructions;
use crate::context::project_instructions::ProjectInstructions;
use crate::context::skills::SkillsContext;
use crate::context::workspace_info::WorkspaceInfo;
use crate::context::ContextProvider;
use crate::event::AgentEvent;
use crate::harness::{
    FixedModelRegistry, MemoryModelPreferences, MemorySessionBackend, ModelPreferences,
    ModelRegistry, SessionBackend,
};
use crate::hooks::HookRegistry;
use crate::permission::{PermissionManager, PermissionRules};
use crate::provider::{build_provider, Provider};
use crate::skills::{discover, SkillCatalog};
use crate::storage::{AppPaths, SessionManager, WorkspacePreferences};
use crate::tools::{
    default_registry_with_web, default_registry_without_skills_with_web, detect_shell, ToolRegistry,
};
use crate::workspace::Workspace;

use super::{budget_from_settings, Agent, RetryPolicy};

/// Creates a provider for one resolved model selection. The factory remains
/// attached to the agent, so later provider/model switches use the same host
/// integration instead of silently falling back to Onemore's HTTP adapters.
pub type ProviderFactory =
    Arc<dyn Fn(ProviderSettings) -> Box<dyn Provider> + Send + Sync + 'static>;

enum SkillsMode {
    Discover,
    Catalog(Arc<SkillCatalog>),
    Disabled,
}

/// Explicit composition boundary for the stateful Onemore harness.
///
/// [`Agent::new`] is equivalent to `AgentBuilder::new(...).build()`. Hosts can
/// replace individual policies without forking the agent loop; omitted
/// components retain Onemore's CLI defaults.
pub struct AgentBuilder {
    models: Box<dyn ModelRegistry>,
    workspace: Workspace,
    shell: String,
    system_prompt: Option<String>,
    max_turns: u32,
    tool_timeout: Option<Duration>,
    permission_rules: PermissionRules,
    data_dir: Option<PathBuf>,
    config_path: Option<PathBuf>,
    provider_factory: ProviderFactory,
    tools: Option<ToolRegistry>,
    context_providers: Option<Vec<Box<dyn ContextProvider>>>,
    additional_context: Vec<Box<dyn ContextProvider>>,
    session_backend: Option<Box<dyn SessionBackend>>,
    model_preferences: Option<Box<dyn ModelPreferences>>,
    skills: SkillsMode,
    permissions: Option<PermissionManager>,
    hooks: Option<HookRegistry>,
    retry_policy: RetryPolicy,
    compaction_settings: CompactionSettings,
    mcp_servers: Vec<McpServerConfig>,
}

impl AgentBuilder {
    pub fn new(config: Config, workspace: Workspace) -> Self {
        let config_path = config.source_path.clone();
        let shell = config.shell.clone();
        let system_prompt = config.system_prompt.clone();
        let max_turns = config.max_turns;
        let retry_policy = config.retry_policy;
        let tool_timeout = config.tool_timeout;
        let compaction_settings = config.compaction;
        let permission_rules = config.permission_rules;
        let mcp_servers = config.mcp_servers.clone();
        let mut builder = Self::with_models(Box::new(config), workspace);
        builder.shell = shell;
        builder.system_prompt = system_prompt;
        builder.max_turns = max_turns;
        builder.retry_policy = retry_policy;
        builder.tool_timeout = tool_timeout;
        builder.compaction_settings = compaction_settings;
        builder.permission_rules = permission_rules;
        builder.config_path = config_path;
        builder.mcp_servers = mcp_servers;
        builder
    }

    /// Build from one resolved provider/model without parsing a file Config.
    pub fn from_provider_settings(settings: ProviderSettings, workspace: Workspace) -> Self {
        Self::from_model_registry(FixedModelRegistry::new(settings), workspace)
    }

    /// Build from a host-owned dynamic model registry.
    pub fn from_model_registry<M>(models: M, workspace: Workspace) -> Self
    where
        M: ModelRegistry + 'static,
    {
        Self::with_models(Box::new(models), workspace)
    }

    fn with_models(models: Box<dyn ModelRegistry>, workspace: Workspace) -> Self {
        AgentBuilder {
            models,
            workspace,
            shell: "auto".into(),
            system_prompt: None,
            max_turns: 200,
            tool_timeout: None,
            permission_rules: PermissionRules::default(),
            data_dir: None,
            config_path: None,
            provider_factory: Arc::new(build_provider),
            tools: None,
            context_providers: None,
            additional_context: Vec::new(),
            session_backend: None,
            model_preferences: None,
            skills: SkillsMode::Discover,
            permissions: None,
            hooks: None,
            retry_policy: RetryPolicy::default(),
            compaction_settings: CompactionSettings::default(),
            mcp_servers: Vec::new(),
        }
    }

    pub fn max_turns(mut self, max_turns: u32) -> Self {
        self.max_turns = max_turns;
        self
    }

    pub fn tool_timeout(mut self, timeout: Option<Duration>) -> Self {
        self.tool_timeout = timeout;
        self
    }

    pub fn system_prompt(mut self, prompt: Option<String>) -> Self {
        self.system_prompt = prompt;
        self
    }

    /// Use this root for sessions, workspace preferences, and user skills.
    /// Primarily useful to embedders and tests that must not touch platform data.
    pub fn data_dir(mut self, data_dir: PathBuf) -> Self {
        self.data_dir = Some(data_dir);
        self
    }

    pub fn provider_factory<F>(mut self, factory: F) -> Self
    where
        F: Fn(ProviderSettings) -> Box<dyn Provider> + Send + Sync + 'static,
    {
        self.provider_factory = Arc::new(factory);
        self
    }

    /// Replace all default tools with a host-owned registry.
    pub fn tools(mut self, tools: ToolRegistry) -> Self {
        self.tools = Some(tools);
        self
    }

    /// Replace the default instructions, project instructions, skill catalog,
    /// and workspace info system sections. An empty vector produces an
    /// intentionally empty system prompt unless additional providers are added.
    pub fn context_providers(mut self, providers: Vec<Box<dyn ContextProvider>>) -> Self {
        self.context_providers = Some(providers);
        self
    }

    /// Append one system-context source after either the defaults or an exact
    /// replacement supplied through [`AgentBuilder::context_providers`].
    pub fn add_context_provider<C>(mut self, provider: C) -> Self
    where
        C: ContextProvider + 'static,
    {
        self.additional_context.push(Box::new(provider));
        self
    }

    /// Replace SQLite session persistence with a host-owned fact backend.
    pub fn session_backend<B>(mut self, backend: B) -> Self
    where
        B: SessionBackend + 'static,
    {
        self.session_backend = Some(Box::new(backend));
        self
    }

    /// Replace JSON workspace preferences with a host-owned implementation.
    pub fn model_preferences<P>(mut self, preferences: P) -> Self
    where
        P: ModelPreferences + 'static,
    {
        self.model_preferences = Some(Box::new(preferences));
        self
    }

    /// Use a host-supplied, already frozen skill catalog without scanning disk.
    pub fn skill_catalog(mut self, catalog: SkillCatalog) -> Self {
        self.skills = SkillsMode::Catalog(Arc::new(catalog));
        self
    }

    /// Remove skill discovery, its system section, and the `load_skill` tool.
    pub fn disable_skills(mut self) -> Self {
        self.skills = SkillsMode::Disabled;
        self
    }

    /// Run without Onemore state directories: facts and preferences stay in
    /// memory, and local skill discovery is disabled.
    pub fn in_memory(mut self) -> Self {
        self.session_backend = Some(Box::new(MemorySessionBackend::with_workspace(
            self.workspace.root().display().to_string(),
        )));
        self.model_preferences = Some(Box::new(MemoryModelPreferences::default()));
        self.skills = SkillsMode::Disabled;
        self
    }

    pub fn permissions(mut self, permissions: PermissionManager) -> Self {
        self.permissions = Some(permissions);
        self
    }

    pub fn hooks(mut self, hooks: HookRegistry) -> Self {
        self.hooks = Some(hooks);
        self
    }

    pub fn retry_policy(mut self, retry_policy: RetryPolicy) -> Self {
        self.retry_policy = retry_policy;
        self
    }

    /// Configure or disable automatic compaction for the stateful harness.
    pub fn compaction(mut self, settings: CompactionSettings) -> Self {
        self.compaction_settings = settings;
        self
    }

    /// stdio MCP server 列表(默认取自 `Config`)。只在使用默认工具装配时生效;
    /// host-owned registry 由宿主自行组合工具。
    pub fn mcp_servers(mut self, servers: Vec<McpServerConfig>) -> Self {
        self.mcp_servers = servers;
        self
    }

    pub fn build(self) -> anyhow::Result<Agent> {
        if self.max_turns == 0 {
            anyhow::bail!("max_turns 必须大于 0");
        }
        self.compaction_settings.validate()?;
        let needs_paths = self.session_backend.is_none()
            || self.model_preferences.is_none()
            || matches!(self.skills, SkillsMode::Discover);
        let paths = if needs_paths {
            let paths = match self.data_dir.clone() {
                Some(data_dir) => AppPaths::from_root(data_dir),
                None => AppPaths::discover()?,
            };
            paths.ensure()?;
            Some(paths)
        } else {
            None
        };

        let shell = detect_shell(&self.shell);
        let reloadable_skills = matches!(self.skills, SkillsMode::Discover);
        let skills_enabled = !matches!(self.skills, SkillsMode::Disabled);
        let (skills, skill_warnings) = match self.skills {
            SkillsMode::Discover => {
                let paths = paths.as_ref().expect("default skills require app paths");
                let discovered = discover(
                    &self.workspace.root().join(".agents").join("skills"),
                    &paths.root.join(".agents").join("skills"),
                );
                (Some(Arc::new(discovered.catalog)), discovered.warnings)
            }
            SkillsMode::Catalog(catalog) => (Some(catalog), Vec::new()),
            SkillsMode::Disabled => (None, Vec::new()),
        };
        let mut project_instructions_warning = None;
        let default_context = self.context_providers.is_none();
        let default_tools = self.tools.is_none();
        let mut extra_context = if let Some(providers) = self.context_providers {
            providers
        } else {
            let instructions = match &skills {
                Some(_) => Instructions::new(self.system_prompt.clone()),
                None => Instructions::without_skills(self.system_prompt.clone()),
            };
            let mut providers: Vec<Box<dyn ContextProvider>> = vec![Box::new(instructions)];
            match ProjectInstructions::discover(&self.workspace) {
                Ok(Some(project_instructions)) => providers.push(Box::new(project_instructions)),
                Ok(None) => {}
                Err(error) => {
                    project_instructions_warning =
                        Some(format!("未加载 workspace AGENTS.md: {error}"));
                }
            }
            if let Some(catalog) = &skills {
                providers.push(Box::new(SkillsContext::new(Arc::clone(catalog))));
            }
            providers.push(Box::new(WorkspaceInfo::new(&shell)));
            providers
        };
        extra_context.extend(self.additional_context);

        let mut active_selection = self.models.initial_selection()?;
        let model_preferences: Box<dyn ModelPreferences> = match self.model_preferences {
            Some(preferences) => preferences,
            None => {
                let paths = paths
                    .as_ref()
                    .expect("default model preferences require app paths");
                Box::new(WorkspacePreferences::load(
                    &paths.workspaces,
                    self.workspace.root(),
                )?)
            }
        };
        if let Some(saved) =
            model_preferences.effort(&active_selection.provider, &active_selection.model)
        {
            let mut preferred = active_selection.clone();
            preferred.effort = saved.to_string();
            if self.models.validate_selection(&preferred).is_ok() {
                active_selection = preferred;
            }
        }

        let settings = self.models.resolve_selection(&active_selection)?;
        let budget = budget_from_settings(&settings);
        let web_label = settings.web.label();
        let tools = match self.tools {
            Some(tools) => {
                if matches!(
                    &settings.web,
                    crate::web::WebCapabilityBinding::HarnessFunction { .. }
                ) && !tools.contains("web_search")
                {
                    anyhow::bail!(
                        "a HarnessFunction Web binding requires web_search in a host-owned tool registry"
                    );
                }
                tools
            }
            None => match &skills {
                Some(catalog) => {
                    default_registry_with_web(shell.clone(), Arc::clone(catalog), &settings.web)
                }
                None => default_registry_without_skills_with_web(shell.clone(), &settings.web),
            }
            .map_err(anyhow::Error::msg)?,
        };
        let provider = (self.provider_factory)(settings);
        let mut tools = tools;
        let mut mcp = crate::mcp::McpHost::empty();
        let mut mcp_notices: Vec<String> = Vec::new();
        if !self.mcp_servers.is_empty() {
            if default_tools {
                let outcome =
                    crate::mcp::McpHost::start(&self.mcp_servers, &|name| tools.contains(name));
                let artifacts_dir = paths.as_ref().map(|paths| paths.root.join("mcp-artifacts"));
                let proxies: Vec<Box<dyn crate::tools::Tool>> = outcome
                    .seeds
                    .into_iter()
                    .map(|seed| {
                        Box::new(crate::tools::mcp_proxy::McpTool::from_seed(
                            seed,
                            artifacts_dir.clone(),
                        )) as Box<dyn crate::tools::Tool>
                    })
                    .collect();
                tools.add_tools(proxies);
                mcp = outcome.host;
                mcp_notices = outcome.notices;
            } else {
                mcp_notices.push("host-owned tool registry:已跳过 [[mcp_servers]] 装配".into());
            }
        }
        let sessions: Box<dyn SessionBackend> = match self.session_backend {
            Some(backend) => backend,
            None => {
                let paths = paths
                    .as_ref()
                    .expect("default session backend requires app paths");
                Box::new(SessionManager::create(
                    paths.sessions.clone(),
                    self.workspace.root(),
                )?)
            }
        };
        let permissions = self
            .permissions
            .unwrap_or_else(|| PermissionManager::new(self.permission_rules));
        let mut startup_events = std::collections::VecDeque::new();
        if let Some(warning) = project_instructions_warning {
            startup_events.push_back(AgentEvent::Notice(warning));
        }
        if let Some(catalog) = &skills {
            startup_events.push_back(AgentEvent::SkillsDiscovered {
                skills: catalog.ordered.clone(),
                warnings: skill_warnings,
            });
        }
        startup_events.push_back(AgentEvent::Notice(format!("Web capability: {}", web_label)));
        for notice in mcp_notices {
            startup_events.push_back(AgentEvent::Notice(notice));
        }

        Ok(Agent {
            workspace: self.workspace,
            tools,
            mcp,
            entries: Vec::new(),
            extra_context,
            provider,
            provider_factory: self.provider_factory,
            active_selection,
            budget,
            compaction_settings: self.compaction_settings,
            retry_policy: self.retry_policy,
            models: self.models,
            max_turns: self.max_turns,
            tool_timeout: self.tool_timeout,
            usage_total: Default::default(),
            sessions,
            model_preferences,
            permissions,
            hooks: self.hooks.unwrap_or_default(),
            startup_events,
            approval_rx: None,
            config_path: self.config_path,
            data_root: paths.as_ref().map(|paths| paths.root.clone()),
            default_context,
            default_tools,
            reloadable_skills: reloadable_skills && skills_enabled,
            deferred: Default::default(),
        })
    }
}
