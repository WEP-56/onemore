use std::collections::VecDeque;
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex};

use super::*;
use crate::message::{Block, Role};
use crate::provider::{ProviderEvent, StreamTerminal};

struct ScriptedModel {
    steps: Mutex<VecDeque<TurnOutput>>,
    prompts: Arc<Mutex<Vec<Vec<ChatMessage>>>>,
}

impl ScriptedModel {
    fn new(steps: Vec<TurnOutput>) -> Self {
        ScriptedModel {
            steps: Mutex::new(steps.into()),
            prompts: Arc::new(Mutex::new(Vec::new())),
        }
    }
}

impl Provider for ScriptedModel {
    fn label(&self) -> String {
        "scripted / core".into()
    }

    fn model(&self) -> &str {
        "core"
    }

    fn stream_turn(
        &self,
        prompt: &PromptContext,
        _tools: &[ToolSpec],
        _on_event: &mut dyn FnMut(ProviderEvent),
        _cancel: &AtomicBool,
    ) -> StreamTerminal {
        self.prompts.lock().unwrap().push(prompt.messages.clone());
        StreamTerminal::Done(
            self.steps
                .lock()
                .unwrap()
                .pop_front()
                .expect("script exhausted"),
        )
    }
}

#[derive(Default)]
struct MemoryHost;

impl AgentLoopHost for MemoryHost {}

fn output(message: ChatMessage, stop: StopReason) -> TurnOutput {
    TurnOutput {
        message,
        usage: Usage::default(),
        stop,
        prompt_fingerprint: None,
    }
}

#[test]
fn direct_loop_needs_only_model_messages_tools_and_callbacks() {
    let model = ScriptedModel::new(vec![output(
        ChatMessage {
            role: Role::Assistant,
            blocks: vec![Block::Text("done".into())],
        },
        StopReason::EndTurn,
    )]);
    let initial = vec![ChatMessage::user_text("work")];
    let mut host = MemoryHost;
    let mut events = Vec::new();
    let cancel = AtomicBool::new(false);

    let outcome = run_agent_loop(
        &model,
        initial,
        &[],
        AgentLoopCallbacks::new(&mut host, &mut |event| events.push(event), &cancel),
    );

    assert!(!outcome.cancelled);
    assert_eq!(outcome.messages.len(), 2);
    assert_eq!(outcome.messages[0].text(), "work");
    assert_eq!(outcome.messages[1].text(), "done");
    assert!(events
        .iter()
        .any(|event| matches!(event, AgentEvent::AssistantMessage(text) if text == "done")));
}

#[test]
fn default_host_pairs_tool_calls_before_the_next_model_turn() {
    let model = ScriptedModel::new(vec![
        output(
            ChatMessage {
                role: Role::Assistant,
                blocks: vec![Block::ToolUse {
                    id: "call-1".into(),
                    name: "missing".into(),
                    input: serde_json::json!({"value": 1}),
                }],
            },
            StopReason::ToolUse,
        ),
        output(
            ChatMessage {
                role: Role::Assistant,
                blocks: vec![Block::Text("recovered".into())],
            },
            StopReason::EndTurn,
        ),
    ]);
    let prompts = Arc::clone(&model.prompts);
    let mut host = MemoryHost;
    let cancel = AtomicBool::new(false);

    let outcome = run_agent_loop(
        &model,
        vec![ChatMessage::user_text("use a tool")],
        &[],
        AgentLoopCallbacks::new(&mut host, &mut |_| {}, &cancel).max_turns(2),
    );

    assert_eq!(outcome.messages.len(), 4);
    let prompts = prompts.lock().unwrap();
    assert_eq!(prompts.len(), 2);
    let results = &prompts[1][2];
    assert!(matches!(
        results.blocks.as_slice(),
        [Block::ToolResult {
            tool_use_id,
            is_error: true,
            ..
        }] if tool_use_id == "call-1"
    ));
}

struct FollowUpHost {
    follow_up: Option<ChatMessage>,
}

impl AgentLoopHost for FollowUpHost {
    fn has_queued_input(&self) -> bool {
        self.follow_up.is_some()
    }

    fn take_follow_up(&mut self) -> Option<ChatMessage> {
        self.follow_up.take()
    }
}

#[test]
fn follow_up_callback_runs_only_after_a_terminal_assistant_turn() {
    let model = ScriptedModel::new(vec![
        output(
            ChatMessage {
                role: Role::Assistant,
                blocks: vec![Block::Text("first complete".into())],
            },
            StopReason::EndTurn,
        ),
        output(
            ChatMessage {
                role: Role::Assistant,
                blocks: vec![Block::Text("follow-up complete".into())],
            },
            StopReason::EndTurn,
        ),
    ]);
    let prompts = Arc::clone(&model.prompts);
    let mut host = FollowUpHost {
        follow_up: Some(ChatMessage::user_text("next task")),
    };
    let cancel = AtomicBool::new(false);

    let outcome = run_agent_loop(
        &model,
        vec![ChatMessage::user_text("first task")],
        &[],
        AgentLoopCallbacks::new(&mut host, &mut |_| {}, &cancel).max_turns(2),
    );

    assert_eq!(outcome.messages.len(), 4);
    let prompts = prompts.lock().unwrap();
    assert_eq!(prompts.len(), 2);
    assert_eq!(prompts[1].last().unwrap().text(), "next task");
}

struct FailingCommitHost;

impl AgentLoopHost for FailingCommitHost {
    fn commit_terminal_turn(
        &mut self,
        _messages: &[ChatMessage],
        _turn: &TurnOutput,
        _emit: &mut dyn FnMut(AgentEvent),
    ) -> anyhow::Result<Vec<ChatMessage>> {
        anyhow::bail!("commit rejected")
    }
}

#[test]
fn failed_commit_does_not_advance_the_core_transcript() {
    let model = ScriptedModel::new(vec![output(
        ChatMessage {
            role: Role::Assistant,
            blocks: vec![Block::Text("uncommitted".into())],
        },
        StopReason::EndTurn,
    )]);
    let mut host = FailingCommitHost;
    let mut events = Vec::new();
    let cancel = AtomicBool::new(false);

    let outcome = run_agent_loop(
        &model,
        vec![ChatMessage::user_text("stable")],
        &[],
        AgentLoopCallbacks::new(&mut host, &mut |event| events.push(event), &cancel),
    );

    assert_eq!(outcome.messages.len(), 1);
    assert_eq!(outcome.messages[0].text(), "stable");
    assert!(events
        .iter()
        .any(|event| matches!(event, AgentEvent::Error(text) if text.contains("commit rejected"))));
}
