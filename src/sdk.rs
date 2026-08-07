//! Stable host-facing data contracts for embedding and RPC adapters.
//!
//! The stateful runtime owns commands and facts. This module exposes only
//! sanitized views derived from that authoritative state; storage payloads and
//! provider-private data are deliberately not part of the contract.

mod controller;
mod shared;
mod view;

pub const PROTOCOL_VERSION: u32 = 1;

pub(crate) use shared::{SessionShared, UiMetadata};
pub(crate) use view::{model_metadata, project_snapshot, SnapshotSource};

pub use controller::{
    spawn_session, AgentSession, CommandReceipt, SessionController, SessionError, SessionErrorCode,
    SessionEvents,
};
pub use view::{
    ApprovalDecisionView, ApprovalRequestView, ApprovalResponseView, ApprovalScopeView,
    AssistantBlockView, AssistantStatus, Capabilities, CommandErrorView, CommandStatus,
    ModelMetadata, ModelSelection, ModelSelectionView, PlanItemView, PlanView, ProgressEvent,
    QueueView, QueuedInputView, ServerInfo, SessionEvent, SessionPhase, SessionSnapshot,
    SessionSummaryView, SkillMetadataView, SkillScopeView, ToolStatus, TranscriptItem, UsageView,
};
