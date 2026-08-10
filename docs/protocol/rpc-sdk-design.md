# OneMore SDK 与 JSONL RPC 设计

状态：RPC/SDK v3 已实现
更新时间：2026-08-10

本文定义 OneMore 下一阶段的本地 SDK 边界和首版 JSONL RPC 协议。目标不是复制 Pi 的
全部 server、client 或扩展系统，而是在现有弱 harness 上建立一个长期可维护的外部入口：
TUI、`--once`、嵌入调用和 RPC 最终都驱动同一个 stateful `Agent` 和同一条
`run_agent_loop` 生产路径。

## 1. 目标与非目标

### 1.1 目标

- 给 Rust 宿主提供小而稳定的 session controller，不要求宿主操作裸 MPSC 通道。
- 给子进程调用者提供严格、版本化、可关联请求与响应的 JSONL 协议。
- 明确区分“命令已接纳”“命令已结束”和“session 已稳定空闲”。
- 用权威 snapshot 表达可恢复状态，用 progress event 表达瞬时流式活动。
- 复用现有事实日志、权限、工具执行、compaction 和模型切换实现，不复制第二套 runtime。
- 保持 ToolUse/ToolResult 配对、原子事实提交和 append-only 日志不变。
- 审批断连时 fail closed；慢客户端不能让事件无限堆积。

### 1.2 首版非目标

- 不实现 length-prefixed CBOR、WebSocket、Unix socket 或 transport 插件系统。
- 不实现多 session server、shared lease、多 lane 或跨进程 durable operation log。
- 不实现工具崩溃重放、provider 请求重放或进程崩溃后的运行恢复。
- 不实现 Pi 的动态扩展包、热重载、TUI renderer、动态 flag/shortcut。
- 不增加绕过 ToolRegistry、hooks、permissions 的 RPC `bash` 或其他执行旁路。
- 不把 `Config`、SQLite schema、`SessionEntryPayload` 或 provider 私有响应直接定为线协议。
- 不为旧实验协议或旧配置保留兼容层；首版直接收敛最终接口。

## 2. 分层与唯一生产路径

```text
Rust embedder         TUI / --once              JSONL client
      |                    |                          |
      +------------ SessionController ---------------+
                           |
                    command admission
                    phase / queue / ack
                           |
                     stateful Agent
                           |
                    run_agent_loop
                           |
       facts / tools / permissions / hooks / compaction
```

边界规则：

1. `SessionController` 是本地 SDK 的唯一命令入口。
2. RPC 只做 JSON DTO 校验、SDK 调用和事件编码，不拥有 agent 行为。
3. TUI 和 `--once` 迁移到同一 admission 路径后，旧的裸通道入口不再公开。
4. SDK 公共 view types 与内部 domain types 分离，由一个 adapter 单向转换。
5. snapshot 只能从已提交事实和 Runtime 当前权威状态生成，不能包含前端乐观状态。

## 3. 本地 SDK 接口

以下接口已在 `src/sdk.rs` 与 `src/sdk/` 实现。首版保持同步、阻塞式 Rust API，与当前线程模型一致；
暂不为此引入 async runtime。

