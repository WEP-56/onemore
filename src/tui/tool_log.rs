use std::collections::VecDeque;
use std::time::Instant;

use crate::sdk::{CommandErrorView, ToolMetadataView, ToolOutputView, ToolStatus, TranscriptItem};

const MAX_TOOL_RECORDS: usize = 64;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolRunStatus {
    Running,
    Succeeded,
    Failed,
}

#[derive(Debug, Clone)]
pub struct ToolRecord {
    pub id: String,
    pub name: String,
    pub invocation_summary: String,
    pub output: Option<ToolOutputView>,
    pub error: Option<String>,
    pub status: ToolRunStatus,
    started_at: Instant,
    measured_elapsed_ms: Option<u64>,
}

impl ToolRecord {
    pub fn elapsed_ms(&self) -> u64 {
        self.output
            .as_ref()
            .and_then(|output| output.metadata.elapsed_ms)
            .or(self.measured_elapsed_ms)
            .unwrap_or_else(|| elapsed_ms(self.started_at))
    }
}

#[derive(Debug, Default)]
pub struct ToolLog {
    records: VecDeque<ToolRecord>,
}

impl ToolLog {
    pub fn clear(&mut self) {
        self.records.clear();
    }

    pub fn is_empty(&self) -> bool {
        self.records.is_empty()
    }

    pub fn len(&self) -> usize {
        self.records.len()
    }

    pub fn start(&mut self, id: String, name: String, invocation_summary: String) {
        if let Some(record) = self.records.iter_mut().find(|record| record.id == id) {
            record.name = name;
            record.invocation_summary = invocation_summary;
            record.output = None;
            record.error = None;
            record.status = ToolRunStatus::Running;
            record.started_at = Instant::now();
            record.measured_elapsed_ms = None;
            return;
        }

        self.records.push_back(ToolRecord {
            id,
            name,
            invocation_summary,
            output: None,
            error: None,
            status: ToolRunStatus::Running,
            started_at: Instant::now(),
            measured_elapsed_ms: None,
        });
        while self.records.len() > MAX_TOOL_RECORDS {
            self.records.pop_front();
        }
    }

    pub fn update(&mut self, id: String, name: String, output: ToolOutputView) {
        let record = self.ensure_record(id, name);
        record.output = Some(merge_output(record.output.take(), output));
    }

    pub fn finish(
        &mut self,
        id: String,
        name: String,
        output: ToolOutputView,
        error: Option<&CommandErrorView>,
    ) {
        let record = self.ensure_record(id, name);
        record.measured_elapsed_ms = Some(elapsed_ms(record.started_at));
        record.output = Some(merge_output(record.output.take(), output));
        record.error = error.map(|error| error.message.clone());
        record.status = if error.is_some() {
            ToolRunStatus::Failed
        } else {
            ToolRunStatus::Succeeded
        };
    }

    pub fn recent(&self, recent_index: usize) -> Option<&ToolRecord> {
        self.records.iter().rev().nth(recent_index)
    }

    pub fn get(&self, id: &str) -> Option<&ToolRecord> {
        self.records.iter().find(|record| record.id == id)
    }

    pub fn find_id(&self, query: &str) -> Option<String> {
        self.records
            .iter()
            .rev()
            .find(|record| record.id == query || record.id.starts_with(query))
            .map(|record| record.id.clone())
    }

    pub fn restore(&mut self, transcript: &[TranscriptItem]) {
        self.clear();
        for item in transcript {
            let TranscriptItem::Tool {
                tool_call_id,
                name,
                summary,
                status,
                output,
            } = item
            else {
                continue;
            };
            self.start(tool_call_id.clone(), name.clone(), summary.clone());
            let record = self
                .records
                .back_mut()
                .expect("restored tool was just inserted");
            record.output = output.as_ref().map(|content| ToolOutputView {
                content: content.clone(),
                summary: String::new(),
                metadata: ToolMetadataView::default(),
            });
            record.status = match status {
                ToolStatus::Succeeded => ToolRunStatus::Succeeded,
                ToolStatus::Failed => ToolRunStatus::Failed,
            };
            record.measured_elapsed_ms = Some(0);
        }
    }

    fn ensure_record(&mut self, id: String, name: String) -> &mut ToolRecord {
        if let Some(index) = self.records.iter().position(|record| record.id == id) {
            return &mut self.records[index];
        }
        self.start(id.clone(), name, String::new());
        self.records
            .iter_mut()
            .find(|record| record.id == id)
            .expect("tool record was just inserted")
    }
}

fn elapsed_ms(started_at: Instant) -> u64 {
    u64::try_from(started_at.elapsed().as_millis()).unwrap_or(u64::MAX)
}

fn merge_output(previous: Option<ToolOutputView>, mut next: ToolOutputView) -> ToolOutputView {
    let Some(previous) = previous else {
        return next;
    };
    next.metadata.command = next.metadata.command.or(previous.metadata.command);
    next.metadata.cwd = next.metadata.cwd.or(previous.metadata.cwd);
    next.metadata.elapsed_ms = next.metadata.elapsed_ms.or(previous.metadata.elapsed_ms);
    next.metadata.exit_code = next.metadata.exit_code.or(previous.metadata.exit_code);
    next
}

#[cfg(test)]
mod tests {
    use super::*;

    fn output(content: &str, summary: &str, elapsed_ms: Option<u64>) -> ToolOutputView {
        ToolOutputView {
            content: content.into(),
            summary: summary.into(),
            metadata: ToolMetadataView {
                elapsed_ms,
                ..ToolMetadataView::default()
            },
        }
    }

    #[test]
    fn concurrent_updates_are_matched_by_stable_id() {
        let mut log = ToolLog::default();
        log.start("a".into(), "first".into(), "one".into());
        log.start("b".into(), "second".into(), "two".into());
        log.update("a".into(), "first".into(), output("half", "1/2", Some(20)));
        log.finish(
            "b".into(),
            "second".into(),
            output("done b", "done", Some(10)),
            None,
        );
        log.finish(
            "a".into(),
            "first".into(),
            output("done a", "done", None),
            None,
        );

        assert_eq!(
            log.get("a").unwrap().output.as_ref().unwrap().content,
            "done a"
        );
        assert_eq!(log.get("a").unwrap().elapsed_ms(), 20);
        assert_eq!(
            log.get("b").unwrap().output.as_ref().unwrap().content,
            "done b"
        );
        assert_eq!(log.recent(0).unwrap().id, "b");
    }

    #[test]
    fn log_has_a_fixed_memory_record_limit() {
        let mut log = ToolLog::default();
        for index in 0..(MAX_TOOL_RECORDS + 3) {
            log.start(index.to_string(), "tool".into(), String::new());
        }
        assert_eq!(log.len(), MAX_TOOL_RECORDS);
        assert!(log.get("0").is_none());
        assert_eq!(
            log.recent(0).unwrap().id,
            (MAX_TOOL_RECORDS + 2).to_string()
        );
    }
}
