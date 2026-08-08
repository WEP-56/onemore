//! Provider-neutral agent loop.
//!
//! The loop owns only model-turn orchestration. Stateful concerns such as fact
//! persistence, planning, compaction, permissions, hooks, and command queues
//! enter through [`AgentLoopHost`] callbacks.

use std::sync::atomic::{AtomicBool, Ordering};

use crate::context::PromptContext;
use crate::event::AgentEvent;
use crate::message::{Block, ChatMessage, Role, StopReason, Usage};
use crate::provider::{Provider, TurnOutput};
use crate::tools::{ToolError, ToolErrorCode, ToolOutcome, ToolSpec};

mod model_call;

#[cfg(test)]
mod tests;

pub use model_call::RetryPolicy;
pub(crate) use model_call::{call_model, ModelCallResult};

/// One provider-emitted tool request in source order.
#[derive(Debug, Clone)]
pub struct ToolCall {
    pub id: String,
    pub name: String,
    pub arguments: serde_json::Value,
}

/// Result returned by the host after executing and committing one tool turn.
pub struct ToolTurnResult {
    /// Full model-visible transcript after the commit succeeds.
    pub messages: Vec<ChatMessage>,
    /// Whether this batch observed cancellation.
    pub cancelled: bool,
    /// Optional graceful-stop reason produced by a host hook.
    pub stop_after_commit: Option<String>,
}

/// Final state of one low-level loop invocation.
pub struct AgentLoopOutcome {
    pub messages: Vec<ChatMessage>,
    pub cancelled: bool,
}

/// Host-owned operations needed at loop checkpoints.
///
/// Implementations may persist facts or maintain an in-memory transcript. A
/// method returning messages must do so only after its whole commit succeeds.
pub trait AgentLoopHost {
    /// Transform the current transcript into the provider request context.
    fn prepare_prompt(
        &mut self,
        messages: &[ChatMessage],
        _tools: &[ToolSpec],
        _emit: &mut dyn FnMut(AgentEvent),
    ) -> anyhow::Result<PromptContext> {
        Ok(PromptContext {
            system_sections: Vec::new(),
            messages: messages.to_vec(),
        })
    }

    /// Observe usage after a successful provider terminal.
    fn record_usage(&mut self, _usage: Usage, _emit: &mut dyn FnMut(AgentEvent)) {}

    /// Execute every call, pair every result, and atomically commit the
    /// assistant message plus all results before returning updated messages.
    fn execute_tool_turn(
        &mut self,
        messages: &[ChatMessage],
        turn: &TurnOutput,
        calls: &[ToolCall],
        emit: &mut dyn FnMut(AgentEvent),
        _cancel: &AtomicBool,
    ) -> anyhow::Result<ToolTurnResult> {
        let mut results = Vec::with_capacity(calls.len());
        for call in calls {
            emit(AgentEvent::ToolCallStarted {
                id: call.id.clone(),
                name: call.name.clone(),
                summary: call.arguments.to_string(),
            });
            let error = if turn.stop == StopReason::MaxTokens {
                ToolError::new(
                    ToolErrorCode::TruncatedInput,
                    "model output was truncated; tool arguments were not executed",
                )
            } else {
                ToolError::new(
                    ToolErrorCode::UnknownTool,
                    "no tool executor was configured for this agent loop",
                )
            };
            let outcome = ToolOutcome::failure(error);
            emit(AgentEvent::ToolCallFinished {
                id: call.id.clone(),
                name: call.name.clone(),
                output: outcome.output.clone(),
                error: outcome.error.clone(),
            });
            results.push(Block::ToolResult {
                tool_use_id: call.id.clone(),
                content: outcome.output.model_text,
                is_error: true,
            });
        }
        let mut committed = messages.to_vec();
        committed.push(turn.message.clone());
        committed.push(ChatMessage {
            role: Role::User,
            blocks: results,
        });
        Ok(ToolTurnResult {
            messages: committed,
            cancelled: false,
            stop_after_commit: None,
        })
    }

    /// Give the host one chance to prevent a terminal assistant stop. Returning
    /// messages means the host already committed the assistant and continuation.
    fn intercept_stop(
        &mut self,
        _messages: &[ChatMessage],
        _turn: &TurnOutput,
        _emit: &mut dyn FnMut(AgentEvent),
    ) -> anyhow::Result<Option<Vec<ChatMessage>>> {
        Ok(None)
    }

    /// Pull newly arrived steering/follow-up work into host-owned queues.
    fn poll_queues(&mut self, _emit: &mut dyn FnMut(AgentEvent), _cancel: &AtomicBool) {}

    fn has_queued_input(&self) -> bool {
        false
    }

