//! # 事件系统:Runtime 与前端之间的唯一契约
//!
//! Agent Loop **从不直接操作 TUI**。它只会:
//! - 从命令通道收 [`AgentCommand`]（用户想干什么）;
//! - 往事件通道发 [`AgentEvent`]（世界发生了什么）。
//!
//! 前端(TUI、`--once` headless 打印器,未来的 GUI/Web)只做一件事:
//! 消费事件流并渲染。本项目自带两个前端消费同一事件流,
//! 就是为了证明这条边界是真的,而不是纸面设计。
//!
//! 通道用 `std::sync::mpsc`,配合"TUI 主线程 + Runtime 工作线程"两线程模型。
//! 取消是唯一走通道之外的信号:一个共享的 `Arc<AtomicBool>`,
//! 因为取消必须能打断一个正忙着读流/跑子进程的 Runtime,
//! 而它忙的时候不会回来看命令通道。

use crate::message::CacheUsage;
use crate::permission::ApprovalRequest;
use crate::plan::PlanItem;
use crate::session::{SessionEntry, SessionSummary};
use crate::skills::SkillMetadata;
use crate::tools::{ToolError, ToolOutput};

/// 前端 → Runtime。
#[derive(Debug, Clone)]
pub enum AgentCommand {
    /// 用户提交了一段输入,开启新的一轮(turn)。
    /// 若在活动运行中到达,Runtime 会把它显式归类为 steering 并提示,
    /// 绝不会隐式并发地开第二个运行。
    UserInput(String),
    /// 活动运行中的方向修正:在**当前完整工具批提交后**注入,
    /// 不打断正在执行的工具(紧急停止请用取消)。空闲时等价于 UserInput。
    Steer(String),
    /// 排队的后续任务:仅在当前任务将要停止时注入。空闲时等价于 UserInput。
    FollowUp(String),
    /// 请求取消当前运行。SessionController 会先设置共享取消标志，命令用于完成终态关联。
    Abort,
    /// 压缩当前会话:调模型生成摘要,作为 Compaction 事实追加
    /// (事实日志不减少,模型视图从摘要之后开始)。
    Compact,
    /// 重新读取配置、项目指令和默认 skill catalog；只在空闲时执行。
    Reload,
    /// 清空会话历史(/clear)。
    ClearConversation,
    /// 切换到 config.toml 里的另一个 provider profile(/provider)。
    SwitchProvider(String),
    /// 在当前 provider 内原子选择模型与思考程度(/model)。
    SelectModel { model: String, effort: String },
    /// SDK/RPC 原子选择 provider、model 与思考程度。
    SetModelSelection {
        provider: String,
        model: String,
        effort: String,
    },
    /// 只调整当前模型的思考程度(/reasoning 或 /effort)。
    SetReasoningEffort(String),
    /// 列出历史会话；all=true 时包含其他 workspace 但仍不可直接加载。
    ListSessions { all: bool },
    /// 恢复当前 workspace 的一个历史会话(`/session <id>`)。
    LoadSession(String),
    /// 退出:Runtime 线程收到后结束自己。活动运行中到达时会请求取消当前轮。
    Shutdown,
}

/// Stateful harness 中输入队列的分类。它只用于进程内事件，不承担 wire 语义。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InputQueueKind {
    Steering,
    FollowUp,
}

/// 会话压缩的触发来源。前端用它区分用户显式请求与 runtime 自动保护。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompactionTrigger {
    Automatic,
    Manual,
}

/// Runtime → 前端。前端拿到的信息足以完整重建对话画面。
#[derive(Debug, Clone)]
pub enum AgentEvent {
    /// 回显用户输入(前端不自己上屏,统一从事件流走,保证 headless 一致)。
    UserMessage(String),

    /// 一轮开始:模型开始处理。
    TurnStarted,

    /// A transient provider failure is waiting before another request attempt.
    RetryScheduled {
        attempt: u32,
        max_retries: u32,
        delay_ms: u64,
        error: String,
    },
    /// The retry backoff completed and the next provider request is starting.
    RetryStarted {
        attempt: u32,
        max_retries: u32,
    },

