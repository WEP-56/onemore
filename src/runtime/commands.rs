//! Stateful command handling and model/context selection.

use std::sync::atomic::{AtomicBool, Ordering};

use crate::config::{ActiveModelSelection, Config, ProviderSettings};
use crate::context::instructions::Instructions;
use crate::context::project_instructions::ProjectInstructions;
use crate::context::skills::SkillsContext;
use crate::context::workspace_info::WorkspaceInfo;
use crate::context::ContextProvider;
use crate::event::{AgentCommand, AgentEvent};
use crate::message::Usage;
use crate::permission::PermissionManager;
use crate::session::{ModelChangeRecord, SessionEntryPayload};
use crate::workspace::Workspace;

use super::agent_loop::RunReport;
use super::inbox::CommandInbox;
use super::{budget_from_settings, Agent, AgentBuilder};
use crate::skills::discover;
use crate::storage::AppPaths;
use crate::tools::{
    default_registry_with_web, default_registry_without_skills_with_web, detect_shell,
    web_search_from_binding,
};

pub(super) struct HandleReport {
    pub keep_running: bool,
    pub run: Option<RunReport>,
}

impl Agent {
    pub fn builder(config: Config, workspace: Workspace) -> AgentBuilder {
        AgentBuilder::new(config, workspace)
    }

    /// Construct directly from one resolved provider/model without a file Config.
    pub fn builder_from_provider(settings: ProviderSettings, workspace: Workspace) -> AgentBuilder {
        AgentBuilder::from_provider_settings(settings, workspace)
    }

    pub fn new(config: Config, workspace: Workspace) -> anyhow::Result<Agent> {
        Self::builder(config, workspace).build()
    }

    /// 显式指定数据目录，供测试与嵌入场景隔离平台数据目录。
    pub fn new_with_data_dir(
        config: Config,
        workspace: Workspace,
        data_dir: std::path::PathBuf,
    ) -> anyhow::Result<Agent> {
        Self::builder(config, workspace).data_dir(data_dir).build()
    }

    pub fn provider_label(&self) -> String {
        self.provider.label()
    }

    pub fn active_selection(&self) -> &ActiveModelSelection {
        &self.active_selection
    }

    pub fn session_id(&self) -> &str {
        self.sessions.current_id()
    }

    /// 处理一条命令;返回 false 表示 Runtime 应当退出。
    /// 无 inbox 版本供 --once 与测试使用:活动运行中没有可注入的输入。
    pub fn handle_command(
        &mut self,
        cmd: AgentCommand,
        emit: &mut dyn FnMut(AgentEvent),
        cancel: &AtomicBool,
    ) -> bool {
        self.handle_command_report(cmd, emit, cancel, None)
            .keep_running
    }

    /// `inbox` 是命令通道的接收端:活动运行会在检查点(完整工具批之后、
    /// 任务将停止时)排干它,把新输入分类为 steering / follow-up,
    /// 把其余命令延迟到本轮结束(见 [`Agent::take_deferred`])。
    #[cfg(test)]
    pub(crate) fn handle_command_with_inbox(
        &mut self,
        cmd: AgentCommand,
        emit: &mut dyn FnMut(AgentEvent),
        cancel: &AtomicBool,
        inbox: Option<&dyn CommandInbox>,
    ) -> bool {
        self.handle_command_report(cmd, emit, cancel, inbox)
            .keep_running
    }

