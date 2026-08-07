use std::sync::atomic::AtomicUsize;
use std::sync::mpsc::{Receiver, Sender};
use std::sync::Mutex;

use super::*;
use crate::sdk::{
    AgentSession, CommandStatus, SessionErrorCode, SessionEvent, SessionPhase, TranscriptItem,
};

struct GatedProvider {
    started: Sender<()>,
    release: Mutex<Receiver<()>>,
}

struct TwoTurnGatedProvider {
    calls: AtomicUsize,
    first_started: Sender<()>,
    first_release: Mutex<Receiver<()>>,
    second_started: Sender<()>,
    second_release: Mutex<Receiver<()>>,
}

struct CancelAwareProvider {
    started: Sender<()>,
    cancelled: Sender<()>,
}

impl Provider for CancelAwareProvider {
    fn label(&self) -> String {
        "cancel-aware / model".into()
    }

    fn model(&self) -> &str {
        "model"
    }

    fn stream_turn(
        &self,
        _prompt: &PromptContext,
        _tools: &[ToolSpec],
        _on_event: &mut dyn FnMut(ProviderEvent),
        cancel: &AtomicBool,
    ) -> StreamTerminal {
        self.started.send(()).unwrap();
        while !cancel.load(Ordering::Relaxed) {
            std::thread::sleep(Duration::from_millis(5));
        }
        self.cancelled.send(()).unwrap();
        StreamTerminal::Aborted(FailedTurn::aborted())
    }
}

impl Provider for TwoTurnGatedProvider {
    fn label(&self) -> String {
        "gated / model".into()
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
        let call = self.calls.fetch_add(1, Ordering::Relaxed);
        let text = match call {
            0 => {
                self.first_started.send(()).unwrap();
                self.first_release.lock().unwrap().recv().unwrap();
                "first"
            }
            1 => {
                self.second_started.send(()).unwrap();
                self.second_release.lock().unwrap().recv().unwrap();
                "second"
            }
            _ => panic!("unexpected provider call {call}"),
        };
        StreamTerminal::Done(output(
            ChatMessage {
                role: Role::Assistant,
                blocks: vec![Block::Text(text.into())],
            },
            StopReason::EndTurn,
        ))
    }
}

struct ListFailBackend {
    inner: crate::harness::MemorySessionBackend,
}

impl crate::harness::SessionBackend for ListFailBackend {
    fn current_id(&self) -> &str {
        self.inner.current_id()
    }

    fn append_payloads(
        &mut self,
        payloads: Vec<SessionEntryPayload>,
        usage: Usage,
    ) -> anyhow::Result<Vec<SessionEntry>> {
        self.inner.append_payloads(payloads, usage)
    }

    fn list(&self) -> anyhow::Result<Vec<crate::session::SessionSummary>> {
        anyhow::bail!("list unavailable")
    }
}

impl Provider for GatedProvider {
    fn label(&self) -> String {
        "gated / model".into()
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
        self.started.send(()).unwrap();
        self.release.lock().unwrap().recv().unwrap();
        StreamTerminal::Done(output(
            ChatMessage {
                role: Role::Assistant,
                blocks: vec![Block::Text("done".into())],
            },
            StopReason::EndTurn,
        ))
    }
}