```rust
pub struct AgentSession {
    pub controller: SessionController,
    pub events: SessionEvents,
}

#[derive(Clone)]
pub struct SessionController { /* private */ }

pub struct SessionEvents { /* single consumer */ }

pub struct CommandReceipt {
    pub command_id: String,
}

pub fn spawn_session(agent: Agent) -> AgentSession;

impl SessionController {
    pub fn prompt(&self, text: impl Into<String>) -> Result<CommandReceipt, SessionError>;
    pub fn steer(&self, text: impl Into<String>) -> Result<CommandReceipt, SessionError>;
    pub fn follow_up(&self, text: impl Into<String>) -> Result<CommandReceipt, SessionError>;
    pub fn abort(&self) -> Result<CommandReceipt, SessionError>;
    pub fn compact(&self) -> Result<CommandReceipt, SessionError>;

    pub fn set_model(&self, selection: ModelSelection) -> Result<CommandReceipt, SessionError>;
    pub fn clear_conversation(&self) -> Result<CommandReceipt, SessionError>;
    pub fn list_sessions(&self) -> Result<Vec<SessionSummaryView>, SessionError>;
    pub fn list_all_sessions(&self) -> Result<Vec<SessionSummaryView>, SessionError>;
    pub fn load_session(&self, id: impl Into<String>) -> Result<CommandReceipt, SessionError>;
    pub fn list_models(&self) -> Result<Vec<ModelMetadata>, SessionError>;

    pub fn snapshot(&self) -> Result<SessionSnapshot, SessionError>;
    pub fn respond_to_approval(
        &self,
        response: ApprovalResponse,
    ) -> Result<(), SessionError>;
    pub fn wait_until_settled(
        &self,
        timeout: Duration,
    ) -> Result<SessionSnapshot, SessionError>;
    pub fn shutdown(&self) -> Result<(), SessionError>;
}

impl SessionEvents {
    pub fn recv(&mut self) -> Result<SessionEvent, SessionDisconnected>;
    pub fn recv_timeout(
        &mut self,
        timeout: Duration,
    ) -> Result<SessionEvent, RecvTimeoutError>;
    pub fn try_recv(&mut self) -> Result<SessionEvent, TryRecvError>;
}
```

接口约束：

- `SessionController` 可克隆，`SessionEvents` 首版只有一个消费者，不做事件 fan-out。
- mutation 方法只等到 Runtime 完成 admission，不等待整个 agent run。
- query 方法必须由 Runtime 线程读取权威状态，不能读取启动时复制出的旧元数据。
- `wait_until_settled` 不消费调用方的事件流，应由 Runtime 内部状态通知完成。
- 所有 mutation 都分配稳定的 `command_id`；事件可用它关联命令。
- `RuntimeHandle` 中公开的 sender、receiver、`AtomicBool` 最终变成私有实现细节。

## 4. 命令接纳与完成语义

### 4.1 三种状态不能混用

| 状态 | 含义 |
|---|---|
| accepted | Runtime 已验证命令并取得所有权，调用方不应重发 |
| finished | 该命令已经成功、失败或取消，必须有明确终态 |
| settled | 没有运行、重试、压缩、审批等待或已接纳的排队输入 |

channel `send()` 成功不等于 accepted。每条命令 envelope 必须携带一次性 ack 通道；只有
Runtime 完成 admission 后才能返回 `CommandReceipt` 或 RPC `ok: true`。

### 4.2 输入命令

- `prompt`：仅在 `idle` 时接纳。忙碌时返回结构化 `busy` 错误，调用方必须显式选择
  `steer` 或 `follow_up`。
- `steer`：运行中在完整工具批提交后注入；空闲时启动一个新 run。
- `follow_up`：运行中在当前任务原本将停止时注入；空闲时启动一个新 run。
- 空字符串或纯空白输入在 admission 阶段拒绝，不产生事实和事件。
- 已接纳输入不得静默丢弃。它最终必须执行，或发出带相同 `command_id` 的 cancelled/failed
  终态。取消收尾不能只发 Notice 后清空队列。
- 首版不承诺进程崩溃后恢复内存队列；该能力属于未来 durable operation，不扩入当前 harness。

### 4.3 其他 mutation

- `abort` 的 accepted 表示取消信号已经设置，不表示当前 run 已完成；等待结束使用
  `wait_until_settled` 或 `settled` event。
- `compact` 的 accepted 表示压缩任务已取得 session 独占执行权；摘要失败通过事件报告，
  不追加 Compaction 事实。
- `set_model` 必须一次提交 provider、model、effort，不能让客户端观察半切换状态。
- `clear_conversation` 和 `load_session` 仅在 `idle` 接纳，成功后清理 session 级权限授权。
- `approval_response` 只有匹配当前 pending request 时成功；过期、重复和错误 ID 明确报错。

### 4.4 settled

`settled` 只允许在 busy 到 idle 的状态迁移时发一次，并且必须晚于：

- 当前模型流 terminal；
- 完整工具批执行与原子提交；
- stop hook、planning reminder 和 follow-up 检查；
- 自动 retry；
- threshold/overflow compaction 及其最多一次恢复重试；
- 审批成功、拒绝、取消或断连收尾；
- 所有已接纳输入的 finished 终态。