    pub(super) fn handle_command_report(
        &mut self,
        cmd: AgentCommand,
        emit: &mut dyn FnMut(AgentEvent),
        cancel: &AtomicBool,
        inbox: Option<&dyn CommandInbox>,
    ) -> HandleReport {
        self.emit_startup_events(emit);
        match cmd {
            // 空闲时三者等价:都开启一个新的运行。
            AgentCommand::UserInput(text)
            | AgentCommand::Steer(text)
            | AgentCommand::FollowUp(text) => {
                let run = self.run_turn(text, emit, cancel, inbox);
                HandleReport {
                    keep_running: !run.shutdown_requested,
                    run: Some(run),
                }
            }
            AgentCommand::Abort => {
                cancel.store(true, Ordering::Relaxed);
                HandleReport {
                    keep_running: true,
                    run: None,
                }
            }
            AgentCommand::Compact => {
                self.compact(emit, cancel);
                HandleReport {
                    keep_running: true,
                    run: None,
                }
            }
            AgentCommand::Reload => {
                self.reload(emit);
                HandleReport {
                    keep_running: true,
                    run: None,
                }
            }
            AgentCommand::ClearConversation => {
                match self.sessions.clear() {
                    Ok(()) => {
                        self.entries.clear();
                        self.usage_total = Usage::default();
                        self.permissions.clear_session_grants();
                        emit(AgentEvent::ConversationCleared);
                    }
                    Err(e) => emit(AgentEvent::Error(format!("清空会话数据库失败: {:#}", e))),
                }
                HandleReport {
                    keep_running: true,
                    run: None,
                }
            }
            AgentCommand::SwitchProvider(name) => {
                match self.preferred_default_selection(&name) {
                    Ok(selection) => self.apply_model_selection(selection, emit),
                    Err(error) => emit(AgentEvent::Error(format!("切换失败: {:#}", error))),
                }
                HandleReport {
                    keep_running: true,
                    run: None,
                }
            }
            AgentCommand::SelectModel { model, effort } => {
                self.apply_model_selection(
                    ActiveModelSelection {
                        provider: self.active_selection.provider.clone(),
                        model,
                        effort,
                    },
                    emit,
                );
                HandleReport {
                    keep_running: true,
                    run: None,
                }
            }
            AgentCommand::SetModelSelection {
                provider,
                model,
                effort,
            } => {
                self.apply_model_selection(
                    ActiveModelSelection {
                        provider,
                        model,
                        effort,
                    },
                    emit,
                );
                HandleReport {
                    keep_running: true,
                    run: None,
                }
            }
            AgentCommand::SetReasoningEffort(effort) => {
                let mut selection = self.active_selection.clone();
                selection.effort = effort;
                self.apply_model_selection(selection, emit);
                HandleReport {
                    keep_running: true,
                    run: None,
                }
            }
            AgentCommand::ListSessions { all } => {
                match self
                    .sessions
                    .list(crate::session::SessionListScope::from(all))
                {
                    Ok(listing) => {
                        for warning in listing.warnings {
                            emit(AgentEvent::Notice(warning));
                        }
                        emit(AgentEvent::SessionsListed {
                            current_id: self.sessions.current_id().to_string(),
                            sessions: listing.sessions,
                        })
                    }
                    Err(e) => emit(AgentEvent::Error(format!("读取会话列表失败: {:#}", e))),
                }
                HandleReport {
                    keep_running: true,
                    run: None,
                }
            }
            AgentCommand::LoadSession(id) => {
                match self.sessions.load(&id) {
                    Ok((entries, usage)) => {
                        self.permissions.clear_session_grants();
                        self.entries = entries.clone();
                        self.usage_total = usage;
                        emit(AgentEvent::SessionLoaded {
                            id: self.sessions.current_id().to_string(),
                            entries,
                            input_tokens: usage.input_tokens,
                            output_tokens: usage.output_tokens,
                            cache: usage.cache,
                        });
                    }
                    Err(e) => emit(AgentEvent::Error(format!("恢复会话失败: {:#}", e))),
                }
                HandleReport {
                    keep_running: true,
                    run: None,
                }
            }
            AgentCommand::Shutdown => HandleReport {
                keep_running: false,
                run: None,
            },
        }
    }

    pub(super) fn emit_startup_events(&mut self, emit: &mut dyn FnMut(AgentEvent)) {
        while let Some(event) = self.startup_events.pop_front() {
            emit(event);
        }
    }

    fn preferred_default_selection(&self, provider: &str) -> anyhow::Result<ActiveModelSelection> {
        let mut selection = self.models.default_selection(provider)?;
        if let Some(saved) = self
            .model_preferences
            .effort(&selection.provider, &selection.model)
        {
            let mut preferred = selection.clone();
            preferred.effort = saved.to_string();
            if self.models.validate_selection(&preferred).is_ok() {
                selection = preferred;
            }
        }
        Ok(selection)
    }

