//! 聊天区:把事件流积累成"单元格"列表,并负责换行与渲染缓存。
//!
//! 一个 Cell 对应画面上一段内容(用户消息 / 助手消息 / 思考 / 工具调用 /
//! 提示 / 错误)。流式增量不断追加到"开放中"的 Cell 上。
//! 换行结果按 (宽度, 版本号) 缓存——只有变过的 Cell 才重新排版,
//! 长对话下每帧渲染依然轻量。

use ratatui::style::{Color, Modifier, Style};
use ratatui::text::Line;
use textwrap::Options;

use crate::plan::{PlanItem, PlanStatus};
use crate::util;

/// 工具输出在聊天区最多显示的视觉行数(完整内容始终在消息历史里)。
const TOOL_PREVIEW_LINES: usize = 10;

pub enum Cell {
    User(String),
    Assistant {
        text: String,
        open: bool,
    },
    Thinking {
        text: String,
        open: bool,
    },
    Tool {
        /// ToolUse 的调用 ID。并发批次的完成事件按完成顺序到达,
        /// 必须按 id 配对,不能假设"最近开放的 Cell 就是这次结束的调用"。
        id: String,
        name: String,
        summary: String,
        progress: Option<String>,
        output: Option<String>,
        is_error: bool,
    },
    Plan {
        revision: u64,
        items: Vec<PlanItem>,
        explanation: Option<String>,
    },
    Compaction {
        id: String,
        automatic: bool,
        estimated_tokens: u64,
        available_tokens: Option<u64>,
        outcome: CompactionOutcome,
    },
    Notice(String),
    Error(String),
}

pub enum CompactionOutcome {
    Running,
    Finished {
        tokens_before: u64,
        summary_chars: usize,
        retained_messages: usize,
    },
    Failed {
        error: String,
        cancelled: bool,
        history_changed: bool,
    },
}

struct Entry {
    cell: Cell,
    version: u64,
    cache: Option<(u16, u64, Vec<Line<'static>>)>,
}

impl Entry {
    fn new(cell: Cell) -> Self {
        Entry {
            cell,
            version: 0,
            cache: None,
        }
    }

    fn touch(&mut self) {
        self.version += 1;
    }

    fn lines(&mut self, width: u16) -> &Vec<Line<'static>> {
        let valid = matches!(&self.cache, Some((w, v, _)) if *w == width && *v == self.version);
        if !valid {
            let lines = build_lines(&self.cell, width);
            self.cache = Some((width, self.version, lines));
        }
        &self.cache.as_ref().unwrap().2
    }
}

#[derive(Default)]
pub struct Transcript {
    entries: Vec<Entry>,
    layout: LayoutCache,
    #[cfg(test)]
    layout_rebuilds: usize,
}

/// 当前宽度下每个 Entry 的起始行偏移。
/// 历史稳定时，滚动只需二分定位可见 Entry，不再遍历整段会话。
#[derive(Default)]
struct LayoutCache {
    width: u16,
    offsets: Vec<usize>,
    total: usize,
    valid: bool,
}

impl Transcript {
    /// Move the finalized prefix into terminal scrollback. Open streaming cells and anything
    /// after them stay in the inline viewport until their turn is complete.
    pub fn drain_finalized_lines(&mut self, width: u16) -> Vec<Line<'static>> {
        let finalized = self
            .entries
            .iter()
            .take_while(|entry| cell_is_finalized(&entry.cell))
            .count();
        if finalized == 0 {
            return Vec::new();
        }