普通 assistant 消息结束或现有 `TurnFinished` 不能直接等同于 settled。

## 5. 应当暴露的数据

SDK 和 RPC 使用同一组稳定 view types。view types 可以派生 serde，但不得与内部事实枚举使用
同一个 Rust 类型，从而允许内部投影和存储继续演进。

### 5.1 ServerInfo

在 SDK 中由 `server_info()` 或 `list_models()` 提供，在 RPC 中由 hello 返回。

```text
ServerInfo
  server_id: String              // 本次进程实例，重启后变化
  protocol_version: u32
  capabilities: Capabilities
  models: Vec<ModelMetadata>

Capabilities
  compaction: bool
  session_management: bool
  interactive_approval: bool
  steering: bool
  follow_up: bool

ModelMetadata
  provider: String
  model: String
  label: String
  supported_efforts: Vec<String>
  default_effort: String
```

模型信息不得包含 API key、认证状态原文、base URL、请求 headers 或 provider 私有配置。

### 5.2 SessionSnapshot

snapshot 是前端重建当前 session 画面的唯一权威来源。

```text
SessionSnapshot
  session_id: String
  revision: u64
  workspace: String
  phase: SessionPhase
  model: ModelSelectionView
  usage: UsageView
  transcript: Vec<TranscriptItem>
  plan: PlanView
  queues: QueueView
  pending_approval: Option<ApprovalRequestView>

SessionPhase
  idle | running | retrying | compacting | waiting_approval | shutting_down

ModelSelectionView
  provider: String
  model: String
  effort: String
  label: String

UsageView
  input_tokens: u64
  output_tokens: u64
  cache_read_tokens: Option<u64>
  cache_write_tokens: Option<u64>

QueueView
  steering: Vec<QueuedInputView>
  follow_up: Vec<QueuedInputView>

QueuedInputView
  command_id: String
  text: String

PlanView
  revision: u64
  items: Vec<PlanItemView>
  explanation: Option<String>
```

`revision` 是本次 Runtime 实例内单调递增的 snapshot revision。phase、queue、审批或 committed
transcript 发生权威变化时递增。重启后通过新的 `server_id` 区分 revision epoch。

### 5.3 TranscriptItem

snapshot 不暴露原始 `SessionEntryPayload`。由事实日志投影为面向前端的封闭枚举：

```text
TranscriptItem
  user_message
    id, parent_id, created_at, text, command_id?

  assistant_message
    id, parent_id, created_at, blocks, status

  tool
    tool_call_id, name, summary, status, output?, error?

  notice
    id, created_at, level, text
```

assistant block 首版只包含：

- `text { text }`
- `thinking { text }`
- `tool_call { id, name, summary }`

明确不暴露：

- `Block::Thinking.raw` 和 `provider_kind`；
- provider 原始 response item、加密 reasoning 或 headers；
- 未校验的工具参数原文；工具调用只暴露人类可读 `summary`；
- `ToolOutput.model_text` 与任意 `details` 的原始 JSON。首版只暴露清洗后的 UI output；
- compaction prompt、system prompt、完整 context、prompt cache key；
- SQLite 文件路径、连接信息、内部 sequence 和 schema version。

如果以后确实需要结构化 tool details，必须为具体公共字段建立稳定 DTO 和清洗规则，不能直接把
`serde_json::Value` 穿透到协议。

### 5.4 SessionSummaryView

```text
SessionSummaryView
  id: String
  title: String
  workspace: String
  message_count: usize
  updated_at: i64
```

`list_sessions()` 与 RPC `list_sessions` 默认只返回当前 workspace。`list_all_sessions()` 或
`list_sessions { all: true }` 用于跨 workspace 发现，摘要必须携带 `workspace`。当前 Runtime 的
workspace、项目指令和权限根在启动时冻结，因此跨 workspace 摘要不可直接传给 `load_session`；
调用方应在目标 workspace 启动新的 Runtime。

### 5.5 ApprovalRequestView