    /// Reload file-backed configuration and the default context/tool assembly while preserving
    /// the session backend, facts, model preferences, and conversation history.
    fn reload(&mut self, emit: &mut dyn FnMut(AgentEvent)) {
        let Some(config_path) = self.config_path.clone() else {
            emit(AgentEvent::Error(
                "当前 Agent 没有可重载的配置文件来源".into(),
            ));
            return;
        };
        let config = match Config::load(&config_path) {
            Ok(config) => config,
            Err(error) => {
                emit(AgentEvent::Error(format!("reload 配置失败: {error:#}")));
                return;
            }
        };
        let shell = detect_shell(&config.shell);
        let config_path = config.source_path.clone();
        let system_prompt = config.system_prompt.clone();
        let max_turns = config.max_turns;
        let retry_policy = config.retry_policy;
        let tool_timeout = config.tool_timeout;
        let compaction_settings = config.compaction;
        let permission_rules = config.permission_rules;

        let paths = match self.data_root.clone() {
            Some(root) => AppPaths::from_root(root),
            None => match AppPaths::discover() {
                Ok(paths) => paths,
                Err(error) => {
                    emit(AgentEvent::Error(format!(
                        "reload 定位数据目录失败: {error:#}"
                    )));
                    return;
                }
            },
        };
        let (skills, skill_warnings) = if self.reloadable_skills {
            let discovered = discover(
                &self.workspace.root().join(".agents").join("skills"),
                &paths.root.join(".agents").join("skills"),
            );
            (
                Some(std::sync::Arc::new(discovered.catalog)),
                discovered.warnings,
            )
        } else {
            (None, Vec::new())
        };

        let mut extra_context = None;
        if self.default_context {
            let mut providers: Vec<Box<dyn ContextProvider>> =
                vec![Box::new(if self.reloadable_skills {
                    Instructions::new(system_prompt)
                } else {
                    Instructions::without_skills(system_prompt)
                })];
            match ProjectInstructions::discover(&self.workspace) {
                Ok(Some(project)) => providers.push(Box::new(project)),
                Ok(None) => {}
                Err(error) => emit(AgentEvent::Notice(format!(
                    "reload 未加载 workspace AGENTS.md: {error}"
                ))),
            }
            if let Some(catalog) = &skills {
                providers.push(Box::new(SkillsContext::new(std::sync::Arc::clone(catalog))));
            }
            providers.push(Box::new(WorkspaceInfo::new(&shell)));
            extra_context = Some(providers);
        }

        let models: Box<dyn crate::harness::ModelRegistry> = Box::new(config);
        let mut selection = match models.initial_selection() {
            Ok(selection) => selection,
            Err(error) => {
                emit(AgentEvent::Error(format!("reload 选择模型失败: {error:#}")));
                return;
            }
        };
        if let Some(saved) = self
            .model_preferences
            .effort(&selection.provider, &selection.model)
        {
            let mut preferred = selection.clone();
            preferred.effort = saved.to_string();
            if models.validate_selection(&preferred).is_ok() {
                selection = preferred;
            }
        }
        let settings = match models.resolve_selection(&selection) {
            Ok(settings) => settings,
            Err(error) => {
                emit(AgentEvent::Error(format!(
                    "reload 解析 provider 失败: {error:#}"
                )));
                return;
            }
        };
        let budget = budget_from_settings(&settings);
        let web_label = settings.web.label();
        let reloaded_tools = if self.default_tools {
            let registry = match &skills {
                Some(catalog) => default_registry_with_web(
                    shell.clone(),
                    std::sync::Arc::clone(catalog),
                    &settings.web,
                ),
                None => default_registry_without_skills_with_web(shell.clone(), &settings.web),
            };
            match registry {
                Ok(registry) => Some(registry),
                Err(error) => {
                    emit(AgentEvent::Error(format!(
                        "reload 构造 Web 工具失败: {error}"
                    )));
                    return;
                }
            }
        } else {
            if matches!(
                &settings.web,
                crate::web::WebCapabilityBinding::HarnessFunction { .. }
            ) && !self.tools.contains("web_search")
            {
                emit(AgentEvent::Error(
                    "reload 后的外部 Web binding 需要 host-owned registry 提供 web_search".into(),
                ));
                return;
            }
            None
        };
        let provider = (self.provider_factory)(settings);
        if let Err(error) = compaction_settings.validate() {
            emit(AgentEvent::Error(format!("reload 压缩配置失败: {error:#}")));
            return;
        }

        if let Some(context) = extra_context {
            self.extra_context = context;
        }
        if let Some(tools) = reloaded_tools {
            self.tools = tools;
        }
        self.models = models;
        self.provider = provider;
        self.active_selection = selection;
        self.budget = budget;
        self.max_turns = max_turns;
        self.retry_policy = retry_policy;
        self.tool_timeout = tool_timeout;
        self.compaction_settings = compaction_settings;
        self.permissions = PermissionManager::new(permission_rules);
        self.config_path = config_path;
        self.data_root = Some(paths.root);
        if let Some(catalog) = skills {
            emit(AgentEvent::SkillsDiscovered {
                skills: catalog.ordered.clone(),
                warnings: skill_warnings,
            });
        }
        emit(AgentEvent::Notice(
            "reload 成功，已重建配置、context providers、工具声明和 skill catalog".into(),
        ));
        emit(AgentEvent::Notice(format!("Web capability: {web_label}")));
    }

