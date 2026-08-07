//! # Agent Runtime:整个项目的心脏
//!
//! `Agent::run_turn` 就是教科书上的 Agent Loop:
//!
//! ```text
//! 用户输入(作为 Message 事实落库)
//!   └─► 事实日志 ──单向投影──► 模型消息 ──预算──► PromptContext
//!        └─► 调模型(Provider,流式)
//!             ├─ 没有工具调用 ──► 本轮结束
//!             └─ 有工具调用 ──► 受控执行(ToolRegistry)
//!                  └─► 结果作为 Observation 事实落库 ──► 回到"投影"
//! ```
//!
//! Runtime 对外只有两条通道(见 `event.rs`)+ 一个取消标志,
//! 由 [`spawn`] 起一个工作线程承载;`--once` 模式则直接在当前线程
//! 调 [`Agent::handle_command`]——同一份循环,两种前端。
//!
//! 阶段 4 之后,这里遵守"事实先行"的持久化纪律:
//! - **事实日志是唯一权威**:Agent 只持有 `Vec<SessionEntry>` 内存镜像,
//!   模型视图每轮由 `session::project_model_messages` 重新投影,
//!   UI-only 事实(Notice 等)永远不会进 Provider。
//! - **历史必须始终合法**:每个 ToolUse 都要有配对的 ToolResult;
//!   带工具的批在提交边界还会被 `validate_new_message_batch` 复核。
//! - **提交失败不装作没事**:任何一批事实写库失败,内存镜像不推进、
//!   本轮立即终止并报错——宁可少跑一轮,不让内存与磁盘历史分叉。
//! - **重试要幂等**:只有"一个字都还没吐出来"的失败才自动重试。

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, Sender};
use std::sync::Arc;
use std::time::Duration;

use crate::config::{ActiveModelSelection, Config, ProviderCatalogEntry};
use crate::context::budget::ContextBudget;
use crate::context::ContextProvider;
use crate::event::{AgentCommand, AgentEvent};
use crate::hooks::HookRegistry;
use crate::message::Usage;
use crate::permission::{ApprovalResponse, PermissionManager};
use crate::provider::Provider;
use crate::session::SessionEntry;
use crate::skills::SkillCatalog;
use crate::storage::{SessionManager, WorkspacePreferences};
use crate::tools::ToolRegistry;
use crate::workspace::Workspace;

mod agent_loop;
mod builder;
mod commands;
mod compaction;
mod tool_execution;

pub use builder::{AgentBuilder, ProviderFactory};

/// 请求级重试策略。只覆盖"尚未产生任何流事件"的失败(重试幂等由调用方保证);
/// 全部决策收敛在 [`RetryPolicy::delay_for`] 这一个纯函数里,便于确定性测试。
#[derive(Debug, Clone, Copy)]
pub struct RetryPolicy {
    /// 最大尝试次数(含首次)。
    pub max_attempts: u32,
    /// 指数退避基数(第 1 次失败后等待 base,之后翻倍)。
    pub base_delay: Duration,
    /// 退避上限(含 jitter 之后)。
    pub max_delay: Duration,
    /// 服务器 Retry-After 超过它就放弃重试:不为一个请求无限期挂住 Runtime。
    pub max_retry_after: Duration,
    /// jitter 种子。相同种子产生相同序列,测试可注入固定值。
    pub jitter_seed: u64,
}

impl Default for RetryPolicy {
    fn default() -> Self {
        RetryPolicy {
            max_attempts: 3,
            base_delay: Duration::from_secs(2),
            max_delay: Duration::from_secs(30),
            max_retry_after: Duration::from_secs(60),
            jitter_seed: 0x9E37_79B9_7F4A_7C15,
        }
    }
}

impl RetryPolicy {
    /// `attempt` 是刚失败的第几次尝试(从 1 开始)。返回 None = 不再重试。
    /// 服务器给出的 Retry-After 优先且不加 jitter;超过上限直接放弃。
    pub fn delay_for(&self, attempt: u32, retry_after: Option<Duration>) -> Option<Duration> {
        if attempt >= self.max_attempts {
            return None;
        }
        if let Some(server_wait) = retry_after {
            if server_wait > self.max_retry_after {
                return None;
            }
            return Some(server_wait);
        }
        let exponent = attempt.saturating_sub(1).min(20);
        let backoff = self
            .base_delay
            .saturating_mul(1u32 << exponent)
            .min(self.max_delay);
        // 加 [0,25%) 的确定性 jitter,避免多客户端整点齐射;最终仍受 max_delay 约束。
        let jitter = backoff.mul_f64(self.jitter_fraction(attempt));
        Some((backoff + jitter).min(self.max_delay))
    }

