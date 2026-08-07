//! Tool preflight, permission checks, scheduling, and result settlement.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use crate::event::AgentEvent;
use crate::permission::{ApprovalDecision, ApprovalRequest, ApprovalScope, PermissionDecision};
use crate::plan::{reduce_plan, validate_transition as validate_plan_transition, PlanSnapshot};
use crate::tools::{
    normalize_outcome, PreparedToolCall, ToolContext, ToolEffect, ToolError, ToolErrorCode,
    ToolOutcome, ToolOutput,
};
use crate::util;

use super::agent_loop::emit_hook_warnings;
use super::Agent;

impl Agent {
    /// 单个调用的 preflight:兼容转换 → schema 校验 → hard deny → pre hook
    /// → 权限复核(Ask 在此阻塞等待审批)。按源顺序在 Runtime 线程执行,
    /// 任何失败都在这里定案为错误结果,绝不进入执行阶段。
    pub(super) fn preflight_tool_call(
        &mut self,
        id: String,
        name: String,
        arguments: &serde_json::Value,
        truncated: bool,
        emit: &mut dyn FnMut(AgentEvent),
        cancel: &AtomicBool,
    ) -> BatchItem {
        let settled = |outcome: ToolOutcome| BatchItem {
            id: id.clone(),
            name: name.clone(),
            state: BatchItemState::Settled(normalize_outcome(outcome.clone())),
            outcome: Some(normalize_outcome(outcome)),
            hook_stop: None,
            finish_emitted: false,
        };

        // `length` 截断的参数可能"语法合法但语义不完整",整批一个都不执行。
        if truncated {
            return settled(ToolOutcome::failure(ToolError::new(
                ToolErrorCode::TruncatedInput,
                "模型输出达到 token 上限，工具参数可能不完整；本次未执行，请重新发起完整工具调用",
            )));
        }

        let mut prepared = match self.tools.prepare(&name, arguments) {
            Ok(prepared) => prepared,
            Err(error) => return settled(ToolOutcome::failure(error)),
        };

        // Hook 之前先执行一次完整策略检查，hard deny 在这里不可逆地终止调用。
        if let PermissionDecision::Deny { reason } =
            self.permissions.evaluate(&prepared, &self.workspace)
        {
            return settled(permission_denied(reason));
        }

        let pre = self.hooks.run_pre_tool(
            &prepared.spec,
            &prepared.arguments,
            self.sessions.current_id(),
        );
        emit_hook_warnings(pre.warnings, emit);
        if let Some(reason) = pre.block {
            return settled(ToolOutcome::failure(ToolError::new(
                ToolErrorCode::HookRejected,
                reason,
            )));
        }

        if pre.arguments != prepared.arguments {
            prepared = match self.tools.prepare(&name, &pre.arguments) {
                Ok(prepared) => prepared,
                Err(error) => {
                    return settled(ToolOutcome::failure(ToolError {
                        code: ToolErrorCode::HookRejected,
                        message: format!("Hook 改写后的参数未通过 preflight: {}", error.message),
                        retryable: false,
                        details: error.details,
                    }))
                }
            };
        }

        match self.permissions.evaluate(&prepared, &self.workspace) {
            PermissionDecision::Allow => {}
            PermissionDecision::Deny { reason } => {
                return settled(permission_denied(reason));
            }
            PermissionDecision::Ask { reason, scopes } => {
                if let Err(error) = self.request_approval(&prepared, reason, scopes, emit, cancel) {
                    return settled(ToolOutcome::failure(error));
                }
            }
        }

        BatchItem {
            id,
            name,
            state: BatchItemState::Ready(prepared),
            outcome: None,
            hook_stop: None,
            finish_emitted: false,
        }
    }

