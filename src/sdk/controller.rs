use std::fmt;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, RecvTimeoutError, SyncSender, TryRecvError};
use std::sync::Arc;
use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::event::AgentCommand;
use crate::permission::{ApprovalDecision, ApprovalResponse, ApprovalScope};
use crate::runtime::inbox::PendingCommand;
use crate::runtime::Agent;

use super::{
    ApprovalDecisionView, ApprovalResponseView, ModelMetadata, ModelSelection, ServerInfo,
    SessionEvent, SessionShared, SessionSnapshot, UiMetadata,
};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CommandReceipt {
    pub command_id: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionErrorCode {
    Busy,
    InvalidRequest,
    Disconnected,
    NotFound,
    Unsupported,
    Timeout,
    Internal,
}

impl SessionErrorCode {
    pub fn as_str(self) -> &'static str {
        match self {
            SessionErrorCode::Busy => "busy",
            SessionErrorCode::InvalidRequest => "invalid_request",
            SessionErrorCode::Disconnected => "disconnected",
            SessionErrorCode::NotFound => "not_found",
            SessionErrorCode::Unsupported => "unsupported",
            SessionErrorCode::Timeout => "timeout",
            SessionErrorCode::Internal => "internal",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SessionError {
    pub code: SessionErrorCode,
    pub message: String,
}

impl SessionError {
    pub(crate) fn new(code: SessionErrorCode, message: impl Into<String>) -> Self {
        SessionError {
            code,
            message: message.into(),
        }
    }

    pub(crate) fn busy() -> Self {
        Self::new(
            SessionErrorCode::Busy,
            "session is running; use steer or follow_up",
        )
    }

    pub(crate) fn disconnected() -> Self {
        Self::new(
            SessionErrorCode::Disconnected,
            "session runtime disconnected",
        )
    }
}

impl fmt::Display for SessionError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}: {}", self.code.as_str(), self.message)
    }
}

impl std::error::Error for SessionError {}

pub struct AgentSession {
    pub controller: SessionController,
    pub events: SessionEvents,
}

#[derive(Clone)]
pub struct SessionController {
    commands: SyncSender<PendingCommand>,
    approvals: std::sync::mpsc::Sender<ApprovalResponse>,
    cancel: Arc<AtomicBool>,
    shared: Arc<SessionShared>,
}

pub struct SessionEvents {
    receiver: Receiver<SessionEvent>,
    cancel: Arc<AtomicBool>,
}

pub fn spawn_session(agent: Agent) -> AgentSession {
    crate::runtime::spawn_session(agent)
}

impl SessionController {
    pub(crate) fn new(
        commands: SyncSender<PendingCommand>,
        approvals: std::sync::mpsc::Sender<ApprovalResponse>,
        cancel: Arc<AtomicBool>,
        shared: Arc<SessionShared>,
    ) -> Self {
        SessionController {
            commands,
            approvals,
            cancel,
            shared,
        }
    }

    pub fn prompt(&self, text: impl Into<String>) -> Result<CommandReceipt, SessionError> {
        self.submit_text(AgentCommand::UserInput, "prompt", text.into())
    }

    pub fn steer(&self, text: impl Into<String>) -> Result<CommandReceipt, SessionError> {
        self.submit_text(AgentCommand::Steer, "steer", text.into())
    }

    pub fn follow_up(&self, text: impl Into<String>) -> Result<CommandReceipt, SessionError> {
        self.submit_text(AgentCommand::FollowUp, "follow_up", text.into())
    }

    pub fn abort(&self) -> Result<CommandReceipt, SessionError> {
        self.cancel.store(true, Ordering::Relaxed);
        self.submit(AgentCommand::Abort)
    }

    pub fn compact(&self) -> Result<CommandReceipt, SessionError> {
        self.submit(AgentCommand::Compact)
    }

    pub fn set_model(&self, selection: ModelSelection) -> Result<CommandReceipt, SessionError> {
        if selection.provider.trim().is_empty()
            || selection.model.trim().is_empty()
            || selection.effort.trim().is_empty()
        {
            return Err(SessionError::new(
                SessionErrorCode::InvalidRequest,
                "provider, model, and effort must not be empty",
            ));
        }
        self.submit(AgentCommand::SetModelSelection {
            provider: selection.provider,
            model: selection.model,
            effort: selection.effort,
        })
    }

    pub fn clear_conversation(&self) -> Result<CommandReceipt, SessionError> {
        self.submit(AgentCommand::ClearConversation)
    }

