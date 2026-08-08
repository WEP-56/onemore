use std::collections::HashMap;
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::config::{ActiveModelSelection, ProviderCatalogEntry};
use crate::message::{Block, Role, Usage};
use crate::permission::{ApprovalRequest, ApprovalScope};
use crate::plan::{reduce_plan, PlanSnapshot, PlanStatus};
use crate::session::{NoticeLevel, SessionEntry, SessionEntryPayload};
use crate::util;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ServerInfo {
    pub server_id: String,
    pub protocol_version: u32,
    pub capabilities: Capabilities,
    pub models: Vec<ModelMetadata>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Capabilities {
    pub compaction: bool,
    pub session_management: bool,
    pub interactive_approval: bool,
    pub steering: bool,
    pub follow_up: bool,
}

impl Default for Capabilities {
    fn default() -> Self {
        Capabilities {
            compaction: true,
            session_management: true,
            interactive_approval: true,
            steering: true,
            follow_up: true,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ModelMetadata {
    pub provider: String,
    pub model: String,
    pub label: String,
    pub supported_efforts: Vec<String>,
    pub default_effort: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionPhase {
    Idle,
    Running,
    Retrying,
    Compacting,
    WaitingApproval,
    ShuttingDown,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SessionSnapshot {
    pub session_id: String,
    pub revision: u64,
    pub workspace: String,
    pub phase: SessionPhase,
    pub model: ModelSelectionView,
    pub usage: UsageView,
    pub transcript: Vec<TranscriptItem>,
    pub plan: PlanView,
    pub queues: QueueView,
    pub pending_approval: Option<ApprovalRequestView>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ModelSelectionView {
    pub provider: String,
    pub model: String,
    pub effort: String,
    pub label: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ModelSelection {
    pub provider: String,
    pub model: String,
    pub effort: String,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UsageView {
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_read_tokens: Option<u64>,
    pub cache_write_tokens: Option<u64>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct QueueView {
    pub steering: Vec<QueuedInputView>,
    pub follow_up: Vec<QueuedInputView>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct QueuedInputView {
    pub command_id: String,
    pub text: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PlanView {
    pub revision: u64,
    pub items: Vec<PlanItemView>,
    pub explanation: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PlanItemView {
    pub id: String,
    pub text: String,
    pub status: PlanStatus,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum TranscriptItem {
    UserMessage {
        id: String,
        parent_id: Option<String>,
        created_at: i64,
        text: String,
        command_id: Option<String>,
    },
    AssistantMessage {
        id: String,
        parent_id: Option<String>,
        created_at: i64,
        blocks: Vec<AssistantBlockView>,
        status: AssistantStatus,
    },
    Tool {
        tool_call_id: String,
        name: String,
        summary: String,
        status: ToolStatus,
        output: Option<String>,
    },
    Notice {
        id: String,
        created_at: i64,
        level: NoticeLevel,
        text: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum AssistantBlockView {
    Text {
        text: String,
    },
    Thinking {
        text: String,
    },
    ToolCall {
        id: String,
        name: String,
        summary: String,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AssistantStatus {
    Complete,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolStatus {
    Succeeded,
    Failed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ApprovalScopeView {
    Once,
    Session,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ApprovalRequestView {
    pub request_id: String,
    pub tool: String,
    pub summary: String,
    pub reason: String,
    pub scopes: Vec<ApprovalScopeView>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ApprovalResponseView {
    pub request_id: String,
    pub decision: ApprovalDecisionView,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ApprovalDecisionView {
    AllowOnce,
    AllowSession,
    Deny,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CommandStatus {
    Succeeded,
    Failed,
    Cancelled,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CommandErrorView {
    pub code: String,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SessionSummaryView {
    pub id: String,
    pub title: String,
    pub workspace: String,
    pub message_count: usize,
    pub updated_at: i64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SkillScopeView {
    Repo,
    User,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SkillMetadataView {
    pub name: String,
    pub description: String,
    pub scope: SkillScopeView,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum SessionEvent {
    SessionSnapshot {
        snapshot: Box<SessionSnapshot>,
    },
    Progress {
        progress: ProgressEvent,
    },
    CommandFinished {
        command_id: String,
        status: CommandStatus,
        error: Option<CommandErrorView>,
    },
    Settled {
        revision: u64,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum ProgressEvent {
    UserMessage {
        text: String,
    },
    RunStarted {
        command_id: String,
    },
    RetryScheduled {
        attempt: u32,
        max_retries: u32,
        delay_ms: u64,
        error: String,
    },
    RetryStarted {
        attempt: u32,
        max_retries: u32,
    },
    AssistantDelta {
        message_id: String,
        content_index: usize,
        kind: String,
        delta: String,
    },
    AssistantFinished {
        message_id: String,
        text: String,
    },
    ToolCallPending {
        name: String,
    },
    ToolStarted {
        tool_call_id: String,
        name: String,
        summary: String,
    },
    ToolUpdated {
        tool_call_id: String,
        name: String,
        output: String,
    },
    ToolFinished {
        tool_call_id: String,
        name: String,
        output: String,
        error: Option<CommandErrorView>,
    },
    ApprovalRequested {
        request: ApprovalRequestView,
    },
    ApprovalResolved {
        request_id: String,
        allowed: bool,
    },
    Notice {
        level: NoticeLevel,
        text: String,
    },
    Error {
        error: CommandErrorView,
    },
    PlanUpdated {
        plan: PlanView,
    },
    SkillsDiscovered {
        skills: Vec<SkillMetadataView>,
        warnings: Vec<String>,
    },
    Usage {
        usage: UsageView,
    },
    ConversationCleared,
    ModelSelectionChanged {
        selection: ModelSelectionView,
    },
    SessionsListed {
        current_id: String,
        sessions: Vec<SessionSummaryView>,
    },
}

pub(crate) struct SnapshotSource<'a> {
    pub session_id: &'a str,
    pub revision: u64,
    pub workspace: &'a Path,
    pub phase: SessionPhase,
    pub selection: &'a ActiveModelSelection,
    pub provider_label: &'a str,
    pub usage: Usage,
    pub entries: &'a [SessionEntry],
    pub queues: QueueView,
    pub pending_approval: Option<ApprovalRequestView>,
}

pub(crate) fn project_snapshot(source: SnapshotSource<'_>) -> SessionSnapshot {
    SessionSnapshot {
        session_id: source.session_id.to_string(),
        revision: source.revision,
        workspace: source.workspace.display().to_string(),
        phase: source.phase,
        model: ModelSelectionView {
            provider: source.selection.provider.clone(),
            model: source.selection.model.clone(),
            effort: source.selection.effort.clone(),
            label: source.provider_label.to_string(),
        },
        usage: source.usage.into(),
        transcript: project_transcript(source.entries),
        plan: reduce_plan(source.entries).snapshot.into(),
        queues: source.queues,
        pending_approval: source.pending_approval,
    }
}

pub(crate) fn model_metadata(catalog: &[ProviderCatalogEntry]) -> Vec<ModelMetadata> {
    catalog
        .iter()
        .flat_map(|provider| {
            provider.models.iter().map(|model| ModelMetadata {
                provider: provider.name.clone(),
                model: model.id.clone(),
                label: format!("{} / {}", provider.name, model.id),
                supported_efforts: model.efforts.clone(),
                default_effort: model.default_effort.clone(),
            })
        })
        .collect()
}

pub(crate) fn project_transcript(entries: &[SessionEntry]) -> Vec<TranscriptItem> {
    let results = tool_results(entries);
    let mut transcript = Vec::new();

    for entry in entries {
        match &entry.payload {
            SessionEntryPayload::Message(record) => match record.message.role {
                Role::User => {
                    let text = record.message.text();
                    if !text.is_empty() {
                        transcript.push(TranscriptItem::UserMessage {
                            id: entry.id.clone(),
                            parent_id: entry.parent_id.clone(),
                            created_at: entry.created_at,
                            text,
                            command_id: None,
                        });
                    }
                }
                Role::Assistant => {
                    let mut blocks = Vec::new();
                    let mut tools = Vec::new();
                    for block in &record.message.blocks {
                        match block {
                            Block::Text(text) => {
                                blocks.push(AssistantBlockView::Text { text: text.clone() });
                            }
                            Block::Thinking { text, .. } => {
                                blocks.push(AssistantBlockView::Thinking { text: text.clone() });
                            }
                            Block::ToolUse { id, name, input } => {
                                let summary = util::args_summary(input);
                                blocks.push(AssistantBlockView::ToolCall {
                                    id: id.clone(),
                                    name: name.clone(),
                                    summary: summary.clone(),
                                });
                                let (status, output) = match results.get(id.as_str()) {
                                    Some((content, false)) => {
                                        (ToolStatus::Succeeded, Some((*content).to_string()))
                                    }
                                    Some((content, true)) => {
                                        (ToolStatus::Failed, Some((*content).to_string()))
                                    }
                                    None => continue,
                                };
                                tools.push(TranscriptItem::Tool {
                                    tool_call_id: id.clone(),
                                    name: name.clone(),
                                    summary,
                                    status,
                                    output,
                                });
                            }
                            Block::ToolResult { .. } => {}
                        }
                    }
                    if !blocks.is_empty() {
                        transcript.push(TranscriptItem::AssistantMessage {
                            id: entry.id.clone(),
                            parent_id: entry.parent_id.clone(),
                            created_at: entry.created_at,
                            blocks,
                            status: AssistantStatus::Complete,
                        });
                    }
                    transcript.extend(tools);
                }
            },
            SessionEntryPayload::Notice(notice) => transcript.push(TranscriptItem::Notice {
                id: entry.id.clone(),
                created_at: entry.created_at,
                level: notice.level,
                text: notice.text.clone(),
            }),
            SessionEntryPayload::Compaction(compaction) => {
                transcript.push(TranscriptItem::Notice {
                    id: entry.id.clone(),
                    created_at: entry.created_at,
                    level: NoticeLevel::Info,
                    text: format!("历史已压缩（压缩前约 {} tokens）", compaction.tokens_before),
                });
            }
            SessionEntryPayload::ModelChange(change) => {
                transcript.push(TranscriptItem::Notice {
                    id: entry.id.clone(),
                    created_at: entry.created_at,
                    level: NoticeLevel::Info,
                    text: format!(
                        "模型切换：{} / {} / effort={}",
                        change.provider, change.model, change.effort
                    ),
                });
            }
            SessionEntryPayload::Artifact(_)
            | SessionEntryPayload::PlanUpdated(_)
            | SessionEntryPayload::PlanReminder(_) => {}
        }
    }

    transcript
}

fn tool_results(entries: &[SessionEntry]) -> HashMap<&str, (&str, bool)> {
    entries
        .iter()
        .filter_map(|entry| match &entry.payload {
            SessionEntryPayload::Message(record) => Some(&record.message),
            _ => None,
        })
        .flat_map(|message| message.blocks.iter())
        .filter_map(|block| match block {
            Block::ToolResult {
                tool_use_id,
                content,
                is_error,
            } => Some((tool_use_id.as_str(), (content.as_str(), *is_error))),
            _ => None,
        })
        .collect()
}

impl From<Usage> for UsageView {
    fn from(usage: Usage) -> Self {
        UsageView {
            input_tokens: usage.input_tokens,
            output_tokens: usage.output_tokens,
            cache_read_tokens: usage.cache.map(|cache| cache.read_tokens),
            cache_write_tokens: usage.cache.map(|cache| cache.write_tokens),
        }
    }
}

impl From<PlanSnapshot> for PlanView {
    fn from(plan: PlanSnapshot) -> Self {
        PlanView {
            revision: plan.revision,
            items: plan
                .items
                .into_iter()
                .map(|item| PlanItemView {
                    id: item.id,
                    text: item.text,
                    status: item.status,
                })
                .collect(),
            explanation: plan.explanation,
        }
    }
}

impl From<&ApprovalRequest> for ApprovalRequestView {
    fn from(request: &ApprovalRequest) -> Self {
        ApprovalRequestView {
            request_id: request.request_id.clone(),
            tool: request.tool.clone(),
            summary: request.summary.clone(),
            reason: request.reason.clone(),
            scopes: request
                .scopes
                .iter()
                .map(|scope| match scope {
                    ApprovalScope::Once => ApprovalScopeView::Once,
                    ApprovalScope::Session => ApprovalScopeView::Session,
                })
                .collect(),
        }
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::message::ChatMessage;
    use crate::plan::{PlanItem, PlanStatus};
    use crate::session::{NoticeRecord, SessionEntryPayload};

    fn entry(id: &str, payload: SessionEntryPayload) -> SessionEntry {
        SessionEntry {
            id: id.into(),
            parent_id: None,
            created_at: 1,
            payload,
        }
    }

    #[test]
    fn snapshot_projection_omits_provider_raw_and_tool_arguments() {
        let entries = vec![
            entry(
                "assistant",
                SessionEntryPayload::message(
                    ChatMessage {
                        role: Role::Assistant,
                        blocks: vec![
                            Block::Thinking {
                                text: "safe reasoning".into(),
                                provider_kind: Some("private-provider".into()),
                                raw: Some(json!({"encrypted": "provider-secret"})),
                            },
                            Block::ToolUse {
                                id: "call-1".into(),
                                name: "read_file".into(),
                                input: json!({"path": "src/main.rs", "private_nested": {"secret": true}}),
                            },
                        ],
                    },
                    None,
                ),
            ),
            entry(
                "result",
                SessionEntryPayload::message(
                    ChatMessage {
                        role: Role::User,
                        blocks: vec![Block::ToolResult {
                            tool_use_id: "call-1".into(),
                            content: "file output".into(),
                            is_error: false,
                        }],
                    },
                    None,
                ),
            ),
        ];

        let projected = project_transcript(&entries);
        let encoded = serde_json::to_string(&projected).unwrap();
        let value = serde_json::to_value(projected).unwrap();
        assert!(encoded.contains("safe reasoning"));
        assert!(encoded.contains("path=src/main.rs"));
        assert!(!encoded.contains("provider-secret"));
        assert!(!encoded.contains("private-provider"));
        assert!(!has_key(&value, "input"));
        assert!(!has_key(&value, "raw"));
        assert!(!has_key(&value, "provider_kind"));
    }

    #[test]
    fn snapshot_projection_uses_committed_plan_and_notice_facts() {
        let entries = vec![
            entry(
                "plan",
                SessionEntryPayload::PlanUpdated(PlanSnapshot {
                    revision: 1,
                    items: vec![PlanItem {
                        id: "work".into(),
                        text: "ship it".into(),
                        status: PlanStatus::InProgress,
                    }],
                    explanation: Some("current".into()),
                }),
            ),
            entry(
                "notice",
                SessionEntryPayload::Notice(NoticeRecord {
                    text: "durable notice".into(),
                    level: NoticeLevel::Warning,
                }),
            ),
        ];
        let selection = ActiveModelSelection {
            provider: "test".into(),
            model: "model".into(),
            effort: "medium".into(),
        };
        let snapshot = project_snapshot(SnapshotSource {
            session_id: "session",
            revision: 3,
            workspace: Path::new("workspace"),
            phase: SessionPhase::Idle,
            selection: &selection,
            provider_label: "test / model",
            usage: Usage::default(),
            entries: &entries,
            queues: QueueView::default(),
            pending_approval: None,
        });

        assert_eq!(snapshot.revision, 3);
        assert_eq!(snapshot.plan.revision, 1);
        assert_eq!(snapshot.plan.items[0].status, PlanStatus::InProgress);
        assert!(matches!(
            snapshot.transcript.as_slice(),
            [TranscriptItem::Notice { text, .. }] if text == "durable notice"
        ));
    }

    #[test]
    fn strict_public_objects_reject_unknown_fields() {
        let error = serde_json::from_value::<UsageView>(json!({
            "input_tokens": 1,
            "output_tokens": 2,
            "cache_read_tokens": null,
            "cache_write_tokens": null,
            "unknown": true
        }))
        .unwrap_err();
        assert!(error.to_string().contains("unknown field"));
    }

    #[test]
    fn model_metadata_contains_no_provider_configuration() {
        let catalog = vec![ProviderCatalogEntry {
            name: "provider".into(),
            default_model: "model".into(),
            models: vec![crate::config::ModelCatalogEntry {
                id: "model".into(),
                context_window: Some(100_000),
                max_tokens: Some(8_000),
                efforts: vec!["low".into(), "high".into()],
                default_effort: "low".into(),
                sends_effort: true,
            }],
        }];

        assert_eq!(
            model_metadata(&catalog),
            vec![ModelMetadata {
                provider: "provider".into(),
                model: "model".into(),
                label: "provider / model".into(),
                supported_efforts: vec!["low".into(), "high".into()],
                default_effort: "low".into(),
            }]
        );
    }

    fn has_key(value: &serde_json::Value, wanted: &str) -> bool {
        match value {
            serde_json::Value::Object(map) => {
                map.contains_key(wanted) || map.values().any(|value| has_key(value, wanted))
            }
            serde_json::Value::Array(values) => values.iter().any(|value| has_key(value, wanted)),
            _ => false,
        }
    }
}
