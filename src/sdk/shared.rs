//! Shared authoritative snapshot state used by controller queries and waits.

use std::collections::BTreeMap;
use std::sync::{Condvar, Mutex};
use std::time::{Duration, Instant};

use crate::config::ProviderCatalogEntry;
use crate::event::InputQueueKind;

use super::{
    ApprovalRequestView, QueuedInputView, ServerInfo, SessionError, SessionErrorCode, SessionPhase,
    SessionSnapshot, SessionSummaryView,
};

pub(crate) struct SessionShared {
    inner: Mutex<SharedState>,
    settled: Condvar,
}

struct SharedState {
    server: ServerInfo,
    snapshot: SessionSnapshot,
    disconnected: bool,
    ui: UiMetadata,
    session_list_generation: u64,
    sessions: Vec<SessionSummaryView>,
    session_list_error: Option<SessionError>,
    approval_claimed: bool,
}

#[derive(Clone)]
pub(crate) struct UiMetadata {
    pub provider_catalog: Vec<ProviderCatalogEntry>,
    pub reasoning_preferences: BTreeMap<String, BTreeMap<String, String>>,
}

impl SessionShared {
    pub(crate) fn new(server: ServerInfo, snapshot: SessionSnapshot, ui: UiMetadata) -> Self {
        SessionShared {
            inner: Mutex::new(SharedState {
                server,
                snapshot,
                disconnected: false,
                ui,
                session_list_generation: 0,
                sessions: Vec::new(),
                session_list_error: None,
                approval_claimed: false,
            }),
            settled: Condvar::new(),
        }
    }

    pub(crate) fn replace_snapshot(&self, snapshot: SessionSnapshot) {
        let is_settled = snapshot.phase == SessionPhase::Idle;
        let mut state = self.inner.lock().unwrap_or_else(|error| error.into_inner());
        state.snapshot = snapshot;
        if is_settled {
            self.settled.notify_all();
        }
    }

    pub(crate) fn update_live(
        &self,
        revision: u64,
        phase: SessionPhase,
        pending_approval: Option<ApprovalRequestView>,
    ) -> SessionSnapshot {
        let mut state = self.inner.lock().unwrap_or_else(|error| error.into_inner());
        let previous_request = state
            .snapshot
            .pending_approval
            .as_ref()
            .map(|request| request.request_id.as_str());
        let next_request = pending_approval
            .as_ref()
            .map(|request| request.request_id.as_str());
        if previous_request != next_request {
            state.approval_claimed = false;
        }
        state.snapshot.revision = revision;
        state.snapshot.phase = phase;
        state.snapshot.pending_approval = pending_approval;
        state.snapshot.clone()
    }

    pub(crate) fn mark_disconnected(&self) {
        let mut state = self.inner.lock().unwrap_or_else(|error| error.into_inner());
        state.disconnected = true;
        self.settled.notify_all();
    }

    pub(crate) fn set_sessions(&self, sessions: Vec<SessionSummaryView>) {
        let mut state = self.inner.lock().unwrap_or_else(|error| error.into_inner());
        state.session_list_generation = state.session_list_generation.saturating_add(1);
        state.sessions = sessions;
        state.session_list_error = None;
        self.settled.notify_all();
    }

    pub(crate) fn set_session_list_error(&self, error: SessionError) {
        let mut state = self.inner.lock().unwrap_or_else(|error| error.into_inner());
        state.session_list_generation = state.session_list_generation.saturating_add(1);
        state.session_list_error = Some(error);
        self.settled.notify_all();
    }

    pub(crate) fn update_queue(
        &self,
        revision: u64,
        command_id: String,
        kind: InputQueueKind,
        text: String,
    ) -> SessionSnapshot {
        let mut state = self.inner.lock().unwrap_or_else(|error| error.into_inner());
        let queued = QueuedInputView { command_id, text };
        match kind {
            InputQueueKind::Steering => state.snapshot.queues.steering.push(queued),
            InputQueueKind::FollowUp => state.snapshot.queues.follow_up.push(queued),
        }
        state.snapshot.revision = revision;
        state.snapshot.clone()
    }