        let mut lines = Vec::new();
        for entry in self.entries.iter_mut().take(finalized) {
            lines.extend(entry.lines(width).iter().cloned());
        }
        self.entries.drain(..finalized);
        self.layout = LayoutCache::default();
        lines
    }

    pub fn push_user(&mut self, text: String) {
        self.entries.push(Entry::new(Cell::User(text)));
        self.invalidate_layout();
    }

    pub fn push_notice(&mut self, text: String) {
        self.entries.push(Entry::new(Cell::Notice(text)));
        self.invalidate_layout();
    }

    pub fn push_error(&mut self, text: String) {
        self.entries.push(Entry::new(Cell::Error(text)));
        self.invalidate_layout();
    }

    pub fn push_plan(&mut self, revision: u64, items: Vec<PlanItem>, explanation: Option<String>) {
        self.entries.push(Entry::new(Cell::Plan {
            revision,
            items,
            explanation,
        }));
        self.invalidate_layout();
    }

    pub fn start_compaction(
        &mut self,
        id: String,
        automatic: bool,
        estimated_tokens: u64,
        available_tokens: Option<u64>,
    ) {
        self.entries.push(Entry::new(Cell::Compaction {
            id,
            automatic,
            estimated_tokens,
            available_tokens,
            outcome: CompactionOutcome::Running,
        }));
        self.invalidate_layout();
    }

    pub fn finish_compaction(
        &mut self,
        id: &str,
        tokens_before: u64,
        summary_chars: usize,
        retained_messages: usize,
    ) {
        self.update_compaction(
            id,
            CompactionOutcome::Finished {
                tokens_before,
                summary_chars,
                retained_messages,
            },
        );
    }

    pub fn fail_compaction(
        &mut self,
        id: &str,
        error: String,
        cancelled: bool,
        history_changed: bool,
    ) {
        self.update_compaction(
            id,
            CompactionOutcome::Failed {
                error,
                cancelled,
                history_changed,
            },
        );
    }

    fn update_compaction(&mut self, id: &str, outcome: CompactionOutcome) {
        if let Some(entry) = self.entries.iter_mut().rev().find(
            |entry| matches!(&entry.cell, Cell::Compaction { id: cell_id, .. } if cell_id == id),
        ) {
            if let Cell::Compaction {
                outcome: current, ..
            } = &mut entry.cell
            {
                *current = outcome;
                entry.touch();
                self.invalidate_layout();
            }
        }
    }

    pub fn clear(&mut self) {
        self.entries.clear();
        self.layout = LayoutCache::default();
    }

    /// 追加助手文本增量;没有开放中的助手 Cell 就新开一个。
    pub fn append_assistant(&mut self, delta: &str) {
        if let Some(e) = self.entries.last_mut() {
            if let Cell::Assistant { text, open: true } = &mut e.cell {
                text.push_str(delta);
                e.touch();
                self.invalidate_layout();
                return;
            }
        }
        // 正文开始意味着前一段思考已经结束。及时封口后，思考内容可以在
        // 当前 loop 结束前进入终端 scrollback，而不会阻塞后续单元格。
        self.close_open_cells();
        self.entries.push(Entry::new(Cell::Assistant {
            text: delta.to_string(),
            open: true,
        }));
        self.invalidate_layout();
    }

    /// 思考增量(同上,但思考与正文可能交替出现,所以各自独立成 Cell)。
    pub fn append_thinking(&mut self, delta: &str) {
        if let Some(e) = self.entries.last_mut() {
            if let Cell::Thinking { text, open: true } = &mut e.cell {
                text.push_str(delta);
                e.touch();
                self.invalidate_layout();
                return;
            }
        }
        self.close_open_cells();
        self.entries.push(Entry::new(Cell::Thinking {
            text: delta.to_string(),
            open: true,
        }));
        self.invalidate_layout();
    }

    /// 助手消息完成:用全文校正开放中的 Cell(流式增量偶有丢失时兜底)。
    pub fn finalize_assistant(&mut self, full: String) {
        let mut full = Some(full);
        for e in self.entries.iter_mut().rev() {
            if let Cell::Assistant { text, open } = &mut e.cell {
                if *open {
                    *text = full.take().expect("final assistant text is available");
                    *open = false;
                    e.touch();
                }
                break;
            }
        }
        if let Some(full) = full {
            if !full.is_empty() {
                self.entries.push(Entry::new(Cell::Assistant {
                    text: full,
                    open: false,
                }));
            }
        }
        // 有些 provider 只有思考增量和最终正文，没有正文增量。消息完成必须
        // 同时封口更早的 Thinking，否则整个 loop 的 finalized prefix 都会被卡住。
        self.close_open_cells();
        self.invalidate_layout();
    }

    /// 关闭所有开放中的 Cell(一轮结束时调用)。
    pub fn close_open_cells(&mut self) {
        for e in self.entries.iter_mut() {
            match &mut e.cell {
                Cell::Assistant { open, .. } | Cell::Thinking { open, .. } if *open => {
                    *open = false;
                    e.touch();
                }
                _ => {}
            }
        }
    }

    pub fn push_tool(&mut self, id: String, name: String, summary: String) {
        // 工具调用标志着本次模型输出结束；即使该响应没有正文，也要封口思考。
        self.close_open_cells();
        self.entries.push(Entry::new(Cell::Tool {
            id,
            name,
            summary,
            progress: None,
            output: None,
            is_error: false,
        }));
        self.invalidate_layout();
    }

    /// 按调用 ID 更新仍在执行中的工具摘要，不把 Cell 提前封口。
    pub fn update_tool(&mut self, id: &str, progress: String) {
        if let Some(entry) = self.entries.iter_mut().rev().find(|entry| {
            matches!(
                &entry.cell,
                Cell::Tool {
                    id: cell_id,
                    output: None,
                    ..
                } if cell_id == id
            )
        }) {
            if let Cell::Tool {
                progress: current, ..
            } = &mut entry.cell
            {
                *current = Some(progress);
                entry.touch();
                self.invalidate_layout();
            }
        }
    }

    /// 按调用 ID 把结果填进对应的工具 Cell(找不到时退回最近一个运行中的)。
    pub fn finish_tool(&mut self, id: &str, output: String, is_error: bool) {
        let mut fallback = None;
        for (index, e) in self.entries.iter().enumerate().rev() {
            if let Cell::Tool {
                id: cell_id,
                output: slot,
                ..
            } = &e.cell
            {
                if slot.is_none() {
                    if cell_id == id {
                        fallback = Some(index);
                        break;
                    }
                    if fallback.is_none() {
                        fallback = Some(index);
                    }
                }
            }
        }
        if let Some(index) = fallback {
            if let Cell::Tool {
                output: slot,
                progress,
                is_error: err_flag,
                ..
            } = &mut self.entries[index].cell
            {
                *slot = Some(output);
                *progress = None;
                *err_flag = is_error;
                self.entries[index].touch();
                self.invalidate_layout();
            }
        }
    }

    /// 取可视窗口内的行。返回 (行, 总行数)。
    /// `scroll_up` = 从底部向上滚了多少行(0 = 贴底跟随)。
    pub fn visible_lines(
        &mut self,
        width: u16,
        height: usize,
        scroll_up: usize,
    ) -> (Vec<Line<'static>>, usize) {
        self.ensure_layout(width);
        let total = self.layout.total;
        if height == 0 || total == 0 {
            return (Vec::new(), total);
        }
        let max_scroll = total.saturating_sub(height);
        let scroll_up = scroll_up.min(max_scroll);
        let start = total.saturating_sub(height + scroll_up);
        let end = (start + height).min(total);

        let first_entry = self
            .layout
            .offsets
            .partition_point(|offset| *offset <= start)
            .saturating_sub(1)
            .min(self.entries.len());
        let after_last_entry = self
            .layout
            .offsets
            .partition_point(|offset| *offset < end)
            .min(self.entries.len());

        let mut out = Vec::with_capacity(height);
        for entry_index in first_entry..after_last_entry {
            let entry_start = self.layout.offsets[entry_index];
            let lines = self.entries[entry_index].lines(width);
            let local_start = start.saturating_sub(entry_start).min(lines.len());
            let local_end = end.saturating_sub(entry_start).min(lines.len());
            out.extend(lines[local_start..local_end].iter().cloned());
        }
        (out, total)
    }

    fn invalidate_layout(&mut self) {
        self.layout.valid = false;
    }

    fn ensure_layout(&mut self, width: u16) {
        if self.layout.valid && self.layout.width == width {
            return;
        }
        let mut offsets = Vec::with_capacity(self.entries.len() + 1);
        let mut total = 0usize;
        offsets.push(total);
        for entry in &mut self.entries {
            total += entry.lines(width).len();
            offsets.push(total);
        }
        self.layout = LayoutCache {
            width,
            offsets,
            total,
            valid: true,
        };
        #[cfg(test)]
        {
            self.layout_rebuilds += 1;
        }
    }
}

