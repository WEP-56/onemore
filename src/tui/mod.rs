//! TUI 前端:消费 AgentEvent 渲染画面,把用户操作变成 AgentCommand。
//!
//! 结构是最朴素的"单线程事件循环":
//! ```text
//! loop {
//!     把 Runtime 事件通道里攒的事件全部应用到界面状态;
//!     poll 终端按键(33ms 超时,顺带当渲染节拍);
//!     有变化才重绘;
//! }
//! ```
//! Runtime 在另一个线程里跑,阻塞的网络/工具调用不会卡住画面。
//!
//! Windows 特有的坑,这里都处理了:
//! - crossterm 在 Windows 会同时上报按键的 Press/Release,必须只认 Press,
//!   否则每个字都打两遍;
//! - 在 conpty 终端(Windows Terminal / VS Code)里,一次按键的
//!   Press+Release 是**同一瞬间**被合成出来的,队列里"有积压"不代表
//!   在粘贴。识别粘贴必须把积压事件取出来看内容(见 `enter_means_newline`),
//!   否则 Enter 永远发不出消息;
//! - 传统控制台没有括号粘贴(bracketed paste),多行粘贴会变成一串
//!   带 Enter 的按键;同样靠上面的内容检查兜底;
//! - 和 Codex 一样使用 inline viewport,已完成的消息写入终端原生 scrollback。
//!   因此不需要鼠标捕获:滚轮滚动终端历史,普通拖动直接选择文本。

mod command;
mod input;
mod picker;
mod tool_log;
mod transcript;

#[cfg(test)]
use std::collections::HashMap;
use std::collections::{BTreeMap, VecDeque};
use std::time::{Duration, Instant};

use ratatui::crossterm::event::{
    self, DisableBracketedPaste, EnableBracketedPaste, Event, KeyCode, KeyEventKind, KeyModifiers,
};
use ratatui::crossterm::execute;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph, Widget, Wrap};
use ratatui::{Frame, TerminalOptions, Viewport};
use unicode_width::UnicodeWidthStr;

use crate::config::{
    ActiveModelSelection, ModelCatalogEntry, ProviderCatalogEntry, DEFAULT_REASONING_EFFORT,
};
use crate::event::AgentCommand;
#[cfg(test)]
use crate::event::AgentEvent;
use crate::message::Usage;
#[cfg(test)]
use crate::message::{Block as MessageBlock, ChatMessage, Role};
use crate::permission::{ApprovalDecision, ApprovalRequest, ApprovalResponse, ApprovalScope};
#[cfg(test)]
use crate::plan::reduce_plan;
use crate::plan::{PlanItem, PlanSnapshot, PlanStatus};
use crate::sdk::{
    AgentSession, CompactionTriggerView, ProgressEvent, SessionController, SessionEvent,
    SessionEvents, SessionPhase, SkillMetadataView, TranscriptItem,
};
#[cfg(test)]
use crate::session::{SessionEntry, SessionEntryPayload};
use crate::util;
use input::InputBox;
use picker::{Picker, PickerItem};
use tool_log::{ToolLog, ToolRecord, ToolRunStatus};
use transcript::Transcript;

