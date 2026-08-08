//! Projection from internal agent events to the stable SDK event stream.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::SyncSender;

use crate::event::{AgentEvent, InputQueueKind};
use crate::plan::PlanSnapshot;
use crate::sdk::{
    ApprovalRequestView, CommandErrorView, ProgressEvent, SessionEvent, SessionPhase,
    SessionShared, SessionSummaryView, SkillMetadataView, SkillScopeView, UsageView,
};
use crate::session::NoticeLevel;
use crate::skills::SkillScope;

pub(super) fn send_event(
    events: &SyncSender<SessionEvent>,
    cancel: &AtomicBool,
    event: SessionEvent,
) -> bool {
    if events.send(event).is_err() {
        cancel.store(true, Ordering::Relaxed);
        false
    } else {
        true
    }
}

pub(super) struct EventAdapter<'a> {
    command_id: Option<&'a str>,
    events: &'a SyncSender<SessionEvent>,
    cancel: &'a AtomicBool,
    shared: &'a SessionShared,
    revision: &'a mut u64,
    message_id: String,
    failed: bool,
    last_error: Option<CommandErrorView>,
}

impl<'a> EventAdapter<'a> {
    pub(super) fn new(
        command_id: Option<&'a str>,
        events: &'a SyncSender<SessionEvent>,
        cancel: &'a AtomicBool,
        shared: &'a SessionShared,
        revision: &'a mut u64,
    ) -> Self {
        EventAdapter {
            command_id,
            events,
            cancel,
            shared,
            revision,
            message_id: uuid::Uuid::new_v4().to_string(),
            failed: false,
            last_error: None,
        }
    }

    pub(super) fn failed(&self) -> bool {
        self.failed
    }

    pub(super) fn last_error(&self) -> Option<&CommandErrorView> {
        self.last_error.as_ref()
    }