fn cell_is_finalized(cell: &Cell) -> bool {
    match cell {
        Cell::User(_) | Cell::Plan { .. } | Cell::Notice(_) | Cell::Error(_) => true,
        Cell::Assistant { open, .. } | Cell::Thinking { open, .. } => !open,
        Cell::Tool { output, .. } => output.is_some(),
        Cell::Compaction { outcome, .. } => !matches!(outcome, CompactionOutcome::Running),
    }
}

// ---- 排版 ----

fn style_user() -> Style {
    Style::default().fg(Color::Cyan)
}
fn style_thinking() -> Style {
    Style::default()
        .fg(Color::DarkGray)
        .add_modifier(Modifier::ITALIC)
}
fn style_tool_head(is_error: bool) -> Style {
    if is_error {
        Style::default().fg(Color::Red)
    } else {
        Style::default().fg(Color::Magenta)
    }
}
fn style_dim() -> Style {
    Style::default().fg(Color::DarkGray)
}

/// 把一段多行文本按宽度换行,首行/续行可带不同前缀,整体一个样式。
fn wrap_styled(
    text: &str,
    width: usize,
    first_prefix: &str,
    cont_prefix: &str,
    style: Style,
) -> Vec<Line<'static>> {
    let mut out = Vec::new();
    for (i, para) in text.split('\n').enumerate() {
        let head = if i == 0 { first_prefix } else { cont_prefix };
        if para.is_empty() {
            out.push(Line::raw(""));
            continue;
        }
        let opts = Options::new(width)
            .initial_indent(head)
            .subsequent_indent(cont_prefix);
        for piece in textwrap::wrap(para, opts) {
            out.push(Line::styled(piece.into_owned(), style));
        }
    }
    out
}

