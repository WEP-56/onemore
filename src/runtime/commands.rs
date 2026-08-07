//! Stateful command handling and model/context selection.

use std::sync::atomic::AtomicBool;
use std::sync::mpsc::Receiver;

use crate::config::{ActiveModelSelection, Config};
use crate::context::budget::{apply_budget, BudgetDecision};
use crate::context::PromptContext;
use crate::event::{AgentCommand, AgentEvent};
use crate::message::{ChatMessage, Usage};
use crate::session::{project_model_messages, ModelChangeRecord, SessionEntryPayload};
use crate::tools::ToolSpec;
use crate::workspace::Workspace;

use super::{budget_from_settings, Agent, AgentBuilder};

impl Agent {
    pub fn builder(config: Config, workspace: Workspace) -> AgentBuilder {
        AgentBuilder::new(config, workspace)
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
        self.emit_skill_discovery(emit);
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

    pub(super) fn emit_skill_discovery(&mut self, emit: &mut dyn FnMut(AgentEvent)) {
        if self.skills_announced {
            return;
        }
        self.skills_announced = true;
        emit(AgentEvent::SkillsDiscovered {
            skills: self.skills.ordered.clone(),
            warnings: std::mem::take(&mut self.skill_warnings),
        });
    }

    fn preferred_default_selection(&self, provider: &str) -> anyhow::Result<ActiveModelSelection> {
        let mut selection = self.config.default_selection(provider)?;
        if let Some(saved) = self
            .workspace_preferences
            .effort(&selection.provider, &selection.model)
        {
            let mut preferred = selection.clone();
            preferred.effort = saved.to_string();
            if self.config.validate_selection(&preferred).is_ok() {
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
        let settings = match self.config.resolve_selection(&selection) {
            Ok(settings) => settings,
            Err(error) => {
                emit(AgentEvent::Error(format!("切换失败: {:#}", error)));
                return;
            }
        };
        let next_budget = budget_from_settings(&settings);
        let next_provider = (self.provider_factory)(settings);
        let default_effort = self
            .config
            .model_default_effort(&selection.provider, &selection.model)
            .expect("resolved selection must have a normalized default effort")
            .to_string();
        let previous_effort = self
            .workspace_preferences
            .effort(&selection.provider, &selection.model)
            .map(str::to_string);
        if let Err(error) = self.workspace_preferences.set_effort(
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
            if let Err(error) = self.workspace_preferences.set_effort(
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

    /// 组装本轮 prompt 的 system 部分(messages 由事实投影 + 预算决定)。
    pub(super) fn build_system_prompt(&self) -> PromptContext {
        let mut prompt = PromptContext::default();
        for c in &self.extra_context {
            c.contribute(&mut prompt, &self.workspace);
        }
        prompt
    }

    /// 投影 + 预算:决定本轮真正发给模型的消息。
    /// 返回 None 表示超出预算被拒绝(事件已发出),调用方应结束本轮。
    pub(super) fn project_for_model(
        &self,
        prompt: &PromptContext,
        specs: &[ToolSpec],
        emit: &mut dyn FnMut(AgentEvent),
    ) -> Option<Vec<ChatMessage>> {
        let projection = project_model_messages(&self.entries);
        for diagnostic in &projection.diagnostics {
            // 防御性修复只该发生在旧库/损坏数据上;必须让用户看见,而不是静默掩盖。
            emit(AgentEvent::Notice(format!("历史投影修复: {}", diagnostic)));
        }
        let system_chars = prompt.system_text().chars().count() as u64;
        let tools_chars = tool_spec_chars(specs);
        match apply_budget(&self.budget, system_chars, tools_chars, projection) {
            BudgetDecision::Send {
                messages, notices, ..
            } => {
                for notice in notices {
                    emit(AgentEvent::Notice(notice));
                }
                Some(messages)
            }
            BudgetDecision::Refuse {
                estimated_tokens,
                available_tokens,
            } => {
                emit(AgentEvent::Error(format!(
                    "上下文估算约 {} tokens,超出可用预算 {}(窗口扣除输出预留)。\
                     未发送请求;请用 /compact 压缩历史,或 /clear 重新开始。",
                    estimated_tokens, available_tokens
                )));
                None
            }
        }
    }
}

/// 工具声明进入请求体的近似字符成本(name + description + schema JSON)。
fn tool_spec_chars(specs: &[ToolSpec]) -> u64 {
    specs
        .iter()
        .map(|spec| {
            (spec.name.chars().count()
                + spec.description.chars().count()
                + spec.schema.to_string().chars().count()
                + 32) as u64
        })
        .sum()
}
