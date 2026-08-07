//! Stateful Onemore adapter for the provider-neutral agent loop.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, TryRecvError};

use crate::agent_loop::{
    run_agent_loop, AgentLoopCallbacks, AgentLoopHost, ToolCall, ToolTurnResult,
};
use crate::context::budget::{apply_budget, BudgetDecision, ContextBudget};
use crate::context::{ContextProvider, PromptContext};
use crate::event::{AgentCommand, AgentEvent};
use crate::harness::SessionBackend;
use crate::hooks::HookRegistry;
use crate::message::{Block, ChatMessage, Role, StopReason, Usage};
use crate::permission::{ApprovalResponse, PermissionManager};
use crate::plan::{reduce_plan, return_in_progress_to_pending, PlanSnapshot};
use crate::provider::TurnOutput;
use crate::session::{
    project_model_messages, ModelProjection, NoticeLevel, NoticeRecord, PlanReminderReason,
    PlanReminderRecord, SessionEntry, SessionEntryPayload,
};
use crate::tools::{ToolEffect, ToolError, ToolErrorCode, ToolOutcome, ToolSpec};
use crate::util;
use crate::workspace::Workspace;

use super::tool_execution::{BatchItemState, DefaultToolExecutor};
use super::Agent;

impl Agent {
    /// Start one stateful run, then delegate every model turn to the public core loop.
    pub(super) fn run_turn(
        &mut self,
        input: String,
        emit: &mut dyn FnMut(AgentEvent),
        cancel: &AtomicBool,
        inbox: Option<&Receiver<AgentCommand>>,
    ) {
        emit(AgentEvent::UserMessage(input.clone()));
        let prompt_hooks = self
            .hooks
            .run_user_prompt(&input, self.sessions.current_id());
        emit_hook_warnings(prompt_hooks.warnings, emit);
        let mut submitted = vec![SessionEntryPayload::message(
            ChatMessage::user_text(input),
            None,
        )];
        for message in prompt_hooks.added_context {
            submitted.push(SessionEntryPayload::message(message, None));
        }
        if !self.commit(submitted, emit) {
            emit(AgentEvent::TurnFinished { cancelled: false });
            return;
        }
        emit(AgentEvent::TurnStarted);
        if let Some(reason) = prompt_hooks.block {
            emit(AgentEvent::Error(reason));
            emit(AgentEvent::TurnFinished { cancelled: false });
            return;
        }

        let mut specs = self.tools.specs();
        specs.sort_by(|left, right| left.name.cmp(&right.name));
        let projection = project_model_messages(&self.entries);
        let messages = projection.messages.clone();
        let approval_rx = self.approval_rx.as_ref();
        let mut host = StatefulLoopHost {
            workspace: &self.workspace,
            tools: &self.tools,
            entries: &mut self.entries,
            extra_context: &self.extra_context,
            budget: self.budget,
            usage_total: &mut self.usage_total,
            sessions: self.sessions.as_mut(),
            permissions: &mut self.permissions,
            hooks: &mut self.hooks,
            approval_rx,
            deferred: &mut self.deferred,
            inbox,
            projection,
            queues: RunQueues::default(),
            tool_timeout: self.tool_timeout,
            stop_hook_active: false,
            plan_reminder_sent: false,
        };
        let callbacks = AgentLoopCallbacks::new(&mut host, emit, cancel)
            .max_turns(self.max_turns)
            .retry_policy(self.retry_policy);
        run_agent_loop(self.provider.as_ref(), messages, &specs, callbacks);
    }

    /// 供宿主循环在一次命令处理后取走延迟命令逐条执行。
    pub fn take_deferred(&mut self) -> Option<AgentCommand> {
        self.deferred.pop_front()
    }
}

struct StatefulLoopHost<'a> {
    workspace: &'a Workspace,
    tools: &'a crate::tools::ToolRegistry,
    entries: &'a mut Vec<SessionEntry>,
    extra_context: &'a [Box<dyn ContextProvider>],
    budget: ContextBudget,
    usage_total: &'a mut Usage,
    sessions: &'a mut dyn SessionBackend,
    permissions: &'a mut PermissionManager,
    hooks: &'a mut HookRegistry,
    approval_rx: Option<&'a Receiver<ApprovalResponse>>,
    deferred: &'a mut VecDeque<AgentCommand>,
    inbox: Option<&'a Receiver<AgentCommand>>,
    projection: ModelProjection,
    queues: RunQueues,
    tool_timeout: Option<std::time::Duration>,
    stop_hook_active: bool,
    plan_reminder_sent: bool,
}

