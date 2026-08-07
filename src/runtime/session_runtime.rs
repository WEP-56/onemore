//! Bounded command/event worker that owns one stateful Agent.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::SyncSender;
use std::sync::Arc;

use crate::event::AgentCommand;
use crate::sdk::{
    model_metadata, project_snapshot, AgentSession, ApprovalRequestView, Capabilities,
    CommandErrorView, CommandStatus, QueueView, ServerInfo, SessionController, SessionEvent,
    SessionEvents, SessionPhase, SessionShared, SessionSnapshot, SnapshotSource, UiMetadata,
    PROTOCOL_VERSION,
};

use super::inbox::PendingCommand;
use super::session_events::{send_event, EventAdapter};
use super::Agent;

const COMMAND_QUEUE_CAPACITY: usize = 64;
const EVENT_QUEUE_CAPACITY: usize = 256;

pub fn spawn_session(agent: Agent) -> AgentSession {
    let provider_catalog = agent.models.provider_catalog();
    let server = ServerInfo {
        server_id: uuid::Uuid::new_v4().to_string(),
        protocol_version: PROTOCOL_VERSION,
        capabilities: Capabilities::default(),
        models: model_metadata(&provider_catalog),
    };
    let snapshot = agent.sdk_snapshot(0, SessionPhase::Idle, QueueView::default(), None);
    let shared = Arc::new(SessionShared::new(
        server,
        snapshot,
        UiMetadata {
            provider_catalog,
            reasoning_preferences: agent.model_preferences.reasoning_efforts(),
        },
    ));
    let cancel = Arc::new(AtomicBool::new(false));
    let (command_tx, command_rx) =
        std::sync::mpsc::sync_channel::<PendingCommand>(COMMAND_QUEUE_CAPACITY);
    let (approval_tx, approval_rx) = std::sync::mpsc::channel();
    let (event_tx, event_rx) = std::sync::mpsc::sync_channel::<SessionEvent>(EVENT_QUEUE_CAPACITY);

    let controller = SessionController::new(
        command_tx,
        approval_tx,
        Arc::clone(&cancel),
        Arc::clone(&shared),
    );
    let worker_cancel = Arc::clone(&cancel);
    let worker_shared = Arc::clone(&shared);
    std::thread::Builder::new()
        .name("agent-session".into())
        .spawn(move || {
            run_worker(
                agent,
                command_rx,
                approval_rx,
                event_tx,
                worker_cancel,
                worker_shared,
            );
        })
        .expect("无法创建 agent session 线程");

    AgentSession {
        controller,
        events: SessionEvents::new(event_rx, cancel),
    }
}

fn run_worker(
    mut agent: Agent,
    command_rx: std::sync::mpsc::Receiver<PendingCommand>,
    approval_rx: std::sync::mpsc::Receiver<crate::permission::ApprovalResponse>,
    events: SyncSender<SessionEvent>,
    cancel: Arc<AtomicBool>,
    shared: Arc<SessionShared>,
) {
    agent.approval_rx = Some(approval_rx);
    let mut revision = 0u64;
    {
        let mut adapter = EventAdapter::new(None, &events, &cancel, &shared, &mut revision);
        agent.emit_startup_events(&mut |event| adapter.emit(event));
    }

    while let Ok(mut pending) = command_rx.recv() {
        let command_id = pending.command_id.clone();
        let command = pending.command.clone();
        let is_list_sessions = matches!(command, AgentCommand::ListSessions);
        if let Err(error) = validate_idle_command(&command) {
            pending.reject(error);
            continue;
        }
        if !matches!(command, AgentCommand::Abort | AgentCommand::Shutdown) {
            cancel.store(false, Ordering::Relaxed);
        }
        revision = revision.saturating_add(1);
        let live = shared.update_live(revision, command_phase(&command), None);
        pending.accept();
        if !send_event(
            &events,
            &cancel,
            SessionEvent::SessionSnapshot {
                snapshot: Box::new(live),
            },
        ) {
            break;
        }

        let mut adapter = EventAdapter::new(
            command_id.as_deref(),
            &events,
            &cancel,
            &shared,
            &mut revision,
        );
        let report = agent.handle_command_report(
            command,
            &mut |event| adapter.emit(event),
            &cancel,
            Some(&command_rx),
        );
        let failed = adapter.failed();
        let last_error = adapter.last_error().cloned();
        drop(adapter);

        if is_list_sessions && failed {
            let message = last_error
                .as_ref()
                .map(|error| error.message.clone())
                .unwrap_or_else(|| "failed to list sessions".into());
            shared.set_session_list_error(crate::sdk::SessionError::new(
                crate::sdk::SessionErrorCode::Internal,
                message,
            ));
        }

        if let Some(command_id) = command_id {
            let status = command_status(report.run.as_ref(), failed);
            let error = (status == CommandStatus::Failed).then_some(
                last_error.clone().unwrap_or_else(|| CommandErrorView {
                    code: "agent_error".into(),
                    message: "command failed".into(),
                }),
            );
            if !send_event(
                &events,
                &cancel,
                SessionEvent::CommandFinished {
                    command_id,
                    status,
                    error,
                },
            ) {
                break;
            }
        }

        if let Some(run) = &report.run {
            if !emit_run_command_results(&events, &cancel, run, last_error.as_ref()) {
                break;
            }
        }

        let next_phase = if report.keep_running {
            SessionPhase::Idle
        } else {
            SessionPhase::ShuttingDown
        };
        revision = revision.saturating_add(1);
        let snapshot = agent.sdk_snapshot(revision, next_phase, QueueView::default(), None);
        shared.replace_snapshot(snapshot.clone());
        if !send_event(
            &events,
            &cancel,
            SessionEvent::SessionSnapshot {
                snapshot: Box::new(snapshot),
            },
        ) {
            break;
        }
        if report.keep_running && !send_event(&events, &cancel, SessionEvent::Settled { revision })
        {
            break;
        }
        if !report.keep_running {
            break;
        }
    }

    cancel.store(true, Ordering::Relaxed);
    shared.mark_disconnected();
}