    /// 一次压缩模型调用已经开始。终态事件用 `id` 与它严格配对。
    CompactionStarted {
        id: String,
        trigger: CompactionTrigger,
        estimated_tokens: u64,
        available_tokens: Option<u64>,
    },
    /// 压缩事实已经成功持久化，内存模型视图可以安全推进。
    CompactionFinished {
        id: String,
        trigger: CompactionTrigger,
        tokens_before: u64,
        summary_chars: usize,
        retained_messages: usize,
    },
    /// 压缩没有提交。`history_changed` 当前必须为 false，用于锁定原子性契约。
    CompactionFailed {
        id: String,
        trigger: CompactionTrigger,
        error: String,
        cancelled: bool,
        history_changed: bool,
    },

    /// 助手文本增量(streaming)。前端应把它追加到"当前助手消息"。
    AssistantDelta(String),
    /// 思考/推理增量(DeepSeek reasoning、Responses reasoning summary 等),
    /// 前端一般用暗色渲染,可选择不显示。
    ThinkingDelta(String),
    /// 一条助手消息结束(文本已完整)。内容是全文,前端可用它兜底校正。
    AssistantMessage(String),

    /// 模型正在流式生成一次工具调用的参数(还没开始执行)。
    /// 只用于状态栏之类的即时反馈,不该在聊天区落一行。
    ToolCallPending {
        name: String,
    },

    /// 模型请求调用工具,即将执行。`summary` 是给人看的一行参数摘要。
    ToolCallStarted {
        id: String,
        name: String,
        summary: String,
    },
    /// 工具执行过程中的结构化进度。工具结束后到达的迟到进度应被忽略。
    ToolCallUpdated {
        id: String,
        name: String,
        output: ToolOutput,
    },
    /// 工具执行完毕。`output` 已截断/清洗,可直接渲染。
    ToolCallFinished {
        id: String,
        name: String,
        output: ToolOutput,
        error: Option<ToolError>,
    },

    /// A committed structured plan snapshot. Frontends must not display speculative state.
    PlanUpdated {
        revision: u64,
        items: Vec<PlanItem>,
        explanation: Option<String>,
    },

    /// The startup-frozen local skill catalog and any discovery diagnostics.
    SkillsDiscovered {
        skills: Vec<SkillMetadata>,
        warnings: Vec<String>,
    },

    /// Runtime 正在独立审批通道上等待；普通命令通道不会承担回复职责。
    PermissionRequested {
        request: ApprovalRequest,
    },
    PermissionResolved {
        request_id: String,
        allowed: bool,
    },

    /// 累计 token 用量(每次模型调用结束后推一次,值为会话累计)。
    Usage {
        input_tokens: u64,
        output_tokens: u64,
        cache: Option<CacheUsage>,
    },

    /// 非致命提示(重试中、provider 已切换、历史已清空……)。
    Notice(String),
    /// 会话历史已清空(前端应同步清空画面)。
    ConversationCleared,
    /// provider/model/effort 已原子变化。
    ModelSelectionChanged {
        provider: String,
        model: String,
        effort: String,
        label: String,
    },
    /// 当前 workspace 的会话列表。
    SessionsListed {
        current_id: String,
        sessions: Vec<SessionSummary>,
    },
    /// Session runtime 已接纳输入并放入对应队列。
    InputQueued {
        command_id: String,
        kind: InputQueueKind,
        text: String,
    },
    /// 排队输入已提交为新的用户消息，不再属于 pending queue。
    InputDequeued {
        command_id: String,
    },
    /// 历史会话已载入；前端据此重建对话画面(含 Notice 等 UI-only 事实)。
    SessionLoaded {
        id: String,
        entries: Vec<SessionEntry>,
        input_tokens: u64,
        output_tokens: u64,
        cache: Option<CacheUsage>,
    },
    /// 本轮出错终止(HTTP 错误、流解析失败……)。会话仍可继续用。
    Error(String),

    /// 一轮结束。`cancelled` = 用户按了 Esc。
    TurnFinished {
        cancelled: bool,
    },
}
