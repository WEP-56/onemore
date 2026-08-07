use std::collections::VecDeque;
use std::io;
use std::sync::atomic::AtomicBool;
use std::sync::mpsc::{Receiver, SyncSender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde_json::{json, Value};

use super::*;
use crate::config::{ApiKind, ProviderProfile, ProviderSettings, ReasoningEffortPolicy};
use crate::context::PromptContext;
use crate::message::{Block, ChatMessage, Role, StopReason, Usage};
use crate::provider::{Provider, ProviderEvent, StreamTerminal, TurnOutput};
use crate::tools::{
    Tool, ToolCapabilities, ToolContext, ToolError, ToolOutput, ToolPermissionSpec, ToolRegistry,
    ToolSpec,
};
use crate::workspace::Workspace;

enum ProviderStep {
    Reply(String),
    ToolCall,
    Gate {
        started: std::sync::mpsc::Sender<()>,
        release: Receiver<()>,
        text: String,
    },
}

struct RpcTestProvider {
    steps: Arc<Mutex<VecDeque<ProviderStep>>>,
}

impl Provider for RpcTestProvider {
    fn label(&self) -> String {
        "rpc-test / model".into()
    }

    fn model(&self) -> &str {
        "model"
    }

    fn stream_turn(
        &self,
        _prompt: &PromptContext,
        _tools: &[ToolSpec],
        _on_event: &mut dyn FnMut(ProviderEvent),
        _cancel: &AtomicBool,
    ) -> StreamTerminal {
        let step = self
            .steps
            .lock()
            .unwrap()
            .pop_front()
            .expect("RPC test provider script exhausted");
        match step {
            ProviderStep::Reply(text) => done(ChatMessage {
                role: Role::Assistant,
                blocks: vec![Block::Text(text)],
            }),
            ProviderStep::ToolCall => StreamTerminal::Done(TurnOutput {
                message: ChatMessage {
                    role: Role::Assistant,
                    blocks: vec![Block::ToolUse {
                        id: "tool-1".into(),
                        name: "approval_test".into(),
                        input: json!({}),
                    }],
                },
                usage: Usage::default(),
                stop: StopReason::ToolUse,
                prompt_fingerprint: None,
            }),
            ProviderStep::Gate {
                started,
                release,
                text,
            } => {
                started.send(()).unwrap();
                release.recv().unwrap();
                done(ChatMessage {
                    role: Role::Assistant,
                    blocks: vec![Block::Text(text)],
                })
            }
        }
    }
}

fn done(message: ChatMessage) -> StreamTerminal {
    StreamTerminal::Done(TurnOutput {
        message,
        usage: Usage::default(),
        stop: StopReason::EndTurn,
        prompt_fingerprint: None,
    })
}

struct ApprovalTool;

impl Tool for ApprovalTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "approval_test".into(),
            description: "test approval transport".into(),
            schema: json!({"type": "object", "additionalProperties": false}),
            capabilities: ToolCapabilities::READ_ONLY,
            permission: ToolPermissionSpec::opaque_side_effect(&[]),
        }
    }

    fn execute(
        &self,
        _args: &Value,
        _context: &mut ToolContext<'_>,
    ) -> Result<ToolOutput, ToolError> {
        Ok(ToolOutput::text("approved"))
    }
}

fn test_agent(steps: Vec<ProviderStep>, tools: ToolRegistry) -> Agent {
    let steps = Arc::new(Mutex::new(steps.into()));
    let factory_steps = Arc::clone(&steps);
    Agent::builder_from_provider(
        ProviderSettings {
            name: "rpc-test".into(),
            api: ApiKind::Responses,
            profile: ProviderProfile::OpenAiResponses,
            base_url: "http://127.0.0.1:1".into(),
            api_key: String::new(),
            model: "model".into(),
            max_tokens: Some(1_024),
            context_window: Some(16_000),
            selected_effort: "medium".into(),
            reasoning_effort: ReasoningEffortPolicy::Omit,
        },
        Workspace::new(std::env::current_dir().unwrap()),
    )
    .in_memory()
    .context_providers(Vec::new())
    .tools(tools)
    .provider_factory(move |_| {
        Box::new(RpcTestProvider {
            steps: Arc::clone(&factory_steps),
        })
    })
    .build()
    .unwrap()
}