fn emit_run_command_results(
    events: &SyncSender<SessionEvent>,
    cancel: &AtomicBool,
    run: &super::agent_loop::RunReport,
    last_error: Option<&CommandErrorView>,
) -> bool {
    let status = if run.cancelled {
        CommandStatus::Cancelled
    } else if run.failed {
        CommandStatus::Failed
    } else {
        CommandStatus::Succeeded
    };
    for command_id in &run.processed_command_ids {
        let error = (status == CommandStatus::Failed).then(|| {
            last_error.cloned().unwrap_or_else(|| CommandErrorView {
                code: "agent_error".into(),
                message: "queued command failed".into(),
            })
        });
        if !send_event(
            events,
            cancel,
            SessionEvent::CommandFinished {
                command_id: command_id.clone(),
                status,
                error,
            },
        ) {
            return false;
        }
    }
    for command_id in &run.cancelled_command_ids {
        if !send_event(
            events,
            cancel,
            SessionEvent::CommandFinished {
                command_id: command_id.clone(),
                status: CommandStatus::Cancelled,
                error: None,
            },
        ) {
            return false;
        }
    }
    for command_id in &run.control_command_ids {
        if !send_event(
            events,
            cancel,
            SessionEvent::CommandFinished {
                command_id: command_id.clone(),
                status: CommandStatus::Succeeded,
                error: None,
            },
        ) {
            return false;
        }
    }
    true
}

fn validate_idle_command(command: &AgentCommand) -> Result<(), crate::sdk::SessionError> {
    let text = match command {
        AgentCommand::UserInput(text)
        | AgentCommand::Steer(text)
        | AgentCommand::FollowUp(text) => Some(text),
        _ => None,
    };
    if text.is_some_and(|text| text.trim().is_empty()) {
        return Err(crate::sdk::SessionError::new(
            crate::sdk::SessionErrorCode::InvalidRequest,
            "input text must not be empty",
        ));
    }
    Ok(())
}

fn command_phase(command: &AgentCommand) -> SessionPhase {
    match command {
        AgentCommand::Compact => SessionPhase::Compacting,
        AgentCommand::Shutdown => SessionPhase::ShuttingDown,
        _ => SessionPhase::Running,
    }
}

fn command_status(run: Option<&super::agent_loop::RunReport>, failed: bool) -> CommandStatus {
    match run {
        Some(run) if run.cancelled => CommandStatus::Cancelled,
        Some(run) if run.failed || failed => CommandStatus::Failed,
        _ if failed => CommandStatus::Failed,
        _ => CommandStatus::Succeeded,
    }
}

impl Agent {
    fn sdk_snapshot(
        &self,
        revision: u64,
        phase: SessionPhase,
        queues: QueueView,
        pending_approval: Option<ApprovalRequestView>,
    ) -> SessionSnapshot {
        let provider_label = self.provider.label();
        project_snapshot(SnapshotSource {
            session_id: self.sessions.current_id(),
            revision,
            workspace: self.workspace.root(),
            phase,
            selection: &self.active_selection,
            provider_label: &provider_label,
            usage: self.usage_total,
            entries: &self.entries,
            queues,
            pending_approval,
        })
    }
}
