//! # Stateful Agent harness
//!
//! [`crate::agent_loop::run_agent_loop`] 是 provider-neutral 核心循环；这里的
//! [`Agent`] 为 CLI 和默认嵌入入口装配事实日志、预算、planning、permissions、
//! hooks、compaction 与 session commands。`Agent::run_turn` 只负责把这些能力
//! 适配成 core callbacks。
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
//! [`crate::sdk::spawn_session`] 用稳定 controller 和有界事件流承载线程宿主；
//! TUI、`--once` 和 RPC adapter 都通过这一个入口驱动同一份循环。
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

use std::sync::mpsc::Receiver;
use std::time::Duration;

use crate::compaction::CompactionSettings;
use crate::config::ActiveModelSelection;
use crate::context::budget::ContextBudget;
use crate::context::ContextProvider;
use crate::event::{AgentCommand, AgentEvent};
use crate::harness::{ModelPreferences, ModelRegistry, SessionBackend};
use crate::hooks::HookRegistry;
use crate::message::Usage;
use crate::permission::{ApprovalResponse, PermissionManager};
use crate::provider::Provider;
use crate::session::SessionEntry;
use crate::tools::ToolRegistry;
use crate::workspace::Workspace;

mod agent_loop;
mod builder;
mod commands;
mod compaction;
pub(crate) mod inbox;
mod session_events;
mod session_runtime;
mod tool_execution;

pub use crate::agent_loop::RetryPolicy;
pub use builder::{AgentBuilder, ProviderFactory};
use compaction::CompactionRuntime;
pub use session_runtime::spawn_session;

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
    compaction_settings: CompactionSettings,
    retry_policy: RetryPolicy,
    models: Box<dyn ModelRegistry>,
    max_turns: u32,
    tool_timeout: Option<Duration>,
    usage_total: Usage,
    sessions: Box<dyn SessionBackend>,
    model_preferences: Box<dyn ModelPreferences>,
    permissions: PermissionManager,
    hooks: HookRegistry,
    startup_events: std::collections::VecDeque<AgentEvent>,
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

#[cfg(test)]
mod tests;
