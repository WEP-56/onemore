//! Conversation compaction shared by the manual command and automatic trigger.

use std::sync::atomic::{AtomicBool, Ordering};

use crate::agent_loop::{call_model, ModelCallResult, RetryPolicy};
use crate::compaction::{prepare_compaction, CompactionPreparation, CompactionSettings};
use crate::context::budget::{estimate_tokens, ContextBudget};
use crate::context::PromptContext;
use crate::event::AgentEvent;
use crate::harness::SessionBackend;
use crate::message::{Block, ChatMessage, Role, Usage};
use crate::plan::{compaction_summary as plan_compaction_summary, reduce_plan};
use crate::provider::Provider;
use crate::session::{project_model_messages, CompactionRecord, SessionEntry, SessionEntryPayload};
use crate::util;

use super::Agent;

/// 压缩会话时喂给模型的系统提示。
const COMPACTION_SYSTEM_PROMPT: &str = "你是会话压缩器。把给出的完整对话压缩成一段可以\
替代旧历史的摘要,必须保留:用户的目标与约束、已完成/未完成的事项、关键文件路径与\
修改内容、重要的命令输出结论、当前待办。省略寒暄与失败后已被纠正的探索。直接输出\
摘要正文,不要加任何前后缀。";

pub(super) enum CompactionOutcome {
    NothingToCompact,
    Committed {
        tokens_before: u64,
        summary_chars: usize,
        retained_messages: usize,
    },
    Cancelled,
    Failed(String),
}

/// The single production path for generating and atomically appending a
/// compaction fact. Durable failure never advances the in-memory fact mirror.
pub(super) struct CompactionRuntime<'a> {
    provider: &'a dyn Provider,
    budget: ContextBudget,
    settings: CompactionSettings,
    retry_policy: RetryPolicy,
    entries: &'a mut Vec<SessionEntry>,
    sessions: &'a mut dyn SessionBackend,
    usage_total: &'a mut Usage,
}

impl<'a> CompactionRuntime<'a> {
    pub(super) fn new(
        provider: &'a dyn Provider,
        budget: ContextBudget,
        settings: CompactionSettings,
        retry_policy: RetryPolicy,
        entries: &'a mut Vec<SessionEntry>,
        sessions: &'a mut dyn SessionBackend,
        usage_total: &'a mut Usage,
    ) -> Self {
        CompactionRuntime {
            provider,
            budget,
            settings,
            retry_policy,
            entries,
            sessions,
            usage_total,
        }
    }

    pub(super) fn compact_if_needed(
        &mut self,
        system_chars: u64,
        tools_chars: u64,
        emit: &mut dyn FnMut(AgentEvent),
        cancel: &AtomicBool,
    ) -> anyhow::Result<Option<crate::session::ModelProjection>> {
        let projection = project_model_messages(self.entries);
        let estimated_tokens = estimate_tokens(system_chars, tools_chars, &projection);
        if !self.settings.should_compact(&self.budget, estimated_tokens) {
            return Ok(None);
        }
        let Some(prepared) = self.prepare(&projection, false) else {
            return Ok(None);
        };
        emit(AgentEvent::Notice(format!(
            "上下文估算约 {estimated_tokens} tokens,已达到自动压缩阈值"
        )));
        match self.compact_prepared(projection, prepared, emit, cancel) {
            CompactionOutcome::NothingToCompact => Ok(None),
            CompactionOutcome::Committed {
                tokens_before,
                summary_chars,
                retained_messages,
            } => {
                emit(AgentEvent::Notice(format!(
                    "已自动压缩历史:压缩前约 {tokens_before} tokens,摘要 {summary_chars} 字符,\
                     保留 {retained_messages} 条最近消息"
                )));
                Ok(Some(project_model_messages(self.entries)))
            }
            CompactionOutcome::Cancelled => {
                cancel.store(true, Ordering::Relaxed);
                anyhow::bail!("自动压缩已取消,历史未变化")
            }
            CompactionOutcome::Failed(error) => {
                anyhow::bail!("自动压缩失败,历史未变化: {error}")
            }
        }
    }

    pub(super) fn compact(
        &mut self,
        emit: &mut dyn FnMut(AgentEvent),
        cancel: &AtomicBool,
    ) -> CompactionOutcome {
        let projection = project_model_messages(self.entries);
        let Some(prepared) = self.prepare(&projection, true) else {
            return CompactionOutcome::NothingToCompact;
        };
        self.compact_prepared(projection, prepared, emit, cancel)
    }

    fn prepare(
        &self,
        projection: &crate::session::ModelProjection,
        include_short_history: bool,
    ) -> Option<CompactionPreparation> {
        prepare_compaction(&projection.messages, self.settings.keep_recent_tokens).or_else(|| {
            (include_short_history && !projection.messages.is_empty()).then(|| {
                CompactionPreparation {
                    messages_to_summarize: projection.messages.clone(),
                    retained_messages: Vec::new(),
                }
            })
        })
    }

