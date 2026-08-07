//! Compaction policy and deterministic model-view preparation.

use crate::context::budget::ContextBudget;
use crate::message::{Block, ChatMessage};
use crate::session::message_chars;

pub(crate) const DEFAULT_RESERVE_TOKENS: u64 = 16_384;
pub(crate) const DEFAULT_KEEP_RECENT_TOKENS: u64 = 20_000;

/// Automatic compaction threshold and recent-context retention policy.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CompactionSettings {
    /// Whether requests may trigger compaction before the hard context limit.
    pub enabled: bool,
    /// Additional input headroom kept below the normal request budget.
    pub reserve_tokens: u64,
    /// Approximate recent message tokens retained verbatim after compaction.
    pub keep_recent_tokens: u64,
}

impl Default for CompactionSettings {
    fn default() -> Self {
        CompactionSettings {
            enabled: true,
            reserve_tokens: DEFAULT_RESERVE_TOKENS,
            keep_recent_tokens: DEFAULT_KEEP_RECENT_TOKENS,
        }
    }
}

impl CompactionSettings {
    pub(crate) fn validate(self) -> anyhow::Result<()> {
        if self.reserve_tokens == 0 {
            anyhow::bail!("compaction.reserve_tokens 必须大于 0");
        }
        if self.keep_recent_tokens == 0 {
            anyhow::bail!("compaction.keep_recent_tokens 必须大于 0");
        }
        Ok(())
    }

    pub(crate) fn should_compact(self, budget: &ContextBudget, estimated_tokens: u64) -> bool {
        if !self.enabled {
            return false;
        }
        let Some(available_input) = budget.available_input() else {
            return false;
        };
        let threshold = available_input.saturating_sub(self.reserve_tokens).max(1);
        estimated_tokens > threshold
    }
}

pub(crate) struct CompactionPreparation {
    pub messages_to_summarize: Vec<ChatMessage>,
    pub retained_messages: Vec<ChatMessage>,
}

/// Split one model projection without leaving a ToolResult at the start of the
/// retained suffix. Returns `None` when the full history is smaller than the
/// retention target; the manual command may explicitly override that no-op.
pub(crate) fn prepare_compaction(
    messages: &[ChatMessage],
    keep_recent_tokens: u64,
) -> Option<CompactionPreparation> {
    if messages.is_empty() {
        return None;
    }

    let mut accumulated = 0u64;
    let mut candidate = None;
    for index in (0..messages.len()).rev() {
        accumulated = accumulated.saturating_add(message_tokens(&messages[index]));
        if accumulated >= keep_recent_tokens {
            candidate = Some(index);
            break;
        }
    }

    let candidate = candidate?;
    let mut cut = (candidate..messages.len())
        .find(|candidate| safe_retained_start(&messages[*candidate]))
        .unwrap_or(messages.len());
    if cut == 0 {
        cut = messages.len();
    }

    Some(CompactionPreparation {
        messages_to_summarize: messages[..cut].to_vec(),
        retained_messages: messages[cut..].to_vec(),
    })
}

fn safe_retained_start(message: &ChatMessage) -> bool {
    !message
        .blocks
        .iter()
        .any(|block| matches!(block, Block::ToolResult { .. }))
}

fn message_tokens(message: &ChatMessage) -> u64 {
    message_chars(message) / 4 + 1
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::message::Role;

    #[test]
    fn automatic_threshold_is_deterministic_and_can_be_disabled() {
        let budget = ContextBudget {
            context_window: Some(1_000),
            reserve_output: 100,
        };
        let settings = CompactionSettings {
            enabled: true,
            reserve_tokens: 100,
            keep_recent_tokens: 10,
        };
        assert!(!settings.should_compact(&budget, 800));
        assert!(settings.should_compact(&budget, 801));
        assert!(!CompactionSettings {
            enabled: false,
            ..settings
        }
        .should_compact(&budget, 900));
    }

    #[test]
    fn split_keeps_tool_roundtrip_on_one_side() {
        let messages = vec![
            ChatMessage::user_text("old request"),
            ChatMessage {
                role: Role::Assistant,
                blocks: vec![Block::ToolUse {
                    id: "call-1".into(),
                    name: "read_file".into(),
                    input: serde_json::json!({"path": "large.txt"}),
                }],
            },
            ChatMessage {
                role: Role::User,
                blocks: vec![Block::ToolResult {
                    tool_use_id: "call-1".into(),
                    content: "x".repeat(4_000),
                    is_error: false,
                }],
            },
            ChatMessage {
                role: Role::Assistant,
                blocks: vec![Block::Text("recent answer".into())],
            },
        ];

        let prepared = prepare_compaction(&messages, 100).unwrap();
        assert_eq!(prepared.messages_to_summarize.len(), 3);
        assert_eq!(prepared.retained_messages.len(), 1);
        assert_eq!(prepared.retained_messages[0].text(), "recent answer");
    }

    #[test]
    fn short_history_has_no_automatic_compaction_prefix() {
        let messages = vec![ChatMessage::user_text("small history")];
        assert!(prepare_compaction(&messages, 20_000).is_none());
    }
}