    pub(super) fn emit(&mut self, event: AgentEvent) {
        let progress = match event {
            AgentEvent::UserMessage(text) => Some(ProgressEvent::UserMessage { text }),
            AgentEvent::TurnStarted => {
                self.command_id.map(|command_id| ProgressEvent::RunStarted {
                    command_id: command_id.to_string(),
                })
            }
            AgentEvent::RetryScheduled {
                attempt,
                max_retries,
                delay_ms,
                error,
            } => {
                self.update_phase(SessionPhase::Retrying);
                Some(ProgressEvent::RetryScheduled {
                    attempt,
                    max_retries,
                    delay_ms,
                    error,
                })
            }
            AgentEvent::RetryStarted {
                attempt,
                max_retries,
            } => {
                self.update_phase(SessionPhase::Running);
                Some(ProgressEvent::RetryStarted {
                    attempt,
                    max_retries,
                })
            }
            AgentEvent::AssistantDelta(delta) => Some(ProgressEvent::AssistantDelta {
                message_id: self.message_id.clone(),
                content_index: 0,
                kind: "text".into(),
                delta,
            }),
            AgentEvent::ThinkingDelta(delta) => Some(ProgressEvent::AssistantDelta {
                message_id: self.message_id.clone(),
                content_index: 0,
                kind: "thinking".into(),
                delta,
            }),
            AgentEvent::AssistantMessage(text) => {
                let message_id =
                    std::mem::replace(&mut self.message_id, uuid::Uuid::new_v4().to_string());
                Some(ProgressEvent::AssistantFinished { message_id, text })
            }
            AgentEvent::ToolCallPending { name } => Some(ProgressEvent::ToolCallPending { name }),
            AgentEvent::ToolCallStarted { id, name, summary } => Some(ProgressEvent::ToolStarted {
                tool_call_id: id,
                name,
                summary,
            }),
            AgentEvent::ToolCallUpdated {
                id, name, output, ..
            } => Some(ProgressEvent::ToolUpdated {
                tool_call_id: id,
                name,
                output: output.ui_text().to_string(),
            }),
            AgentEvent::ToolCallFinished {
                id,
                name,
                output,
                error,
            } => Some(ProgressEvent::ToolFinished {
                tool_call_id: id,
                name,
                output: output.ui_text().to_string(),
                error: error.map(|error| CommandErrorView {
                    code: error.code.as_str().into(),
                    message: error.message,
                }),
            }),
            AgentEvent::PlanUpdated {
                revision,
                items,
                explanation,
            } => Some(ProgressEvent::PlanUpdated {
                plan: PlanSnapshot {
                    revision,
                    items,
                    explanation,
                }
                .into(),
            }),
            AgentEvent::SkillsDiscovered { skills, warnings } => {
                Some(ProgressEvent::SkillsDiscovered {
                    skills: skills
                        .into_iter()
                        .map(|skill| SkillMetadataView {
                            name: skill.name,
                            description: skill.description,
                            scope: match skill.scope {
                                SkillScope::Repo => SkillScopeView::Repo,
                                SkillScope::User => SkillScopeView::User,
                            },
                        })
                        .collect(),
                    warnings,
                })
            }
            AgentEvent::PermissionRequested { request } => {
                let view = ApprovalRequestView::from(&request);
                self.update_approval(SessionPhase::WaitingApproval, Some(view.clone()));
                Some(ProgressEvent::ApprovalRequested { request: view })
            }
            AgentEvent::PermissionResolved {
                request_id,
                allowed,
            } => {
                self.update_approval(SessionPhase::Running, None);
                Some(ProgressEvent::ApprovalResolved {
                    request_id,
                    allowed,
                })
            }
            AgentEvent::Usage {
                input_tokens,
                output_tokens,
                cache,
            } => Some(ProgressEvent::Usage {
                usage: UsageView {
                    input_tokens,
                    output_tokens,
                    cache_read_tokens: cache.map(|cache| cache.read_tokens),
                    cache_write_tokens: cache.map(|cache| cache.write_tokens),
                },
            }),
            AgentEvent::Notice(text) => Some(ProgressEvent::Notice {
                level: NoticeLevel::Info,
                text,
            }),
            AgentEvent::Error(message) => {
                let error = CommandErrorView {
                    code: "agent_error".into(),
                    message,
                };
                self.failed = true;
                self.last_error = Some(error.clone());
                Some(ProgressEvent::Error { error })
            }
            AgentEvent::ConversationCleared => Some(ProgressEvent::ConversationCleared),
            AgentEvent::ModelSelectionChanged {
                provider,
                model,
                effort,
                label,
            } => Some(ProgressEvent::ModelSelectionChanged {
                selection: crate::sdk::ModelSelectionView {
                    provider,
                    model,
                    effort,
                    label,
                },
            }),
            AgentEvent::SessionsListed {
                current_id,
                sessions,
            } => {
                let sessions = sessions
                    .into_iter()
                    .map(|session| SessionSummaryView {
                        id: session.id,
                        title: session.title,
                        workspace: session.workspace,
                        message_count: session.message_count,
                        updated_at: session.updated_at,
                    })
                    .collect::<Vec<_>>();
                self.shared.set_sessions(sessions.clone());
                Some(ProgressEvent::SessionsListed {
                    current_id,
                    sessions,
                })
            }
            AgentEvent::InputQueued {
                command_id,
                kind,
                text,
            } => {
                self.update_queue(command_id, kind, text);
                None
            }
            AgentEvent::InputDequeued { command_id } => {
                self.remove_queued(&command_id);
                None
            }
            AgentEvent::SessionLoaded { .. } | AgentEvent::TurnFinished { .. } => None,
        };
        if let Some(progress) = progress {
            let _ = send_event(
                self.events,
                self.cancel,
                SessionEvent::Progress { progress },
            );
        }
    }

    fn update_approval(&mut self, phase: SessionPhase, pending: Option<ApprovalRequestView>) {
        *self.revision = self.revision.saturating_add(1);
        let snapshot = self.shared.update_live(*self.revision, phase, pending);
        let _ = send_event(
            self.events,
            self.cancel,
            SessionEvent::SessionSnapshot {
                snapshot: Box::new(snapshot),
            },
        );
    }