```text
ApprovalRequestView
  request_id: String
  tool: String
  summary: String
  reason: String
  scopes: Vec<ApprovalScopeView>

ApprovalScopeView
  once | session

ApprovalDecisionView
  allow_once | allow_session | deny
```

审批只暴露当前权限系统已经生成的 summary 和 reason，不补充原始工具参数。RPC 输入结束、输出
断开、client 退出或审批响应通道关闭时，一律按 deny 收尾，并产生 ToolResult。

### 5.6 SessionEvent

事件分为权威状态和瞬时进度：

```text
SessionEvent
  session_snapshot { snapshot }
  progress { progress }
  command_finished { command_id, status, error? }
  settled { revision }

CommandStatus
  succeeded | failed | cancelled

ProgressEvent
  run_started { command_id }
  retry_scheduled { attempt, max_retries, delay_ms, error }
  retry_started { attempt, max_retries }
  compaction_started { compaction_id, trigger, estimated_tokens, available_tokens? }
  compaction_finished { compaction_id, trigger, tokens_before, summary_chars, retained_messages }
  compaction_failed { compaction_id, trigger, error, cancelled, history_changed }
  assistant_delta { message_id, content_index, kind, delta }
  tool_started { tool_call_id, name, summary }
  tool_updated { tool_call_id, name, output }
  tool_finished { tool_call_id, name, output, error? }
  approval_requested { request }
  approval_resolved { request_id, allowed }
  notice { level, text }

ToolOutputView
  content
  summary
  metadata { command?, cwd?, elapsed_ms?, exit_code? }
```

约束：

- delta 只携带新增片段，不携带累计 assistant message。
- `retry_scheduled` 前先发布 phase=`retrying` 的 snapshot；`retry_started` 前恢复到
  phase=`running`，压缩调用内的重试则恢复到 phase=`compacting`。客户端无需从 Notice 文本
  猜测重试或压缩状态。
- 每个 `compaction_started` 必须按 `compaction_id` 配对一个 finished/failed 终态；trigger
  区分 `automatic` 与 `manual`，失败事件明确说明取消状态及历史是否改变。
- `session_snapshot.transcript` 和最终 transcript item 是权威值；客户端可用它纠正 delta 组装。
- streaming 中尚未完成的工具参数不进入 snapshot，也绝不能执行。
- tool progress 到达 `tool_finished` 后关闭，迟到更新必须忽略。
- `tool_updated.output` 与 `tool_finished.output` 使用同一个封闭 view：`content` 是经过清理和
  长度限制的模型正文，`summary` 用于紧凑状态行，`metadata` 只允许命令、工作目录、耗时和
  退出码；工具任意 `details`、provider raw 和未完成参数不得穿过 SDK/RPC 边界。
- 错误使用稳定 `code + message`；内部 anyhow chain 只写 stderr，不进入协议。

## 6. JSONL RPC v3

### 6.1 Framing

- stdin/stdout 使用 UTF-8 JSONL，一行一个完整 JSON object。
- 只以 LF (`0x0A`) 分帧；字符串里的 `U+2028`、`U+2029` 不是分隔符。
- 接收时可容忍 LF 前一个 CR，但发送始终只写 LF。
- 首版设定固定最大行长；超过限制返回 protocol error 后关闭连接。
- request ID 最多 256 UTF-8 bytes；单连接最多使用 65,536 个不同 ID，达到上限后安全关闭。
- stdout 只允许协议帧；日志、诊断和 panic 信息走 stderr。
- 每个 object 都严格拒绝未知字段，Rust DTO 使用 `#[serde(deny_unknown_fields)]`。
- malformed JSON、未知 tag、重复 request ID 都是明确协议错误，不能静默忽略。

### 6.2 Hello

client 的第一帧必须是：

```json
{"type":"hello","version":3}
```

成功响应：

```json
{
  "type":"hello",
  "version":3,
  "server":{"server_id":"srv-1","protocol_version":3,"capabilities":{"compaction":true,"session_management":true,"interactive_approval":true,"steering":true,"follow_up":true},"models":[]},
  "snapshot":{"session_id":"session-1","revision":0,"workspace":"E:\\work","phase":"idle","model":{"provider":"openai","model":"gpt-5","effort":"medium","label":"OpenAI / gpt-5"},"usage":{"input_tokens":0,"output_tokens":0,"cache_read_tokens":null,"cache_write_tokens":null},"transcript":[],"plan":{"revision":0,"items":[],"explanation":null},"queues":{"steering":[],"follow_up":[]},"pending_approval":null}
}
```