    pub(crate) fn remove_queued(&self, revision: u64, command_id: &str) -> SessionSnapshot {
        let mut state = self.inner.lock().unwrap_or_else(|error| error.into_inner());
        state
            .snapshot
            .queues
            .steering
            .retain(|queued| queued.command_id != command_id);
        state
            .snapshot
            .queues
            .follow_up
            .retain(|queued| queued.command_id != command_id);
        state.snapshot.revision = revision;
        state.snapshot.clone()
    }

    pub(crate) fn snapshot(&self) -> Result<SessionSnapshot, SessionError> {
        let state = self.inner.lock().unwrap_or_else(|error| error.into_inner());
        if state.disconnected {
            return Err(SessionError::disconnected());
        }
        Ok(state.snapshot.clone())
    }

    pub(crate) fn server_info(&self) -> ServerInfo {
        self.inner
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .server
            .clone()
    }

    pub(crate) fn ui_metadata(&self) -> UiMetadata {
        self.inner
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .ui
            .clone()
    }

    pub(crate) fn is_disconnected(&self) -> bool {
        self.inner
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .disconnected
    }

    pub(crate) fn session_list_generation(&self) -> Result<u64, SessionError> {
        let state = self.inner.lock().unwrap_or_else(|error| error.into_inner());
        if state.disconnected {
            Err(SessionError::disconnected())
        } else {
            Ok(state.session_list_generation)
        }
    }

    pub(crate) fn wait_for_session_list(
        &self,
        previous_generation: u64,
    ) -> Result<Vec<SessionSummaryView>, SessionError> {
        let mut state = self.inner.lock().unwrap_or_else(|error| error.into_inner());
        loop {
            if state.disconnected {
                return Err(SessionError::disconnected());
            }
            if state.session_list_generation > previous_generation {
                if let Some(error) = state.session_list_error.clone() {
                    return Err(error);
                }
                return Ok(state.sessions.clone());
            }
            state = self
                .settled
                .wait(state)
                .unwrap_or_else(|error| error.into_inner());
        }
    }

    pub(crate) fn claim_approval(&self, request_id: &str) -> Result<(), SessionError> {
        let mut state = self.inner.lock().unwrap_or_else(|error| error.into_inner());
        if state.disconnected {
            return Err(SessionError::disconnected());
        }
        let Some(pending) = state.snapshot.pending_approval.as_ref() else {
            return Err(SessionError::new(
                SessionErrorCode::InvalidRequest,
                "session is not waiting for approval",
            ));
        };
        if pending.request_id != request_id {
            return Err(SessionError::new(
                SessionErrorCode::InvalidRequest,
                "approval request_id does not match the pending request",
            ));
        }
        if state.approval_claimed {
            return Err(SessionError::new(
                SessionErrorCode::InvalidRequest,
                "approval response was already submitted",
            ));
        }
        state.approval_claimed = true;
        Ok(())
    }

    pub(crate) fn release_approval_claim(&self, request_id: &str) {
        let mut state = self.inner.lock().unwrap_or_else(|error| error.into_inner());
        if state
            .snapshot
            .pending_approval
            .as_ref()
            .is_some_and(|pending| pending.request_id == request_id)
        {
            state.approval_claimed = false;
        }
    }

    pub(crate) fn wait_until_settled(
        &self,
        timeout: Duration,
    ) -> Result<SessionSnapshot, SessionError> {
        let deadline = Instant::now() + timeout;
        let mut state = self.inner.lock().unwrap_or_else(|error| error.into_inner());
        loop {
            if state.disconnected {
                return Err(SessionError::disconnected());
            }
            if state.snapshot.phase == SessionPhase::Idle {
                return Ok(state.snapshot.clone());
            }
            let now = Instant::now();
            if now >= deadline {
                return Err(SessionError::new(
                    SessionErrorCode::Timeout,
                    "timed out waiting for session to settle",
                ));
            }
            let remaining = deadline.saturating_duration_since(now);
            let (next, wait) = self
                .settled
                .wait_timeout(state, remaining)
                .unwrap_or_else(|error| error.into_inner());
            state = next;
            if wait.timed_out() && state.snapshot.phase != SessionPhase::Idle {
                return Err(SessionError::new(
                    SessionErrorCode::Timeout,
                    "timed out waiting for session to settle",
                ));
            }
        }
    }
}