    /// 执行一批已 preflight 的调用。
    ///
    /// 调度规则(与 Pi 一致的保守策略):
    /// - 全部 Ready 调用都是 ParallelSafe 且多于一个 → 受上限并发;
    ///   任一 Sequential 工具使整批退回串行(cap=1,按源顺序启动)。
    /// - `ToolCallUpdated`/`ToolCallFinished` 按**完成顺序**发出(UI 及时);
    ///   历史 ToolResult 由调用方按**源顺序**组装(相同输入产生相同 prompt)。
    /// - 全局取消传播到每个调用的组合标志;尚未启动的调用直接定案为取消,
    ///   每个 ToolUse 无论如何都有配对结果。
    /// - 配置了工具超时的调用逾期后置组合标志;工具因此中止的结果被改写为
    ///   Timeout。工具无视标志坚持完成的,保留其真实结果(副作用已发生)。
    /// - settle 之后到达的迟到进度被忽略;post hook 在协调线程按完成顺序运行。
    pub(super) fn execute_tool_batch(
        &mut self,
        items: &mut [BatchItem],
        emit: &mut dyn FnMut(AgentEvent),
        global_cancel: &AtomicBool,
    ) {
        const PARALLEL_TOOL_LIMIT: usize = 4;
        let ready: Vec<usize> = items
            .iter()
            .enumerate()
            .filter(|(_, item)| matches!(item.state, BatchItemState::Ready(_)))
            .map(|(index, _)| index)
            .collect();
        if ready.is_empty() {
            return;
        }
        let all_parallel_safe = ready.iter().all(|&index| {
            let BatchItemState::Ready(prepared) = &items[index].state else {
                return false;
            };
            prepared.spec.capabilities.execution_mode
                == crate::tools::ToolExecutionMode::ParallelSafe
        });
        let cap = if all_parallel_safe && ready.len() > 1 {
            PARALLEL_TOOL_LIMIT.min(ready.len())
        } else {
            1
        };
        let timeout = self.config.tool_timeout;
        let session_id = self.sessions.current_id().to_string();
        let mut batch_plan = reduce_plan(&self.entries).snapshot;
        // 字段级拆分借用:worker 只读 tools/workspace,协调侧独占 hooks。
        let tools = &self.tools;
        let workspace = &self.workspace;
        let hooks = &mut self.hooks;

        enum WorkerMsg {
            Progress { slot: usize, output: ToolOutput },
            Done { slot: usize, outcome: ToolOutcome },
        }

        let (tx, rx) = std::sync::mpsc::channel::<WorkerMsg>();
        let flags: Vec<Arc<AtomicBool>> = ready
            .iter()
            .map(|_| Arc::new(AtomicBool::new(false)))
            .collect();
        let mut deadlines: Vec<Option<std::time::Instant>> = vec![None; ready.len()];
        let mut settled: Vec<bool> = vec![false; ready.len()];

        std::thread::scope(|scope| {
            let mut next = 0usize;
            let mut running = 0usize;
            let mut completed = 0usize;
            let mut cancel_propagated = false;
            while completed < ready.len() {
                // 全局取消 → 一次性传播到所有已启动调用的组合标志。
                if !cancel_propagated && global_cancel.load(Ordering::Relaxed) {
                    for flag in &flags {
                        flag.store(true, Ordering::Relaxed);
                    }
                    cancel_propagated = true;
                }
                // 超时检查:逾期调用置组合标志(工具在下一个检查点中止)。
                let now = std::time::Instant::now();
                for slot in 0..ready.len() {
                    if settled[slot] {
                        continue;
                    }
                    if deadlines[slot].is_some_and(|deadline| now >= deadline) {
                        flags[slot].store(true, Ordering::Relaxed);
                    }
                }
                // 启动新 worker;取消后不再启动,直接定案为取消结果。
                while running < cap && next < ready.len() {
                    let slot = next;
                    next += 1;
                    let item_index = ready[slot];
                    if global_cancel.load(Ordering::Relaxed) {
                        let outcome = ToolOutcome::failure(ToolError::new(
                            ToolErrorCode::Aborted,
                            "[用户取消,本工具未执行]",
                        ));
                        emit(AgentEvent::ToolCallFinished {
                            id: items[item_index].id.clone(),
                            name: items[item_index].name.clone(),
                            output: outcome.output.clone(),
                            error: outcome.error.clone(),
                        });
                        items[item_index].outcome = Some(outcome);
                        items[item_index].finish_emitted = true;
                        settled[slot] = true;
                        completed += 1;
                        continue;
                    }
                    let BatchItemState::Ready(prepared) = &items[item_index].state else {
                        unreachable!("ready 列表只含 Ready 项");
                    };
                    let prepared = prepared.clone();
                    let flag = Arc::clone(&flags[slot]);
                    let progress_tx = tx.clone();
                    let done_tx = tx.clone();
                    let sid = session_id.clone();
                    let current_plan = batch_plan.clone();
                    deadlines[slot] = timeout.map(|limit| std::time::Instant::now() + limit);
                    scope.spawn(move || {
                        let mut progress = move |update: ToolOutput| {
                            let _ = progress_tx.send(WorkerMsg::Progress {
                                slot,
                                output: update,
                            });
                        };
                        let mut ctx = ToolContext {
                            workspace,
                            cancel: &flag,
                            session_id: &sid,
                            current_plan,
                            progress: &mut progress,
                            effects: Vec::new(),
                        };
                        let mut outcome = tools.execute_prepared(&prepared, &mut ctx);
                        if outcome.error.is_none() {
                            outcome.effects = ctx.take_effects();
                        }
                        let _ = done_tx.send(WorkerMsg::Done { slot, outcome });
                    });
                    running += 1;
                }
                if running == 0 {
                    // 剩余项都在取消分支同步定案了。
                    continue;
                }
                match rx.recv_timeout(Duration::from_millis(25)) {
                    Ok(WorkerMsg::Progress { slot, output }) => {
                        // settle 之后的迟到进度被忽略。
                        if !settled[slot] {
                            let item = &items[ready[slot]];
                            emit(AgentEvent::ToolCallUpdated {
                                id: item.id.clone(),
                                name: item.name.clone(),
                                output,
                            });
                        }
                    }
                    Ok(WorkerMsg::Done { slot, mut outcome }) => {
                        running -= 1;
                        completed += 1;
                        settled[slot] = true;
                        // 因超时标志而中止的结果改写为 Timeout,与用户取消区分。
                        let timed_out = deadlines[slot]
                            .is_some_and(|deadline| std::time::Instant::now() >= deadline)
                            && !global_cancel.load(Ordering::Relaxed);
                        if timed_out {
                            if let Some(error) = outcome.error.as_mut() {
                                if error.code == ToolErrorCode::Aborted {
                                    error.code = ToolErrorCode::Timeout;
                                    error.message = format!(
                                        "工具执行超过 {:.0}s 上限被中止: {}",
                                        timeout.unwrap_or_default().as_secs_f64(),
                                        error.message
                                    );
                                    outcome.output.model_text = error.message.clone();
                                }
                            }
                        }
                        let item_index = ready[slot];
                        let (mut post_outcome, hook_stop) = {
                            let BatchItemState::Ready(prepared) = &items[item_index].state else {
                                unreachable!("ready 列表只含 Ready 项");
                            };
                            let post = hooks.run_post_tool(
                                &prepared.spec,
                                &prepared.arguments,
                                outcome,
                                &session_id,
                            );
                            emit_hook_warnings(post.warnings, emit);
                            (normalize_outcome(post.outcome), post.stop_after_commit)
                        };
                        if let Err(error) =
                            apply_tool_effects(&mut batch_plan, &post_outcome.effects)
                        {
                            post_outcome = ToolOutcome::failure(ToolError::new(
                                ToolErrorCode::Internal,
                                format!("invalid harness tool effect: {error}"),
                            ));
                        }
                        if post_outcome.effects.is_empty() {
                            emit(AgentEvent::ToolCallFinished {
                                id: items[item_index].id.clone(),
                                name: items[item_index].name.clone(),
                                output: post_outcome.output.clone(),
                                error: post_outcome.error.clone(),
                            });
                            items[item_index].finish_emitted = true;
                        }
                        items[item_index].outcome = Some(post_outcome);
                        items[item_index].hook_stop = hook_stop;
                    }
                    Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
                    Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
                }
            }
        });
    }