const SPINNER: &[&str] = &["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];
/// 输入区内容最多显示的行数(超出滚动)。
const INPUT_MAX_ROWS: usize = 6;
/// Inline viewport 只承载 live 内容和 composer；已完成消息在原生 scrollback 中。
/// 小窗口保持紧凑，大窗口给流式思考、正文和工具状态更多可见空间。
const MIN_INLINE_VIEWPORT_ROWS: u16 = 8;
const MAX_INLINE_VIEWPORT_ROWS: u16 = 16;
const HELP_TEXT: &str = "斜杠命令\n\
  /model             选择当前 provider 的模型与思考程度\n\
  /reasoning         调整当前模型的思考程度\n\
  /provider          选择 provider(对话历史保留)\n\
  /session [ID|all]  列出或恢复历史会话；all 显示其他 workspace\n\
  /skill [名称]       选择并加载一个本地技能\n\
  /tools [调用 ID]   浏览工具调用，或按 ID 打开完整输出\n\
  /mcp               查看 MCP server 状态(era、工具数、故障)\n\
  /compact           压缩历史(摘要替代模型视图,事实保留)\n\
  /queue <内容>      排队后续任务(当前任务结束后执行)\n\
  /reload            重新加载配置、项目指令、skills 与 MCP servers\n\
  /clear             清空会话\n\
  /quit              退出\n\
\n\
运行中输入并回车 = steering:在当前一批工具完成后注入,修正方向;Esc 取消当前轮\n\
编辑: Ctrl+A/E 行首/行尾 · Ctrl+W 删前一词 · Ctrl+K 删到行尾 · Alt+←/→ 按词移动\n\
操作: ↑/↓ 浏览候选或历史 · Tab 补全 · Ctrl+T 最近工具 · Alt+P 计划 · Esc 关闭/取消 · 滚轮浏览终端历史 · 鼠标拖动选择文本";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PickerKind {
    Provider,
    Model,
    Reasoning,
    Session,
    Skill,
}

#[derive(Debug)]
enum Overlay {
    Picker {
        kind: PickerKind,
        picker: Picker,
    },
    Loading {
        kind: PickerKind,
        title: String,
    },
    Approval {
        request: ApprovalRequest,
        selected: usize,
    },
    ToolList {
        selected: usize,
    },
    ToolDetail {
        tool_call_id: String,
        scroll: usize,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ApprovalChoice {
    Once,
    Session,
    Deny,
}

#[derive(Debug)]
struct PendingReasoningSelection {
    model: String,
    effort_only: bool,
}

pub fn run(session: AgentSession) -> anyhow::Result<()> {
    let (_, terminal_rows) = ratatui::crossterm::terminal::size()?;
    let viewport_height = inline_viewport_rows(terminal_rows);
    let mut terminal = ratatui::try_init_with_options(TerminalOptions {
        viewport: Viewport::Inline(viewport_height),
    })?;
    let _ = execute!(std::io::stdout(), EnableBracketedPaste);

    let mut app = App::from_session(session)?;
    app.transcript.push_notice(format!(
        "Onemore 已就绪({}) · 会话 {},输入内容开始对话,/help 查看命令",
        app.provider_label,
        short_id(&app.session_id)
    ));

    let result = app.event_loop(&mut terminal);

    let _ = execute!(std::io::stdout(), DisableBracketedPaste);
    ratatui::restore();
    result
}

fn inline_viewport_rows(terminal_rows: u16) -> u16 {
    (terminal_rows / 2)
        .clamp(MIN_INLINE_VIEWPORT_ROWS, MAX_INLINE_VIEWPORT_ROWS)
        .min(terminal_rows.max(1))
}

fn plan_snapshot_from_view(plan: crate::sdk::PlanView) -> PlanSnapshot {
    PlanSnapshot {
        revision: plan.revision,
        items: plan
            .items
            .into_iter()
            .map(|item| PlanItem {
                id: item.id,
                text: item.text,
                status: item.status,
            })
            .collect(),
        explanation: plan.explanation,
    }
}

trait UiRuntime {
    fn submit(&self, command: AgentCommand);
    fn cancel(&self);
    fn shutdown(&self);
    fn approve(&self, response: ApprovalResponse);
}

impl UiRuntime for SessionController {
    fn submit(&self, command: AgentCommand) {
        // Admission may wait for a model checkpoint; the UI thread must keep draining events.
        let controller = self.clone();
        let _ = std::thread::Builder::new()
            .name("tui-command-admission".into())
            .spawn(move || {
                let _ = controller.submit_raw(command);
            });
    }

    fn cancel(&self) {
        self.cancel_now();
    }

    fn shutdown(&self) {
        self.cancel_now();
        let controller = self.clone();
        let _ = std::thread::Builder::new()
            .name("tui-shutdown".into())
            .spawn(move || {
                let _ = controller.send_detached(AgentCommand::Shutdown);
            });
    }

    fn approve(&self, response: ApprovalResponse) {
        let decision = match response.decision {
            ApprovalDecision::Allow(ApprovalScope::Once) => {
                crate::sdk::ApprovalDecisionView::AllowOnce
            }
            ApprovalDecision::Allow(ApprovalScope::Session) => {
                crate::sdk::ApprovalDecisionView::AllowSession
            }
            ApprovalDecision::Deny => crate::sdk::ApprovalDecisionView::Deny,
        };
        let _ = self.respond_to_approval(crate::sdk::ApprovalResponseView {
            request_id: response.request_id,
            decision,
        });
    }
}

struct App {
    runtime: Box<dyn UiRuntime>,
    events: Option<SessionEvents>,
    transcript: Transcript,
    input: InputBox,
    overlay: Option<Overlay>,
    slash_selected: usize,
    slash_dismissed: Option<String>,

    /// 已从终端读出、还没处理的事件(Enter 的粘贴检测会预读一批进来)。
    pending_events: VecDeque<Event>,

    // 输入历史(↑/↓ 翻阅)
    history: Vec<String>,
    history_idx: Option<usize>,
    history_draft: String,

    busy: bool,
    compaction_active: bool,
    status_note: String,
    provider_label: String,
    active_selection: ActiveModelSelection,
    provider_catalog: Vec<ProviderCatalogEntry>,
    skills: Vec<SkillMetadataView>,
    reasoning_preferences: BTreeMap<String, BTreeMap<String, String>>,
    pending_reasoning: Option<PendingReasoningSelection>,
    session_id: String,
    usage: Usage,
    current_plan: PlanSnapshot,
    plan_collapsed: bool,
    tool_log: ToolLog,
    tool_detail_max_scroll: usize,
    last_overlay_height: u16,
    scroll_up: usize,
    last_transcript_height: u16,

    spinner_frame: usize,
    last_spin: Instant,
    quit_armed_at: Option<Instant>,
    should_quit: bool,
    force_clear: bool,
}

impl App {
    fn from_session(session: AgentSession) -> anyhow::Result<App> {
        let snapshot = session.controller.snapshot()?;
        let ui = session.controller.ui_metadata();
        let current_plan = plan_snapshot_from_view(snapshot.plan);
        let active_selection = ActiveModelSelection {
            provider: snapshot.model.provider,
            model: snapshot.model.model,
            effort: snapshot.model.effort,
        };
        let mut app = App::new(
            Box::new(session.controller),
            Some(session.events),
            snapshot.model.label,
            active_selection,
            ui.provider_catalog,
            ui.reasoning_preferences,
            snapshot.session_id,
        );
        app.tool_log.restore(&snapshot.transcript);
        app.restore_snapshot_transcript(&snapshot.transcript);
        app.current_plan = current_plan;
        Ok(app)
    }

    fn new(
        runtime: Box<dyn UiRuntime>,
        events: Option<SessionEvents>,
        provider_label: String,
        active_selection: ActiveModelSelection,
        provider_catalog: Vec<ProviderCatalogEntry>,
        reasoning_preferences: BTreeMap<String, BTreeMap<String, String>>,
        session_id: String,
    ) -> App {
        App {
            runtime,
            events,
            transcript: Transcript::default(),
            input: InputBox::default(),
            overlay: None,
            slash_selected: 0,
            slash_dismissed: None,
            pending_events: VecDeque::new(),
            history: Vec::new(),
            history_idx: None,
            history_draft: String::new(),
            busy: false,
            compaction_active: false,
            status_note: String::new(),
            provider_label,
            active_selection,
            provider_catalog,
            skills: Vec::new(),
            reasoning_preferences,
            pending_reasoning: None,
            session_id,
            usage: Usage::default(),
            current_plan: PlanSnapshot::default(),
            plan_collapsed: false,
            tool_log: ToolLog::default(),
            tool_detail_max_scroll: 0,
            last_overlay_height: 1,
            scroll_up: 0,
            last_transcript_height: 20,
            spinner_frame: 0,
            last_spin: Instant::now(),
            quit_armed_at: None,
            should_quit: false,
            force_clear: false,
        }
    }

    fn event_loop(&mut self, terminal: &mut ratatui::DefaultTerminal) -> anyhow::Result<()> {
        let mut dirty = true;
        loop {
            // 1. 应用 Runtime 事件
            loop {
                let event = self
                    .events
                    .as_mut()
                    .and_then(|events| events.try_recv().ok());
                let Some(event) = event else { break };
                self.on_session_event(event);
                dirty = true;
            }
            // 2. 终端输入:一帧内把积压处理干净(粘贴洪峰时避免一字符一帧),
            //    上限防止超大粘贴饿死渲染
            let mut budget = 512;
            while budget > 0 {
                let ev = if let Some(e) = self.pending_events.pop_front() {
                    e
                } else if event::poll(Duration::ZERO)? {
                    event::read()?
                } else {
                    break;
                };
                dirty |= self.on_terminal_event(ev);
                budget -= 1;
            }
            // 空闲时阻塞等待(33ms 超时 ≈ 渲染节拍)
            if !dirty && event::poll(Duration::from_millis(33))? {
                let ev = event::read()?;
                dirty |= self.on_terminal_event(ev);
            }
            // 3. 忙碌时转 spinner
            if self.busy && self.last_spin.elapsed() > Duration::from_millis(100) {
                self.spinner_frame = (self.spinner_frame + 1) % SPINNER.len();
                self.last_spin = Instant::now();
                dirty = true;
            }
            // 双击退出的提示过期后恢复状态栏
            if let Some(t) = self.quit_armed_at {
                if t.elapsed() > Duration::from_secs(2) {
                    self.quit_armed_at = None;
                    dirty = true;
                }
            }
            dirty |= self.commit_history(terminal)?;
            // 4. 渲染
            if self.force_clear {
                terminal.clear()?;
                self.force_clear = false;
            }
            if dirty {
                terminal.draw(|f| self.draw(f))?;
                dirty = false;
            }
            if self.should_quit {
                return Ok(());
            }
        }
    }

    fn commit_history(&mut self, terminal: &mut ratatui::DefaultTerminal) -> std::io::Result<bool> {
        let width = terminal.size()?.width.max(1);
        let lines = self.transcript.drain_finalized_lines(width);
        if lines.is_empty() {
            return Ok(false);
        }
        let height = u16::try_from(lines.len()).unwrap_or(u16::MAX);
        terminal.insert_before(height, move |buffer| {
            Paragraph::new(lines).render(buffer.area, buffer);
            clear_wide_continuation_cells(buffer);
        })?;
        self.scroll_up = 0;
        Ok(true)
    }

    /// 返回事件是否改变了界面。
    fn on_terminal_event(&mut self, ev: Event) -> bool {
        match ev {
            Event::Key(k) if k.kind == KeyEventKind::Press => {
                self.on_key(k.code, k.modifiers);
                true
            }
            Event::Paste(s) => {
                match &mut self.overlay {
                    Some(Overlay::Picker { picker, .. }) => {
                        for c in s.chars() {
                            picker.push_filter(c);
                        }
                    }
                    Some(Overlay::Loading { .. })
                    | Some(Overlay::Approval { .. })
                    | Some(Overlay::ToolList { .. })
                    | Some(Overlay::ToolDetail { .. }) => {}
                    None => {
                        self.input.insert_str(&s);
                        self.on_input_changed();
                    }
                }
                true
            }
            Event::Resize(_, _) => true,
            _ => false,
        }
    }

    // ---- Runtime 事件 → 界面状态 ----

    fn on_session_event(&mut self, event: SessionEvent) {
        match event {
            SessionEvent::SessionSnapshot { snapshot } => {
                let snapshot = *snapshot;
                let session_changed = self.session_id != snapshot.session_id;
                self.session_id = snapshot.session_id;
                self.provider_label = snapshot.model.label;
                self.active_selection = ActiveModelSelection {
                    provider: snapshot.model.provider,
                    model: snapshot.model.model,
                    effort: snapshot.model.effort,
                };
                self.usage = Usage {
                    input_tokens: snapshot.usage.input_tokens,
                    output_tokens: snapshot.usage.output_tokens,
                    cache: match (
                        snapshot.usage.cache_read_tokens,
                        snapshot.usage.cache_write_tokens,
                    ) {
                        (None, None) => None,
                        (read, write) => Some(crate::message::CacheUsage {
                            read_tokens: read.unwrap_or(0),
                            write_tokens: write.unwrap_or(0),
                        }),
                    },
                };
                self.current_plan = plan_snapshot_from_view(snapshot.plan);
                self.busy = snapshot.phase != SessionPhase::Idle;
                match snapshot.phase {
                    SessionPhase::Compacting => self.compaction_active = true,
                    SessionPhase::Retrying => {}
                    _ => self.compaction_active = false,
                }
                self.status_note = match snapshot.phase {
                    SessionPhase::Idle => String::new(),
                    SessionPhase::Running => "思考中".into(),
                    SessionPhase::Retrying => "重试中".into(),
                    SessionPhase::Compacting => "压缩中".into(),
                    SessionPhase::WaitingApproval => "等待审批".into(),
                    SessionPhase::ShuttingDown => "退出中".into(),
                };
                if session_changed {
                    self.tool_log.restore(&snapshot.transcript);
                    self.restore_snapshot_transcript(&snapshot.transcript);
                    self.scroll_up = 0;
                }
            }
            SessionEvent::Progress { progress } => self.on_progress(progress),
            SessionEvent::CommandFinished { status, .. } => {
                if status == crate::sdk::CommandStatus::Cancelled {
                    self.transcript.push_notice("已取消".into());
                }
            }
            SessionEvent::Settled { .. } => {
                self.busy = false;
                self.compaction_active = false;
                self.status_note.clear();
                self.transcript.close_open_cells();
            }
        }
    }

    fn on_progress(&mut self, progress: ProgressEvent) {
        match progress {
            ProgressEvent::UserMessage { text } => self.transcript.push_user(text),
            ProgressEvent::RunStarted { .. } => {
                self.busy = true;
                self.status_note = "思考中".into();
            }
            ProgressEvent::RetryScheduled {
                attempt,
                max_retries,
                delay_ms,
                error,
            } => {
                self.status_note = if self.compaction_active {
                    format!(
                        "压缩重试 {}/{}，等待 {:.1}s",
                        attempt,
                        max_retries,
                        delay_ms as f64 / 1000.0
                    )
                } else {
                    format!(
                        "重试 {}/{}，等待 {:.1}s",
                        attempt,
                        max_retries,
                        delay_ms as f64 / 1000.0
                    )
                };
                self.transcript.push_notice(format!(
                    "{}，{:.1}s 后重试({}/{})",
                    error,
                    delay_ms as f64 / 1000.0,
                    attempt,
                    max_retries
                ));
            }
            ProgressEvent::RetryStarted { .. } => {
                self.status_note = if self.compaction_active {
                    "正在继续压缩历史".into()
                } else {
                    "正在重新连接模型".into()
                }
            }
            ProgressEvent::CompactionStarted {
                compaction_id,
                trigger,
                estimated_tokens,
                available_tokens,
            } => {
                self.busy = true;
                self.compaction_active = true;
                self.status_note = match trigger {
                    CompactionTriggerView::Automatic => "正在自动压缩历史".into(),
                    CompactionTriggerView::Manual => "正在手动压缩历史".into(),
                };
                self.transcript.start_compaction(
                    compaction_id,
                    trigger == CompactionTriggerView::Automatic,
                    estimated_tokens,
                    available_tokens,
                );
            }
            ProgressEvent::CompactionFinished {
                compaction_id,
                trigger,
                tokens_before,
                summary_chars,
                retained_messages,
            } => {
                self.compaction_active = false;
                self.status_note = match trigger {
                    CompactionTriggerView::Automatic => "压缩完成，继续思考".into(),
                    CompactionTriggerView::Manual => "压缩完成".into(),
                };
                self.transcript.finish_compaction(
                    &compaction_id,
                    tokens_before,
                    summary_chars,
                    retained_messages,
                );
            }
            ProgressEvent::CompactionFailed {
                compaction_id,
                error,
                cancelled,
                history_changed,
                ..
            } => {
                self.compaction_active = false;
                self.status_note = if cancelled {
                    "压缩已取消".into()
                } else {
                    "压缩失败".into()
                };
                self.transcript
                    .fail_compaction(&compaction_id, error, cancelled, history_changed);
            }
            ProgressEvent::AssistantDelta { kind, delta, .. } if kind == "thinking" => {
                self.transcript.append_thinking(&delta)
            }
            ProgressEvent::AssistantDelta { delta, .. } => self.transcript.append_assistant(&delta),
            ProgressEvent::AssistantFinished { text, .. } => {
                self.transcript.finalize_assistant(text)
            }
            ProgressEvent::ToolCallPending { name } => {
                self.status_note = format!("正在生成 {} 的参数", name)
            }
            ProgressEvent::ToolStarted {
                tool_call_id,
                name,
                summary,
            } => {
                self.status_note = format!("执行 {}", name);
                self.tool_log
                    .start(tool_call_id.clone(), name.clone(), summary.clone());
                self.transcript.push_tool(tool_call_id, name, summary);
            }
            ProgressEvent::ToolUpdated {
                tool_call_id,
                name,
                output,
            } => {
                self.status_note = format!("工具进度: {}", output.summary);
                self.transcript
                    .update_tool(&tool_call_id, output.summary.clone());
                self.tool_log.update(tool_call_id, name, output);
            }
            ProgressEvent::ToolFinished {
                tool_call_id,
                name,
                output,
                error,
            } => {
                self.status_note = "思考中".into();
                self.transcript
                    .finish_tool(&tool_call_id, output.content.clone(), error.is_some());
                self.tool_log
                    .finish(tool_call_id, name, output, error.as_ref());
            }
            ProgressEvent::ApprovalRequested { request } => {
                self.status_note = format!("等待审批: {}", request.tool);
                self.overlay = Some(Overlay::Approval {
                    request: ApprovalRequest {
                        request_id: request.request_id,
                        tool: request.tool,
                        summary: request.summary,
                        reason: request.reason,
                        scopes: request
                            .scopes
                            .into_iter()
                            .map(|scope| match scope {
                                crate::sdk::ApprovalScopeView::Once => ApprovalScope::Once,
                                crate::sdk::ApprovalScopeView::Session => ApprovalScope::Session,
                            })
                            .collect(),
                        details: crate::permission::ApprovalDetails {
                            command: request.command,
                            cwd: request.cwd,
                            targets: request.targets,
                        },
                    },
                    selected: 0,
                });
            }
            ProgressEvent::ApprovalResolved {
                request_id,
                allowed,
            } => {
                if matches!(
                    &self.overlay,
                    Some(Overlay::Approval { request, .. }) if request.request_id == request_id
                ) {
                    self.overlay = None;
                }
                self.status_note = if allowed {
                    "审批通过，正在执行".into()
                } else {
                    "审批未通过".into()
                };
            }
            ProgressEvent::Notice { text, .. } => self.transcript.push_notice(text),
            ProgressEvent::Error { error } => {
                if matches!(self.overlay, Some(Overlay::Loading { .. })) {
                    self.overlay = None;
                }
                self.transcript.push_error(error.message);
            }
            ProgressEvent::PlanUpdated { plan } => {
                let plan = plan_snapshot_from_view(plan);
                self.transcript.push_plan(
                    plan.revision,
                    plan.items.clone(),
                    plan.explanation.clone(),
                );
                self.current_plan = plan;
            }
            ProgressEvent::SkillsDiscovered { skills, warnings } => {
                self.skills = skills.clone();
                self.status_note = format!("已发现 {} 个技能", skills.len());
                if !skills.is_empty() {
                    self.transcript
                        .push_notice(format!("已发现 {} 个可用技能", skills.len()));
                }
                for warning in warnings {
                    self.transcript
                        .push_notice(format!("技能发现警告: {}", warning));
                }
            }
            ProgressEvent::Usage { usage } => {
                self.usage = Usage {
                    input_tokens: usage.input_tokens,
                    output_tokens: usage.output_tokens,
                    cache: match (usage.cache_read_tokens, usage.cache_write_tokens) {
                        (None, None) => None,
                        (read, write) => Some(crate::message::CacheUsage {
                            read_tokens: read.unwrap_or(0),
                            write_tokens: write.unwrap_or(0),
                        }),
                    },
                };
            }
            ProgressEvent::ConversationCleared => {
                self.transcript.clear();
                self.tool_log.clear();
                if matches!(
                    self.overlay,
                    Some(Overlay::ToolList { .. } | Overlay::ToolDetail { .. })
                ) {
                    self.overlay = None;
                }
                self.usage = Usage::default();
                self.current_plan = PlanSnapshot::default();
                self.transcript.push_notice("会话已清空".into());
            }
            ProgressEvent::ModelSelectionChanged { selection } => {
                self.apply_model_view(
                    selection.provider,
                    selection.model,
                    selection.effort,
                    selection.label,
                );
            }
            ProgressEvent::SessionsListed {
                current_id,
                sessions,
            } => self.show_session_picker(current_id, sessions),
        }
    }

    fn apply_model_view(&mut self, provider: String, model: String, effort: String, label: String) {
        let default_effort = self
            .provider_catalog
            .iter()
            .find(|entry| entry.name == provider)
            .and_then(|entry| entry.models.iter().find(|entry| entry.id == model))
            .map(|entry| entry.default_effort.clone())
            .unwrap_or_else(|| DEFAULT_REASONING_EFFORT.to_string());
        self.provider_label = label;
        self.active_selection = ActiveModelSelection {
            provider: provider.clone(),
            model: model.clone(),
            effort: effort.clone(),
        };
        if effort == default_effort {
            if let Some(models) = self.reasoning_preferences.get_mut(&provider) {
                models.remove(&model);
                if models.is_empty() {
                    self.reasoning_preferences.remove(&provider);
                }
            }
        } else {
            self.reasoning_preferences
                .entry(provider)
                .or_default()
                .insert(model, effort);
        }
    }

    fn show_session_picker(
        &mut self,
        current_id: String,
        sessions: Vec<crate::sdk::SessionSummaryView>,
    ) {
        if !matches!(
            self.overlay,
            Some(Overlay::Loading {
                kind: PickerKind::Session,
                ..
            })
        ) {
            return;
        }
        let items = sessions
            .into_iter()
            .map(|session| {
                let is_current = session.id == current_id;
                let label = if session.title.is_empty() {
                    format!("会话 {}", short_id(&session.id))
                } else {
                    session.title
                };
                PickerItem {
                    label,
                    description: format!(
                        "{} 条消息 · {} · {}{}",
                        session.message_count,
                        short_id(&session.id),
                        session.workspace,
                        if is_current { " · 当前" } else { "" }
                    ),
                    value: Some(session.id),
                    current: is_current,
                }
            })
            .collect();
        self.overlay = Some(Overlay::Picker {
            kind: PickerKind::Session,
            picker: Picker::new("恢复会话", items),
        });
    }

    fn restore_snapshot_transcript(&mut self, items: &[TranscriptItem]) {
        self.transcript.clear();
        for item in items {
            match item {
                TranscriptItem::UserMessage { text, .. } => {
                    self.transcript.push_user(text.clone());
                }
                TranscriptItem::AssistantMessage { blocks, .. } => {
                    let mut text = String::new();
                    for block in blocks {
                        match block {
                            crate::sdk::AssistantBlockView::Text { text: block } => {
                                text.push_str(block);
                            }
                            crate::sdk::AssistantBlockView::Thinking { text } => {
                                self.transcript.append_thinking(text);
                            }
                            crate::sdk::AssistantBlockView::ToolCall { .. } => {}
                        }
                    }
                    if !text.is_empty() {
                        self.transcript.append_assistant(&text);
                        self.transcript.finalize_assistant(text);
                    }
                }
                TranscriptItem::Tool {
                    tool_call_id,
                    name,
                    summary,
                    status,
                    output,
                } => {
                    self.transcript
                        .push_tool(tool_call_id.clone(), name.clone(), summary.clone());
                    self.transcript.finish_tool(
                        tool_call_id,
                        output.clone().unwrap_or_default(),
                        *status == crate::sdk::ToolStatus::Failed,
                    );
                }
                TranscriptItem::Notice { text, .. } => {
                    self.transcript.push_notice(text.clone());
                }
            }
        }
        self.transcript.close_open_cells();
    }

    #[cfg(test)]
    fn on_agent_event(&mut self, ev: AgentEvent) {
        match ev {
            AgentEvent::UserMessage(t) => self.transcript.push_user(t),
            AgentEvent::TurnStarted => {
                self.busy = true;
                self.status_note = "思考中".into();
            }
            AgentEvent::RetryScheduled {
                attempt,
                max_retries,
                delay_ms,
                error,
            } => {
                self.status_note = if self.compaction_active {
                    format!(
                        "压缩重试 {}/{}，等待 {:.1}s",
                        attempt,
                        max_retries,
                        delay_ms as f64 / 1000.0
                    )
                } else {
                    format!(
                        "重试 {}/{}，等待 {:.1}s",
                        attempt,
                        max_retries,
                        delay_ms as f64 / 1000.0
                    )
                };
                self.transcript.push_notice(format!(
                    "{}，{:.1}s 后重试({}/{})",
                    error,
                    delay_ms as f64 / 1000.0,
                    attempt,
                    max_retries
                ));
            }
            AgentEvent::RetryStarted { .. } => {
                self.status_note = if self.compaction_active {
                    "正在继续压缩历史".into()
                } else {
                    "正在重新连接模型".into()
                }
            }
            AgentEvent::CompactionStarted {
                id,
                trigger,
                estimated_tokens,
                available_tokens,
            } => {
                self.busy = true;
                self.compaction_active = true;
                self.status_note = match trigger {
                    crate::event::CompactionTrigger::Automatic => "正在自动压缩历史".into(),
                    crate::event::CompactionTrigger::Manual => "正在手动压缩历史".into(),
                };
                self.transcript.start_compaction(
                    id,
                    trigger == crate::event::CompactionTrigger::Automatic,
                    estimated_tokens,
                    available_tokens,
                );
            }
            AgentEvent::CompactionFinished {
                id,
                trigger,
                tokens_before,
                summary_chars,
                retained_messages,
            } => {
                self.compaction_active = false;
                self.status_note = match trigger {
                    crate::event::CompactionTrigger::Automatic => "压缩完成，继续思考".into(),
                    crate::event::CompactionTrigger::Manual => "压缩完成".into(),
                };
                self.transcript.finish_compaction(
                    &id,
                    tokens_before,
                    summary_chars,
                    retained_messages,
                );
            }
            AgentEvent::CompactionFailed {
                id,
                error,
                cancelled,
                history_changed,
                ..
            } => {
                self.compaction_active = false;
                self.status_note = if cancelled {
                    "压缩已取消".into()
                } else {
                    "压缩失败".into()
                };
                self.transcript
                    .fail_compaction(&id, error, cancelled, history_changed);
            }
            AgentEvent::AssistantDelta(t) => self.transcript.append_assistant(&t),
            AgentEvent::ThinkingDelta(t) => self.transcript.append_thinking(&t),
            AgentEvent::AssistantMessage(full) => self.transcript.finalize_assistant(full),
            AgentEvent::ToolCallPending { name } => {
                self.status_note = format!("正在生成 {} 的参数", name);
            }
            AgentEvent::ToolCallStarted { id, name, summary } => {
                self.status_note = format!("执行 {}", name);
                self.tool_log
                    .start(id.clone(), name.clone(), summary.clone());
                self.transcript.push_tool(id, name, summary);
            }
            AgentEvent::ToolCallUpdated { id, name, output } => {
                self.status_note = format!("工具进度: {}", output.ui_text());
                self.transcript
                    .update_tool(&id, output.ui_text().to_string());
                self.tool_log
                    .update(id, name, crate::sdk::ToolOutputView::from(&output));
            }
            AgentEvent::ToolCallFinished {
                id,
                name,
                output,
                error,
            } => {
                self.status_note = "思考中".into();
                self.transcript
                    .finish_tool(&id, output.model_text.clone(), error.is_some());
                let error_view = error.as_ref().map(|error| crate::sdk::CommandErrorView {
                    code: error.code.as_str().into(),
                    message: error.message.clone(),
                });
                self.tool_log.finish(
                    id,
                    name,
                    crate::sdk::ToolOutputView::from(&output),
                    error_view.as_ref(),
                );
            }
            AgentEvent::PlanUpdated {
                revision,
                items,
                explanation,
            } => {
                self.transcript
                    .push_plan(revision, items.clone(), explanation.clone());
                self.current_plan = PlanSnapshot {
                    revision,
                    items,
                    explanation,
                };
            }
            AgentEvent::SkillsDiscovered { skills, warnings } => {
                self.skills = skills
                    .iter()
                    .map(|skill| SkillMetadataView {
                        name: skill.name.clone(),
                        description: skill.description.clone(),
                        scope: match skill.scope {
                            crate::skills::SkillScope::Repo => crate::sdk::SkillScopeView::Repo,
                            crate::skills::SkillScope::User => crate::sdk::SkillScopeView::User,
                        },
                    })
                    .collect();
                self.status_note = format!("已发现 {} 个技能", skills.len());
                if !skills.is_empty() {
                    self.transcript
                        .push_notice(format!("已发现 {} 个可用技能", skills.len()));
                }
                for warning in warnings {
                    self.transcript
                        .push_notice(format!("技能发现警告: {}", warning));
                }
            }
            AgentEvent::PermissionRequested { request } => {
                self.status_note = format!("等待审批: {}", request.tool);
                self.overlay = Some(Overlay::Approval {
                    request,
                    selected: 0,
                });
            }
            AgentEvent::PermissionResolved {
                request_id,
                allowed,
            } => {
                if matches!(
                    &self.overlay,
                    Some(Overlay::Approval { request, .. }) if request.request_id == request_id
                ) {
                    self.overlay = None;
                }
                self.status_note = if allowed {
                    "审批通过，正在执行".into()
                } else {
                    "审批未通过".into()
                };
            }
            AgentEvent::Usage {
                input_tokens,
                output_tokens,
                cache,
            } => {
                self.usage = Usage {
                    input_tokens,
                    output_tokens,
                    cache,
                }
            }
            AgentEvent::Notice(t) => self.transcript.push_notice(t),
            AgentEvent::Error(t) => {
                if matches!(self.overlay, Some(Overlay::Loading { .. })) {
                    self.overlay = None;
                }
                self.transcript.push_error(t);
            }
            AgentEvent::ConversationCleared => {
                self.transcript.clear();
                self.tool_log.clear();
                self.usage = Usage::default();
                self.transcript.push_notice("会话已清空".into());
            }
            AgentEvent::ModelSelectionChanged {
                provider,
                model,
                effort,
                label,
            } => {
                let default_effort = self
                    .provider_catalog
                    .iter()
                    .find(|entry| entry.name == provider)
                    .and_then(|entry| entry.models.iter().find(|entry| entry.id == model))
                    .map(|entry| entry.default_effort.clone())
                    .unwrap_or_else(|| DEFAULT_REASONING_EFFORT.to_string());
                self.provider_label = label;
                self.active_selection = ActiveModelSelection {
                    provider: provider.clone(),
                    model: model.clone(),
                    effort: effort.clone(),
                };
                if effort == default_effort {
                    if let Some(models) = self.reasoning_preferences.get_mut(&provider) {
                        models.remove(&model);
                        if models.is_empty() {
                            self.reasoning_preferences.remove(&provider);
                        }
                    }
                } else {
                    self.reasoning_preferences
                        .entry(provider)
                        .or_default()
                        .insert(model, effort);
                }
            }
            AgentEvent::SessionsListed {
                current_id,
                sessions,
            } => {
                if matches!(
                    self.overlay,
                    Some(Overlay::Loading {
                        kind: PickerKind::Session,
                        ..
                    })
                ) {
                    let items = sessions
                        .into_iter()
                        .map(|session| {
                            let is_current = session.id == current_id;
                            let label = if session.title.is_empty() {
                                format!("会话 {}", short_id(&session.id))
                            } else {
                                session.title
                            };
                            PickerItem {
                                label,
                                description: format!(
                                    "{} 条消息 · {}{}",
                                    session.message_count,
                                    short_id(&session.id),
                                    if is_current { " · 当前" } else { "" }
                                ),
                                value: Some(session.id),
                                current: is_current,
                            }
                        })
                        .collect();
                    self.overlay = Some(Overlay::Picker {
                        kind: PickerKind::Session,
                        picker: Picker::new("恢复会话", items),
                    });
                }
            }
            AgentEvent::SessionLoaded {
                id,
                entries,
                input_tokens,
                output_tokens,
                cache,
            } => {
                self.session_id = id;
                self.usage = Usage {
                    input_tokens,
                    output_tokens,
                    cache,
                };
                let message_count = entries
                    .iter()
                    .filter(|entry| matches!(entry.payload, SessionEntryPayload::Message(_)))
                    .count();
                self.restore_transcript(&entries);
                self.transcript.push_notice(format!(
                    "已恢复会话 {}({} 条历史消息,{} 条事实)",
                    short_id(&self.session_id),
                    message_count,
                    entries.len()
                ));
                self.scroll_up = 0;
            }
            AgentEvent::InputQueued { .. } | AgentEvent::InputDequeued { .. } => {}
            AgentEvent::TurnFinished { cancelled } => {
                self.busy = false;
                self.status_note.clear();
                self.transcript.close_open_cells();
                if cancelled {
                    self.transcript.push_notice("已取消".into());
                }
            }
        }
    }

    /// 按事实日志重建画面:Message 还原对话与工具单元,
    /// Notice/Compaction/ModelChange 等 UI-only 事实以提示行呈现。
    #[cfg(test)]
    fn restore_transcript(&mut self, entries: &[SessionEntry]) {
        let plan = reduce_plan(entries);
        let results: HashMap<&str, (&str, bool)> = entries
            .iter()
            .filter_map(|entry| match &entry.payload {
                SessionEntryPayload::Message(record) => Some(&record.message),
                _ => None,
            })
            .flat_map(|message| message.blocks.iter())
            .filter_map(|block| match block {
                MessageBlock::ToolResult {
                    tool_use_id,
                    content,
                    is_error,
                    ..
                } => Some((tool_use_id.as_str(), (content.as_str(), *is_error))),
                _ => None,
            })
            .collect();

        self.transcript.clear();
        self.tool_log.clear();
        self.current_plan = plan.snapshot.clone();
        for entry in entries {
            match &entry.payload {
                SessionEntryPayload::Message(record) => {
                    self.restore_message(&record.message, &results)
                }
                SessionEntryPayload::Notice(notice) => {
                    self.transcript.push_notice(notice.text.clone());
                }
                SessionEntryPayload::Compaction(compaction) => {
                    self.transcript.push_notice(format!(
                        "—— 历史已压缩(压缩前约 {} tokens);此后模型视图从摘要开始 ——",
                        compaction.tokens_before
                    ));
                }
                SessionEntryPayload::ModelChange(change) => {
                    self.transcript
                        .push_notice(format!("模型切换: {}", change.provider));
                }
                SessionEntryPayload::Artifact(_)
                | SessionEntryPayload::PlanUpdated(_)
                | SessionEntryPayload::PlanReminder(_) => {}
            }
        }
        if plan.snapshot.revision > 0 {
            self.transcript.push_plan(
                plan.snapshot.revision,
                plan.snapshot.items,
                plan.snapshot.explanation,
            );
        }
        for diagnostic in plan.diagnostics {
            self.transcript
                .push_notice(format!("计划事实修复: {diagnostic}"));
        }
    }

    #[cfg(test)]
    fn restore_message(&mut self, message: &ChatMessage, results: &HashMap<&str, (&str, bool)>) {
        match message.role {
            Role::User => {
                for block in &message.blocks {
                    if let MessageBlock::Text(text) = block {
                        self.transcript.push_user(text.clone());
                    }
                }
            }
            Role::Assistant => {
                for block in &message.blocks {
                    match block {
                        MessageBlock::Text(text) => {
                            self.transcript.append_assistant(text);
                        }
                        MessageBlock::Thinking { text, .. } if !text.is_empty() => {
                            self.transcript.append_thinking(text);
                        }
                        MessageBlock::ToolUse { id, name, input } => {
                            self.tool_log.start(
                                id.clone(),
                                name.clone(),
                                util::args_summary(input),
                            );
                            self.transcript.push_tool(
                                id.clone(),
                                name.clone(),
                                util::args_summary(input),
                            );
                            if let Some((output, is_error)) = results.get(id.as_str()) {
                                self.transcript.finish_tool(
                                    id,
                                    util::truncate_middle(output, 4000),
                                    *is_error,
                                );
                                let view = crate::sdk::ToolOutputView {
                                    content: util::truncate_middle(output, 4000),
                                    summary: String::new(),
                                    metadata: crate::sdk::ToolMetadataView::default(),
                                };
                                let error = if *is_error {
                                    Some(crate::sdk::CommandErrorView {
                                        code: "tool_error".into(),
                                        message: view.content.clone(),
                                    })
                                } else {
                                    None
                                };
                                self.tool_log.finish(
                                    id.clone(),
                                    name.clone(),
                                    view,
                                    error.as_ref(),
                                );
                            }
                        }
                        _ => {}
                    }
                }
                self.transcript.close_open_cells();
            }
        }
    }

    // ---- 按键 ----

    fn on_key(&mut self, code: KeyCode, mods: KeyModifiers) {
        if self.overlay.is_some() {
            self.on_overlay_key(code, mods);
            return;
        }

        let slash_open = !self.slash_matches().is_empty();
        match code {
            KeyCode::Char('c') if mods.contains(KeyModifiers::CONTROL) => self.on_ctrl_c(),
            KeyCode::Char('t') if mods.contains(KeyModifiers::CONTROL) => {
                if let Some(record) = self.tool_log.recent(0) {
                    self.overlay = Some(Overlay::ToolDetail {
                        tool_call_id: record.id.clone(),
                        scroll: 0,
                    });
                } else {
                    self.transcript.push_notice("当前会话还没有工具调用".into());
                }
            }
            KeyCode::Char('l') if mods.contains(KeyModifiers::CONTROL) => {
                self.force_clear = true;
            }
            KeyCode::Char('p') if mods.contains(KeyModifiers::ALT) => {
                if self.current_plan.revision > 0 && !self.current_plan.items.is_empty() {
                    self.plan_collapsed = !self.plan_collapsed;
                }
            }
            KeyCode::Char('a') if mods.contains(KeyModifiers::CONTROL) => self.input.move_start(),
            KeyCode::Char('e') if mods.contains(KeyModifiers::CONTROL) => self.input.move_end_all(),
            KeyCode::Char('w') if mods.contains(KeyModifiers::CONTROL) => {
                self.input.delete_word_left();
                self.on_input_changed();
            }
            KeyCode::Char('k') if mods.contains(KeyModifiers::CONTROL) => {
                self.input.delete_to_line_end();
                self.on_input_changed();
            }
            KeyCode::Char('u') if mods.contains(KeyModifiers::CONTROL) => {
                self.input.clear();
                self.on_input_changed();
            }
            KeyCode::Char(c) if !mods.contains(KeyModifiers::CONTROL) => {
                self.input.insert_char(c);
                self.on_input_changed();
            }
            KeyCode::Tab if slash_open => self.complete_slash_command(),
            KeyCode::BackTab if slash_open => self.move_slash_selection(false),
            KeyCode::Tab => {
                self.input.insert_str("    ");
                self.on_input_changed();
            }
            KeyCode::Enter => {
                if slash_open {
                    self.run_selected_slash_command();
                    return;
                }
                // 依次检查修饰键、可靠的行尾反斜杠语法、粘贴洪峰。短路求值避免
                // 普通 Shift+Enter 也去预读终端事件。
                let newline = mods
                    .intersects(KeyModifiers::SHIFT | KeyModifiers::ALT | KeyModifiers::CONTROL)
                    || self.input.pop_trailing_backslash()
                    || self.enter_means_newline();
                if newline {
                    self.input.insert_char('\n');
                    self.on_input_changed();
                } else {
                    self.submit();
                }
            }
            KeyCode::Backspace => {
                self.input.backspace();
                self.on_input_changed();
            }
            KeyCode::Delete => {
                self.input.delete();
                self.on_input_changed();
            }
            KeyCode::Left if mods.intersects(KeyModifiers::ALT | KeyModifiers::CONTROL) => {
                self.input.move_word_left()
            }
            KeyCode::Right if mods.intersects(KeyModifiers::ALT | KeyModifiers::CONTROL) => {
                self.input.move_word_right()
            }
            KeyCode::Left => self.input.move_left(),
            KeyCode::Right => self.input.move_right(),
            KeyCode::Home => self.input.move_home(),
            KeyCode::End => self.input.move_end(),
            KeyCode::Up if slash_open => self.move_slash_selection(false),
            KeyCode::Down if slash_open => self.move_slash_selection(true),
            KeyCode::Up => {
                if self.input.is_multiline() {
                    self.input.move_vertical(true);
                } else {
                    self.history_prev();
                }
            }
            KeyCode::Down => {
                if self.input.is_multiline() {
                    self.input.move_vertical(false);
                } else {
                    self.history_next();
                }
            }
            KeyCode::PageUp => {
                self.scroll_up = self
                    .scroll_up
                    .saturating_add(self.last_transcript_height.max(1) as usize / 2);
            }
            KeyCode::PageDown => {
                self.scroll_up = self
                    .scroll_up
                    .saturating_sub(self.last_transcript_height.max(1) as usize / 2);
            }
            KeyCode::Esc => {
                if slash_open {
                    self.slash_dismissed = Some(self.input.text().to_string());
                } else if self.busy {
                    // 请求取消当前轮;Runtime 在下一个流事件/工具间隙生效
                    self.runtime.cancel();
                    self.status_note = "取消中…".into();
                } else if self.scroll_up > 0 {
                    self.scroll_up = 0;
                } else {
                    self.input.clear();
                    self.on_input_changed();
                }
            }
            _ => {}
        }
    }

    fn on_input_changed(&mut self) {
        self.slash_selected = 0;
        self.slash_dismissed = None;
        self.history_idx = None;
    }

    fn slash_query(&self) -> Option<&str> {
        let text = self.input.text();
        if self.slash_dismissed.as_deref() == Some(text) || !text.starts_with('/') {
            return None;
        }
        let line_end = text.find('\n').unwrap_or(text.len());
        if self.input.cursor() > line_end {
            return None;
        }
        let first_line = &text[..line_end];
        let command_end = first_line
            .find(char::is_whitespace)
            .unwrap_or(first_line.len());
        if self.input.cursor() > command_end || first_line[1..].chars().any(char::is_whitespace) {
            return None;
        }
        Some(&first_line[1..command_end])
    }

    fn slash_matches(&self) -> Vec<&'static command::CommandSpec> {
        self.slash_query().map(command::matches).unwrap_or_default()
    }

    fn move_slash_selection(&mut self, down: bool) {
        let len = self.slash_matches().len();
        if len == 0 {
            return;
        }
        self.slash_selected = if down {
            (self.slash_selected + 1) % len
        } else if self.slash_selected == 0 {
            len - 1
        } else {
            self.slash_selected - 1
        };
    }

    fn selected_slash_command(&self) -> Option<&'static command::CommandSpec> {
        let matches = self.slash_matches();
        matches
            .get(self.slash_selected.min(matches.len().saturating_sub(1)))
            .copied()
    }

    fn complete_slash_command(&mut self) {
        if let Some(spec) = self.selected_slash_command() {
            let suffix = if spec.accepts_args { " " } else { "" };
            self.input.set(format!("/{}{}", spec.name, suffix));
            self.slash_selected = 0;
            self.slash_dismissed = spec.accepts_args.then(|| self.input.text().to_string());
        }
    }

    fn run_selected_slash_command(&mut self) {
        if let Some(spec) = self.selected_slash_command() {
            self.input.clear();
            self.slash_dismissed = None;
            self.execute_slash(spec.command, "");
        }
    }

    fn on_overlay_key(&mut self, code: KeyCode, mods: KeyModifiers) {
        let mut accept_picker = false;
        let mut close = false;
        let mut approval: Option<ApprovalResponse> = None;
        let mut open_tool: Option<String> = None;
        let mut show_tool_list = false;
        match self.overlay.as_mut().expect("overlay checked above") {
            Overlay::Picker { picker, .. } => match code {
                KeyCode::Esc => close = true,
                KeyCode::Up | KeyCode::BackTab => picker.move_up(),
                KeyCode::Down | KeyCode::Tab => picker.move_down(),
                KeyCode::Enter => accept_picker = true,
                KeyCode::Backspace => picker.pop_filter(),
                KeyCode::Char('c') if mods.contains(KeyModifiers::CONTROL) => close = true,
                KeyCode::Char(c) if !mods.contains(KeyModifiers::CONTROL) => picker.push_filter(c),
                _ => {}
            },
            Overlay::Loading { .. } => {
                if matches!(code, KeyCode::Esc)
                    || matches!(code, KeyCode::Char('c') if mods.contains(KeyModifiers::CONTROL))
                {
                    close = true;
                }
            }
            Overlay::Approval { request, selected } => match code {
                KeyCode::Up | KeyCode::Left | KeyCode::BackTab => {
                    let len = approval_choices(request).len();
                    *selected = if *selected == 0 {
                        len.saturating_sub(1)
                    } else {
                        *selected - 1
                    };
                }
                KeyCode::Down | KeyCode::Right | KeyCode::Tab => {
                    let len = approval_choices(request).len();
                    *selected = (*selected + 1) % len.max(1);
                }
                KeyCode::Enter => {
                    if let Some(choice) = approval_choices(request).get(*selected).copied() {
                        approval = Some(ApprovalResponse {
                            request_id: request.request_id.clone(),
                            decision: approval_decision(choice),
                        });
                    }
                }
                KeyCode::Char('y') if request.scopes.contains(&ApprovalScope::Once) => {
                    approval = Some(ApprovalResponse {
                        request_id: request.request_id.clone(),
                        decision: ApprovalDecision::Allow(ApprovalScope::Once),
                    });
                }
                KeyCode::Char('a') if request.scopes.contains(&ApprovalScope::Session) => {
                    approval = Some(ApprovalResponse {
                        request_id: request.request_id.clone(),
                        decision: ApprovalDecision::Allow(ApprovalScope::Session),
                    });
                }
                KeyCode::Esc | KeyCode::Char('n') => {
                    approval = Some(ApprovalResponse {
                        request_id: request.request_id.clone(),
                        decision: ApprovalDecision::Deny,
                    });
                }
                KeyCode::Char('c') if mods.contains(KeyModifiers::CONTROL) => {
                    approval = Some(ApprovalResponse {
                        request_id: request.request_id.clone(),
                        decision: ApprovalDecision::Deny,
                    });
                }
                _ => {}
            },
            Overlay::ToolList { selected } => match code {
                KeyCode::Esc => close = true,
                KeyCode::Char('c') if mods.contains(KeyModifiers::CONTROL) => close = true,
                KeyCode::Char('t') if mods.contains(KeyModifiers::CONTROL) => close = true,
                KeyCode::Up | KeyCode::BackTab => *selected = selected.saturating_sub(1),
                KeyCode::Down | KeyCode::Tab => {
                    *selected = (*selected + 1).min(self.tool_log.len().saturating_sub(1));
                }
                KeyCode::Home => *selected = 0,
                KeyCode::End => *selected = self.tool_log.len().saturating_sub(1),
                KeyCode::Enter => {
                    open_tool = self
                        .tool_log
                        .recent(*selected)
                        .map(|record| record.id.clone());
                }
                _ => {}
            },
            Overlay::ToolDetail { scroll, .. } => match code {
                KeyCode::Esc => close = true,
                KeyCode::Char('c') if mods.contains(KeyModifiers::CONTROL) => close = true,
                KeyCode::Char('t') if mods.contains(KeyModifiers::CONTROL) => close = true,
                KeyCode::Up => *scroll = scroll.saturating_sub(1),
                KeyCode::Down => *scroll = (*scroll + 1).min(self.tool_detail_max_scroll),
                KeyCode::PageUp => {
                    *scroll = scroll.saturating_sub(self.last_overlay_height.max(1) as usize);
                }
                KeyCode::PageDown => {
                    *scroll = scroll
                        .saturating_add(self.last_overlay_height.max(1) as usize)
                        .min(self.tool_detail_max_scroll);
                }
                KeyCode::Home => *scroll = 0,
                KeyCode::End => *scroll = self.tool_detail_max_scroll,
                KeyCode::Backspace => show_tool_list = true,
                _ => {}
            },
        }
        if let Some(response) = approval {
            self.runtime.approve(response);
            self.overlay = None;
        } else if accept_picker {
            self.accept_picker();
        } else if let Some(tool_call_id) = open_tool {
            self.overlay = Some(Overlay::ToolDetail {
                tool_call_id,
                scroll: 0,
            });
        } else if show_tool_list {
            self.overlay = Some(Overlay::ToolList { selected: 0 });
        } else if close {
            self.overlay = None;
            self.pending_reasoning = None;
        }
    }

    fn accept_picker(&mut self) {
        let selected = match &self.overlay {
            Some(Overlay::Picker { kind, picker }) => picker.selected().map(|item| (*kind, item)),
            _ => None,
        };
        let Some((kind, item)) = selected else { return };
        match (kind, item.value) {
            (PickerKind::Provider, Some(provider)) => {
                self.runtime.submit(AgentCommand::SwitchProvider(provider));
                self.overlay = None;
            }
            (PickerKind::Model, Some(model)) => {
                self.open_reasoning_picker(model, false);
            }
            (PickerKind::Reasoning, Some(effort)) => {
                let Some(pending) = self.pending_reasoning.take() else {
                    self.overlay = None;
                    return;
                };
                let command = if pending.effort_only {
                    AgentCommand::SetReasoningEffort(effort)
                } else {
                    AgentCommand::SelectModel {
                        model: pending.model,
                        effort,
                    }
                };
                self.runtime.submit(command);
                self.overlay = None;
            }
            (PickerKind::Session, Some(session_id)) => {
                self.runtime.submit(AgentCommand::LoadSession(session_id));
                self.overlay = None;
            }
            (PickerKind::Skill, Some(name)) => {
                self.submit_skill(name);
            }
            (
                PickerKind::Provider
                | PickerKind::Model
                | PickerKind::Reasoning
                | PickerKind::Session
                | PickerKind::Skill,
                None,
            ) => {}
        }
    }

    /// 判定这个 Enter 是"粘贴内容里的换行"还是"用户按下发送"。
    ///
    /// 背景:conpty 终端(Windows Terminal / VS Code)把一次按键合成为
    /// **同一瞬间**入队的 Press+Release 两条记录,所以"队列非空"完全
    /// 不能说明在粘贴——必须把积压事件读出来看内容:
    /// 粘贴时,这个 Enter 后面必然紧跟着更多**字符按下**事件;
    /// 手动按 Enter 时,积压里最多只有 Release 之类的噪音。
    /// 预读的事件存进 `pending_events`,主循环照常消费,一个不丢。
    ///
    /// 8ms 小超时是给 conpty 管道分块留的余量(超大粘贴可能在块边界
    /// 短暂断流);人手两次按键的间隔远大于它,不会把打字误判成粘贴。
    /// 代价是每次发送多约 8ms 延迟,无感。
    ///
    /// 已知取舍:粘贴内容若以换行结尾,最后那个换行会触发发送——
    /// 与把命令粘进 shell 的行为一致。
    fn enter_means_newline(&mut self) -> bool {
        while let Ok(true) = event::poll(Duration::from_millis(8)) {
            match event::read() {
                Ok(ev) => self.pending_events.push_back(ev),
                Err(_) => break,
            }
        }
        self.pending_events.iter().any(|ev| match ev {
            Event::Key(k) if k.kind == KeyEventKind::Press => {
                matches!(k.code, KeyCode::Char(_) | KeyCode::Enter | KeyCode::Tab)
            }
            Event::Paste(_) => true,
            _ => false,
        })
    }

    fn on_ctrl_c(&mut self) {
        // 规则:输入非空 → 清空输入;否则第一次按 → 预备退出(忙碌时顺带取消),
        // 2 秒内再按 → 真退出。任何状态下连按两次都能离开。
        if let Some(t) = self.quit_armed_at {
            if t.elapsed() <= Duration::from_secs(2) {
                self.quit();
                return;
            }
        }
        if !self.input.is_empty() {
            self.input.clear();
            return;
        }
        if self.busy {
            self.runtime.cancel();
        }
        self.quit_armed_at = Some(Instant::now());
    }

    fn quit(&mut self) {
        self.runtime.shutdown();
        self.should_quit = true;
    }

    // ---- 提交与命令 ----

    fn submit(&mut self) {
        let raw = self.input.take();
        let text = raw.trim().to_string();
        if text.is_empty() {
            return;
        }
        if let Some(rest) = text.strip_prefix('/') {
            self.handle_slash(rest.trim());
            return;
        }
        if self.busy {
            // 运行中回车 = steering:Runtime 会在当前完整工具批之后注入。
            self.history.push(text.clone());
            self.history_idx = None;
            self.transcript
                .push_notice("已排队为 steering,将在当前一批工具完成后注入(Esc 取消本轮)".into());
            self.runtime.submit(AgentCommand::Steer(text));
            return;
        }
        self.history.push(text.clone());
        self.history_idx = None;
        self.scroll_up = 0;
        self.runtime.submit(AgentCommand::UserInput(text));
    }

    fn handle_slash(&mut self, cmd: &str) {
        let (head, rest) = match cmd.split_once(char::is_whitespace) {
            Some((h, r)) => (h, r.trim()),
            None => (cmd, ""),
        };
        let Some(spec) = command::find(head) else {
            self.transcript
                .push_error(format!("未知命令 /{},输入 / 查看可用命令", head));
            return;
        };
        self.execute_slash(spec.command, rest);
    }

    fn execute_slash(&mut self, command: command::SlashCommand, rest: &str) {
        match command {
            command::SlashCommand::Help => self.transcript.push_notice(HELP_TEXT.into()),
            command::SlashCommand::Quit => self.quit(),
            command::SlashCommand::Clear => {
                // 命令走通道排队,真正清空以 ConversationCleared 事件为准
                self.runtime.submit(AgentCommand::ClearConversation);
            }
            command::SlashCommand::Compact => {
                self.runtime.submit(AgentCommand::Compact);
            }
            command::SlashCommand::Reload => {
                self.runtime.submit(AgentCommand::Reload);
            }
            command::SlashCommand::Mcp => {
                self.runtime.submit(AgentCommand::McpStatus);
            }
            command::SlashCommand::Queue => {
                if rest.is_empty() {
                    self.transcript
                        .push_error("/queue 需要内容,例如 /queue 跑一遍测试".into());
                } else {
                    self.transcript
                        .push_notice("已排队为后续任务,当前任务结束后执行".into());
                    self.runtime
                        .submit(AgentCommand::FollowUp(rest.to_string()));
                }
            }
            command::SlashCommand::Session => {
                if rest.is_empty() || rest.eq_ignore_ascii_case("all") {
                    self.overlay = Some(Overlay::Loading {
                        kind: PickerKind::Session,
                        title: "正在读取会话…".into(),
                    });
                    self.runtime.submit(AgentCommand::ListSessions {
                        all: rest.eq_ignore_ascii_case("all"),
                    });
                } else {
                    self.runtime
                        .submit(AgentCommand::LoadSession(rest.to_string()));
                }
            }
            command::SlashCommand::Skill => {
                if rest.is_empty() {
                    self.open_skill_picker();
                } else {
                    self.submit_skill(rest.to_string());
                }
            }
            command::SlashCommand::Tools => self.open_tools(rest),
            command::SlashCommand::Provider => {
                if rest.is_empty() {
                    self.open_provider_picker();
                } else {
                    self.runtime
                        .submit(AgentCommand::SwitchProvider(rest.to_string()));
                }
            }
            command::SlashCommand::Model => {
                if rest.is_empty() {
                    self.open_model_picker();
                } else {
                    self.select_model_from_args(rest);
                }
            }
            command::SlashCommand::Reasoning => {
                if rest.is_empty() {
                    self.open_reasoning_picker(self.active_selection.model.clone(), true);
                } else {
                    self.set_reasoning_from_args(rest);
                }
            }
        }
    }

    fn open_tools(&mut self, query: &str) {
        if self.tool_log.is_empty() {
            self.transcript.push_notice("当前会话还没有工具调用".into());
            return;
        }
        if query.is_empty() {
            self.overlay = Some(Overlay::ToolList { selected: 0 });
            return;
        }
        if let Some(tool_call_id) = self.tool_log.find_id(query) {
            self.overlay = Some(Overlay::ToolDetail {
                tool_call_id,
                scroll: 0,
            });
        } else {
            self.transcript
                .push_error(format!("没有匹配 {:?} 的工具调用", query));
        }
    }

    fn open_skill_picker(&mut self) {
        if self.skills.is_empty() {
            self.transcript
                .push_notice("当前 Runtime 没有发现可用技能".into());
            return;
        }
        let items = self
            .skills
            .iter()
            .map(|skill| PickerItem {
                label: skill.name.clone(),
                description: format!(
                    "{} · {}",
                    match skill.scope {
                        crate::sdk::SkillScopeView::Repo => "repo",
                        crate::sdk::SkillScopeView::User => "user",
                    },
                    skill.description
                ),
                value: Some(skill.name.clone()),
                current: false,
            })
            .collect();
        self.overlay = Some(Overlay::Picker {
            kind: PickerKind::Skill,
            picker: Picker::new("选择技能", items),
        });
    }

    fn submit_skill(&mut self, name: String) {
        if !self.skills.iter().any(|skill| skill.name == name) {
            self.transcript
                .push_error(format!("未发现技能 {:?}，请先重启或检查技能目录", name));
            self.overlay = None;
            return;
        }
        self.overlay = None;
        let text = format!("请先加载并遵循技能 {:?}，然后继续处理这个请求。", name);
        self.history.push(text.clone());
        self.history_idx = None;
        self.scroll_up = 0;
        self.runtime.submit(AgentCommand::UserInput(text));
    }

    fn open_provider_picker(&mut self) {
        let items = self
            .provider_catalog
            .iter()
            .map(|provider| PickerItem {
                label: provider.name.clone(),
                description: if provider.name == self.active_selection.provider {
                    "当前 provider".into()
                } else {
                    "config.toml 中的 profile".into()
                },
                value: Some(provider.name.clone()),
                current: provider.name == self.active_selection.provider,
            })
            .collect();
        self.overlay = Some(Overlay::Picker {
            kind: PickerKind::Provider,
            picker: Picker::new("选择 provider", items),
        });
    }

    fn open_model_picker(&mut self) {
        let Some(provider) = self.current_provider() else {
            self.transcript.push_error(format!(
                "当前 provider {:?} 不在配置目录中",
                self.active_selection.provider
            ));
            return;
        };
        let items = provider
            .models
            .iter()
            .map(|model| PickerItem {
                description: if model.id == self.active_selection.model {
                    "当前模型".into()
                } else {
                    "来自 config.toml".into()
                },
                current: model.id == self.active_selection.model,
                value: Some(model.id.clone()),
                label: model.id.clone(),
            })
            .collect();
        self.overlay = Some(Overlay::Picker {
            kind: PickerKind::Model,
            picker: Picker::new("选择模型", items),
        });
    }

    fn open_reasoning_picker(&mut self, model_id: String, effort_only: bool) {
        let Some(model) = self.current_model(&model_id).cloned() else {
            self.transcript.push_error(format!(
                "provider {:?} 没有模型 {:?}",
                self.active_selection.provider, model_id
            ));
            return;
        };
        let selected_effort = if model_id == self.active_selection.model {
            self.active_selection.effort.clone()
        } else {
            self.saved_effort(&model_id)
                .filter(|saved| model.efforts.iter().any(|effort| effort == saved))
                .unwrap_or(&model.default_effort)
                .to_string()
        };
        let items = model
            .efforts
            .iter()
            .map(|effort| PickerItem {
                label: effort.clone(),
                description: if model.sends_effort {
                    "发送给 provider".into()
                } else {
                    "默认程度，不发送 effort 字段".into()
                },
                value: Some(effort.clone()),
                current: effort == &selected_effort,
            })
            .collect();
        self.pending_reasoning = Some(PendingReasoningSelection {
            model: model_id.clone(),
            effort_only,
        });
        self.overlay = Some(Overlay::Picker {
            kind: PickerKind::Reasoning,
            picker: Picker::new(format!("选择 {} 的思考程度", model_id), items),
        });
    }

    fn select_model_from_args(&mut self, rest: &str) {
        let mut args = rest.split_whitespace();
        let model = args.next().unwrap_or_default();
        let effort = args.next();
        if args.next().is_some() {
            self.transcript
                .push_error("用法: /model <模型> [思考程度]".into());
            return;
        }
        let Some(entry) = self.current_model(model) else {
            self.transcript.push_error(format!(
                "provider {:?} 没有模型 {:?}",
                self.active_selection.provider, model
            ));
            return;
        };
        match effort {
            None => self.open_reasoning_picker(model.to_string(), false),
            Some(effort) if entry.efforts.iter().any(|item| item == effort) => {
                self.runtime.submit(AgentCommand::SelectModel {
                    model: model.to_string(),
                    effort: effort.to_string(),
                });
            }
            Some(effort) => self.transcript.push_error(format!(
                "模型 {:?} 不支持思考程度 {:?}，可选: {}",
                model,
                effort,
                entry.efforts.join(", ")
            )),
        }
    }

    fn set_reasoning_from_args(&mut self, rest: &str) {
        let args = rest.split_whitespace().collect::<Vec<_>>();
        if args.len() != 1 {
            self.transcript
                .push_error("用法: /reasoning <思考程度>".into());
            return;
        }
        let effort = args[0];
        let Some(model) = self.current_model(&self.active_selection.model) else {
            self.transcript.push_error("当前模型不在配置目录中".into());
            return;
        };
        if !model.efforts.iter().any(|item| item == effort) {
            self.transcript.push_error(format!(
                "当前模型不支持思考程度 {:?}，可选: {}",
                effort,
                model.efforts.join(", ")
            ));
            return;
        }
        self.runtime
            .submit(AgentCommand::SetReasoningEffort(effort.to_string()));
    }

    fn current_provider(&self) -> Option<&ProviderCatalogEntry> {
        self.provider_catalog
            .iter()
            .find(|provider| provider.name == self.active_selection.provider)
    }

    fn current_model(&self, model: &str) -> Option<&ModelCatalogEntry> {
        self.current_provider()?
            .models
            .iter()
            .find(|entry| entry.id == model)
    }

    fn saved_effort(&self, model: &str) -> Option<&str> {
        self.reasoning_preferences
            .get(&self.active_selection.provider)
            .and_then(|models| models.get(model))
            .map(String::as_str)
    }

    // ---- 输入历史 ----

    fn history_prev(&mut self) {
        if self.history.is_empty() {
            return;
        }
        let idx = match self.history_idx {
            None => {
                self.history_draft = self.input.take();
                self.history.len() - 1
            }
            Some(0) => 0,
            Some(i) => i - 1,
        };
        self.history_idx = Some(idx);
        self.input.set(self.history[idx].clone());
    }

    fn history_next(&mut self) {
        let Some(idx) = self.history_idx else { return };
        if idx + 1 < self.history.len() {
            self.history_idx = Some(idx + 1);
            self.input.set(self.history[idx + 1].clone());
        } else {
            self.history_idx = None;
            let draft = std::mem::take(&mut self.history_draft);
            self.input.set(draft);
        }
    }

    // ---- 渲染 ----

    fn draw(&mut self, frame: &mut Frame) {
        let area = frame.area();
        if area.width < 10 || area.height < 6 {
            frame.render_widget(Paragraph::new("窗口太小"), area);
            return;
        }

        if let Some(Overlay::ToolList { selected }) = self.overlay.as_ref() {
            self.last_overlay_height = area.height.saturating_sub(3).max(1);
            draw_tool_list(frame, area, &self.tool_log, *selected);
            return;
        }
        if let Some(Overlay::ToolDetail {
            tool_call_id,
            scroll,
        }) = self.overlay.as_ref()
        {
            let tool_call_id = tool_call_id.clone();
            let scroll = *scroll;
            self.last_overlay_height = area.height.saturating_sub(3).max(1);
            self.tool_detail_max_scroll =
                draw_tool_detail(frame, area, self.tool_log.get(&tool_call_id), scroll);
            if let Some(Overlay::ToolDetail { scroll, .. }) = self.overlay.as_mut() {
                *scroll = (*scroll).min(self.tool_detail_max_scroll);
            }
            return;
        }

        if self.overlay.is_some() {
            let bottom_h = match &self.overlay {
                Some(Overlay::Picker { picker, .. }) => picker.preferred_height(),
                Some(Overlay::Loading { .. }) => 5,
                Some(Overlay::Approval { .. }) => 8,
                Some(Overlay::ToolList { .. } | Overlay::ToolDetail { .. }) => 0,
                None => 0,
            }
            .min(area.height.saturating_sub(2));
            let [t_area, overlay_area] =
                Layout::vertical([Constraint::Min(1), Constraint::Length(bottom_h)]).areas(area);
            self.draw_transcript(frame, t_area);
            match &mut self.overlay {
                Some(Overlay::Picker { picker, .. }) => picker.render(frame, overlay_area),
                Some(Overlay::Loading { title, .. }) => draw_loading(frame, overlay_area, title),
                Some(Overlay::Approval { request, selected }) => {
                    draw_approval(frame, overlay_area, request, *selected)
                }
                Some(Overlay::ToolList { .. } | Overlay::ToolDetail { .. }) => {}
                None => {}
            }
            return;
        }

        let iv = self
            .input
            .view(area.width.saturating_sub(6), INPUT_MAX_ROWS);
        let input_h = (iv.total_rows.clamp(1, INPUT_MAX_ROWS) as u16) + 2;
        let slash_matches = self.slash_matches();
        let popup_h = slash_matches.len().min(7) as u16;
        let reserved = popup_h.saturating_add(input_h).saturating_add(1);
        let max_plan_h = area.height.saturating_sub(reserved).saturating_sub(1);
        let plan_h = self.plan_panel_height().min(max_plan_h);
        let [t_area, plan_area, p_area, i_area, s_area] = Layout::vertical([
            Constraint::Min(1),
            Constraint::Length(plan_h),
            Constraint::Length(popup_h),
            Constraint::Length(input_h),
            Constraint::Length(1),
        ])
        .areas(area);
        self.draw_transcript(frame, t_area);

        if plan_h > 0 {
            self.draw_plan_panel(frame, plan_area);
        }

        if popup_h > 0 {
            self.draw_slash_popup(frame, p_area, &slash_matches);
        }

        self.draw_composer(frame, i_area, &iv);
        frame.render_widget(self.status_line(), s_area);
    }

    fn plan_panel_height(&self) -> u16 {
        if self.current_plan.revision == 0 || self.current_plan.items.is_empty() {
            return 0;
        }
        if self.plan_collapsed {
            return 1;
        }

        let active = self
            .current_plan
            .items
            .iter()
            .filter(|item| item.status == PlanStatus::InProgress)
            .count();
        let pending = self
            .current_plan
            .items
            .iter()
            .filter(|item| item.status == PlanStatus::Pending)
            .count();
        let explanation = usize::from(
            self.current_plan
                .explanation
                .as_deref()
                .is_some_and(|text| !text.trim().is_empty()),
        );
        let hidden_pending = usize::from(pending > 3);
        1 + u16::try_from(explanation + active + pending.min(3) + hidden_pending)
            .unwrap_or(u16::MAX)
    }

    fn draw_plan_panel(&self, frame: &mut Frame, area: Rect) {
        let counts = self.current_plan.counts();
        let total = self.current_plan.items.len();
        let marker = if self.plan_collapsed { "▸" } else { "▾" };
        let title = Line::from(vec![
            Span::styled(
                format!(" {} 计划 ", marker),
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                format!(
                    "{}/{} 完成 · {} 进行中 · {} 待处理 · #{} ",
                    counts.completed,
                    total,
                    counts.in_progress,
                    counts.pending,
                    self.current_plan.revision
                ),
                Style::default().fg(Color::DarkGray),
            ),
            Span::styled(" Alt+P ", Style::default().fg(Color::DarkGray)),
        ]);
        let block = Block::default()
            .borders(Borders::TOP)
            .border_style(Style::default().fg(Color::DarkGray))
            .title(title);
        let inner = block.inner(area);
        frame.render_widget(block, area);
        if self.plan_collapsed || inner.height == 0 {
            return;
        }

        let mut lines = Vec::new();
        if let Some(explanation) = self
            .current_plan
            .explanation
            .as_deref()
            .map(str::trim)
            .filter(|text| !text.is_empty())
        {
            lines.push(Line::from(vec![
                Span::styled("  最近  ", Style::default().fg(Color::DarkGray)),
                Span::styled(
                    explanation.to_string(),
                    Style::default().fg(Color::DarkGray),
                ),
            ]));
        }
        for item in self
            .current_plan
            .items
            .iter()
            .filter(|item| item.status == PlanStatus::InProgress)
        {
            lines.push(Line::from(vec![
                Span::styled("  › ", Style::default().fg(Color::Cyan)),
                Span::styled(
                    item.text.clone(),
                    Style::default()
                        .fg(Color::Cyan)
                        .add_modifier(Modifier::BOLD),
                ),
            ]));
        }
        let pending: Vec<&PlanItem> = self
            .current_plan
            .items
            .iter()
            .filter(|item| item.status == PlanStatus::Pending)
            .collect();
        for item in pending.iter().take(3) {
            lines.push(Line::from(vec![
                Span::styled("  · ", Style::default().fg(Color::DarkGray)),
                Span::raw(item.text.clone()),
            ]));
        }
        if pending.len() > 3 {
            lines.push(Line::styled(
                format!("    … 另有 {} 项待处理", pending.len() - 3),
                Style::default().fg(Color::DarkGray),
            ));
        }
        frame.render_widget(Paragraph::new(lines).wrap(Wrap { trim: true }), inner);
    }

    fn draw_transcript(&mut self, frame: &mut Frame, area: Rect) {
        self.last_transcript_height = area.height;
        let horizontal_margin = if area.width >= 100 { 3 } else { 1 };
        let t_inner = Rect {
            x: area.x + horizontal_margin,
            y: area.y,
            width: area.width.saturating_sub(horizontal_margin * 2),
            height: area.height,
        };
        let (lines, total) =
            self.transcript
                .visible_lines(t_inner.width, t_inner.height as usize, self.scroll_up);
        self.scroll_up = self
            .scroll_up
            .min(total.saturating_sub(t_inner.height as usize));
        frame.render_widget(Paragraph::new(lines), t_inner);
    }

    fn draw_slash_popup(&self, frame: &mut Frame, area: Rect, matches: &[&command::CommandSpec]) {
        frame.render_widget(Clear, area);
        let name_width = matches
            .iter()
            .map(|spec| spec.name.len() + 1)
            .max()
            .unwrap_or(0);
        let lines: Vec<Line> = matches
            .iter()
            .take(area.height as usize)
            .enumerate()
            .map(|(index, spec)| {
                let active = index == self.slash_selected.min(matches.len().saturating_sub(1));
                let style = if active {
                    Style::default()
                        .fg(Color::Cyan)
                        .add_modifier(Modifier::BOLD)
                } else {
                    Style::default()
                };
                Line::from(vec![
                    Span::styled(
                        format!(
                            "  {:<width$}",
                            format!("/{}", spec.name),
                            width = name_width
                        ),
                        style,
                    ),
                    Span::styled(spec.description, Style::default().fg(Color::DarkGray)),
                ])
            })
            .collect();
        frame.render_widget(Paragraph::new(lines), area);
    }

    fn draw_composer(&self, frame: &mut Frame, area: Rect, iv: &input::InputView) {
        let accent = if self.busy {
            Color::DarkGray
        } else {
            Color::Cyan
        };
        let block = Block::default()
            .borders(Borders::TOP | Borders::BOTTOM)
            .border_style(Style::default().fg(Color::DarkGray));
        let input_inner = block.inner(area);
        frame.render_widget(block, area);
        frame.render_widget(
            Paragraph::new("›").style(Style::default().fg(accent).add_modifier(Modifier::BOLD)),
            Rect {
                x: input_inner.x + 1,
                width: 1,
                ..input_inner
            },
        );
        let text_area = Rect {
            x: input_inner.x + 3,
            width: input_inner.width.saturating_sub(4),
            ..input_inner
        };
        if self.input.is_empty() {
            frame.render_widget(
                Paragraph::new("向 Onemore 提问，输入 / 查看命令")
                    .style(Style::default().fg(Color::DarkGray)),
                text_area,
            );
        } else {
            let rows: Vec<Line> = iv.rows.iter().map(|r| Line::raw(r.clone())).collect();
            frame.render_widget(Paragraph::new(rows), text_area);
        }
        frame.set_cursor_position((
            text_area.x + iv.cursor_col.min(text_area.width.saturating_sub(1)),
            text_area.y + (iv.cursor_row as u16).min(text_area.height.saturating_sub(1)),
        ));
    }

    fn status_line(&self) -> Paragraph<'static> {
        let dim = Style::default().fg(Color::DarkGray);
        let mut spans: Vec<Span> = Vec::new();
        if self.busy {
            spans.push(Span::styled(
                SPINNER[self.spinner_frame].to_string(),
                Style::default().fg(Color::Cyan),
            ));
            spans.push(Span::styled(
                format!(" {} ", self.status_note),
                Style::default().fg(Color::Cyan),
            ));
        } else {
            spans.push(Span::styled("  ready ", Style::default().fg(Color::Green)));
        }
        spans.push(Span::styled(format!("  {} ", self.provider_label), dim));
        spans.push(Span::styled(
            format!(
                "  ↑{} ↓{} ",
                util::fmt_tokens(self.usage.input_tokens),
                util::fmt_tokens(self.usage.output_tokens)
            ),
            dim,
        ));
        if let Some(cache) = self.usage.cache {
            let ratio = cache
                .read_tokens
                .saturating_mul(100)
                .checked_div(self.usage.input_tokens)
                .unwrap_or(0);
            spans.push(Span::styled(
                format!(
                    " cache {}%/w{} ",
                    ratio,
                    util::fmt_tokens(cache.write_tokens)
                ),
                dim,
            ));
        }
        let hint = if self.quit_armed_at.is_some() {
            "  再按一次 Ctrl+C 退出".to_string()
        } else if self.scroll_up > 0 {
            format!("  已上翻 {} 行,Esc 回到底部", self.scroll_up)
        } else if self.busy {
            if self.tool_log.is_empty() {
                "  Esc 取消".to_string()
            } else {
                "  Ctrl+T 工具详情 · Esc 取消".to_string()
            }
        } else {
            "  Enter 发送 · Shift+Enter 换行 · Ctrl+T 工具 · / 命令".to_string()
        };
        spans.push(Span::styled(hint, dim.add_modifier(Modifier::DIM)));
        Paragraph::new(Line::from(spans))
    }
}