    fn compact_prepared(
        &mut self,
        projection: crate::session::ModelProjection,
        prepared: CompactionPreparation,
        emit: &mut dyn FnMut(AgentEvent),
        cancel: &AtomicBool,
    ) -> CompactionOutcome {
        let transcript = render_transcript_for_compaction(&prepared.messages_to_summarize);
        if transcript.is_empty() {
            return CompactionOutcome::NothingToCompact;
        }

        let tokens_before = estimate_tokens(0, 0, &projection);
        let mut request_text = format!(
            "以下是一段完整的对话记录:\n\n{}\n\n请压缩以上对话。直接输出摘要正文。",
            transcript
        );
        // 压缩请求自身也要守预算:超长时折叠中段,保住开头(目标)与结尾(现状)。
        if let Some(available) = self.budget.available_input() {
            let max_chars = (available as usize).saturating_mul(3); // ~4 字符/token,留余量
            request_text = util::truncate_middle(&request_text, max_chars);
        }
        let mut prompt = PromptContext::default();
        prompt
            .system_sections
            .push(COMPACTION_SYSTEM_PROMPT.to_string());
        prompt.messages = vec![ChatMessage::user_text(request_text)];

        let output = match call_model(
            self.provider,
            &prompt,
            &[],
            self.retry_policy,
            false,
            emit,
            cancel,
        ) {
            ModelCallResult::Done(output) => output,
            ModelCallResult::Cancelled(_) => return CompactionOutcome::Cancelled,
            ModelCallResult::Failed(failed) => {
                return CompactionOutcome::Failed(failed.error.to_string())
            }
        };

        let summary = output.message.text().trim().to_string();
        self.usage_total.add(output.usage);
        emit(AgentEvent::Usage {
            input_tokens: self.usage_total.input_tokens,
            output_tokens: self.usage_total.output_tokens,
            cache: self.usage_total.cache,
        });
        if summary.is_empty() {
            return CompactionOutcome::Failed("模型返回了空摘要".into());
        }

        let summary_chars = summary.chars().count();
        let mut persisted_summary = summary;
        if let Some(plan_summary) = plan_compaction_summary(&reduce_plan(self.entries).snapshot) {
            persisted_summary.push_str("\n\n");
            persisted_summary.push_str(&plan_summary);
        }
        let retained_messages = prepared.retained_messages.len();
        let payload = SessionEntryPayload::Compaction(CompactionRecord {
            summary: persisted_summary,
            tokens_before,
            retained_messages: prepared.retained_messages,
        });
        match self
            .sessions
            .append_payloads(vec![payload], *self.usage_total)
        {
            Ok(mut appended) => self.entries.append(&mut appended),
            Err(error) => {
                return CompactionOutcome::Failed(format!(
                    "保存会话失败,压缩事实未写入,内存历史未推进: {error:#}"
                ))
            }
        }

        CompactionOutcome::Committed {
            tokens_before,
            summary_chars,
            retained_messages,
        }
    }
}

impl Agent {
    /// /compact:调模型生成摘要,追加 Compaction 事实。
    /// 事实条数只增不减;此后模型视图从摘要和保留尾部开始,旧事实仍在日志与 UI 中。
    pub(super) fn compact(&mut self, emit: &mut dyn FnMut(AgentEvent), cancel: &AtomicBool) {
        if project_model_messages(&self.entries).messages.is_empty() {
            emit(AgentEvent::Notice("当前会话没有可压缩的历史".into()));
            return;
        }
        emit(AgentEvent::TurnStarted);
        let outcome = CompactionRuntime::new(
            self.provider.as_ref(),
            self.budget,
            self.compaction_settings,
            self.retry_policy,
            &mut self.entries,
            self.sessions.as_mut(),
            &mut self.usage_total,
        )
        .compact(emit, cancel);
        match outcome {
            CompactionOutcome::NothingToCompact => {
                emit(AgentEvent::Notice("当前会话没有可压缩的历史".into()));
                emit(AgentEvent::TurnFinished { cancelled: false });
            }
            CompactionOutcome::Committed {
                tokens_before,
                summary_chars,
                retained_messages,
            } => {
                emit(AgentEvent::Notice(format!(
                    "历史已压缩:压缩前估算约 {tokens_before} tokens,摘要 {summary_chars} 字符,\
                     保留 {retained_messages} 条最近消息。事实日志保留全部原始记录。"
                )));
                emit(AgentEvent::TurnFinished { cancelled: false });
            }
            CompactionOutcome::Cancelled => {
                emit(AgentEvent::Notice("压缩已取消,历史未变化".into()));
                emit(AgentEvent::TurnFinished { cancelled: true });
            }
            CompactionOutcome::Failed(error) => {
                emit(AgentEvent::Error(format!("压缩失败,历史未变化: {error}")));
                emit(AgentEvent::TurnFinished { cancelled: false });
            }
        }
    }
}

/// 把模型视图渲染成纯文本对话记录(压缩请求专用)。
/// 工具调用/结果转成文字描述,思考过程与厂商 reasoning 原始数据一律丢弃。
fn render_transcript_for_compaction(messages: &[ChatMessage]) -> String {
    const TOOL_RESULT_CHARS: usize = 1_500;
    let mut out = String::new();
    for message in messages {
        let label = match message.role {
            Role::User => "用户",
            Role::Assistant => "助手",
        };
        let mut body = String::new();
        for block in &message.blocks {
            match block {
                Block::Text(text) if !text.trim().is_empty() => {
                    body.push_str(text.trim_end());
                    body.push('\n');
                }
                Block::Text(_) | Block::Thinking { .. } => {}
                Block::ToolUse { name, input, .. } => {
                    body.push_str(&format!(
                        "[调用工具 {}({})]\n",
                        name,
                        util::args_summary(input)
                    ));
                }
                Block::ToolResult {
                    content, is_error, ..
                } => {
                    body.push_str(if *is_error {
                        "[工具失败] "
                    } else {
                        "[工具结果] "
                    });
                    body.push_str(&util::truncate_middle(content, TOOL_RESULT_CHARS));
                    body.push('\n');
                }
            }
        }
        let body = body.trim_end();
        if body.is_empty() {
            continue;
        }
        out.push_str(label);
        out.push_str(":\n");
        out.push_str(body);
        out.push_str("\n\n");
    }
    out.trim_end().to_string()
}
