use std::collections::BTreeMap;
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{bail, Result};

use crate::message::{Block, Role, Usage};
use crate::plan::validate_plan_append;
use crate::session::{
    validate_new_message_batch, SessionEntry, SessionEntryPayload, SessionSummary,
};

use super::{ModelPreferences, SessionBackend};

/// One-session, append-only backend for embedded and test runtimes.
pub struct MemorySessionBackend {
    id: String,
    title: String,
    entries: Vec<SessionEntry>,
    usage: Usage,
    updated_at: i64,
}

impl MemorySessionBackend {
    pub fn new() -> Self {
        MemorySessionBackend {
            id: uuid::Uuid::new_v4().to_string(),
            title: String::new(),
            entries: Vec::new(),
            usage: Usage::default(),
            updated_at: unix_timestamp(),
        }
    }
}

impl Default for MemorySessionBackend {
    fn default() -> Self {
        Self::new()
    }
}

impl SessionBackend for MemorySessionBackend {
    fn current_id(&self) -> &str {
        &self.id
    }

    fn append_payloads(
        &mut self,
        payloads: Vec<SessionEntryPayload>,
        usage: Usage,
    ) -> Result<Vec<SessionEntry>> {
        if payloads.is_empty() {
            return Ok(Vec::new());
        }
        validate_new_message_batch(&payloads).map_err(anyhow::Error::msg)?;
        validate_plan_append(&self.entries, &payloads)
            .map_err(|error| anyhow::Error::msg(error.message))?;

        let now = unix_timestamp();
        let mut parent_id = self.entries.last().map(|entry| entry.id.clone());
        let mut appended = Vec::with_capacity(payloads.len());
        for payload in payloads {
            let entry = SessionEntry {
                id: uuid::Uuid::new_v4().to_string(),
                parent_id: parent_id.clone(),
                created_at: now,
                payload,
            };
            parent_id = Some(entry.id.clone());
            appended.push(entry);
        }

        if self.title.is_empty() {
            self.title = first_user_text(&appended);
        }
        self.entries.extend(appended.iter().cloned());
        self.usage = usage;
        self.updated_at = now;
        Ok(appended)
    }

    fn clear(&mut self) -> Result<()> {
        self.title.clear();
        self.entries.clear();
        self.usage = Usage::default();
        self.updated_at = unix_timestamp();
        Ok(())
    }

    fn list(&self) -> Result<Vec<SessionSummary>> {
        Ok(vec![SessionSummary {
            id: self.id.clone(),
            title: self.title.clone(),
            message_count: self
                .entries
                .iter()
                .filter(|entry| matches!(entry.payload, SessionEntryPayload::Message(_)))
                .count(),
            updated_at: self.updated_at,
        }])
    }

    fn load(&mut self, requested_id: &str) -> Result<(Vec<SessionEntry>, Usage)> {
        let requested_id = requested_id.trim();
        if requested_id.is_empty() || !self.id.starts_with(requested_id) {
            bail!("当前内存 backend 找不到会话 {}", requested_id);
        }
        Ok((self.entries.clone(), self.usage))
    }
}

/// Non-persistent reasoning preferences with the same default-elision rules as
/// the JSON-backed CLI implementation.
#[derive(Default)]
pub struct MemoryModelPreferences {
    reasoning_efforts: BTreeMap<String, BTreeMap<String, String>>,
}

impl ModelPreferences for MemoryModelPreferences {
    fn effort(&self, provider: &str, model: &str) -> Option<&str> {
        self.reasoning_efforts
            .get(provider)
            .and_then(|models| models.get(model))
            .map(String::as_str)
    }

    fn reasoning_efforts(&self) -> BTreeMap<String, BTreeMap<String, String>> {
        self.reasoning_efforts.clone()
    }

    fn set_effort(
        &mut self,
        provider: &str,
        model: &str,
        effort: &str,
        default_effort: &str,
    ) -> Result<()> {
        if effort == default_effort {
            if let Some(models) = self.reasoning_efforts.get_mut(provider) {
                models.remove(model);
                if models.is_empty() {
                    self.reasoning_efforts.remove(provider);
                }
            }
        } else {
            self.reasoning_efforts
                .entry(provider.to_string())
                .or_default()
                .insert(model.to_string(), effort.to_string());
        }
        Ok(())
    }
}

fn first_user_text(entries: &[SessionEntry]) -> String {
    entries
        .iter()
        .filter_map(|entry| match &entry.payload {
            SessionEntryPayload::Message(record) if record.message.role == Role::User => {
                record.message.blocks.iter().find_map(|block| match block {
                    Block::Text(text) if !text.trim().is_empty() => Some(text.trim()),
                    _ => None,
                })
            }
            _ => None,
        })
        .next()
        .unwrap_or("")
        .chars()
        .take(80)
        .collect()
}

fn unix_timestamp() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::message::ChatMessage;

    #[test]
    fn memory_backend_rejects_half_tool_batches_without_advancing() {
        let mut backend = MemorySessionBackend::new();
        let payload = SessionEntryPayload::message(
            crate::message::ChatMessage {
                role: Role::Assistant,
                blocks: vec![Block::ToolUse {
                    id: "call-1".into(),
                    name: "read_file".into(),
                    input: serde_json::json!({ "path": "x" }),
                }],
            },
            None,
        );

        assert!(backend
            .append_payloads(vec![payload], Usage::default())
            .is_err());
        let id = backend.current_id().to_string();
        assert!(backend.load(&id).unwrap().0.is_empty());
    }

    #[test]
    fn memory_backend_lists_and_clears_committed_facts() {
        let mut backend = MemorySessionBackend::new();
        backend
            .append_payloads(
                vec![SessionEntryPayload::message(
                    ChatMessage::user_text("hello"),
                    None,
                )],
                Usage::default(),
            )
            .unwrap();

        let summary = backend.list().unwrap().pop().unwrap();
        assert_eq!(summary.title, "hello");
        assert_eq!(summary.message_count, 1);
        backend.clear().unwrap();
        let id = backend.current_id().to_string();
        assert!(backend.load(&id).unwrap().0.is_empty());
    }
}
