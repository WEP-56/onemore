//! Stateful command handling and model/context selection.

use std::sync::atomic::AtomicBool;
use std::sync::mpsc::Receiver;

use crate::config::{ActiveModelSelection, Config, ProviderSettings};
use crate::event::{AgentCommand, AgentEvent};
use crate::message::Usage;
use crate::session::{ModelChangeRecord, SessionEntryPayload};
use crate::workspace::Workspace;

use super::{budget_from_settings, Agent, AgentBuilder};

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
        self.handle_command_with_inbox(cmd, emit, cancel, None)
    }

    /// `inbox` 是命令通道的接收端:活动运行会在检查点(完整工具批之后、
    /// 任务将停止时)排干它,把新输入分类为 steering / follow-up,
    /// 把其余命令延迟到本轮结束(见 [`Agent::take_deferred`])。
    pub fn handle_command_with_inbox(
        &mut self,
        cmd: AgentCommand,
        emit: &mut dyn FnMut(AgentEvent),
        cancel: &AtomicBool,
        inbox: Option<&Receiver<AgentCommand>>,
    ) -> bool {
        self.emit_startup_events(emit);
        match cmd {
            // 空闲时三者等价:都开启一个新的运行。
            AgentCommand::UserInput(text)
            | AgentCommand::Steer(text)
            | AgentCommand::FollowUp(text) => {
                self.run_turn(text, emit, cancel, inbox);
                true
            }
            AgentCommand::Compact => {
                self.compact(emit, cancel);
                true
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
                true
            }
            AgentCommand::SwitchProvider(name) => {
                match self.preferred_default_selection(&name) {
                    Ok(selection) => self.apply_model_selection(selection, emit),
                    Err(error) => emit(AgentEvent::Error(format!("切换失败: {:#}", error))),
                }
                true
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
                true
            }
            AgentCommand::SetReasoningEffort(effort) => {
                let mut selection = self.active_selection.clone();
                selection.effort = effort;
                self.apply_model_selection(selection, emit);
                true
            }
            AgentCommand::ListSessions => {
                match self.sessions.list() {
                    Ok(sessions) => emit(AgentEvent::SessionsListed {
                        current_id: self.sessions.current_id().to_string(),
                        sessions,
                    }),
                    Err(e) => emit(AgentEvent::Error(format!("读取会话列表失败: {:#}", e))),
                }
                true
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
                true
            }
            AgentCommand::Shutdown => false,
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