    fn request_approval(
        &mut self,
        prepared: &PreparedToolCall,
        reason: String,
        scopes: Vec<ApprovalScope>,
        emit: &mut dyn FnMut(AgentEvent),
        cancel: &AtomicBool,
    ) -> Result<(), ToolError> {
        let request_id = uuid::Uuid::new_v4().to_string();
        let request = ApprovalRequest {
            request_id: request_id.clone(),
            tool: prepared.spec.name.clone(),
            summary: util::args_summary(&prepared.arguments),
            reason,
            scopes,
        };
        emit(AgentEvent::PermissionRequested { request });

        let Some(receiver) = self.approval_rx.as_ref() else {
            emit(AgentEvent::PermissionResolved {
                request_id,
                allowed: false,
            });
            return Err(ToolError::new(
                ToolErrorCode::PermissionDenied,
                "当前前端不支持交互审批；请在 [permissions] 中显式 allow 或改用 TUI",
            ));
        };

        loop {
            if cancel.load(Ordering::Relaxed) {
                emit(AgentEvent::PermissionResolved {
                    request_id,
                    allowed: false,
                });
                return Err(ToolError::new(
                    ToolErrorCode::Aborted,
                    "等待审批时被用户取消",
                ));
            }
            match receiver.recv_timeout(Duration::from_millis(50)) {
                Ok(response) if response.request_id != request_id => {
                    emit(AgentEvent::Notice(format!(
                        "忽略过期审批响应 {}",
                        response.request_id
                    )));
                }
                Ok(response) => match response.decision {
                    ApprovalDecision::Allow(scope) => {
                        if scope == ApprovalScope::Session {
                            self.permissions.remember_session_grant(prepared);
                        }
                        emit(AgentEvent::PermissionResolved {
                            request_id,
                            allowed: true,
                        });
                        return Ok(());
                    }
                    ApprovalDecision::Deny => {
                        emit(AgentEvent::PermissionResolved {
                            request_id,
                            allowed: false,
                        });
                        return Err(ToolError::new(
                            ToolErrorCode::PermissionDenied,
                            "用户拒绝了本次工具调用",
                        ));
                    }
                },
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
                Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                    emit(AgentEvent::PermissionResolved {
                        request_id,
                        allowed: false,
                    });
                    return Err(ToolError::new(
                        ToolErrorCode::PermissionDenied,
                        "审批通道已关闭，本次工具未执行",
                    ));
                }
            }
        }
    }
}

/// 工具批中的一个调用。preflight 后要么 Ready(可执行),
/// 要么 Settled(已定案为错误/拒绝,绝不执行);执行完成后 outcome 必定非空。
pub(super) struct BatchItem {
    pub(super) id: String,
    pub(super) name: String,
    pub(super) state: BatchItemState,
    pub(super) outcome: Option<ToolOutcome>,
    pub(super) hook_stop: Option<String>,
    pub(super) finish_emitted: bool,
}

pub(super) enum BatchItemState {
    Ready(PreparedToolCall),
    Settled(ToolOutcome),
}

fn permission_denied(reason: String) -> ToolOutcome {
    ToolOutcome::failure(ToolError::new(ToolErrorCode::PermissionDenied, reason))
}

fn apply_tool_effects(
    current_plan: &mut PlanSnapshot,
    effects: &[ToolEffect],
) -> Result<(), String> {
    for effect in effects {
        match effect {
            ToolEffect::PlanUpdated(snapshot) => {
                validate_plan_transition(current_plan, snapshot).map_err(|error| error.message)?;
                *current_plan = snapshot.clone();
            }
        }
    }
    Ok(())
}
