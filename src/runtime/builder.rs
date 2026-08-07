use std::path::PathBuf;
use std::sync::Arc;

use crate::config::{Config, ProviderSettings};
use crate::context::instructions::Instructions;
use crate::context::skills::SkillsContext;
use crate::context::workspace_info::WorkspaceInfo;
use crate::context::ContextProvider;
use crate::hooks::HookRegistry;
use crate::permission::PermissionManager;
use crate::provider::{build_provider, Provider};
use crate::skills::discover;
use crate::storage::{AppPaths, SessionManager, WorkspacePreferences};
use crate::tools::{default_registry, detect_shell, ToolRegistry};
use crate::workspace::Workspace;

use super::{budget_from_settings, Agent, RetryPolicy};

/// Creates a provider for one resolved model selection. The factory remains
/// attached to the agent, so later provider/model switches use the same host
/// integration instead of silently falling back to Onemore's HTTP adapters.
pub type ProviderFactory =
    Arc<dyn Fn(ProviderSettings) -> Box<dyn Provider> + Send + Sync + 'static>;

/// Explicit composition boundary for the stateful Onemore harness.
///
/// [`Agent::new`] is equivalent to `AgentBuilder::new(...).build()`. Hosts can
/// replace individual policies without forking the agent loop; omitted
/// components retain Onemore's CLI defaults.
pub struct AgentBuilder {
    config: Config,
    workspace: Workspace,
    data_dir: Option<PathBuf>,
    provider_factory: ProviderFactory,
    tools: Option<ToolRegistry>,
    context_providers: Option<Vec<Box<dyn ContextProvider>>>,
    additional_context: Vec<Box<dyn ContextProvider>>,
    permissions: Option<PermissionManager>,
    hooks: Option<HookRegistry>,
    retry_policy: RetryPolicy,
}

impl AgentBuilder {
    pub fn new(config: Config, workspace: Workspace) -> Self {
        AgentBuilder {
            config,
            workspace,
            data_dir: None,
            provider_factory: Arc::new(build_provider),
            tools: None,
            context_providers: None,
            additional_context: Vec::new(),
            permissions: None,
            hooks: None,
            retry_policy: RetryPolicy::default(),
        }
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

    /// Replace the default instructions, workspace info, and skill catalog
    /// system sections. An empty vector produces an intentionally empty system
    /// prompt unless additional providers are added.
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
        let paths = match self.data_dir {
            Some(data_dir) => AppPaths::from_root(data_dir),
            None => AppPaths::discover()?,
        };
        paths.ensure()?;

        let shell = detect_shell(&self.config.shell);
        let discovered = discover(
            &self.workspace.root().join(".onemore").join("skills"),
            &paths.root.join("skills"),
        );
        let skills = Arc::new(discovered.catalog);
        let mut extra_context = self.context_providers.unwrap_or_else(|| {
            vec![
                Box::new(Instructions::new(self.config.system_prompt.clone()))
                    as Box<dyn ContextProvider>,
                Box::new(WorkspaceInfo::new(&shell)),
                Box::new(SkillsContext::new(skills.clone())),
            ]
        });
        extra_context.extend(self.additional_context);

        let mut active_selection = self
            .config
            .default_selection(&self.config.active_provider)?;
        let workspace_preferences =
            WorkspacePreferences::load(&paths.workspaces, self.workspace.root())?;
        if let Some(saved) =
            workspace_preferences.effort(&active_selection.provider, &active_selection.model)
        {
            let mut preferred = active_selection.clone();
            preferred.effort = saved.to_string();
            if self.config.validate_selection(&preferred).is_ok() {
                active_selection = preferred;
            }
        }

        let settings = self.config.resolve_selection(&active_selection)?;
        let budget = budget_from_settings(&settings);
        let provider = (self.provider_factory)(settings);
        let sessions = SessionManager::create(paths.sessions, self.workspace.root())?;
        let permissions = self
            .permissions
            .unwrap_or_else(|| PermissionManager::new(self.config.permission_rules));

        Ok(Agent {
            workspace: self.workspace,
            tools: self
                .tools
                .unwrap_or_else(|| default_registry(shell, skills.clone())),
            entries: Vec::new(),
            extra_context,
            provider,
            provider_factory: self.provider_factory,
            active_selection,
            budget,
            retry_policy: self.retry_policy,
            config: self.config,
            usage_total: Default::default(),
            sessions,
            workspace_preferences,
            permissions,
            hooks: self.hooks.unwrap_or_default(),
            skills,
            skill_warnings: discovered.warnings,
            skills_announced: false,
            approval_rx: None,
            deferred: Default::default(),
        })
    }
}