fn draw_tool_list(frame: &mut Frame, area: Rect, log: &ToolLog, selected: usize) {
    frame.render_widget(Clear, area);
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::DarkGray))
        .title(Span::styled(
            format!(" 工具调用 ({}) ", log.len()),
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        ));
    let inner = block.inner(area);
    frame.render_widget(block, area);
    if inner.height == 0 {
        return;
    }
    let [list_area, footer_area] =
        Layout::vertical([Constraint::Min(1), Constraint::Length(1)]).areas(inner);
    let selected = selected.min(log.len().saturating_sub(1));
    let page = list_area.height.max(1) as usize;
    let start = selected.saturating_sub(page.saturating_sub(1));
    let lines = (start..(start + page).min(log.len()))
        .filter_map(|index| {
            let record = log.recent(index)?;
            let (status, status_style) = match record.status {
                ToolRunStatus::Running => ("◐", Style::default().fg(Color::Cyan)),
                ToolRunStatus::Succeeded => ("✓", Style::default().fg(Color::Green)),
                ToolRunStatus::Failed => ("×", Style::default().fg(Color::Red)),
            };
            let active = index == selected;
            let style = if active {
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default()
            };
            let prefix = if active { "›" } else { " " };
            let details = if record.invocation_summary.is_empty() {
                String::new()
            } else {
                format!("  {}", record.invocation_summary)
            };
            let text = format!("{:<16} {}{}", record.name, short_id(&record.id), details);
            Some(Line::from(vec![
                Span::styled(format!("{} {} ", prefix, status), status_style),
                Span::styled(
                    util::ellipsis(&text, list_area.width.saturating_sub(4) as usize),
                    style,
                ),
            ]))
        })
        .collect::<Vec<_>>();
    frame.render_widget(Paragraph::new(lines), list_area);
    frame.render_widget(
        Paragraph::new(" ↑↓ 选择 · Enter 详情 · Esc/Ctrl+T 关闭")
            .style(Style::default().fg(Color::DarkGray)),
        footer_area,
    );
}

