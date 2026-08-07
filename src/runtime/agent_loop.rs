//! Core agent loop, input queues, and provider call lifecycle.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Receiver;
use std::time::Duration;

use crate::context::PromptContext;
use crate::event::{AgentCommand, AgentEvent};
use crate::message::{Block, ChatMessage, Role, StopReason};
use crate::plan::{reduce_plan, return_in_progress_to_pending, PlanSnapshot};
use crate::provider::{FailedTurn, ProviderEvent, StreamTerminal};
use crate::session::{
    NoticeLevel, NoticeRecord, PlanReminderReason, PlanReminderRecord, SessionEntryPayload,
};
use crate::tools::{ToolEffect, ToolError, ToolErrorCode, ToolOutcome, ToolSpec};
use crate::util;

use super::tool_execution::{BatchItem, BatchItemState};
use super::Agent;

impl Agent {
    /// Agent Loop 本体。一次调用是一个完整的"运行"(ActiveRun):
    /// 单线程结构保证同一 Agent 同时最多一个运行;运行期间到达的命令由
    /// [`Agent::drain_inbox`] 在检查点显式分类,不靠 mpsc 排队时机隐式决定。
    pub(super) fn run_turn(
        &mut self,
        input: String,
        emit: &mut dyn FnMut(AgentEvent),
        cancel: &AtomicBool,
        inbox: Option<&Receiver<AgentCommand>>,
    ) {
        let mut queues = RunQueues::default();
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
            self.finish_run(inbox, queues, false, emit);
            return;
        }
        emit(AgentEvent::TurnStarted);
        if let Some(reason) = prompt_hooks.block {
            emit(AgentEvent::Error(reason));
            self.finish_run(inbox, queues, false, emit);
            return;
        }