fn build_lines(cell: &Cell, width: u16) -> Vec<Line<'static>> {
    let w = (width as usize).max(8);
    let mut lines = match cell {
        Cell::User(t) => wrap_styled(t, w, "❯ ", "  ", style_user()),
        Cell::Assistant { text, .. } => {
            if text.is_empty() {
                Vec::new()
            } else {
                wrap_styled(text, w, "", "", Style::default())
            }
        }
        Cell::Thinking { text, .. } => {
            if text.is_empty() {
                Vec::new()
            } else {
                wrap_styled(text, w, "· ", "  ", style_thinking())
            }
        }
        Cell::Tool {
            name,
            summary,
            progress,
            output,
            is_error,
            ..
        } => {
            let head = if summary.is_empty() {
                name.clone()
            } else {
                format!("{}({})", name, summary)
            };
            let mut v = wrap_styled(&head, w, "● ", "  ", style_tool_head(*is_error));
            match output {
                None => v.push(Line::styled(
                    format!(
                        "  {}",
                        progress
                            .as_deref()
                            .filter(|text| !text.is_empty())
                            .unwrap_or("运行中…")
                    ),
                    style_dim(),
                )),
                Some(out) => {
                    let body_style = if *is_error {
                        Style::default().fg(Color::Red).add_modifier(Modifier::DIM)
                    } else {
                        style_dim()
                    };
                    let logical: Vec<&str> = out.lines().collect();
                    let content_width = w.saturating_sub(2).max(8);
                    if logical.len() <= TOOL_PREVIEW_LINES {
                        for line in logical {
                            v.push(Line::styled(
                                format!("  {}", util::ellipsis(line, content_width)),
                                body_style,
                            ));
                        }
                    } else {
                        let head = 6;
                        let tail = TOOL_PREVIEW_LINES.saturating_sub(head + 1);
                        for line in logical.iter().take(head) {
                            v.push(Line::styled(
                                format!("  {}", util::ellipsis(line, content_width)),
                                body_style,
                            ));
                        }
                        v.push(Line::styled(
                            format!(
                                "  … 省略 {} 行 · Ctrl+T 查看完整内容",
                                logical.len() - head - tail
                            ),
                            style_dim(),
                        ));
                        for line in logical.iter().skip(logical.len() - tail) {
                            v.push(Line::styled(
                                format!("  {}", util::ellipsis(line, content_width)),
                                body_style,
                            ));
                        }
                    }
                }
            }
            v
        }
        Cell::Plan {
            revision,
            items,
            explanation,
        } => {
            let mut lines = vec![Line::styled(
                if items.is_empty() {
                    format!("计划 #{} 已清空", revision)
                } else {
                    format!("计划 #{}", revision)
                },
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
            )];
            if let Some(explanation) = explanation {
                lines.extend(wrap_styled(explanation, w, "  ", "  ", style_dim()));
            }
            for item in items {
                let (marker, style) = match item.status {
                    PlanStatus::Pending => ("[ ]", Style::default()),
                    PlanStatus::InProgress => (
                        "[>]",
                        Style::default()
                            .fg(Color::Yellow)
                            .add_modifier(Modifier::BOLD),
                    ),
                    PlanStatus::Completed => ("[x]", style_dim()),
                };
                lines.extend(wrap_styled(
                    &format!("{}: {}", item.id, item.text),
                    w,
                    &format!("  {} ", marker),
                    "      ",
                    style,
                ));
            }
            lines
        }
        Cell::Compaction {
            automatic,
            estimated_tokens,
            available_tokens,
            outcome,
            ..
        } => {
            let source = if *automatic { "自动" } else { "手动" };
            match outcome {
                CompactionOutcome::Running => {
                    let mut lines = vec![Line::styled(
                        format!("◐ 正在{}压缩历史", source),
                        Style::default()
                            .fg(Color::Cyan)
                            .add_modifier(Modifier::BOLD),
                    )];
                    let budget = match available_tokens {
                        Some(available) => format!(
                            "上下文约 {} / {} tokens",
                            util::fmt_tokens(*estimated_tokens),
                            util::fmt_tokens(*available)
                        ),
                        None => format!("上下文约 {} tokens", util::fmt_tokens(*estimated_tokens)),
                    };
                    lines.extend(wrap_styled(&budget, w, "  ", "  ", style_dim()));
                    lines
                }
                CompactionOutcome::Finished {
                    tokens_before,
                    summary_chars,
                    retained_messages,
                } => {
                    let mut lines = vec![Line::styled(
                        format!("✓ {}压缩完成", source),
                        Style::default()
                            .fg(Color::Green)
                            .add_modifier(Modifier::BOLD),
                    )];
                    lines.extend(wrap_styled(
                        &format!(
                            "压缩前约 {} tokens · 摘要 {} 字符 · 保留 {} 条消息",
                            util::fmt_tokens(*tokens_before),
                            summary_chars,
                            retained_messages
                        ),
                        w,
                        "  ",
                        "  ",
                        style_dim(),
                    ));
                    lines
                }
                CompactionOutcome::Failed {
                    error,
                    cancelled,
                    history_changed,
                } => {
                    let title = if *cancelled {
                        format!("■ {}压缩已取消", source)
                    } else {
                        format!("✖ {}压缩失败", source)
                    };
                    let style = if *cancelled {
                        Style::default().fg(Color::Yellow)
                    } else {
                        Style::default().fg(Color::Red)
                    }
                    .add_modifier(Modifier::BOLD);
                    let mut lines = vec![Line::styled(title, style)];
                    lines.extend(wrap_styled(error, w, "  ", "  ", style_dim()));
                    lines.push(Line::styled(
                        if *history_changed {
                            "  历史已改变"
                        } else {
                            "  历史未改变"
                        },
                        style_dim(),
                    ));
                    lines
                }
            }
        }
        Cell::Notice(t) => wrap_styled(t, w, "· ", "  ", style_dim()),
        Cell::Error(t) => wrap_styled(t, w, "✖ ", "  ", Style::default().fg(Color::Red)),
    };
    if lines.is_empty() {
        return lines;
    }
    lines.push(Line::raw("")); // 单元格之间空一行
    lines
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stable_scrolling_reuses_layout_index() {
        let mut transcript = Transcript::default();
        for i in 0..500 {
            transcript.push_notice(format!("第 {} 条较长的历史消息，用来制造多行内容", i));
        }

        let (_, total) = transcript.visible_lines(40, 20, 0);
        assert!(total > 500);
        assert_eq!(transcript.layout_rebuilds, 1);
        for scroll_up in (0..300).step_by(3) {
            let (visible, same_total) = transcript.visible_lines(40, 20, scroll_up);
            assert!(!visible.is_empty());
            assert_eq!(same_total, total);
        }
        assert_eq!(
            transcript.layout_rebuilds, 1,
            "只改变 scroll_up 不应重新遍历全部 Entry"
        );

        transcript.push_notice("新增消息会让布局失效".into());
        transcript.visible_lines(40, 20, 0);
        assert_eq!(transcript.layout_rebuilds, 2);
        transcript.visible_lines(60, 20, 0);
        assert_eq!(transcript.layout_rebuilds, 3, "宽度变化必须重新换行");
    }

    #[test]
    fn drains_only_the_finalized_prefix() {
        let mut transcript = Transcript::default();
        transcript.push_user("hello".into());
        transcript.append_assistant("streaming");
        transcript.push_notice("after open cell".into());

        let first = transcript.drain_finalized_lines(40);
        assert!(format!("{first:?}").contains("hello"));
        assert!(transcript.drain_finalized_lines(40).is_empty());

        transcript.close_open_cells();
        let rest = transcript.drain_finalized_lines(40);
        let rendered = format!("{rest:?}");
        assert!(rendered.contains("streaming"));
        assert!(rendered.contains("after open cell"));
    }

    #[test]
    fn finalizing_cjk_text_does_not_insert_spaces() {
        let mut transcript = Transcript::default();
        transcript.append_assistant("完成。提交信息");
        transcript.finalize_assistant("完成。提交信息".into());

        let lines = transcript.drain_finalized_lines(40);
        let text = lines
            .iter()
            .flat_map(|line| line.spans.iter())
            .map(|span| span.content.as_ref())
            .collect::<String>();
        assert!(text.contains("完成。提交信息"));
        assert!(!text.contains("完 成"));
    }

    #[test]
    fn streaming_phase_changes_release_finalized_prefix_during_loop() {
        let mut transcript = Transcript::default();

        transcript.append_thinking("先检查项目结构");
        transcript.append_assistant("我先读取配置。");
        let thinking = format!("{:?}", transcript.drain_finalized_lines(80));
        assert!(thinking.contains("先检查项目结构"));
        assert!(transcript.drain_finalized_lines(80).is_empty());

        transcript.finalize_assistant("我先读取配置。".into());
        let assistant = format!("{:?}", transcript.drain_finalized_lines(80));
        assert!(assistant.contains("我先读取配置。"));

        transcript.append_thinking("需要调用工具");
        transcript.push_tool("tool-1".into(), "read_file".into(), "config.toml".into());
        let before_tool = format!("{:?}", transcript.drain_finalized_lines(80));
        assert!(before_tool.contains("需要调用工具"));
        assert!(transcript.drain_finalized_lines(80).is_empty());

        let running = format!("{:?}", transcript.visible_lines(80, 8, 0).0);
        assert!(running.contains("read_file"));
        assert!(running.contains("运行中"));

        transcript.finish_tool("tool-1", "读取完成".into(), false);
        let tool = format!("{:?}", transcript.drain_finalized_lines(80));
        assert!(tool.contains("read_file"));
        assert!(tool.contains("读取完成"));
    }

    #[test]
    fn assistant_finished_closes_thinking_without_text_deltas() {
        let mut transcript = Transcript::default();
        transcript.append_thinking("只流式返回了思考");
        transcript.finalize_assistant("最终回复".into());

        let rendered = format!("{:?}", transcript.drain_finalized_lines(80));
        assert!(rendered.contains("只流式返回了思考"));
        assert!(rendered.contains("最终回复"));
        assert!(transcript.drain_finalized_lines(80).is_empty());
    }

    #[test]
    fn plan_cell_renders_true_statuses_and_clear_state() {
        let mut transcript = Transcript::default();
        transcript.push_plan(
            3,
            vec![
                PlanItem {
                    id: "active".into(),
                    text: "正在处理".into(),
                    status: PlanStatus::InProgress,
                },
                PlanItem {
                    id: "later".into(),
                    text: "稍后处理".into(),
                    status: PlanStatus::Pending,
                },
            ],
            None,
        );
        transcript.push_plan(4, Vec::new(), None);

        let rendered = format!("{:?}", transcript.drain_finalized_lines(60));
        assert!(rendered.contains("计划 #3"));
        assert!(rendered.contains("[>] active"));
        assert!(rendered.contains("[ ] later"));
        assert!(rendered.contains("计划 #4 已清空"));
        assert!(!rendered.contains("[x] active"));
    }

    #[test]
    fn compaction_cell_stays_live_then_renders_one_terminal_summary() {
        let mut transcript = Transcript::default();
        transcript.start_compaction("compact-1".into(), true, 128_000, Some(151_000));

        assert!(transcript.drain_finalized_lines(80).is_empty());
        let running = format!("{:?}", transcript.visible_lines(80, 10, 0).0);
        assert!(running.contains("正在自动压缩历史"));
        assert!(running.contains("128k / 151k tokens"));

        transcript.finish_compaction("compact-1", 120_000, 4_000, 12);
        let finished = format!("{:?}", transcript.drain_finalized_lines(80));
        assert!(finished.contains("自动压缩完成"));
        assert!(finished.contains("摘要 4000 字符"));
        assert!(finished.contains("保留 12 条消息"));
    }

    #[test]
    fn failed_compaction_reports_unchanged_history() {
        let mut transcript = Transcript::default();
        transcript.start_compaction("compact-2".into(), false, 42_000, None);
        transcript.fail_compaction("compact-2", "provider unavailable".into(), false, false);

        let rendered = format!("{:?}", transcript.drain_finalized_lines(80));
        assert!(rendered.contains("手动压缩失败"));
        assert!(rendered.contains("provider unavailable"));
        assert!(rendered.contains("历史未改变"));
    }
}