fn draw_tool_detail(
    frame: &mut Frame,
    area: Rect,
    record: Option<&ToolRecord>,
    scroll: usize,
) -> usize {
    frame.render_widget(Clear, area);
    let (title, border_color) = match record.map(|record| record.status) {
        Some(ToolRunStatus::Running) => (" 工具详情 · 运行中 ", Color::Cyan),
        Some(ToolRunStatus::Succeeded) => (" 工具详情 · 已完成 ", Color::Green),
        Some(ToolRunStatus::Failed) => (" 工具详情 · 失败 ", Color::Red),
        None => (" 工具详情 ", Color::DarkGray),
    };
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(border_color))
        .title(Span::styled(
            title,
            Style::default()
                .fg(border_color)
                .add_modifier(Modifier::BOLD),
        ));
    let inner = block.inner(area);
    frame.render_widget(block, area);
    if inner.height == 0 {
        return 0;
    }
    let [content_area, footer_area] =
        Layout::vertical([Constraint::Min(1), Constraint::Length(1)]).areas(inner);
    let mut lines = Vec::new();
    if let Some(record) = record {
        push_tool_field(
            &mut lines,
            "工具",
            &record.name,
            content_area.width,
            Style::default().add_modifier(Modifier::BOLD),
        );
        push_tool_field(
            &mut lines,
            "调用 ID",
            &record.id,
            content_area.width,
            Style::default().fg(Color::DarkGray),
        );
        if !record.invocation_summary.is_empty() {
            push_tool_field(
                &mut lines,
                "参数",
                &record.invocation_summary,
                content_area.width,
                Style::default(),
            );
        }
        if let Some(output) = &record.output {
            if let Some(command) = output.metadata.command.as_deref() {
                push_tool_field(
                    &mut lines,
                    "命令",
                    command,
                    content_area.width,
                    Style::default().fg(Color::Yellow),
                );
            }
            if let Some(cwd) = output.metadata.cwd.as_deref() {
                push_tool_field(
                    &mut lines,
                    "目录",
                    cwd,
                    content_area.width,
                    Style::default().fg(Color::DarkGray),
                );
            }
        }
        let exit = record
            .output
            .as_ref()
            .and_then(|output| output.metadata.exit_code)
            .map(|code| format!(" · exit {code}"))
            .unwrap_or_default();
        push_tool_field(
            &mut lines,
            "耗时",
            &format!("{} ms{}", record.elapsed_ms(), exit),
            content_area.width,
            Style::default().fg(Color::DarkGray),
        );
        if let Some(error) = record.error.as_deref() {
            push_tool_field(
                &mut lines,
                "错误",
                error,
                content_area.width,
                Style::default().fg(Color::Red),
            );
        }
        if let Some(output) = &record.output {
            if !output.summary.is_empty() {
                push_tool_field(
                    &mut lines,
                    "状态",
                    &output.summary,
                    content_area.width,
                    Style::default().fg(Color::DarkGray),
                );
            }
            lines.push(Line::raw(""));
            lines.push(Line::styled(
                "完整输出",
                Style::default().add_modifier(Modifier::BOLD),
            ));
            if output.content.is_empty() {
                lines.push(Line::styled(
                    "(暂无输出)",
                    Style::default().fg(Color::DarkGray),
                ));
            } else {
                for logical in output.content.split('\n') {
                    push_wrapped(&mut lines, logical, content_area.width, Style::default());
                }
            }
        } else {
            lines.push(Line::raw(""));
            lines.push(Line::styled(
                "工具正在运行，进度事件会在此实时更新。",
                Style::default().fg(Color::DarkGray),
            ));
        }
    } else {
        lines.push(Line::styled(
            "该工具记录已不可用。",
            Style::default().fg(Color::Red),
        ));
    }

    let page = content_area.height.max(1) as usize;
    let max_scroll = lines.len().saturating_sub(page);
    let scroll = scroll.min(max_scroll);
    let end = (scroll + page).min(lines.len());
    frame.render_widget(Paragraph::new(lines[scroll..end].to_vec()), content_area);
    frame.render_widget(
        Paragraph::new(format!(
            " ↑↓/PgUp/PgDn 滚动 · Home/End · Backspace 列表 · Esc/Ctrl+T 关闭   {}/{}",
            if lines.is_empty() { 0 } else { scroll + 1 },
            lines.len()
        ))
        .style(Style::default().fg(Color::DarkGray)),
        footer_area,
    );
    max_scroll
}