    /// Optional harness continuation, such as a planning reminder. Returning
    /// messages means the assistant and continuation were committed atomically.
    fn prepare_next_turn(
        &mut self,
        _messages: &[ChatMessage],
        _turn: &TurnOutput,
        _has_queued_input: bool,
        _can_continue: bool,
        _emit: &mut dyn FnMut(AgentEvent),
    ) -> anyhow::Result<Option<Vec<ChatMessage>>> {
        Ok(None)
    }

    /// Commit a terminal assistant response after continuation callbacks decline.
    fn commit_terminal_turn(
        &mut self,
        messages: &[ChatMessage],
        turn: &TurnOutput,
        _emit: &mut dyn FnMut(AgentEvent),
    ) -> anyhow::Result<Vec<ChatMessage>> {
        let mut committed = messages.to_vec();
        committed.push(turn.message.clone());
        Ok(committed)
    }

    fn take_steering(&mut self) -> Option<ChatMessage> {
        None
    }

    fn take_follow_up(&mut self) -> Option<ChatMessage> {
        None
    }

    /// Commit one queued input and return the new model-visible transcript.
    fn commit_input(
        &mut self,
        messages: &[ChatMessage],
        input: ChatMessage,
        _emit: &mut dyn FnMut(AgentEvent),
    ) -> anyhow::Result<Vec<ChatMessage>> {
        let mut committed = messages.to_vec();
        committed.push(input);
        Ok(committed)
    }

    /// Run host cleanup after every loop exit, including cancellation repair.
    fn finish(&mut self, _cancelled: bool, _emit: &mut dyn FnMut(AgentEvent)) {}
}

/// Callback bundle for [`run_agent_loop`].
pub struct AgentLoopCallbacks<'a> {
    pub host: &'a mut dyn AgentLoopHost,
    pub emit: &'a mut dyn FnMut(AgentEvent),
    pub cancel: &'a AtomicBool,
    pub max_turns: u32,
    pub retry_policy: RetryPolicy,
}

impl<'a> AgentLoopCallbacks<'a> {
    pub fn new(
        host: &'a mut dyn AgentLoopHost,
        emit: &'a mut dyn FnMut(AgentEvent),
        cancel: &'a AtomicBool,
    ) -> Self {
        AgentLoopCallbacks {
            host,
            emit,
            cancel,
            max_turns: 200,
            retry_policy: RetryPolicy::default(),
        }
    }

    pub fn max_turns(mut self, max_turns: u32) -> Self {
        self.max_turns = max_turns;
        self
    }

    pub fn retry_policy(mut self, retry_policy: RetryPolicy) -> Self {
        self.retry_policy = retry_policy;
        self
    }
}