    fn update_phase(&mut self, phase: SessionPhase) {
        *self.revision = self.revision.saturating_add(1);
        let snapshot = self.shared.update_live(*self.revision, phase, None);
        let _ = send_event(
            self.events,
            self.cancel,
            SessionEvent::SessionSnapshot {
                snapshot: Box::new(snapshot),
            },
        );
    }

    fn update_queue(&mut self, command_id: String, kind: InputQueueKind, text: String) {
        *self.revision = self.revision.saturating_add(1);
        let snapshot = self
            .shared
            .update_queue(*self.revision, command_id, kind, text);
        let _ = send_event(
            self.events,
            self.cancel,
            SessionEvent::SessionSnapshot {
                snapshot: Box::new(snapshot),
            },
        );
    }

    fn remove_queued(&mut self, command_id: &str) {
        *self.revision = self.revision.saturating_add(1);
        let snapshot = self.shared.remove_queued(*self.revision, command_id);
        let _ = send_event(
            self.events,
            self.cancel,
            SessionEvent::SessionSnapshot {
                snapshot: Box::new(snapshot),
            },
        );
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::*;
    use crate::sdk::{
        Capabilities, ModelSelectionView, PlanView, QueueView, ServerInfo, SessionSnapshot,
        UiMetadata, UsageView, PROTOCOL_VERSION,
    };

    fn shared() -> SessionShared {
        SessionShared::new(
            ServerInfo {
                server_id: "server".into(),
                protocol_version: PROTOCOL_VERSION,
                capabilities: Capabilities::default(),
                models: Vec::new(),
            },
            SessionSnapshot {
                session_id: "session".into(),
                revision: 0,
                workspace: "workspace".into(),
                phase: SessionPhase::Running,
                model: ModelSelectionView {
                    provider: "provider".into(),
                    model: "model".into(),
                    effort: "medium".into(),
                    label: "provider / model".into(),
                },
                usage: UsageView::default(),
                transcript: Vec::new(),
                plan: PlanView::default(),
                queues: QueueView::default(),
                pending_approval: None,
            },
            UiMetadata {
                provider_catalog: Vec::new(),
                reasoning_preferences: BTreeMap::new(),
            },
        )
    }

    #[test]
    fn retry_events_publish_phase_snapshots_before_progress() {
        let shared = shared();
        let cancel = AtomicBool::new(false);
        let (sender, receiver) = std::sync::mpsc::sync_channel(8);
        let mut revision = 0;
        {
            let mut adapter =
                EventAdapter::new(Some("command"), &sender, &cancel, &shared, &mut revision);
            adapter.emit(AgentEvent::RetryScheduled {
                attempt: 1,
                max_retries: 7,
                delay_ms: 1000,
                error: "connection reset".into(),
            });
            adapter.emit(AgentEvent::RetryStarted {
                attempt: 1,
                max_retries: 7,
            });
        }

        let events = receiver.try_iter().collect::<Vec<_>>();
        assert!(matches!(
            &events[0],
            SessionEvent::SessionSnapshot { snapshot }
                if snapshot.phase == SessionPhase::Retrying && snapshot.revision == 1
        ));
        assert!(matches!(
            &events[1],
            SessionEvent::Progress {
                progress: ProgressEvent::RetryScheduled {
                    attempt: 1,
                    max_retries: 7,
                    delay_ms: 1000,
                    error,
                }
            } if error == "connection reset"
        ));
        assert!(matches!(
            &events[2],
            SessionEvent::SessionSnapshot { snapshot }
                if snapshot.phase == SessionPhase::Running && snapshot.revision == 2
        ));
        assert!(matches!(
            &events[3],
            SessionEvent::Progress {
                progress: ProgressEvent::RetryStarted {
                    attempt: 1,
                    max_retries: 7,
                }
            }
        ));
        assert_eq!(events.len(), 4);
        assert_eq!(shared.snapshot().unwrap().phase, SessionPhase::Running);
    }
}