fn push_tool_field(
    lines: &mut Vec<Line<'static>>,
    label: &str,
    value: &str,
    width: u16,
    value_style: Style,
) {
    let prefix = format!("{label}  ");
    let prefix_width = UnicodeWidthStr::width(prefix.as_str());
    let continuation = " ".repeat(prefix_width);
    let available = width.saturating_sub(prefix_width as u16).max(8) as usize;
    let wrapped = textwrap::wrap(value, textwrap::Options::new(available));
    if wrapped.is_empty() {
        lines.push(Line::styled(prefix, Style::default().fg(Color::DarkGray)));
        return;
    }
    for (index, part) in wrapped.into_iter().enumerate() {
        lines.push(Line::from(vec![
            Span::styled(
                if index == 0 {
                    prefix.clone()
                } else {
                    continuation.clone()
                },
                Style::default().fg(Color::DarkGray),
            ),
            Span::styled(part.into_owned(), value_style),
        ]));
    }
}

fn push_wrapped(lines: &mut Vec<Line<'static>>, text: &str, width: u16, style: Style) {
    if text.is_empty() {
        lines.push(Line::raw(""));
        return;
    }
    for part in textwrap::wrap(text, textwrap::Options::new(width.max(8) as usize)) {
        lines.push(Line::styled(part.into_owned(), style));
    }
}