        let mut specs: Vec<ToolSpec> = self.tools.specs();
        specs.sort_by(|left, right| left.name.cmp(&right.name));
        let mut stop_hook_active = false;
        let mut plan_reminder_sent = false;
        for round in 0..self.config.max_turns {
            if cancel.load(Ordering::Relaxed) {
                self.finish_run(inbox, queues, true, emit);
                return;
            }

            // ---- 1. 投影 + 预算 + 调模型(带"未开播才重试"的重试) ----
            let prompt = {
                let mut prompt = self.build_system_prompt();
                let Some(messages) = self.project_for_model(&prompt, &specs, emit) else {
                    self.finish_run(inbox, queues, false, emit);
                    return;
                };
                prompt.messages = messages;
                prompt
            };
            let output = match self.call_model(&prompt, &specs, true, emit, cancel) {
                CallResult::Cancelled(failed) => {
                    emit(AgentEvent::Error(failed.error.to_string()));
                    // 半截 assistant 输出直接丢弃:历史停在 user 消息上,仍然合法
                    self.finish_run(inbox, queues, true, emit);
                    return;
                }
                CallResult::Failed(failed) => {
                    emit(AgentEvent::Error(failed.error.to_string()));
                    self.finish_run(inbox, queues, false, emit);
                    return;
                }
                CallResult::Done(out) => out,
            };

            self.usage_total.add(output.usage);
            emit(AgentEvent::Usage {
                input_tokens: self.usage_total.input_tokens,
                output_tokens: self.usage_total.output_tokens,
                cache: self.usage_total.cache,
            });

            // ---- 2. assistant 消息成为事实(携带本次真实 usage) ----
            let text = output.message.text();
            if !text.is_empty() {
                emit(AgentEvent::AssistantMessage(text));
            }
            let calls: Vec<(String, String, serde_json::Value)> = output
                .message
                .tool_uses()
                .into_iter()
                .map(|(id, name, args)| (id.to_string(), name.to_string(), args.clone()))
                .collect();
            if let Some((id, name, _)) = calls
                .iter()
                .find(|(id, name, _)| id.trim().is_empty() || name.trim().is_empty())
            {
                emit(AgentEvent::Error(format!(
                    "模型返回了无效工具调用(id={:?},name={:?});\
                     本次 assistant 消息未写入历史，会话仍可继续",
                    id, name
                )));
                self.finish_run(inbox, queues, false, emit);
                return;
            }
            let assistant_message = output.message;
            let assistant_payload = SessionEntryPayload::message_with_prompt(
                assistant_message.clone(),
                output.usage,
                output.prompt_fingerprint,
            );

            // ---- 3. 没有工具调用 → 当前任务将停止 ----
            if calls.is_empty() {
                if !stop_hook_active {
                    let stop = self
                        .hooks
                        .run_stop(&assistant_message, self.sessions.current_id());
                    emit_hook_warnings(stop.warnings, emit);
                    if let Some(reason) = stop.prevent_stop {
                        let continuation = ChatMessage::user_text(format!(
                            "[Stop Hook 要求继续] {}。请完成检查后给出最终答复。",
                            reason
                        ));
                        if !self.commit(
                            vec![
                                assistant_payload,
                                SessionEntryPayload::message(continuation, None),
                            ],
                            emit,
                        ) {
                            self.finish_run(inbox, queues, false, emit);
                            return;
                        }
                        emit(AgentEvent::Notice(reason));
                        stop_hook_active = true;
                        continue;
                    }
                }
                // A queued user correction/task takes precedence over the automatic reminder.
                // Drain now; the existing stop path below will commit this assistant message
                // before injecting exactly one queued input.
                self.drain_inbox(inbox, &mut queues, emit, cancel);
                let has_queued_input = !queues.steering.is_empty() || !queues.follow_up.is_empty();
                if output.stop != StopReason::MaxTokens
                    && !plan_reminder_sent
                    && !has_queued_input
                    && !cancel.load(Ordering::Relaxed)
                    && round + 1 < self.config.max_turns
                {
                    let plan = reduce_plan(&self.entries).snapshot;
                    if plan.has_active_items() {
                        if !self.commit(
                            vec![
                                assistant_payload,
                                SessionEntryPayload::PlanReminder(PlanReminderRecord {
                                    revision: plan.revision,
                                    reason: PlanReminderReason::Continue,
                                }),
                            ],
                            emit,
                        ) {
                            self.finish_run(inbox, queues, false, emit);
                            return;
                        }
                        emit(AgentEvent::Notice(format!(
                            "计划 #{} 仍有未完成项，已要求模型继续一次",
                            plan.revision
                        )));
                        plan_reminder_sent = true;
                        continue;
                    }
                }
                let mut payloads = vec![assistant_payload];
                if output.stop == StopReason::MaxTokens {
                    // UI-only 事实:提示截断,但绝不进入模型上下文。
                    payloads.push(SessionEntryPayload::Notice(NoticeRecord {
                        text: "输出撞到 max_tokens 上限,可能不完整".into(),
                        level: NoticeLevel::Warning,
                    }));
                }
                if !self.commit(payloads, emit) {
                    self.finish_run(inbox, queues, false, emit);
                    return;
                }
                if output.stop == StopReason::MaxTokens {
                    emit(AgentEvent::Notice(
                        "输出撞到 max_tokens 上限,可能不完整".into(),
                    ));
                }
                // steering 仍属于当前工作;follow-up 只在这里(任务将停止时)注入。
                self.drain_inbox(inbox, &mut queues, emit, cancel);
                let next = queues
                    .steering
                    .pop_front()
                    .or_else(|| queues.follow_up.pop_front());
                match next {
                    Some(text) if !cancel.load(Ordering::Relaxed) => {
                        if !self.inject_queued_input(text, emit) {
                            self.finish_run(inbox, queues, false, emit);
                            return;
                        }
                        continue;
                    }
                    _ => {
                        self.finish_run(inbox, queues, cancel.load(Ordering::Relaxed), emit);
                        return;
                    }
                }
            }

            // ---- 4. 执行工具批:preflight 按源顺序,执行可受控并发,
            //         结果(Observation)按 ToolUse 源顺序作为事实写回 ----
            let mut items: Vec<BatchItem> = Vec::with_capacity(calls.len());
            for (id, name, args) in calls {
                emit(AgentEvent::ToolCallStarted {
                    id: id.clone(),
                    name: name.clone(),
                    summary: util::args_summary(&args),
                });
                let truncated = output.stop == StopReason::MaxTokens;
                let mut item = self.preflight_tool_call(id, name, &args, truncated, emit, cancel);
                if let BatchItemState::Settled(outcome) = &item.state {
                    // preflight 定案(校验失败/拒绝/截断):立即闭合该调用的事件。
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
            self.execute_tool_batch(&mut items, emit, cancel);

            let mut was_cancelled = false;
            let mut stop_after_commit = None;
            let mut results: Vec<Block> = Vec::with_capacity(items.len());
            let mut plan_updates = Vec::new();
            let mut deferred_finishes = Vec::new();
            for item in items {
                let outcome = item.outcome.unwrap_or_else(|| {
                    // 防御:执行器保证每个调用都有结果;若缺失,补错误而不是丢配对。
                    ToolOutcome::failure(ToolError::new(
                        ToolErrorCode::Internal,
                        "[内部错误:工具执行器未产生结果]",
                    ))
                });
                if stop_after_commit.is_none() {
                    stop_after_commit = item.hook_stop;
                }
                was_cancelled |= outcome
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
            was_cancelled |= cancel.load(Ordering::Relaxed);
            let result_message = ChatMessage {
                role: Role::User,
                blocks: results,
            };
            // ToolUse、harness-owned effects 与所有 ToolResult 必须原子落库。
            let mut payloads = Vec::with_capacity(plan_updates.len() + 2);
            payloads.push(assistant_payload);
            payloads.extend(
                plan_updates
                    .iter()
                    .cloned()
                    .map(SessionEntryPayload::PlanUpdated),
            );
            payloads.push(SessionEntryPayload::message(result_message, None));
            if !self.commit(payloads, emit) {
                self.finish_run(inbox, queues, was_cancelled, emit);
                return;
            }
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
            if was_cancelled {
                self.finish_run(inbox, queues, true, emit);
                return;
            }
            if let Some(reason) = stop_after_commit {
                emit(AgentEvent::Notice(reason));
                self.finish_run(inbox, queues, false, emit);
                return;
            }
            // ---- 5. 完整工具批已提交:steering 的唯一注入点 ----
            // 不在单个工具之间注入:避免"模型要求写文件,用户中途改口,
            // 文件到底写没写"的隐式状态。紧急停止走取消。
            self.drain_inbox(inbox, &mut queues, emit, cancel);
            if !cancel.load(Ordering::Relaxed) {
                if let Some(text) = queues.steering.pop_front() {
                    if !self.inject_queued_input(text, emit) {
                        self.finish_run(inbox, queues, false, emit);
                        return;
                    }
                }
            }
            // 回到循环顶部,把 Observation(以及可能的 steering)喂给模型
        }

        emit(AgentEvent::Notice(format!(
            "连续调用模型达到上限({} 次),强制结束本轮;可直接输入\"继续\"接着跑",
            self.config.max_turns
        )));
        self.finish_run(inbox, queues, false, emit);
    }

    /// 排干命令通道,把活动运行期间到达的命令显式分类。
    /// 直接输入 → steering(附提示);Steer/FollowUp → 对应队列;
    /// Shutdown → 请求取消当前轮并延迟退出;其余命令延迟到本轮结束执行。
    fn drain_inbox(
        &mut self,
        inbox: Option<&Receiver<AgentCommand>>,
        queues: &mut RunQueues,
        emit: &mut dyn FnMut(AgentEvent),
        cancel: &AtomicBool,
    ) {
        let Some(rx) = inbox else { return };
        while let Ok(cmd) = rx.try_recv() {
            match cmd {
                AgentCommand::UserInput(text) => {
                    emit(AgentEvent::Notice(
                        "当前轮进行中:该输入已按 steering 排队,将在本批工具完成后注入".into(),
                    ));
                    queues.steering.push_back(text);
                }
                AgentCommand::Steer(text) => queues.steering.push_back(text),
                AgentCommand::FollowUp(text) => queues.follow_up.push_back(text),
                AgentCommand::Shutdown => {
                    emit(AgentEvent::Notice("收到退出请求,正在结束当前轮…".into()));
                    cancel.store(true, Ordering::Relaxed);
                    self.deferred.push_back(AgentCommand::Shutdown);
                }
                other => self.deferred.push_back(other),
            }
        }
    }

    /// 注入一条排队输入(steering / follow-up):成为 Message 事实并回显。
    /// 有意不重跑 user-prompt hooks:排队输入属于当前运行的一部分,
    /// hooks 的"提交新 prompt"语义只对开启新运行的输入生效。
    fn inject_queued_input(&mut self, text: String, emit: &mut dyn FnMut(AgentEvent)) -> bool {
        emit(AgentEvent::UserMessage(text.clone()));
        self.commit(
            vec![SessionEntryPayload::message(
                ChatMessage::user_text(text),
                None,
            )],
            emit,
        )
    }

    /// 结束一次运行:取消时把通道里尚未取走的输入一并排干丢弃
    /// (取消清理队列;正常结束时留在通道里的输入会自然成为下一轮命令)。
    fn finish_run(
        &mut self,
        inbox: Option<&Receiver<AgentCommand>>,
        mut queues: RunQueues,
        cancelled: bool,
        emit: &mut dyn FnMut(AgentEvent),
    ) {
        if cancelled {
            if let Some(rx) = inbox {
                while let Ok(cmd) = rx.try_recv() {
                    match cmd {
                        AgentCommand::UserInput(text) | AgentCommand::Steer(text) => {
                            queues.steering.push_back(text)
                        }
                        AgentCommand::FollowUp(text) => queues.follow_up.push_back(text),
                        AgentCommand::Shutdown => self.deferred.push_back(AgentCommand::Shutdown),
                        other => self.deferred.push_back(other),
                    }
                }
            }
            let current_plan = reduce_plan(&self.entries).snapshot;
            if let Some(repaired) = return_in_progress_to_pending(&current_plan) {
                let payloads = vec![
                    SessionEntryPayload::PlanUpdated(repaired.clone()),
                    SessionEntryPayload::PlanReminder(PlanReminderRecord {
                        revision: repaired.revision,
                        reason: PlanReminderReason::Cancelled,
                    }),
                ];
                if self.commit(payloads, emit) {
                    emit_plan_updated(&repaired, emit);
                }
            }
        }
        let dropped = queues.steering.len() + queues.follow_up.len();
        if dropped > 0 {
            emit(AgentEvent::Notice(format!(
                "本轮结束,已丢弃 {} 条未注入的排队输入",
                dropped
            )));
        }
        emit(AgentEvent::TurnFinished { cancelled });
    }

    /// 供宿主循环在一次命令处理后取走"延迟命令"逐条执行。
    pub fn take_deferred(&mut self) -> Option<AgentCommand> {
        self.deferred.pop_front()
    }

    /// 单次模型调用 + 重试策略。`forward_stream` 为 false 时不把流式增量
    /// 转发成对话事件(用于压缩这类"非对话"调用)。
    pub(super) fn call_model(
        &self,
        prompt: &PromptContext,
        specs: &[ToolSpec],
        forward_stream: bool,
        emit: &mut dyn FnMut(AgentEvent),
        cancel: &AtomicBool,
    ) -> CallResult {
        let mut attempt = 1u32;
        loop {
            let mut emitted_any = false;
            let mut forward = |pe: ProviderEvent| {
                emitted_any = true;
                if !forward_stream {
                    return;
                }
                emit(match pe {
                    ProviderEvent::TextDelta(t) => AgentEvent::AssistantDelta(t),
                    ProviderEvent::ThinkingDelta(t) => AgentEvent::ThinkingDelta(t),
                    ProviderEvent::ToolCallBegun { name } => AgentEvent::ToolCallPending { name },
                });
            };
            match self
                .provider
                .stream_turn(prompt, specs, &mut forward, cancel)
            {
                StreamTerminal::Done(out) => return CallResult::Done(out),
                StreamTerminal::Aborted(failed) => return CallResult::Cancelled(failed),
                StreamTerminal::Error(failed) => {
                    // 重试幂等:只有一个流事件都没产生的失败才可重播。
                    let delay = if failed.error.retryable && !emitted_any {
                        self.retry_policy
                            .delay_for(attempt, failed.error.retry_after)
                    } else {
                        None
                    };
                    let Some(wait) = delay else {
                        return CallResult::Failed(failed);
                    };
                    emit(AgentEvent::Notice(format!(
                        "{},{:.1}s 后重试({}/{})",
                        failed.error,
                        wait.as_secs_f64(),
                        attempt,
                        self.retry_policy.max_attempts - 1
                    )));
                    // 分片睡眠,期间可被取消
                    let mut slept = Duration::ZERO;
                    while slept < wait {
                        if cancel.load(Ordering::Relaxed) {
                            return CallResult::Cancelled(FailedTurn::aborted());
                        }
                        std::thread::sleep(Duration::from_millis(100));
                        slept += Duration::from_millis(100);
                    }
                    attempt += 1;
                }
            }
        }
    }
}

pub(super) enum CallResult {
    Done(crate::provider::TurnOutput),
    Cancelled(FailedTurn),
    Failed(FailedTurn),
}

/// 一次运行的两个输入队列。语义不同,不能合并:
/// steering 改变正在进行的任务(完整工具批后注入),
/// follow-up 等当前任务将停止时才注入(one-at-a-time,每检查点取最老一条)。
#[derive(Default)]
struct RunQueues {
    steering: std::collections::VecDeque<String>,
    follow_up: std::collections::VecDeque<String>,
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