impl StatefulLoopHost<'_> {
    fn commit(&mut self, payloads: Vec<SessionEntryPayload>) -> anyhow::Result<Vec<ChatMessage>> {
        let mut appended = self
            .sessions
            .append_payloads(payloads, *self.usage_total)
            .map_err(|error| {
                anyhow::anyhow!(
                    "保存会话失败,本批事实未写入,已停止本轮以避免内存与磁盘历史分叉: {error:#}"
                )
            })?;
        self.entries.append(&mut appended);
        self.projection = project_model_messages(self.entries);
        Ok(self.projection.messages.clone())
    }

    fn assistant_payload(turn: &TurnOutput) -> SessionEntryPayload {
        SessionEntryPayload::message_with_prompt(
            turn.message.clone(),
            turn.usage,
            turn.prompt_fingerprint.clone(),
        )
    }

    fn commit_or_emit(
        &mut self,
        payloads: Vec<SessionEntryPayload>,
        emit: &mut dyn FnMut(AgentEvent),
    ) -> bool {
        match self.commit(payloads) {
            Ok(_) => true,
            Err(error) => {
                emit(AgentEvent::Error(format!("{error:#}")));
                false
            }
        }
    }
}

impl AgentLoopHost for StatefulLoopHost<'_> {
    fn prepare_prompt(
        &mut self,
        messages: &[ChatMessage],
        tools: &[ToolSpec],
        emit: &mut dyn FnMut(AgentEvent),
    ) -> anyhow::Result<PromptContext> {
        let mut prompt = PromptContext::default();
        for context in self.extra_context {
            context.contribute(&mut prompt, self.workspace);
        }
        let mut projection = self.projection.clone();
        projection.messages = messages.to_vec();
        for diagnostic in &projection.diagnostics {
            emit(AgentEvent::Notice(format!("历史投影修复: {diagnostic}")));
        }
        let system_chars = prompt.system_text().chars().count() as u64;
        let tools_chars = tool_spec_chars(tools);
        match apply_budget(&self.budget, system_chars, tools_chars, projection) {
            BudgetDecision::Send {
                messages, notices, ..
            } => {
                for notice in notices {
                    emit(AgentEvent::Notice(notice));
                }
                prompt.messages = messages;
                Ok(prompt)
            }
            BudgetDecision::Refuse {
                estimated_tokens,
                available_tokens,
            } => anyhow::bail!(
                "上下文估算约 {estimated_tokens} tokens,超出可用预算 {available_tokens}\
                 (窗口扣除输出预留)。未发送请求;请用 /compact 压缩历史,或 /clear 重新开始。"
            ),
        }
    }

    fn record_usage(&mut self, usage: Usage, emit: &mut dyn FnMut(AgentEvent)) {
        self.usage_total.add(usage);
        emit(AgentEvent::Usage {
            input_tokens: self.usage_total.input_tokens,
            output_tokens: self.usage_total.output_tokens,
            cache: self.usage_total.cache,
        });
    }

    fn execute_tool_turn(
        &mut self,
        _messages: &[ChatMessage],
        turn: &TurnOutput,
        calls: &[ToolCall],
        emit: &mut dyn FnMut(AgentEvent),
        cancel: &AtomicBool,
    ) -> anyhow::Result<ToolTurnResult> {
        let session_id = self.sessions.current_id().to_string();
        let mut items = Vec::with_capacity(calls.len());
        {
            let mut executor = DefaultToolExecutor {
                workspace: self.workspace,
                tools: self.tools,
                entries: self.entries.as_slice(),
                permissions: self.permissions,
                hooks: self.hooks,
                approval_rx: self.approval_rx,
                session_id: &session_id,
                tool_timeout: self.tool_timeout,
            };
            for call in calls {
                emit(AgentEvent::ToolCallStarted {
                    id: call.id.clone(),
                    name: call.name.clone(),
                    summary: util::args_summary(&call.arguments),
                });
                let mut item = executor.preflight_tool_call(
                    call.id.clone(),
                    call.name.clone(),
                    &call.arguments,
                    turn.stop == StopReason::MaxTokens,
                    emit,
                    cancel,
                );
                if let BatchItemState::Settled(outcome) = &item.state {
                    emit(AgentEvent::ToolCallFinished {
                        id: item.id.clone(),
                        name: item.name.clone(),
                        output: outcome.output.clone(),
                        error: outcome.error.clone(),
                    });
                    item.finish_emitted = true;
                }
                items.push(item);
            }
            executor.execute_tool_batch(&mut items, emit, cancel);
        }

        let mut cancelled = false;
        let mut stop_after_commit = None;
        let mut results = Vec::with_capacity(items.len());
        let mut plan_updates = Vec::new();
        let mut deferred_finishes = Vec::new();
        for item in items {
            let outcome = item.outcome.unwrap_or_else(|| {
                ToolOutcome::failure(ToolError::new(
                    ToolErrorCode::Internal,
                    "[内部错误:工具执行器未产生结果]",
                ))
            });
            if stop_after_commit.is_none() {
                stop_after_commit = item.hook_stop;
            }
            cancelled |= outcome
                .error
                .as_ref()
                .is_some_and(|error| error.code == ToolErrorCode::Aborted);
            for effect in outcome.effects {
                match effect {
                    ToolEffect::PlanUpdated(snapshot) => plan_updates.push(snapshot),
                }
            }
            if !item.finish_emitted {
                deferred_finishes.push((
                    item.id.clone(),
                    item.name.clone(),
                    outcome.output.clone(),
                    outcome.error.clone(),
                ));
            }
            results.push(Block::ToolResult {
                tool_use_id: item.id,
                content: outcome.output.model_text,
                is_error: outcome.error.is_some(),
            });
        }
        cancelled |= cancel.load(Ordering::Relaxed);
        let mut payloads = Vec::with_capacity(plan_updates.len() + 2);
        payloads.push(Self::assistant_payload(turn));
        payloads.extend(
            plan_updates
                .iter()
                .cloned()
                .map(SessionEntryPayload::PlanUpdated),
        );
        payloads.push(SessionEntryPayload::message(
            ChatMessage {
                role: Role::User,
                blocks: results,
            },
            None,
        ));
        let messages = self.commit(payloads)?;
        for (id, name, output, error) in deferred_finishes {
            emit(AgentEvent::ToolCallFinished {
                id,
                name,
                output,
                error,
            });
        }
        for snapshot in plan_updates {
            emit_plan_updated(&snapshot, emit);
        }
        Ok(ToolTurnResult {
            messages,
            cancelled,
            stop_after_commit,
        })
    }

    fn intercept_stop(
        &mut self,
        _messages: &[ChatMessage],
        turn: &TurnOutput,
        emit: &mut dyn FnMut(AgentEvent),
    ) -> anyhow::Result<Option<Vec<ChatMessage>>> {
        if self.stop_hook_active {
            return Ok(None);
        }
        let stop = self
            .hooks
            .run_stop(&turn.message, self.sessions.current_id());
        emit_hook_warnings(stop.warnings, emit);
        let Some(reason) = stop.prevent_stop else {
            return Ok(None);
        };
        let continuation = ChatMessage::user_text(format!(
            "[Stop Hook 要求继续] {reason}。请完成检查后给出最终答复。"
        ));
        let messages = self.commit(vec![
            Self::assistant_payload(turn),
            SessionEntryPayload::message(continuation, None),
        ])?;
        emit(AgentEvent::Notice(reason));
        self.stop_hook_active = true;
        Ok(Some(messages))
    }

    fn poll_queues(&mut self, emit: &mut dyn FnMut(AgentEvent), cancel: &AtomicBool) {
        let Some(inbox) = self.inbox else { return };
        loop {
            match inbox.try_recv() {
                Ok(AgentCommand::UserInput(text)) => {
                    emit(AgentEvent::Notice(
                        "当前轮进行中:该输入已按 steering 排队,将在本批工具完成后注入".into(),
                    ));
                    self.queues.steering.push_back(text);
                }
                Ok(AgentCommand::Steer(text)) => self.queues.steering.push_back(text),
                Ok(AgentCommand::FollowUp(text)) => self.queues.follow_up.push_back(text),
                Ok(AgentCommand::Shutdown) => {
                    emit(AgentEvent::Notice("收到退出请求,正在结束当前轮…".into()));
                    cancel.store(true, Ordering::Relaxed);
                    self.deferred.push_back(AgentCommand::Shutdown);
                }
                Ok(other) => self.deferred.push_back(other),
                Err(TryRecvError::Empty | TryRecvError::Disconnected) => break,
            }
        }
    }

    fn has_queued_input(&self) -> bool {
        !self.queues.steering.is_empty() || !self.queues.follow_up.is_empty()
    }

    fn prepare_next_turn(
        &mut self,
        _messages: &[ChatMessage],
        turn: &TurnOutput,
        has_queued_input: bool,
        can_continue: bool,
        emit: &mut dyn FnMut(AgentEvent),
    ) -> anyhow::Result<Option<Vec<ChatMessage>>> {
        if turn.stop == StopReason::MaxTokens
            || self.plan_reminder_sent
            || has_queued_input
            || !can_continue
        {
            return Ok(None);
        }
        let plan = reduce_plan(self.entries).snapshot;
        if !plan.has_active_items() {
            return Ok(None);
        }
        let messages = self.commit(vec![
            Self::assistant_payload(turn),
            SessionEntryPayload::PlanReminder(PlanReminderRecord {
                revision: plan.revision,
                reason: PlanReminderReason::Continue,
            }),
        ])?;
        emit(AgentEvent::Notice(format!(
            "计划 #{} 仍有未完成项，已要求模型继续一次",
            plan.revision
        )));
        self.plan_reminder_sent = true;
        Ok(Some(messages))
    }

    fn commit_terminal_turn(
        &mut self,
        _messages: &[ChatMessage],
        turn: &TurnOutput,
        _emit: &mut dyn FnMut(AgentEvent),
    ) -> anyhow::Result<Vec<ChatMessage>> {
        let mut payloads = vec![Self::assistant_payload(turn)];
        if turn.stop == StopReason::MaxTokens {
            payloads.push(SessionEntryPayload::Notice(NoticeRecord {
                text: "输出撞到 max_tokens 上限,可能不完整".into(),
                level: NoticeLevel::Warning,
            }));
        }
        self.commit(payloads)
    }

    fn take_steering(&mut self) -> Option<ChatMessage> {
        self.queues.steering.pop_front().map(ChatMessage::user_text)
    }

    fn take_follow_up(&mut self) -> Option<ChatMessage> {
        self.queues
            .follow_up
            .pop_front()
            .map(ChatMessage::user_text)
    }

    fn commit_input(
        &mut self,
        _messages: &[ChatMessage],
        input: ChatMessage,
        emit: &mut dyn FnMut(AgentEvent),
    ) -> anyhow::Result<Vec<ChatMessage>> {
        emit(AgentEvent::UserMessage(input.text()));
        self.commit(vec![SessionEntryPayload::message(input, None)])
    }

    fn finish(&mut self, cancelled: bool, emit: &mut dyn FnMut(AgentEvent)) {
        if cancelled {
            if let Some(inbox) = self.inbox {
                loop {
                    match inbox.try_recv() {
                        Ok(AgentCommand::UserInput(text) | AgentCommand::Steer(text)) => {
                            self.queues.steering.push_back(text);
                        }
                        Ok(AgentCommand::FollowUp(text)) => {
                            self.queues.follow_up.push_back(text);
                        }
                        Ok(AgentCommand::Shutdown) => {
                            self.deferred.push_back(AgentCommand::Shutdown);
                        }
                        Ok(other) => self.deferred.push_back(other),
                        Err(TryRecvError::Empty | TryRecvError::Disconnected) => break,
                    }
                }
            }
            let current_plan = reduce_plan(self.entries).snapshot;
            if let Some(repaired) = return_in_progress_to_pending(&current_plan) {
                let payloads = vec![
                    SessionEntryPayload::PlanUpdated(repaired.clone()),
                    SessionEntryPayload::PlanReminder(PlanReminderRecord {
                        revision: repaired.revision,
                        reason: PlanReminderReason::Cancelled,
                    }),
                ];
                if self.commit_or_emit(payloads, emit) {
                    emit_plan_updated(&repaired, emit);
                }
            }
        }
        let dropped = self.queues.steering.len() + self.queues.follow_up.len();
        if dropped > 0 {
            emit(AgentEvent::Notice(format!(
                "本轮结束,已丢弃 {dropped} 条未注入的排队输入"
            )));
        }
        emit(AgentEvent::TurnFinished { cancelled });
    }
}

#[derive(Default)]
struct RunQueues {
    steering: VecDeque<String>,
    follow_up: VecDeque<String>,
}

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

pub(super) fn emit_plan_updated(snapshot: &PlanSnapshot, emit: &mut dyn FnMut(AgentEvent)) {
    emit(AgentEvent::PlanUpdated {
        revision: snapshot.revision,
        items: snapshot.items.clone(),
        explanation: snapshot.explanation.clone(),
    });
}

pub(super) fn emit_hook_warnings(warnings: Vec<String>, emit: &mut dyn FnMut(AgentEvent)) {
    for warning in warnings {
        emit(AgentEvent::Notice(warning));
    }
}