/// Ratatui 0.29 的 inline `insert_before` 会把临时 Buffer 的每一格都交给 backend，
/// 不像正常 diff 渲染那样跳过宽字符的 continuation cell。把这些占位格改为空
/// symbol，避免它们在中文字符的第二列重新打印一个空格。
fn clear_wide_continuation_cells(buffer: &mut ratatui::buffer::Buffer) {
    for y in buffer.area.top()..buffer.area.bottom() {
        let mut continuation = 0usize;
        for x in buffer.area.left()..buffer.area.right() {
            if continuation > 0 {
                if let Some(cell) = buffer.cell_mut((x, y)) {
                    cell.set_symbol("");
                }
                continuation -= 1;
                continue;
            }
            continuation = buffer
                .cell((x, y))
                .map(|cell| UnicodeWidthStr::width(cell.symbol()).saturating_sub(1))
                .unwrap_or(0);
        }
    }
}

fn draw_loading(frame: &mut Frame, area: Rect, title: &str) {
    frame.render_widget(Clear, area);
    let block = Block::default()
        .borders(Borders::TOP)
        .border_style(Style::default().fg(Color::DarkGray));
    let inner = block.inner(area);
    frame.render_widget(block, area);
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled("  ⠋ ", Style::default().fg(Color::Cyan)),
            Span::styled(title.to_string(), Style::default().fg(Color::DarkGray)),
        ])),
        inner,
    );
}