    /// splitmix64 变体:同 (seed, attempt) 恒定,取值 [0, 0.25)。
    fn jitter_fraction(&self, attempt: u32) -> f64 {
        let mut x = self
            .jitter_seed
            .wrapping_add((attempt as u64 + 1).wrapping_mul(0x9E37_79B9_7F4A_7C15));
        x ^= x >> 33;
        x = x.wrapping_mul(0xFF51_AFD7_ED55_8CCD);
        x ^= x >> 33;
        (x % 1000) as f64 / 4000.0
    }
}

pub struct Agent {
    workspace: Workspace,
    tools: ToolRegistry,
    /// 会话事实日志的内存镜像。只在对应批次成功落库后推进(见 [`Agent::commit`])。
    entries: Vec<SessionEntry>,
    /// 上下文源(system 片段)。想加 Planning/Memory/Workspace Map,往这里 push 即可。
    extra_context: Vec<Box<dyn ContextProvider>>,
    provider: Box<dyn Provider>,
    provider_factory: ProviderFactory,
    active_selection: ActiveModelSelection,
    budget: ContextBudget,
    retry_policy: RetryPolicy,
    config: Config,
    usage_total: Usage,
    sessions: SessionManager,
    workspace_preferences: WorkspacePreferences,
    permissions: PermissionManager,
    hooks: HookRegistry,
    skills: std::sync::Arc<SkillCatalog>,
    skill_warnings: Vec<String>,
    skills_announced: bool,
    approval_rx: Option<Receiver<ApprovalResponse>>,
    /// 活动运行中收到、需要等本轮结束再执行的命令(/clear、/provider 等)。
    deferred: std::collections::VecDeque<AgentCommand>,
}

fn budget_from_settings(settings: &crate::config::ProviderSettings) -> ContextBudget {
    ContextBudget {
        context_window: settings.context_window,
        // 输出预留:显式 max_tokens,否则一个保守默认。
        reserve_output: settings.max_tokens.unwrap_or(8192),
    }
}

/// 前端持有的 Runtime 句柄。
pub struct RuntimeHandle {
    pub commands: Sender<AgentCommand>,
    pub approvals: Sender<ApprovalResponse>,
    pub events: Receiver<AgentEvent>,
    /// 置 true 请求取消当前轮;Runtime 会在收尾后自行复位。
    pub cancel: Arc<AtomicBool>,
    pub provider_label: String,
    pub active_selection: ActiveModelSelection,
    pub provider_catalog: Vec<ProviderCatalogEntry>,
    pub reasoning_preferences:
        std::collections::BTreeMap<String, std::collections::BTreeMap<String, String>>,
    pub session_id: String,
}

/// 把 Agent 装进工作线程,返回通道句柄。TUI 前端用这个;
/// headless 前端不需要线程,直接调 `Agent::handle_command`。
pub fn spawn(agent: Agent) -> RuntimeHandle {
    let provider_label = agent.provider_label();
    let active_selection = agent.active_selection.clone();
    let provider_catalog = agent.config.provider_catalog();
    let reasoning_preferences = agent.workspace_preferences.reasoning_efforts();
    let session_id = agent.session_id().to_string();
    let (cmd_tx, cmd_rx) = std::sync::mpsc::channel::<AgentCommand>();
    let (approval_tx, approval_rx) = std::sync::mpsc::channel::<ApprovalResponse>();
    let (evt_tx, evt_rx) = std::sync::mpsc::channel::<AgentEvent>();
    let cancel = Arc::new(AtomicBool::new(false));
    let cancel_worker = cancel.clone();

    std::thread::Builder::new()
        .name("agent-runtime".into())
        .spawn(move || {
            let mut agent = agent;
            agent.approval_rx = Some(approval_rx);
            let mut emit = |e: AgentEvent| {
                // 前端先退出时 send 会失败,忽略即可(线程随后收到 Shutdown 或通道关闭)
                let _ = evt_tx.send(e);
            };
            agent.emit_skill_discovery(&mut emit);
            loop {
                // 活动运行中延迟的命令(/clear、/provider、Shutdown…)优先于新命令。
                let cmd = match agent.take_deferred() {
                    Some(cmd) => cmd,
                    None => match cmd_rx.recv() {
                        Ok(cmd) => cmd,
                        Err(_) => break,
                    },
                };
                // 新命令开始前复位取消标志(上一轮的取消不该波及这一轮)
                cancel_worker.store(false, Ordering::Relaxed);
                if !agent.handle_command_with_inbox(cmd, &mut emit, &cancel_worker, Some(&cmd_rx)) {
                    break;
                }
            }
        })
        .expect("无法创建 runtime 线程");

    RuntimeHandle {
        commands: cmd_tx,
        approvals: approval_tx,
        events: evt_rx,
        cancel,
        provider_label,
        active_selection,
        provider_catalog,
        reasoning_preferences,
        session_id,
    }
}

#[cfg(test)]
mod tests;
