//! Conversation compaction as an optional runtime command.

use std::sync::atomic::AtomicBool;

use crate::agent_loop::{call_model, ModelCallResult};
use crate::context::budget::estimate_tokens;
use crate::context::PromptContext;
use crate::event::AgentEvent;
use crate::message::{Block, ChatMessage, Role};
use crate::plan::{compaction_summary as plan_compaction_summary, reduce_plan};
use crate::session::{project_model_messages, CompactionRecord, SessionEntryPayload};
use crate::util;

use super::Agent;

/// 压缩会话时喂给模型的系统提示。
const COMPACTION_SYSTEM_PROMPT: &str = "你是会话压缩器。把给出的完整对话压缩成一段可以\
替代旧历史的摘要,必须保留:用户的目标与约束、已完成/未完成的事项、关键文件路径与\
修改内容、重要的命令输出结论、当前待办。省略寒暄与失败后已被纠正的探索。直接输出\
摘要正文,不要加任何前后缀。";

impl Agent {
    /// /compact:调模型生成摘要,追加 Compaction 事实。
    /// 事实条数只增不减;此后模型视图从摘要开始,旧事实仍在日志与 UI 中。
    ///
    /// 压缩请求**不复用结构化历史**,而是把模型视图渲染成纯文本对话记录、
    /// 单条 user 消息发出:摘要调用声明零工具,但历史里带着
    /// ToolUse/ToolResult 块与厂商 reasoning 回传项——这种"有 tool 块却无
    /// tools 声明"的请求形状在 Anthropic 上是显式 400,在 OpenAI 兼容后端/
    /// 网关上也常被拒(表现为 502)。对话在这里是被摘要的**数据**,不是要
    /// 续写的上下文,纯文本才是对两种 API 都合法的形状。
    pub(super) fn compact(&mut self, emit: &mut dyn FnMut(AgentEvent), cancel: &AtomicBool) {
        let projection = project_model_messages(&self.entries);
        let transcript = render_transcript_for_compaction(&projection.messages);
        if transcript.is_empty() {
            emit(AgentEvent::Notice("当前会话没有可压缩的历史".into()));
            return;
        }
        emit(AgentEvent::TurnStarted);
        let tokens_before = estimate_tokens(0, 0, &projection);
        let mut request_text = format!(
            "以下是一段完整的对话记录:\n\n{}\n\n请压缩以上对话。直接输出摘要正文。",
            transcript
        );
        // 压缩请求自身也要守预算:超长时折叠中段,保住开头(目标)与结尾(现状)。
        if let Some(window) = self.budget.context_window {
            let available = window.saturating_sub(self.budget.reserve_output).max(1);
            let max_chars = (available as usize).saturating_mul(3); // ~4 字符/token,留余量
            request_text = util::truncate_middle(&request_text, max_chars);
        }
        let mut prompt = PromptContext::default();
        prompt
            .system_sections
            .push(COMPACTION_SYSTEM_PROMPT.to_string());
        prompt.messages = vec![ChatMessage::user_text(request_text)];
        // 压缩调用不提供工具、不把流式增量当作助手正文转发(它不是对话内容)。
        match call_model(
            self.provider.as_ref(),
            &prompt,
            &[],
            self.retry_policy,
            false,
            emit,
            cancel,
        ) {
            ModelCallResult::Done(output) => {
                let summary = output.message.text().trim().to_string();
                self.usage_total.add(output.usage);
                emit(AgentEvent::Usage {
                    input_tokens: self.usage_total.input_tokens,
                    output_tokens: self.usage_total.output_tokens,
                    cache: self.usage_total.cache,
                });
                if summary.is_empty() {
                    emit(AgentEvent::Error("压缩失败:模型返回了空摘要".into()));
                    emit(AgentEvent::TurnFinished { cancelled: false });
                    return;
                }
                let mut persisted_summary = summary.clone();
                if let Some(plan_summary) =
                    plan_compaction_summary(&reduce_plan(&self.entries).snapshot)
                {
                    persisted_summary.push_str("\n\n");
                    persisted_summary.push_str(&plan_summary);
                }
                let committed = self.commit(
                    vec![SessionEntryPayload::Compaction(CompactionRecord {
                        summary: persisted_summary,
                        tokens_before,
                    })],
                    emit,
                );
                if committed {
                    emit(AgentEvent::Notice(format!(
                        "历史已压缩:压缩前估算约 {} tokens,摘要 {} 字符。\
                         事实日志保留全部原始记录。",
                        tokens_before,
                        summary.chars().count()
                    )));
                }
                emit(AgentEvent::TurnFinished { cancelled: false });
            }
            ModelCallResult::Cancelled(_) => {
                emit(AgentEvent::Notice("压缩已取消,历史未变化".into()));
                emit(AgentEvent::TurnFinished { cancelled: true });
            }
            ModelCallResult::Failed(failed) => {
                emit(AgentEvent::Error(format!(
                    "压缩失败,历史未变化: {}",
                    failed.error
                )));
                emit(AgentEvent::TurnFinished { cancelled: false });
            }
        }
    }
}

/// 把模型视图渲染成纯文本对话记录(压缩请求专用)。
/// 工具调用/结果转成文字描述,思考过程与厂商 reasoning 原始数据一律丢弃——
/// 它们对摘要没有价值,回传反而会造成跨请求的协议问题。
fn render_transcript_for_compaction(messages: &[ChatMessage]) -> String {
    /// 单个工具结果进入摘要输入的字符上限(保头保尾)。
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