fn approval_choices(request: &ApprovalRequest) -> Vec<ApprovalChoice> {
    let mut choices = Vec::with_capacity(3);
    if request.scopes.contains(&ApprovalScope::Once) {
        choices.push(ApprovalChoice::Once);
    }
    if request.scopes.contains(&ApprovalScope::Session) {
        choices.push(ApprovalChoice::Session);
    }
    choices.push(ApprovalChoice::Deny);
    choices
}

fn approval_decision(choice: ApprovalChoice) -> ApprovalDecision {
    match choice {
        ApprovalChoice::Once => ApprovalDecision::Allow(ApprovalScope::Once),
        ApprovalChoice::Session => ApprovalDecision::Allow(ApprovalScope::Session),
        ApprovalChoice::Deny => ApprovalDecision::Deny,
    }
}

fn draw_approval(frame: &mut Frame, area: Rect, request: &ApprovalRequest, selected: usize) {
    frame.render_widget(Clear, area);
    let block = Block::default()
        .borders(Borders::TOP)
        .border_style(Style::default().fg(Color::Yellow))
        .title(Span::styled(
            " 工具审批 ",
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        ));
    let inner = block.inner(area);
    frame.render_widget(block, area);
    let mut options = vec![Span::raw("  ")];
    for (index, choice) in approval_choices(request).iter().enumerate() {
        let label = match choice {
            ApprovalChoice::Once => "允许一次",
            ApprovalChoice::Session => "本会话允许",
            ApprovalChoice::Deny => "拒绝",
        };
        let style = if index == selected {
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD)
        } else if *choice == ApprovalChoice::Deny {
            Style::default().fg(Color::Red)
        } else {
            Style::default()
        };
        options.push(Span::styled(
            format!("{} {}  ", if index == selected { "›" } else { " " }, label),
            style,
        ));
    }
    let text = vec![
        Line::from(vec![
            Span::styled("  工具  ", Style::default().fg(Color::DarkGray)),
            Span::styled(&request.tool, Style::default().add_modifier(Modifier::BOLD)),
        ]),
        Line::from(vec![
            Span::styled("  参数  ", Style::default().fg(Color::DarkGray)),
            Span::raw(util::ellipsis(&request.summary, 180)),
        ]),
        Line::from(vec![
            Span::styled("  目录  ", Style::default().fg(Color::DarkGray)),
            Span::raw(
                request
                    .details
                    .cwd
                    .as_deref()
                    .map(|cwd| util::ellipsis(cwd, 180))
                    .unwrap_or_else(|| "-".into()),
            ),
        ]),
        Line::from(vec![
            Span::styled("  原因  ", Style::default().fg(Color::DarkGray)),
            Span::raw(request.reason.clone()),
        ]),
        Line::from(options),
        Line::from(Span::styled(
            "  ←→/↑↓ 选择  Enter 确认  Esc 拒绝",
            Style::default().fg(Color::DarkGray),
        )),
    ];
    frame.render_widget(Paragraph::new(text).wrap(Wrap { trim: true }), inner);
}