版本不匹配：

```json
{"type":"hello_error","error":{"code":"version_mismatch","message":"unsupported protocol version 1"}}
```

hello 失败后进程退出，不尝试兼容或降级。

v3 采用精确版本协商：同一 `version` 内只允许保持现有字段语义的实现修复；任何新增必填字段、
tag 改名或语义不兼容变化都必须提升协议版本。服务端不猜测、不降级，也不为实现前的实验报文
保留兼容分支。

### 6.3 Request / Response envelope

```json
{"type":"request","id":"req-1","request":{"command":"prompt","text":"检查当前项目"}}
```

mutation 被接纳：

```json
{"type":"response","id":"req-1","ok":true,"result":{"command":"prompt","command_id":"cmd-1"}}
```

admission 失败：

```json
{"type":"response","id":"req-1","ok":false,"error":{"code":"busy","message":"session is running; use steer or follow_up"}}
```

event envelope：

```json
{"type":"event","event":{"type":"settled","revision":9}}
```

request `id` 由 client 分配，只负责关联本次 response；`command_id` 由 Runtime 分配，用于关联
已接纳命令与后续事件。每个 request 恰好一个 response。

### 6.4 v3 命令集

```text
prompt { text }
steer { text }
follow_up { text }
abort
compact
set_model { provider, model, effort }
clear_conversation
list_sessions { all?: bool }
load_session { session_id }
list_models
get_snapshot
approval_response { request_id, decision }
shutdown
```

首版不提供 `cycle_model`、`cycle_effort` 等 UI convenience command；客户端读取模型目录后明确
提交目标值。也不提供任意 slash command RPC，slash 解析继续属于具体 CLI 前端。

query 命令在 response 的 `result` 中返回数据，不产生 `command_id`。mutation 命令返回
`command_id`，完成状态通过事件报告。`shutdown` 成功 response flush 后再退出。
`list_sessions` 省略 `all` 时等价于 `all: false`；`all: true` 只扩展枚举范围，不放宽
`load_session` 的 workspace 限制。

### 6.5 背压和断连

- Runtime 到 JSON writer 使用有界队列。
- writer 每写一行都检查 write/flush 错误；broken pipe 触发 abort 和 shutdown。
- 不能因为前端退出而继续静默运行有副作用的工具。
- 慢 reader 达到队列上限时，Runtime 在事件边界施加背压，不允许无限占用内存。
- 工具子进程取消、审批 fail-closed、ToolResult 配对和最终事实提交仍由现有 runtime 负责。
- stdin EOF 等价于 transport shutdown：取消当前运行、拒绝 pending approval、完成安全收尾后退出。

## 7. 使用示例

以下示例使用当前 v3 接口。

### 7.1 Rust 嵌入

```rust
use std::time::Duration;

use onemore::runtime::Agent;
use onemore::sdk::{spawn_session, SessionEvent, SessionPhase};

let agent = Agent::builder_from_provider(settings, workspace)
    .in_memory()
    .provider_factory(provider_factory)
    .build()?;

let mut session = spawn_session(agent);
let receipt = session.controller.prompt("检查 src/runtime.rs")?;

while let Ok(event) = session.events.recv() {
    match event {
        SessionEvent::Progress { progress } => eprintln!("{progress:?}"),
        SessionEvent::CommandFinished { command_id, status, .. }
            if command_id == receipt.command_id =>
        {
            eprintln!("command finished: {status:?}");
        }
        SessionEvent::Settled { .. } => break,
        _ => {}
    }
}

let snapshot = session
    .controller
    .wait_until_settled(Duration::from_secs(60))?;
assert_eq!(snapshot.phase, SessionPhase::Idle);
```

### 7.2 RPC prompt 与流式事件

client：

```json
{"type":"hello","version":3}
{"type":"request","id":"req-1","request":{"command":"prompt","text":"解释当前 runtime 边界"}}
```