#[derive(Clone)]
struct RecordingWriter {
    bytes: Arc<Mutex<Vec<u8>>>,
}

impl io::Write for RecordingWriter {
    fn write(&mut self, buffer: &[u8]) -> io::Result<usize> {
        self.bytes.lock().unwrap().extend_from_slice(buffer);
        Ok(buffer.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

type Server = (
    SyncSender<InputFrame>,
    Arc<Mutex<Vec<u8>>>,
    std::thread::JoinHandle<anyhow::Result<()>>,
);

fn start_server(agent: Agent) -> Server {
    let (input_tx, input_rx) = std::sync::mpsc::sync_channel(64);
    let bytes = Arc::new(Mutex::new(Vec::new()));
    let writer_bytes = Arc::clone(&bytes);
    let join = std::thread::spawn(move || {
        let mut writer = RecordingWriter {
            bytes: writer_bytes,
        };
        serve(agent, input_rx, &mut writer)
    });
    (input_tx, bytes, join)
}

fn send(input: &SyncSender<InputFrame>, value: Value) {
    input.send(InputFrame::Line(value.to_string())).unwrap();
}

fn frames(bytes: &Arc<Mutex<Vec<u8>>>) -> Vec<Value> {
    let text = String::from_utf8(bytes.lock().unwrap().clone()).unwrap();
    text.lines()
        .filter_map(|line| serde_json::from_str(line).ok())
        .collect()
}

fn wait_for(bytes: &Arc<Mutex<Vec<u8>>>, predicate: impl Fn(&[Value]) -> bool) -> Vec<Value> {
    let deadline = Instant::now() + Duration::from_secs(3);
    loop {
        let current = frames(bytes);
        if predicate(&current) {
            return current;
        }
        assert!(
            Instant::now() < deadline,
            "timed out waiting for RPC frames: {current:#?}"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
}

fn has_response(frames: &[Value], id: &str, ok: bool) -> bool {
    frames
        .iter()
        .any(|frame| frame["type"] == "response" && frame["id"] == id && frame["ok"] == ok)
}

fn shutdown(
    input: SyncSender<InputFrame>,
    bytes: &Arc<Mutex<Vec<u8>>>,
    join: std::thread::JoinHandle<anyhow::Result<()>>,
) {
    send(
        &input,
        json!({"type": "request", "id": "shutdown", "request": {"command": "shutdown"}}),
    );
    wait_for(bytes, |frames| has_response(frames, "shutdown", true));
    join.join().unwrap().unwrap();
}

#[test]
fn prompt_events_snapshot_and_protocol_errors_share_one_jsonl_stream() {
    let agent = test_agent(
        vec![ProviderStep::Reply("answer".into())],
        ToolRegistry::new(Vec::new()),
    );
    let (input, bytes, join) = start_server(agent);
    send(&input, json!({"type": "hello", "version": 1}));
    wait_for(&bytes, |frames| {
        frames.iter().any(|frame| frame["type"] == "hello")
    });

    send(
        &input,
        json!({"type": "request", "id": "prompt", "request": {"command": "prompt", "text": "question"}}),
    );
    let current = wait_for(&bytes, |frames| {
        has_response(frames, "prompt", true)
            && frames
                .iter()
                .any(|frame| frame.pointer("/event/type") == Some(&json!("settled")))
    });
    let command_id = current
        .iter()
        .find(|frame| frame["id"] == "prompt")
        .unwrap()
        .pointer("/result/command_id")
        .and_then(Value::as_str)
        .unwrap();
    let finished = current
        .iter()
        .position(|frame| {
            frame.pointer("/event/type") == Some(&json!("command_finished"))
                && frame.pointer("/event/command_id") == Some(&json!(command_id))
        })
        .unwrap();
    let settled = current
        .iter()
        .position(|frame| frame.pointer("/event/type") == Some(&json!("settled")))
        .unwrap();
    assert!(finished < settled);

    send(
        &input,
        json!({"type": "request", "id": "snapshot", "request": {"command": "get_snapshot"}}),
    );
    let current = wait_for(&bytes, |frames| has_response(frames, "snapshot", true));
    let snapshot = current
        .iter()
        .find(|frame| frame["id"] == "snapshot")
        .unwrap();
    assert!(snapshot
        .pointer("/result/snapshot/transcript")
        .and_then(Value::as_array)
        .unwrap()
        .iter()
        .any(|item| item.to_string().contains("answer")));

    send(
        &input,
        json!({"type": "request", "id": "snapshot", "request": {"command": "get_snapshot"}}),
    );
    input
        .send(InputFrame::Line(
            json!({"type": "request", "id": "bad", "request": {"command": "get_snapshot"}, "extra": true}).to_string(),
        ))
        .unwrap();
    input
        .send(InputFrame::Line(
            json!({"type": "request", "id": "x".repeat(MAX_REQUEST_ID_BYTES + 1), "request": {"command": "get_snapshot"}}).to_string(),
        ))
        .unwrap();
    let current = wait_for(&bytes, |frames| {
        frames.iter().any(|frame| {
            frame["id"] == "snapshot"
                && frame.pointer("/error/code") == Some(&json!("duplicate_request_id"))
        }) && frames
            .iter()
            .filter(|frame| {
                frame["type"] == "protocol_error"
                    && frame.pointer("/error/code") == Some(&json!("invalid_request"))
            })
            .count()
            >= 2
    });
    let raw = String::from_utf8(bytes.lock().unwrap().clone()).unwrap();
    assert!(!raw.contains("\r\n"));
    assert_eq!(raw.lines().count(), current.len());

    shutdown(input, &bytes, join);
}

#[test]
fn active_second_prompt_is_rejected_by_runtime_admission() {
    let (started_tx, started_rx) = std::sync::mpsc::channel();
    let (release_tx, release_rx) = std::sync::mpsc::channel();
    let agent = test_agent(
        vec![ProviderStep::Gate {
            started: started_tx,
            release: release_rx,
            text: "first answer".into(),
        }],
        ToolRegistry::new(Vec::new()),
    );
    let (input, bytes, join) = start_server(agent);
    send(&input, json!({"type": "hello", "version": 1}));
    wait_for(&bytes, |frames| !frames.is_empty());
    send(
        &input,
        json!({"type": "request", "id": "first", "request": {"command": "prompt", "text": "one"}}),
    );
    started_rx.recv_timeout(Duration::from_secs(2)).unwrap();
    send(
        &input,
        json!({"type": "request", "id": "second", "request": {"command": "prompt", "text": "two"}}),
    );
    std::thread::sleep(Duration::from_millis(50));
    release_tx.send(()).unwrap();
    let current = wait_for(&bytes, |frames| has_response(frames, "second", false));
    assert!(current.iter().any(|frame| {
        frame["id"] == "second" && frame.pointer("/error/code") == Some(&json!("busy"))
    }));
    wait_for(&bytes, |frames| {
        frames
            .iter()
            .any(|frame| frame.pointer("/event/type") == Some(&json!("settled")))
    });
    shutdown(input, &bytes, join);
}

#[test]
fn approval_roundtrip_is_correlated_and_duplicate_response_fails_closed() {
    let agent = test_agent(
        vec![ProviderStep::ToolCall, ProviderStep::Reply("done".into())],
        ToolRegistry::new(vec![Box::new(ApprovalTool)]),
    );
    let (input, bytes, join) = start_server(agent);
    send(&input, json!({"type": "hello", "version": 1}));
    wait_for(&bytes, |frames| !frames.is_empty());
    send(
        &input,
        json!({"type": "request", "id": "prompt", "request": {"command": "prompt", "text": "use tool"}}),
    );
    let current = wait_for(&bytes, |frames| {
        frames.iter().any(|frame| {
            frame.pointer("/event/progress/type") == Some(&json!("approval_requested"))
        })
    });
    let request_id = current
        .iter()
        .find_map(|frame| {
            (frame.pointer("/event/progress/type") == Some(&json!("approval_requested"))).then(
                || {
                    frame
                        .pointer("/event/progress/request/request_id")
                        .and_then(Value::as_str)
                        .unwrap()
                        .to_string()
                },
            )
        })
        .unwrap();
    send(
        &input,
        json!({"type": "request", "id": "approve", "request": {
            "command": "approval_response", "request_id": request_id, "decision": "allow_once"
        }}),
    );
    wait_for(&bytes, |frames| has_response(frames, "approve", true));
    send(
        &input,
        json!({"type": "request", "id": "duplicate-approval", "request": {
            "command": "approval_response", "request_id": request_id, "decision": "deny"
        }}),
    );
    let current = wait_for(&bytes, |frames| {
        has_response(frames, "duplicate-approval", false)
            && frames
                .iter()
                .any(|frame| frame.pointer("/event/type") == Some(&json!("settled")))
    });
    assert!(current.iter().any(|frame| {
        frame["id"] == "duplicate-approval"
            && frame.pointer("/error/code") == Some(&json!("invalid_request"))
    }));
    assert!(current.iter().any(|frame| {
        frame.pointer("/event/progress/type") == Some(&json!("tool_finished"))
            && frame.pointer("/event/progress/error") == Some(&Value::Null)
    }));
    shutdown(input, &bytes, join);
}

#[test]
fn version_mismatch_eof_and_broken_pipe_terminate_the_server() {
    let wrong = test_agent(Vec::new(), ToolRegistry::new(Vec::new()));
    let (input, bytes, join) = start_server(wrong);
    send(&input, json!({"type": "hello", "version": 2}));
    join.join().unwrap().unwrap();
    assert!(frames(&bytes).iter().any(|frame| {
        frame["type"] == "hello_error"
            && frame.pointer("/error/code") == Some(&json!("version_mismatch"))
    }));

    let (started_tx, started_rx) = std::sync::mpsc::channel();
    let (release_tx, release_rx) = std::sync::mpsc::channel();
    let eof_agent = test_agent(
        vec![ProviderStep::Gate {
            started: started_tx,
            release: release_rx,
            text: "ignored".into(),
        }],
        ToolRegistry::new(Vec::new()),
    );
    let (input, bytes, join) = start_server(eof_agent);
    send(&input, json!({"type": "hello", "version": 1}));
    wait_for(&bytes, |frames| !frames.is_empty());
    send(
        &input,
        json!({"type": "request", "id": "prompt", "request": {"command": "prompt", "text": "wait"}}),
    );
    started_rx.recv_timeout(Duration::from_secs(2)).unwrap();
    input.send(InputFrame::Eof).unwrap();
    std::thread::sleep(Duration::from_millis(50));
    release_tx.send(()).unwrap();
    let deadline = Instant::now() + Duration::from_secs(3);
    while !join.is_finished() {
        assert!(
            Instant::now() < deadline,
            "EOF did not terminate RPC server"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
    join.join().unwrap().unwrap();
    let eof_frames = frames(&bytes);
    assert!(
        eof_frames
            .iter()
            .any(|frame| frame.pointer("/event/status") == Some(&json!("cancelled"))),
        "EOF terminal frames: {eof_frames:#?}"
    );

    struct BrokenWriter;
    impl io::Write for BrokenWriter {
        fn write(&mut self, _buffer: &[u8]) -> io::Result<usize> {
            Err(io::Error::new(io::ErrorKind::BrokenPipe, "client closed"))
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    let broken_agent = test_agent(Vec::new(), ToolRegistry::new(Vec::new()));
    let (input_tx, input_rx) = std::sync::mpsc::sync_channel(1);
    send(&input_tx, json!({"type": "hello", "version": 1}));
    let error = serve(broken_agent, input_rx, &mut BrokenWriter).unwrap_err();
    assert!(format!("{error:#}").contains("client closed"));
}