    fn apply_model_selection(
        &mut self,
        selection: ActiveModelSelection,
        emit: &mut dyn FnMut(AgentEvent),
    ) {
        let settings = match self.models.resolve_selection(&selection) {
            Ok(settings) => settings,
            Err(error) => {
                emit(AgentEvent::Error(format!("切换失败: {:#}", error)));
                return;
            }
        };
        let next_budget = budget_from_settings(&settings);
        let web_label = settings.web.label();
        let next_web_search = if self.default_tools {
            match web_search_from_binding(&settings.web) {
                Ok(tool) => tool,
                Err(error) => {
                    emit(AgentEvent::Error(format!(
                        "切换失败，无法构造 Web 工具: {error}"
                    )));
                    return;
                }
            }
        } else {
            if matches!(
                &settings.web,
                crate::web::WebCapabilityBinding::HarnessFunction { .. }
            ) && !self.tools.contains("web_search")
            {
                emit(AgentEvent::Error(
                    "切换失败：外部 Web binding 需要 host-owned registry 提供 web_search".into(),
                ));
                return;
            }
            None
        };
        let next_provider = (self.provider_factory)(settings);
        let default_effort = self
            .models
            .model_default_effort(&selection.provider, &selection.model)
            .expect("resolved selection must have a normalized default effort");
        let previous_effort = self
            .model_preferences
            .effort(&selection.provider, &selection.model)
            .map(str::to_string);
        if let Err(error) = self.model_preferences.set_effort(
            &selection.provider,
            &selection.model,
            &selection.effort,
            &default_effort,
        ) {
            emit(AgentEvent::Error(format!(
                "保存 workspace 思考程度失败,未切换模型: {:#}",
                error
            )));
            return;
        }
        if !self.record_model_change(&selection, emit) {
            let restore = previous_effort.as_deref().unwrap_or(&default_effort);
            if let Err(error) = self.model_preferences.set_effort(
                &selection.provider,
                &selection.model,
                restore,
                &default_effort,
            ) {
                emit(AgentEvent::Error(format!(
                    "回滚 workspace 思考程度失败: {:#}",
                    error
                )));
            }
            return;
        }
        self.provider = next_provider;
        if self.default_tools {
            self.tools.replace_web_search(next_web_search);
        }
        self.budget = next_budget;
        self.active_selection = selection.clone();
        let label = self.provider.label();
        emit(AgentEvent::ModelSelectionChanged {
            provider: selection.provider,
            model: selection.model,
            effort: selection.effort,
            label: label.clone(),
        });
        emit(AgentEvent::Notice(format!("已切换到 {}(历史保留)", label)));
        emit(AgentEvent::Notice(format!("Web capability: {web_label}")));
    }

    /// provider/model/effort 变化是会话事实:恢复会话时可以据此解释历史。
    fn record_model_change(
        &mut self,
        selection: &ActiveModelSelection,
        emit: &mut dyn FnMut(AgentEvent),
    ) -> bool {
        let payload = SessionEntryPayload::ModelChange(ModelChangeRecord {
            provider: selection.provider.clone(),
            model: selection.model.clone(),
            effort: selection.effort.clone(),
        });
        self.commit(vec![payload], emit)
    }

    /// 把一批事实原子落库;成功则推进内存镜像并返回 true。
    /// 失败时内存镜像不动(与磁盘保持一致),调用方必须终止当前活动。
    pub(super) fn commit(
        &mut self,
        payloads: Vec<SessionEntryPayload>,
        emit: &mut dyn FnMut(AgentEvent),
    ) -> bool {
        match self.sessions.append_payloads(payloads, self.usage_total) {
            Ok(mut appended) => {
                self.entries.append(&mut appended);
                true
            }
            Err(e) => {
                emit(AgentEvent::Error(format!(
                    "保存会话失败,本批事实未写入,已停止本轮以避免内存与磁盘历史分叉: {:#}",
                    e
                )));
                false
            }
        }
    }

    #[cfg(test)]
    pub(super) fn build_system_prompt(&self) -> crate::context::PromptContext {
        let mut prompt = crate::context::PromptContext::default();
        for context in &self.extra_context {
            context.contribute(&mut prompt, &self.workspace);
        }
        prompt
    }
}