server：

```json
{"type":"hello","version":3,"server":{"server_id":"srv-1","protocol_version":3,"capabilities":{"compaction":true,"session_management":true,"interactive_approval":true,"steering":true,"follow_up":true},"models":[]},"snapshot":{"session_id":"session-1","revision":0,"workspace":"E:\\work","phase":"idle","model":{"provider":"openai","model":"gpt-5","effort":"medium","label":"OpenAI / gpt-5"},"usage":{"input_tokens":0,"output_tokens":0,"cache_read_tokens":null,"cache_write_tokens":null},"transcript":[],"plan":{"revision":0,"items":[],"explanation":null},"queues":{"steering":[],"follow_up":[]},"pending_approval":null}}
{"type":"response","id":"req-1","ok":true,"result":{"command":"prompt","command_id":"cmd-1"}}
{"type":"event","event":{"type":"progress","progress":{"type":"run_started","command_id":"cmd-1"}}}
{"type":"event","event":{"type":"progress","progress":{"type":"assistant_delta","message_id":"msg-1","content_index":0,"kind":"text","delta":"当前 "}}}
{"type":"event","event":{"type":"progress","progress":{"type":"assistant_delta","message_id":"msg-1","content_index":0,"kind":"text","delta":"runtime..."}}}
{"type":"event","event":{"type":"command_finished","command_id":"cmd-1","status":"succeeded","error":null}}
{"type":"event","event":{"type":"settled","revision":5}}
```

client 必须在发送 prompt 前开始消费事件。response 与 progress 都来自同一个 stdout 序列，
但不能假设“收到 response 后才可能收到所有相关 progress”；只能依赖 request ID、command ID
和各自的类型语义。

### 7.3 运行中 steering

```json
{"type":"request","id":"req-2","request":{"command":"steer","text":"先不要改代码，只输出风险"}}
{"type":"response","id":"req-2","ok":true,"result":{"command":"steer","command_id":"cmd-2"}}
```

snapshot 的 `queues.steering` 随后包含 `cmd-2`。它在完整工具批提交后被注入；客户端不需要
修改本地 transcript 来进行乐观展示。

### 7.4 审批往返

server：

```json
{"type":"event","event":{"type":"progress","progress":{"type":"approval_requested","request":{"request_id":"approval-1","tool":"run_command","summary":"cargo test --locked","reason":"command execution requires approval","scopes":["once","session"]}}}}
```

client：

```json
{"type":"request","id":"req-3","request":{"command":"approval_response","request_id":"approval-1","decision":"allow_once"}}
```

server：

```json
{"type":"response","id":"req-3","ok":true,"result":{"command":"approval_response"}}
{"type":"event","event":{"type":"progress","progress":{"type":"approval_resolved","request_id":"approval-1","allowed":true}}}
```

## 8. 维护规则

- v3 内只接受保持现有字段语义的实现修复；线协议不直接暴露内部 runtime 类型。
- 新命令或必填字段必须提升协议版本，并同步更新 SDK view、wire tests、README 和集成示例。
- transport 断连继续 fail closed；accepted 命令必须逐个得到终态，最终 snapshot 必须能够纠正
  客户端对流式事件的临时组装。
- 分支、插件、多进程 session server 等能力只有出现明确产品需求后才单独设计，不能扩张 v3。

## 9. 完成定义

RPC/SDK 阶段只有同时满足以下条件才算完成：

1. Rust embedder、TUI、`--once` 和 RPC 通过同一个 SessionController admission 路径驱动同一个
   stateful Agent。
2. RPC 成功响应确实代表 Runtime 已接纳，而不是仅仅写入 channel。
3. 每个已接纳命令都有明确终态，settled 不早发、不漏发、不重复发。
4. 任意时刻取得的最新 snapshot 都只包含权威状态，并可重建前端画面。
5. 审批和 transport 断连 fail closed，慢客户端不会导致无界内存增长。
6. 没有执行旁路，ToolUse/ToolResult、原子提交和 append-only 不变量全部保持。
7. 完整测试、Clippy、fmt、rustdoc 和 `git diff --check` 全部通过。