    pub fn load_session(
        &self,
        session_id: impl Into<String>,
    ) -> Result<CommandReceipt, SessionError> {
        let session_id = session_id.into();
        if session_id.trim().is_empty() {
            return Err(SessionError::new(
                SessionErrorCode::InvalidRequest,
                "session_id must not be empty",
            ));
        }
        self.submit(AgentCommand::LoadSession(session_id))
    }

    pub fn list_sessions(&self) -> Result<Vec<super::SessionSummaryView>, SessionError> {
        self.list_sessions_with_scope(false)
    }

    pub fn list_all_sessions(&self) -> Result<Vec<super::SessionSummaryView>, SessionError> {
        self.list_sessions_with_scope(true)
    }

    fn list_sessions_with_scope(
        &self,
        all: bool,
    ) -> Result<Vec<super::SessionSummaryView>, SessionError> {
        let generation = self.shared.session_list_generation()?;
        self.submit(AgentCommand::ListSessions { all })?;
        self.shared.wait_for_session_list(generation)
    }

    pub fn list_models(&self) -> Vec<ModelMetadata> {
        self.shared.server_info().models
    }

    pub fn server_info(&self) -> ServerInfo {
        self.shared.server_info()
    }

    pub(crate) fn ui_metadata(&self) -> UiMetadata {
        self.shared.ui_metadata()
    }

    pub fn snapshot(&self) -> Result<SessionSnapshot, SessionError> {
        self.shared.snapshot()
    }

    pub fn respond_to_approval(&self, response: ApprovalResponseView) -> Result<(), SessionError> {
        self.shared.claim_approval(&response.request_id)?;
        let decision = match response.decision {
            ApprovalDecisionView::AllowOnce => ApprovalDecision::Allow(ApprovalScope::Once),
            ApprovalDecisionView::AllowSession => ApprovalDecision::Allow(ApprovalScope::Session),
            ApprovalDecisionView::Deny => ApprovalDecision::Deny,
        };
        if self
            .approvals
            .send(ApprovalResponse {
                request_id: response.request_id.clone(),
                decision,
            })
            .is_err()
        {
            self.shared.release_approval_claim(&response.request_id);
            return Err(SessionError::disconnected());
        }
        Ok(())
    }

    pub fn wait_until_settled(&self, timeout: Duration) -> Result<SessionSnapshot, SessionError> {
        self.shared.wait_until_settled(timeout)
    }

    pub fn shutdown(&self) -> Result<CommandReceipt, SessionError> {
        self.cancel.store(true, Ordering::Relaxed);
        self.submit(AgentCommand::Shutdown)
    }

    pub(crate) fn submit_raw(&self, command: AgentCommand) -> Result<CommandReceipt, SessionError> {
        self.submit(command)
    }

    pub(crate) fn cancel_now(&self) {
        self.cancel.store(true, Ordering::Relaxed);
    }

    pub(crate) fn send_detached(&self, command: AgentCommand) -> Result<(), SessionError> {
        self.commands
            .send(PendingCommand::detached(command))
            .map_err(|_| SessionError::disconnected())
    }

    fn submit_text(
        &self,
        make: impl FnOnce(String) -> AgentCommand,
        name: &str,
        text: String,
    ) -> Result<CommandReceipt, SessionError> {
        if text.trim().is_empty() {
            return Err(SessionError::new(
                SessionErrorCode::InvalidRequest,
                format!("{name} text must not be empty"),
            ));
        }
        self.submit(make(text))
    }

    fn submit(&self, command: AgentCommand) -> Result<CommandReceipt, SessionError> {
        if self.shared.is_disconnected() {
            return Err(SessionError::disconnected());
        }
        let command_id = uuid::Uuid::new_v4().to_string();
        let (admission_tx, admission_rx) = std::sync::mpsc::sync_channel(1);
        self.commands
            .send(PendingCommand::session(command_id, command, admission_tx))
            .map_err(|_| SessionError::disconnected())?;
        admission_rx
            .recv()
            .map_err(|_| SessionError::disconnected())?
    }
}

impl SessionEvents {
    pub(crate) fn new(receiver: Receiver<SessionEvent>, cancel: Arc<AtomicBool>) -> Self {
        SessionEvents { receiver, cancel }
    }

    pub fn recv(&mut self) -> Result<SessionEvent, std::sync::mpsc::RecvError> {
        self.receiver.recv()
    }

    pub fn recv_timeout(&mut self, timeout: Duration) -> Result<SessionEvent, RecvTimeoutError> {
        self.receiver.recv_timeout(timeout)
    }

    pub fn try_recv(&mut self) -> Result<SessionEvent, TryRecvError> {
        self.receiver.try_recv()
    }
}

impl Drop for SessionEvents {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
}