fn short_id(id: &str) -> &str {
    id.get(..8).unwrap_or(id)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::skills::SkillMetadata;
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;
    use std::sync::atomic::AtomicBool;
    use std::sync::Arc;

    struct FakeUiRuntime {
        commands: std::sync::mpsc::Sender<AgentCommand>,
        approvals: std::sync::mpsc::Sender<ApprovalResponse>,
        cancel: Arc<AtomicBool>,
    }

    impl UiRuntime for FakeUiRuntime {
        fn submit(&self, command: AgentCommand) {
            let _ = self.commands.send(command);
        }

        fn cancel(&self) {
            self.cancel
                .store(true, std::sync::atomic::Ordering::Relaxed);
        }

        fn shutdown(&self) {
            self.cancel();
            let _ = self.commands.send(AgentCommand::Shutdown);
        }

        fn approve(&self, response: ApprovalResponse) {
            let _ = self.approvals.send(response);
        }
    }

    /// 造一个没有真实 Runtime 的 App;返回事件发送端与命令接收端,
    /// 便于测试注入事件、断言提交行为。
    fn dummy_app() -> (
        App,
        std::sync::mpsc::Sender<AgentEvent>,
        std::sync::mpsc::Receiver<AgentCommand>,
        std::sync::mpsc::Receiver<ApprovalResponse>,
    ) {
        let (cmd_tx, cmd_rx) = std::sync::mpsc::channel();
        let (approval_tx, approval_rx) = std::sync::mpsc::channel();
        let (evt_tx, _evt_rx) = std::sync::mpsc::channel();
        let runtime = FakeUiRuntime {
            commands: cmd_tx,
            approvals: approval_tx,
            cancel: Arc::new(AtomicBool::new(false)),
        };
        let active_selection = ActiveModelSelection {
            provider: "mock".into(),
            model: "test-model".into(),
            effort: "medium".into(),
        };
        let provider_catalog = vec![
            ProviderCatalogEntry {
                name: "mock".into(),
                default_model: "test-model".into(),
                models: vec![
                    ModelCatalogEntry {
                        id: "test-model".into(),
                        context_window: Some(100_000),
                        max_tokens: Some(8_000),
                        efforts: vec!["low".into(), "medium".into(), "high".into()],
                        default_effort: "medium".into(),
                        sends_effort: true,
                    },
                    ModelCatalogEntry {
                        id: "other-model".into(),
                        context_window: Some(50_000),
                        max_tokens: Some(4_000),
                        efforts: vec!["low".into(), "high".into(), "max".into()],
                        default_effort: "low".into(),
                        sends_effort: true,
                    },
                ],
            },
            ProviderCatalogEntry {
                name: "other-provider".into(),
                default_model: "foreign-model".into(),
                models: vec![ModelCatalogEntry {
                    id: "foreign-model".into(),
                    context_window: Some(32_000),
                    max_tokens: None,
                    efforts: vec!["medium".into()],
                    default_effort: "medium".into(),
                    sends_effort: false,
                }],
            },
        ];
        let reasoning_preferences = BTreeMap::from([(
            "mock".into(),
            BTreeMap::from([("other-model".into(), "high".into())]),
        )]);
        (
            App::new(
                Box::new(runtime),
                None,
                "mock / test-model / effort=medium".into(),
                active_selection,
                provider_catalog,
                reasoning_preferences,
                "12345678-1234-1234-1234-123456789abc".into(),
            ),
            evt_tx,
            cmd_rx,
            approval_rx,
        )
    }

    /// 完整过一遍事件流 + 输入操作 + 各种尺寸渲染,不允许 panic。
    #[test]
    fn renders_without_panic() {
        let (mut app, _evt, _cmd, _approvals) = dummy_app();
        // 模拟一轮对话的事件序列
        app.on_agent_event(AgentEvent::UserMessage(
            "读一下 main.rs,中文也要能换行显示".into(),
        ));
        app.on_agent_event(AgentEvent::TurnStarted);
        app.on_agent_event(AgentEvent::ThinkingDelta("让我想想……".into()));
        app.on_agent_event(AgentEvent::AssistantDelta("好的,".into()));
        app.on_agent_event(AgentEvent::AssistantDelta("我来读取。".into()));
        app.on_agent_event(AgentEvent::ToolCallPending {
            name: "read_file".into(),
        });
        app.on_agent_event(AgentEvent::ToolCallStarted {
            id: "t1".into(),
            name: "read_file".into(),
            summary: "path=src/main.rs".into(),
        });
        app.on_agent_event(AgentEvent::ToolCallFinished {
            id: "t1".into(),
            name: "read_file".into(),
            output: crate::tools::ToolOutput::text(
                (1..=30)
                    .map(|i| format!("{} | line", i))
                    .collect::<Vec<_>>()
                    .join("\n"),
            ),
            error: None,
        });
        app.on_agent_event(AgentEvent::Usage {
            input_tokens: 1234,
            output_tokens: 567,
            cache: None,
        });
        app.on_agent_event(AgentEvent::AssistantMessage(
            "好的,我来读取。完成了。".into(),
        ));
        app.on_agent_event(AgentEvent::Error("演示一个错误".into()));
        app.on_agent_event(AgentEvent::TurnFinished { cancelled: false });

        // 输入一些中英混排 + 多行
        for c in "写个 hello".chars() {
            app.on_key(KeyCode::Char(c), KeyModifiers::NONE);
        }
        app.on_key(KeyCode::Enter, KeyModifiers::SHIFT);
        for c in "第二行".chars() {
            app.on_key(KeyCode::Char(c), KeyModifiers::NONE);
        }
        app.on_key(KeyCode::PageUp, KeyModifiers::NONE);

        for (w, h) in [(80u16, 24u16), (120, 40), (20, 8), (10, 6), (9, 5)] {
            let mut term = Terminal::new(TestBackend::new(w, h)).unwrap();
            term.draw(|f| app.draw(f)).unwrap();
        }

        // 抽查:正常尺寸下画面里能看到状态栏的 provider 名
        let mut term = Terminal::new(TestBackend::new(100, 30)).unwrap();
        term.draw(|f| app.draw(f)).unwrap();
        let content = format!("{:?}", term.backend().buffer());
        assert!(content.contains("mock"), "状态栏应显示 provider 名");
    }

    #[test]
    fn active_loop_releases_and_renders_each_streaming_phase_before_settled() {
        let (mut app, _evt, _cmd, _approvals) = dummy_app();
        app.on_progress(ProgressEvent::RunStarted {
            command_id: "command-1".into(),
        });
        app.on_progress(ProgressEvent::AssistantDelta {
            message_id: "message-1".into(),
            content_index: 0,
            kind: "thinking".into(),
            delta: "正在分析项目结构".into(),
        });
        app.on_progress(ProgressEvent::AssistantDelta {
            message_id: "message-1".into(),
            content_index: 0,
            kind: "text".into(),
            delta: "先读取配置。".into(),
        });

        let committed_thinking = format!("{:?}", app.transcript.drain_finalized_lines(80));
        assert!(committed_thinking.contains("正在分析项目结构"));
        let mut terminal = Terminal::new(TestBackend::new(80, 12)).unwrap();
        terminal.draw(|frame| app.draw(frame)).unwrap();
        let live_assistant = format!("{:?}", terminal.backend().buffer());
        assert!(live_assistant.contains("先读取配置。"));

        app.on_progress(ProgressEvent::AssistantFinished {
            message_id: "message-1".into(),
            text: "先读取配置。".into(),
        });
        let committed_assistant = format!("{:?}", app.transcript.drain_finalized_lines(80));
        assert!(committed_assistant.contains("先读取配置。"));

        app.on_progress(ProgressEvent::ToolStarted {
            tool_call_id: "tool-1".into(),
            name: "read_file".into(),
            summary: "config.toml".into(),
        });
        terminal.draw(|frame| app.draw(frame)).unwrap();
        let running_tool = format!("{:?}", terminal.backend().buffer());
        assert!(running_tool.contains("read_file"));
        assert!(running_tool.contains("运行中"));

        app.on_progress(ProgressEvent::ToolFinished {
            tool_call_id: "tool-1".into(),
            name: "read_file".into(),
            output: crate::sdk::ToolOutputView {
                content: "读取完成".into(),
                summary: "已读取 config.toml".into(),
                metadata: crate::sdk::ToolMetadataView::default(),
            },
            error: None,
        });
        let committed_tool = format!("{:?}", app.transcript.drain_finalized_lines(80));
        assert!(committed_tool.contains("read_file"));
        assert!(committed_tool.contains("读取完成"));
        assert!(app.busy, "Settled 前 loop 应保持运行状态");
    }

    #[test]
    fn tool_progress_updates_by_id_and_detail_overlay_stays_live() {
        let (mut app, _evt, _cmd, _approvals) = dummy_app();
        app.on_progress(ProgressEvent::RunStarted {
            command_id: "command-tools".into(),
        });
        app.on_progress(ProgressEvent::ToolStarted {
            tool_call_id: "tool-a".into(),
            name: "run_command".into(),
            summary: "cargo test".into(),
        });
        app.on_progress(ProgressEvent::ToolStarted {
            tool_call_id: "tool-b".into(),
            name: "read_file".into(),
            summary: "README.md".into(),
        });
        app.on_progress(ProgressEvent::ToolUpdated {
            tool_call_id: "tool-a".into(),
            name: "run_command".into(),
            output: crate::sdk::ToolOutputView {
                content: "first line".into(),
                summary: "1/3 complete".into(),
                metadata: crate::sdk::ToolMetadataView::default(),
            },
        });
        let mut terminal = Terminal::new(TestBackend::new(90, 18)).unwrap();
        terminal.draw(|frame| app.draw(frame)).unwrap();
        let live = format!("{:?}", terminal.backend().buffer());
        assert!(live.contains("1/3 complete"));
        assert!(live.contains("run_command"));

        app.on_progress(ProgressEvent::ToolFinished {
            tool_call_id: "tool-a".into(),
            name: "run_command".into(),
            output: crate::sdk::ToolOutputView {
                content: "stdout\nlast line".into(),
                summary: "命令执行成功".into(),
                metadata: crate::sdk::ToolMetadataView {
                    command: Some("cargo test".into()),
                    cwd: Some("E:\\work".into()),
                    elapsed_ms: Some(1250),
                    exit_code: Some(0),
                },
            },
            error: None,
        });
        app.handle_slash("tools");
        assert!(matches!(app.overlay, Some(Overlay::ToolList { .. })));
        app.on_key(KeyCode::Down, KeyModifiers::NONE);
        app.on_key(KeyCode::Enter, KeyModifiers::NONE);
        assert!(matches!(app.overlay, Some(Overlay::ToolDetail { .. })));
        terminal.draw(|frame| app.draw(frame)).unwrap();
        let detail = format!("{:?}", terminal.backend().buffer());
        assert!(detail.contains("cargo test"));
        assert!(detail.contains("stdout"));
        assert!(detail.contains("1250 ms"));
        assert!(app.busy, "工具详情打开时不能阻塞 active loop");
    }

    #[test]
    fn inline_viewport_scales_with_terminal_height() {
        assert_eq!(inline_viewport_rows(6), 6);
        assert_eq!(inline_viewport_rows(16), 8);
        assert_eq!(inline_viewport_rows(24), 12);
        assert_eq!(inline_viewport_rows(40), 16);
    }

    #[test]
    fn plan_panel_stays_visible_after_history_drains_and_can_collapse() {
        let (mut app, _evt, _cmd, _approvals) = dummy_app();
        app.on_progress(ProgressEvent::PlanUpdated {
            plan: crate::sdk::PlanView {
                revision: 3,
                items: vec![
                    crate::sdk::PlanItemView {
                        id: "active".into(),
                        text: "实现固定计划面板".into(),
                        status: PlanStatus::InProgress,
                    },
                    crate::sdk::PlanItemView {
                        id: "pending".into(),
                        text: "补充工具详情".into(),
                        status: PlanStatus::Pending,
                    },
                    crate::sdk::PlanItemView {
                        id: "done".into(),
                        text: "修复流式输出".into(),
                        status: PlanStatus::Completed,
                    },
                ],
                explanation: Some("计划进入界面实现阶段".into()),
            },
        });

        let history = format!("{:?}", app.transcript.drain_finalized_lines(80));
        assert!(history.contains("计划 #3"));
        let mut terminal = Terminal::new(TestBackend::new(80, 16)).unwrap();
        terminal.draw(|frame| app.draw(frame)).unwrap();
        let expanded = format!("{:?}", terminal.backend().buffer());
        assert!(expanded.contains("1/3 完成"));
        assert!(expanded.contains("实现固定计划面板"));
        assert!(expanded.contains("补充工具详情"));
        assert!(expanded.contains("计划进入界面实现阶段"));

        app.on_key(KeyCode::Char('p'), KeyModifiers::ALT);
        let mut collapsed_terminal = Terminal::new(TestBackend::new(80, 16)).unwrap();
        collapsed_terminal.draw(|frame| app.draw(frame)).unwrap();
        let collapsed = format!("{:?}", collapsed_terminal.backend().buffer());
        assert!(collapsed.contains("1/3 完成"));
        assert!(!collapsed.contains("实现固定计划面板"));
        assert!(!collapsed.contains("计划进入界面实现阶段"));
    }

    /// 事件驱动的滚动边界:滚过头会被钳制,不会越界 panic。
    #[test]
    fn scroll_is_clamped() {
        let (mut app, _evt, _cmd, _approvals) = dummy_app();
        for i in 0..50 {
            app.on_agent_event(AgentEvent::Notice(format!("第 {} 条", i)));
        }
        app.scroll_up = 10_000;
        let mut term = Terminal::new(TestBackend::new(60, 20)).unwrap();
        term.draw(|f| app.draw(f)).unwrap();
        assert!(app.scroll_up < 10_000, "渲染后 scroll 应被钳制");
    }

    /// Enter 必须发送——即使事件队列里躺着 Release 噪音(conpty 终端
    /// 会把按下/抬起同时入队,这曾导致 Enter 永远被当成换行)。
    #[test]
    fn enter_submits() {
        let (mut app, _evt, cmd, _approvals) = dummy_app();
        for c in "你好".chars() {
            app.on_key(KeyCode::Char(c), KeyModifiers::NONE);
        }
        app.on_key(KeyCode::Enter, KeyModifiers::NONE);
        match cmd.try_recv() {
            Ok(AgentCommand::UserInput(t)) => assert_eq!(t, "你好"),
            other => panic!("应收到 UserInput,得到 {:?}", other),
        }
    }

    #[test]
    fn session_slash_commands_are_forwarded() {
        let (mut app, _evt, cmd, _approvals) = dummy_app();
        app.handle_slash("session");
        assert!(matches!(
            cmd.recv().unwrap(),
            AgentCommand::ListSessions { all: false }
        ));

        app.handle_slash("session abc12345");
        match cmd.recv().unwrap() {
            AgentCommand::LoadSession(id) => assert_eq!(id, "abc12345"),
            other => panic!("应收到 LoadSession，得到 {:?}", other),
        }
    }

    #[test]
    fn session_list_becomes_an_interactive_picker() {
        use crate::storage::SessionSummary;

        let (mut app, _evt, cmd, _approvals) = dummy_app();
        app.handle_slash("session");
        assert!(matches!(
            cmd.recv().unwrap(),
            AgentCommand::ListSessions { all: false }
        ));
        assert!(matches!(
            app.overlay,
            Some(Overlay::Loading {
                kind: PickerKind::Session,
                ..
            })
        ));

        app.on_agent_event(AgentEvent::SessionsListed {
            current_id: "current-session".into(),
            sessions: vec![
                SessionSummary {
                    id: "current-session".into(),
                    title: "当前工作".into(),
                    workspace: "E:\\work".into(),
                    message_count: 8,
                    updated_at: 2,
                },
                SessionSummary {
                    id: "older-session".into(),
                    title: "旧会话".into(),
                    workspace: "E:\\work".into(),
                    message_count: 3,
                    updated_at: 1,
                },
            ],
        });
        assert!(matches!(
            app.overlay,
            Some(Overlay::Picker {
                kind: PickerKind::Session,
                ..
            })
        ));

        app.on_key(KeyCode::Down, KeyModifiers::NONE);
        app.on_key(KeyCode::Enter, KeyModifiers::NONE);
        assert!(matches!(
            cmd.try_recv(),
            Ok(AgentCommand::LoadSession(id)) if id == "older-session"
        ));
    }

    /// 行尾反斜杠 + Enter = 续行,不发送;下一次 Enter 正常发送多行内容。
    #[test]
    fn backslash_enter_continues_line() {
        let (mut app, _evt, cmd, _approvals) = dummy_app();
        for c in "第一行\\".chars() {
            app.on_key(KeyCode::Char(c), KeyModifiers::NONE);
        }
        app.on_key(KeyCode::Enter, KeyModifiers::NONE);
        assert!(cmd.try_recv().is_err(), "续行不应发送");
        for c in "第二行".chars() {
            app.on_key(KeyCode::Char(c), KeyModifiers::NONE);
        }
        app.on_key(KeyCode::Enter, KeyModifiers::NONE);
        match cmd.try_recv() {
            Ok(AgentCommand::UserInput(t)) => assert_eq!(t, "第一行\n第二行"),
            other => panic!("应收到 UserInput,得到 {:?}", other),
        }
    }

    #[test]
    fn skill_picker_dispatches_an_ordinary_user_input() {
        let (mut app, _evt, cmd, _approvals) = dummy_app();
        app.on_agent_event(AgentEvent::SkillsDiscovered {
            skills: vec![SkillMetadata {
                name: "demo".into(),
                description: "demo skill".into(),
                scope: crate::skills::SkillScope::Repo,
                path: "workspace/.agents/skills/demo/SKILL.md".into(),
                content_hash: [0; 32],
            }],
            warnings: Vec::new(),
        });
        app.handle_slash("skill");
        assert!(matches!(
            app.overlay,
            Some(Overlay::Picker {
                kind: PickerKind::Skill,
                ..
            })
        ));
        app.on_key(KeyCode::Enter, KeyModifiers::NONE);
        match cmd.try_recv() {
            Ok(AgentCommand::UserInput(text)) => assert!(text.contains("demo")),
            other => panic!("应收到 UserInput,得到 {:?}", other),
        }
    }

    #[test]
    fn slash_popup_navigates_completes_and_dispatches() {
        let (mut app, _evt, cmd, _approvals) = dummy_app();
        app.on_key(KeyCode::Char('/'), KeyModifiers::NONE);
        app.on_key(KeyCode::Char('p'), KeyModifiers::NONE);
        assert_eq!(app.selected_slash_command().unwrap().name, "provider");

        app.on_key(KeyCode::Tab, KeyModifiers::NONE);
        assert_eq!(app.input.text(), "/provider ");
        for c in "mock".chars() {
            app.on_key(KeyCode::Char(c), KeyModifiers::NONE);
        }
        app.on_key(KeyCode::Enter, KeyModifiers::NONE);
        assert!(app.overlay.is_none());
        assert!(matches!(
            cmd.try_recv(),
            Ok(AgentCommand::SwitchProvider(name)) if name == "mock"
        ));
    }

    #[test]
    fn provider_picker_sends_selected_provider() {
        let (mut app, _evt, cmd, _approvals) = dummy_app();
        app.handle_slash("provider");
        assert!(matches!(
            app.overlay,
            Some(Overlay::Picker {
                kind: PickerKind::Provider,
                ..
            })
        ));
        app.on_key(KeyCode::Enter, KeyModifiers::NONE);
        match cmd.try_recv() {
            Ok(AgentCommand::SwitchProvider(name)) => assert_eq!(name, "mock"),
            other => panic!("应收到 SwitchProvider,得到 {:?}", other),
        }
    }

    #[test]
    fn model_picker_filters_provider_and_selects_effort_before_sending() {
        let (mut app, _evt, cmd, _approvals) = dummy_app();
        app.handle_slash("model");
        let Some(Overlay::Picker { kind, picker }) = &app.overlay else {
            panic!("应打开模型选择器");
        };
        assert_eq!(*kind, PickerKind::Model);
        assert_eq!(
            picker
                .items
                .iter()
                .map(|item| item.label.as_str())
                .collect::<Vec<_>>(),
            vec!["test-model", "other-model"]
        );

        app.on_key(KeyCode::Down, KeyModifiers::NONE);
        app.on_key(KeyCode::Enter, KeyModifiers::NONE);
        assert!(cmd.try_recv().is_err(), "选择模型时不应提前提交");
        assert!(matches!(
            app.overlay,
            Some(Overlay::Picker {
                kind: PickerKind::Reasoning,
                ..
            })
        ));
        let Some(Overlay::Picker { picker, .. }) = &app.overlay else {
            unreachable!();
        };
        assert_eq!(
            picker
                .items
                .iter()
                .map(|item| (item.label.as_str(), item.current))
                .collect::<Vec<_>>(),
            vec![("low", false), ("high", true), ("max", false)]
        );
        let mut terminal = Terminal::new(TestBackend::new(72, 16)).unwrap();
        terminal.draw(|frame| app.draw(frame)).unwrap();
        let rendered = format!("{:?}", terminal.backend().buffer());
        assert!(rendered.contains("low"));
        assert!(rendered.contains("high"));
        assert!(rendered.contains("max"));
        assert!(!rendered.contains("medium"));
        app.on_key(KeyCode::Enter, KeyModifiers::NONE);
        match cmd.try_recv() {
            Ok(AgentCommand::SelectModel { model, effort }) => {
                assert_eq!(model, "other-model");
                assert_eq!(effort, "high");
            }
            other => panic!("应收到原子 SelectModel,得到 {:?}", other),
        }
    }

    #[test]
    fn model_default_effort_clears_the_workspace_override() {
        let (mut app, _evt, _cmd, _approvals) = dummy_app();
        assert_eq!(app.saved_effort("other-model"), Some("high"));
        app.on_agent_event(AgentEvent::ModelSelectionChanged {
            provider: "mock".into(),
            model: "other-model".into(),
            effort: "low".into(),
            label: "mock / other-model / effort=low".into(),
        });
        assert_eq!(app.saved_effort("other-model"), None);
    }

    #[test]
    fn approval_overlay_uses_the_dedicated_response_channel() {
        let (mut app, _evt, _cmd, approvals) = dummy_app();
        app.on_agent_event(AgentEvent::PermissionRequested {
            request: ApprovalRequest {
                request_id: "approval-once".into(),
                tool: "dynamic_tool".into(),
                summary: "action=true".into(),
                reason: "external side effect".into(),
                scopes: vec![ApprovalScope::Once, ApprovalScope::Session],
                details: crate::permission::ApprovalDetails::default(),
            },
        });
        assert!(matches!(app.overlay, Some(Overlay::Approval { .. })));
        let mut term = Terminal::new(TestBackend::new(72, 18)).unwrap();
        term.draw(|frame| app.draw(frame)).unwrap();
        let content = format!("{:?}", term.backend().buffer());
        assert!(content.contains("允许一次"));
        assert!(content.contains("本会话允许"));
        assert!(content.contains("拒绝"));
        app.on_key(KeyCode::Down, KeyModifiers::NONE);
        app.on_key(KeyCode::Enter, KeyModifiers::NONE);
        assert_eq!(
            approvals.recv().unwrap(),
            ApprovalResponse {
                request_id: "approval-once".into(),
                decision: ApprovalDecision::Allow(ApprovalScope::Session),
            }
        );
        assert!(app.overlay.is_none());

        app.on_agent_event(AgentEvent::PermissionRequested {
            request: ApprovalRequest {
                request_id: "approval-deny".into(),
                tool: "dynamic_tool".into(),
                summary: String::new(),
                reason: "ask".into(),
                scopes: vec![ApprovalScope::Once],
                details: crate::permission::ApprovalDetails::default(),
            },
        });
        app.on_key(KeyCode::Right, KeyModifiers::NONE);
        app.on_key(KeyCode::Enter, KeyModifiers::NONE);
        assert_eq!(
            approvals.recv().unwrap(),
            ApprovalResponse {
                request_id: "approval-deny".into(),
                decision: ApprovalDecision::Deny,
            }
        );
    }

    #[test]
    fn slash_and_picker_views_render_expected_content() {
        let (mut app, _evt, _cmd, _approvals) = dummy_app();
        app.input.set("/mo".into());
        let mut term = Terminal::new(TestBackend::new(72, 18)).unwrap();
        term.draw(|f| app.draw(f)).unwrap();
        let content = format!("{:?}", term.backend().buffer());
        assert!(content.contains("/model"));

        app.handle_slash("model");
        term.draw(|f| app.draw(f)).unwrap();
        let content = format!("{:?}", term.backend().buffer());
        assert!(content.contains("选择模型"));
        assert!(content.contains("test-model"));
        assert!(content.contains("other-model"));
        assert!(!content.contains("foreign-model"));
    }

    #[test]
    fn scrollback_buffer_clears_wide_character_continuations() {
        let area = Rect::new(0, 0, 8, 1);
        let mut buffer = ratatui::buffer::Buffer::empty(area);
        Paragraph::new("完成").render(area, &mut buffer);

        clear_wide_continuation_cells(&mut buffer);

        assert_eq!(buffer.cell((0, 0)).unwrap().symbol(), "完");
        assert_eq!(buffer.cell((1, 0)).unwrap().symbol(), "");
        assert_eq!(buffer.cell((2, 0)).unwrap().symbol(), "成");
        assert_eq!(buffer.cell((3, 0)).unwrap().symbol(), "");
    }
}