/// Run the production agent loop from a model-visible transcript.
///
/// The four inputs intentionally mirror the low-level Pi boundary: model,
/// messages, tools, and callbacks. The same function is used by the CLI and by
/// embedded in-memory agents.
pub fn run_agent_loop(
    model: &dyn Provider,
    mut messages: Vec<ChatMessage>,
    tools: &[ToolSpec],
    callbacks: AgentLoopCallbacks<'_>,
) -> AgentLoopOutcome {
    for round in 0..callbacks.max_turns {
        if callbacks.cancel.load(Ordering::Relaxed) {
            return finish(callbacks, messages, true);
        }

        let prompt = match callbacks
            .host
            .prepare_prompt(&messages, tools, callbacks.emit)
        {
            Ok(prompt) => prompt,
            Err(error) => {
                (callbacks.emit)(AgentEvent::Error(format!("{error:#}")));
                let cancelled = callbacks.cancel.load(Ordering::Relaxed);
                return finish(callbacks, messages, cancelled);
            }
        };
        let output = match call_model(
            model,
            &prompt,
            tools,
            callbacks.retry_policy,
            true,
            callbacks.emit,
            callbacks.cancel,
        ) {
            ModelCallResult::Cancelled(failed) => {
                (callbacks.emit)(AgentEvent::Error(failed.error.to_string()));
                return finish(callbacks, messages, true);
            }
            ModelCallResult::Failed(failed) => {
                (callbacks.emit)(AgentEvent::Error(failed.error.to_string()));
                return finish(callbacks, messages, false);
            }
            ModelCallResult::Done(output) => output,
        };

        callbacks.host.record_usage(output.usage, callbacks.emit);
        let text = output.message.text();
        if !text.is_empty() {
            (callbacks.emit)(AgentEvent::AssistantMessage(text));
        }
        let calls: Vec<ToolCall> = output
            .message
            .tool_uses()
            .into_iter()
            .map(|(id, name, arguments)| ToolCall {
                id: id.to_string(),
                name: name.to_string(),
                arguments: arguments.clone(),
            })
            .collect();
        if let Some(call) = calls
            .iter()
            .find(|call| call.id.trim().is_empty() || call.name.trim().is_empty())
        {
            (callbacks.emit)(AgentEvent::Error(format!(
                "模型返回了无效工具调用(id={:?},name={:?});\
                 本次 assistant 消息未写入历史，会话仍可继续",
                call.id, call.name
            )));
            return finish(callbacks, messages, false);
        }

        if calls.is_empty() {
            match callbacks
                .host
                .intercept_stop(&messages, &output, callbacks.emit)
            {
                Ok(Some(next_messages)) => {
                    messages = next_messages;
                    continue;
                }
                Ok(None) => {}
                Err(error) => {
                    (callbacks.emit)(AgentEvent::Error(format!("{error:#}")));
                    return finish(callbacks, messages, false);
                }
            }

            callbacks.host.poll_queues(callbacks.emit, callbacks.cancel);
            let has_queued_input = callbacks.host.has_queued_input();
            match callbacks.host.prepare_next_turn(
                &messages,
                &output,
                has_queued_input,
                round + 1 < callbacks.max_turns,
                callbacks.emit,
            ) {
                Ok(Some(next_messages)) => {
                    messages = next_messages;
                    continue;
                }
                Ok(None) => {}
                Err(error) => {
                    (callbacks.emit)(AgentEvent::Error(format!("{error:#}")));
                    return finish(callbacks, messages, false);
                }
            }

            messages = match callbacks
                .host
                .commit_terminal_turn(&messages, &output, callbacks.emit)
            {
                Ok(messages) => messages,
                Err(error) => {
                    (callbacks.emit)(AgentEvent::Error(format!("{error:#}")));
                    return finish(callbacks, messages, false);
                }
            };
            if output.stop == StopReason::MaxTokens {
                (callbacks.emit)(AgentEvent::Notice(
                    "输出撞到 max_tokens 上限,可能不完整".into(),
                ));
            }

            callbacks.host.poll_queues(callbacks.emit, callbacks.cancel);
            let next = callbacks
                .host
                .take_steering()
                .or_else(|| callbacks.host.take_follow_up());
            if let Some(input) = next {
                if !callbacks.cancel.load(Ordering::Relaxed) {
                    messages = match callbacks
                        .host
                        .commit_input(&messages, input, callbacks.emit)
                    {
                        Ok(messages) => messages,
                        Err(error) => {
                            (callbacks.emit)(AgentEvent::Error(format!("{error:#}")));
                            return finish(callbacks, messages, false);
                        }
                    };
                    continue;
                }
            }
            let cancelled = callbacks.cancel.load(Ordering::Relaxed);
            return finish(callbacks, messages, cancelled);
        }

        let tool_turn = match callbacks.host.execute_tool_turn(
            &messages,
            &output,
            &calls,
            callbacks.emit,
            callbacks.cancel,
        ) {
            Ok(result) => result,
            Err(error) => {
                (callbacks.emit)(AgentEvent::Error(format!("{error:#}")));
                let cancelled = callbacks.cancel.load(Ordering::Relaxed);
                return finish(callbacks, messages, cancelled);
            }
        };
        messages = tool_turn.messages;
        if tool_turn.cancelled || callbacks.cancel.load(Ordering::Relaxed) {
            return finish(callbacks, messages, true);
        }
        if let Some(reason) = tool_turn.stop_after_commit {
            (callbacks.emit)(AgentEvent::Notice(reason));
            return finish(callbacks, messages, false);
        }

        callbacks.host.poll_queues(callbacks.emit, callbacks.cancel);
        if let Some(input) = callbacks.host.take_steering() {
            messages = match callbacks
                .host
                .commit_input(&messages, input, callbacks.emit)
            {
                Ok(messages) => messages,
                Err(error) => {
                    (callbacks.emit)(AgentEvent::Error(format!("{error:#}")));
                    return finish(callbacks, messages, false);
                }
            };
        }
    }

    (callbacks.emit)(AgentEvent::Notice(format!(
        "连续调用模型达到上限({} 次),强制结束本轮;可直接输入\"继续\"接着跑",
        callbacks.max_turns
    )));
    finish(callbacks, messages, false)
}

fn finish(
    callbacks: AgentLoopCallbacks<'_>,
    messages: Vec<ChatMessage>,
    cancelled: bool,
) -> AgentLoopOutcome {
    callbacks.host.finish(cancelled, callbacks.emit);
    AgentLoopOutcome {
        messages,
        cancelled,
    }
}
