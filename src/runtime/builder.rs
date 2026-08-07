use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use crate::config::{Config, ProviderSettings};
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
use crate::tools::{default_registry, default_registry_without_skills, detect_shell, ToolRegistry};
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
}

impl AgentBuilder {
    pub fn new(config: Config, workspace: Workspace) -> Self {
        let shell = config.shell.clone();
        let system_prompt = config.system_prompt.clone();
        let max_turns = config.max_turns;
        let tool_timeout = config.tool_timeout;
        let permission_rules = config.permission_rules;
        let mut builder = Self::with_models(Box::new(config), workspace);
        builder.shell = shell;
        builder.system_prompt = system_prompt;
        builder.max_turns = max_turns;
        builder.tool_timeout = tool_timeout;
        builder.permission_rules = permission_rules;
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
            max_turns: 50,
            tool_timeout: None,
            permission_rules: PermissionRules::default(),
            data_dir: None,
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
        self.session_backend = Some(Box::new(MemorySessionBackend::new()));
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

    pub fn build(self) -> anyhow::Result<Agent> {
        if self.max_turns == 0 {
            anyhow::bail!("max_turns 必须大于 0");
        }
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
        let (skills, skill_warnings) = match self.skills {
            SkillsMode::Discover => {
                let paths = paths.as_ref().expect("default skills require app paths");
                let discovered = discover(
                    &self.workspace.root().join(".onemore").join("skills"),
                    &paths.root.join("skills"),
                );
                (Some(Arc::new(discovered.catalog)), discovered.warnings)
            }
            SkillsMode::Catalog(catalog) => (Some(catalog), Vec::new()),
            SkillsMode::Disabled => (None, Vec::new()),
        };
        let mut project_instructions_warning = None;
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
        let provider = (self.provider_factory)(settings);
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

        Ok(Agent {
            workspace: self.workspace,
            tools: self.tools.unwrap_or_else(|| match &skills {
                Some(catalog) => default_registry(shell, Arc::clone(catalog)),
                None => default_registry_without_skills(shell),
            }),
            entries: Vec::new(),
            extra_context,
            provider,
            provider_factory: self.provider_factory,
            active_selection,
            budget,
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
            deferred: Default::default(),
        })
    }
}