#[test]
fn controller_receipt_finishes_before_settled_snapshot() {
    let root = temp_root("sdk-settled");
    let mut agent = Agent::new_with_data_dir(
        config(&root),
        Workspace::new(root.clone()),
        root.join("data"),
    )
    .unwrap();
    agent.provider = Box::new(ScriptedProvider::new(vec![ScriptStep::Output(output(
        ChatMessage {
            role: Role::Assistant,
            blocks: vec![Block::Text("answer".into())],
        },
        StopReason::EndTurn,
    ))]));

    let mut session = spawn_session(agent);
    let receipt = session.controller.prompt("question").unwrap();
    let snapshot = session
        .controller
        .wait_until_settled(Duration::from_secs(2))
        .unwrap();
    assert_eq!(snapshot.phase, SessionPhase::Idle);
    assert!(snapshot.transcript.iter().any(
        |item| matches!(item, TranscriptItem::UserMessage { text, .. } if text == "question")
    ));
    assert!(snapshot.transcript.iter().any(|item| matches!(
        item,
        TranscriptItem::AssistantMessage { blocks, .. }
            if blocks.iter().any(|block| matches!(
                block,
                crate::sdk::AssistantBlockView::Text { text } if text == "answer"
            ))
    )));

    let mut ordered = Vec::new();
    loop {
        let event = session.events.recv_timeout(Duration::from_secs(2)).unwrap();
        match &event {
            SessionEvent::CommandFinished {
                command_id, status, ..
            } if command_id == &receipt.command_id => {
                assert_eq!(*status, CommandStatus::Succeeded);
                ordered.push("finished");
            }
            SessionEvent::Settled { .. } => {
                ordered.push("settled");
                break;
            }
            _ => {}
        }
    }
    assert_eq!(ordered, vec!["finished", "settled"]);
    session.controller.shutdown().unwrap();
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn prompt_during_active_run_is_rejected_at_runtime_admission() {
    let root = temp_root("sdk-busy");
    let mut agent = Agent::new_with_data_dir(
        config(&root),
        Workspace::new(root.clone()),
        root.join("data"),
    )
    .unwrap();
    let (started_tx, started_rx) = std::sync::mpsc::channel();
    let (release_tx, release_rx) = std::sync::mpsc::channel();
    agent.provider = Box::new(GatedProvider {
        started: started_tx,
        release: Mutex::new(release_rx),
    });

    let session = spawn_session(agent);
    session.controller.prompt("first").unwrap();
    started_rx.recv_timeout(Duration::from_secs(2)).unwrap();
    let release = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(50));
        release_tx.send(()).unwrap();
    });
    let error = session.controller.prompt("ambiguous second").unwrap_err();
    assert_eq!(error.code, SessionErrorCode::Busy);
    release.join().unwrap();
    session
        .controller
        .wait_until_settled(Duration::from_secs(2))
        .unwrap();
    session.controller.shutdown().unwrap();
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn accepted_steering_is_visible_in_queue_and_has_one_terminal_event() {
    let root = temp_root("sdk-queue");
    let mut agent = Agent::new_with_data_dir(
        config(&root),
        Workspace::new(root.clone()),
        root.join("data"),
    )
    .unwrap();
    let (first_started_tx, first_started_rx) = std::sync::mpsc::channel();
    let (first_release_tx, first_release_rx) = std::sync::mpsc::channel();
    let (second_started_tx, second_started_rx) = std::sync::mpsc::channel();
    let (second_release_tx, second_release_rx) = std::sync::mpsc::channel();
    agent.provider = Box::new(TwoTurnGatedProvider {
        calls: AtomicUsize::new(0),
        first_started: first_started_tx,
        first_release: Mutex::new(first_release_rx),
        second_started: second_started_tx,
        second_release: Mutex::new(second_release_rx),
    });

    let mut session = spawn_session(agent);
    let root_receipt = session.controller.prompt("first task").unwrap();
    first_started_rx
        .recv_timeout(Duration::from_secs(2))
        .unwrap();
    let steering_controller = session.controller.clone();
    let steering = std::thread::spawn(move || steering_controller.steer("change direction"));
    std::thread::sleep(Duration::from_millis(50));
    first_release_tx.send(()).unwrap();
    let steering_receipt = steering.join().unwrap().unwrap();
    second_started_rx
        .recv_timeout(Duration::from_secs(2))
        .unwrap();

    let mut observed = Vec::new();
    while let Ok(event) = session.events.try_recv() {
        observed.push(event);
    }
    let queued_revision = observed.iter().find_map(|event| match event {
        SessionEvent::SessionSnapshot { snapshot }
            if snapshot
                .queues
                .steering
                .iter()
                .any(|queued| queued.command_id == steering_receipt.command_id) =>
        {
            Some(snapshot.revision)
        }
        _ => None,
    });
    let queued_revision = queued_revision.expect("accepted steering must enter snapshot queue");
    assert!(observed.iter().any(|event| matches!(
        event,
        SessionEvent::SessionSnapshot { snapshot }
            if snapshot.revision > queued_revision && snapshot.queues.steering.is_empty()
    )));

    second_release_tx.send(()).unwrap();
    session
        .controller
        .wait_until_settled(Duration::from_secs(2))
        .unwrap();
    loop {
        let event = session.events.recv_timeout(Duration::from_secs(2)).unwrap();
        let settled = matches!(event, SessionEvent::Settled { .. });
        observed.push(event);
        if settled {
            break;
        }
    }
    for receipt in [&root_receipt, &steering_receipt] {
        assert_eq!(
            observed
                .iter()
                .filter(|event| matches!(
                    event,
                    SessionEvent::CommandFinished { command_id, .. }
                        if command_id == &receipt.command_id
                ))
                .count(),
            1
        );
    }
    session.controller.shutdown().unwrap();
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn list_sessions_backend_error_wakes_the_waiting_controller() {
    let root = temp_root("sdk-list-error");
    let agent = Agent::builder(config(&root), Workspace::new(root.clone()))
        .data_dir(root.join("data"))
        .session_backend(ListFailBackend {
            inner: crate::harness::MemorySessionBackend::new(),
        })
        .build()
        .unwrap();
    let session = spawn_session(agent);

    let error = session.controller.list_sessions().unwrap_err();
    assert_eq!(error.code, SessionErrorCode::Internal);
    assert!(error.message.contains("list unavailable"));

    session.controller.shutdown().unwrap();
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn dropping_the_event_consumer_cancels_the_active_provider() {
    let root = temp_root("sdk-event-drop");
    let mut agent = Agent::new_with_data_dir(
        config(&root),
        Workspace::new(root.clone()),
        root.join("data"),
    )
    .unwrap();
    let (started_tx, started_rx) = std::sync::mpsc::channel();
    let (cancelled_tx, cancelled_rx) = std::sync::mpsc::channel();
    agent.provider = Box::new(CancelAwareProvider {
        started: started_tx,
        cancelled: cancelled_tx,
    });

    let AgentSession { controller, events } = spawn_session(agent);
    controller.prompt("wait").unwrap();
    started_rx.recv_timeout(Duration::from_secs(2)).unwrap();
    drop(events);
    cancelled_rx
        .recv_timeout(Duration::from_secs(2))
        .expect("dropping SessionEvents must cancel the provider immediately");

    drop(controller);
    let _ = std::fs::remove_dir_all(root);
}
