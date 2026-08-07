# Onemore Pi 化重构接手文档

更新日期：2026-08-07  
工作区：`E:\onemore-cli`  
分支：`main`  
当前 HEAD：`3473c60 制作远程协议施工文档`

## 目标与约束

目标是把 Onemore 演进成接近 Pi 理念的 Rust agent：核心 loop 薄、可组合，CLI、持久化、
planning、compaction、permissions、skills 等属于可替换 harness。

用户明确要求：

- 可以直接修改最终接口，不需要兼容旧数据或旧配置迁移。
- 不要复制一套未被 CLI 使用的“纯 loop”；必须让生产路径使用抽出的核心。
- 单文件行数和目录可读性属于架构质量，按职责拆分，不做无语义的任意切片。
- 保留现有可靠性不变量，不能用“更简洁”换取静默失败或历史损坏。

## 当前 Git 状态

HEAD 已包含 provider 可靠性、弱 harness、根目录 `AGENTS.md`、自动上下文压缩，以及
`docs/rpc-sdk-design.md` 的 RPC/SDK v1 规格。当前工作区是该规格的未提交实现，涉及 SDK view、
SessionController、TUI/`--once` 迁移和 JSONL RPC。不要 reset、checkout 或覆盖这些文件；
以 `git status --short` 为准继续收尾。

## HEAD 已完成的可靠性修复

这些内容已经在 `2a3e029` 中，不应重做：

- OpenAI `prompt_cache_key` 固定为 64 字符稳定语义哈希。
- Anthropic/Responses 裸 `[DONE]` 不再冒充协议 terminal。
- 非 `max_output_tokens` 的 `response.incomplete` 明确失败。
- model change 后旧模型 usage baseline 失效，直到新模型返回真实 usage。
- runtime 从约 3,800 行拆为 builder、commands、agent loop、tool execution、compaction。
- runtime 测试按 builder、permissions、planning、history、queues、concurrency 分组。

## 当前未提交改动已完成什么

### 1. Agent 不再依赖具体状态实现

公开接口位于 `src/harness.rs`：

- `ModelRegistry`：初始选择、模型解析、校验、默认 effort 和 provider catalog。
- `SessionBackend`：原子追加 facts；clear/list/load 是可选管理能力。
- `ModelPreferences`：读取和保存 provider/model effort 覆盖。

`Agent` 当前只持有上述 trait object，不再持有：

- `Config`
- `SessionManager`
- `WorkspacePreferences`
- `SkillCatalog`

默认 CLI 适配关系：

```text
Config                -> ModelRegistry
SessionManager        -> SessionBackend
WorkspacePreferences  -> ModelPreferences
```

### 2. 无配置、无状态目录的嵌入入口

单模型宿主可以直接使用：

```rust
Agent::builder_from_provider(settings, workspace)
    .in_memory()
    .provider_factory(host_factory)
    .build()?;
```

该路径不读取 `config.toml`，不创建 SQLite、workspace preference 或 skills 目录。

多模型宿主可以调用 `AgentBuilder::from_model_registry(...)`。固定单模型实现位于
`src/harness/model.rs`；内存 session/preferences 位于 `src/harness/memory.rs`。

### 3. Skills 已成为真正可选组件

`AgentBuilder` 支持：

- 默认本地 discovery。
- `skill_catalog(...)` 注入冻结 catalog。
- `disable_skills()` 完全关闭。
- `in_memory()` 默认关闭 skills。

关闭时同时移除：

- 本地目录扫描。
- `SkillsContext` system section。
- `load_skill` tool。
- 默认 prompt 中的 skills 指令。
- `SkillsDiscovered` startup event。

显式 `system_prompt` 始终保持宿主原文，不会被 builder 改写。

### 4. Startup 状态不再污染 Agent 类型

Builder 把 skill metadata/warnings 预生成成 `startup_events`。`Agent` 只排空一次事件队列，
不再为了 UI 启动事件持有 `SkillCatalog`。

### 5. 文件结构继续收敛

当前关键文件规模（以最终 `wc`/PowerShell 行数为准）：

```text
src/agent_loop.rs                  428
src/agent_loop/model_call.rs       124
src/agent_loop/tests.rs            230
src/runtime/agent_loop.rs          524
src/runtime/tool_execution.rs      460
src/runtime/commands.rs            278
src/runtime/builder.rs             346
src/runtime/commands.rs            332
src/harness.rs                      75
src/harness/model.rs                89
src/harness/memory.rs              227
src/config.rs                       844  (测试已迁到 src/config/tests.rs)
src/storage.rs                      767  (测试已迁到 src/storage/tests.rs)
```

`src/config/tests.rs` 中 `config.example.toml` 的相对路径应保持
`include_str!("../../config.example.toml")`。

## 必须保持的可靠性不变量

后续抽纯 loop 时，以下行为不可回退：

1. Provider 每次调用必须终止于 `Done / Error / Aborted`，裸 EOF 或裸 `[DONE]` 不是成功。
2. 自动 retry 只允许发生在尚未产生任何流事件时，避免重复副作用或重复文本。
3. ToolUse 必须恰好配对一个 ToolResult；截断的工具参数不得执行。
4. 一批 assistant ToolUse、harness effects 和全部 ToolResult 必须原子提交。
5. `SessionBackend::append_payloads` 返回错误时，Agent 内存 facts 不得推进。
6. `MemorySessionBackend` 与 SQLite backend 都必须执行 message batch 和 plan append 校验。
7. 权限顺序必须保持：prepare/schema -> hard deny -> pre-hook -> re-prepare -> permission。
8. 并发工具 UI 事件按完成顺序，持久 ToolResult 按源顺序。
9. model change 后不得复用旧 tokenizer/provider 的 usage baseline。
10. prompt cache identity 只包含稳定语义；动态消息、session ID、时间戳不得进入 key。
11. ProviderFactory 必须在启动和后续 provider/model 切换中持续复用。
12. `disable_skills()` 后 prompt 和 tool schema 都不能残留 skills 能力。

## 已完成验证

当前未提交状态已经通过：

```text
cargo test --locked
  161 unit tests passed
  8 wire tests passed

cargo clippy --locked --all-targets -- -D warnings
cargo fmt --all --check
$env:RUSTDOCFLAGS='-D warnings'; cargo doc --locked --no-deps
git diff --check
```

Cargo 在当前 Windows 文件系统上会提示 incremental cache 无法 hard link、自动退回复制；
这是环境警告，不是 Rust/Clippy 代码告警。

关键新增回归测试：

- `runtime::tests::builder::direct_in_memory_builder_needs_no_config_state_or_skills_directories`
- `harness::memory::tests::memory_backend_rejects_half_tool_batches_without_advancing`
- `harness::memory::tests::memory_backend_lists_and_clears_committed_facts`
- `context::instructions::tests::skill_guidance_tracks_the_available_harness`

## 已完成：生产路径使用公开纯 Loop

公开入口位于 `src/agent_loop.rs`：

```rust
run_agent_loop(model, messages, tools, callbacks)
```

- core 只拥有 provider 调用、assistant/tool 回合状态机、retry 和 queue 检查点。
- `AgentLoopHost` callback 负责 prompt 变换、原子提交、工具回合、steering/follow-up 与收尾。
- `src/runtime/agent_loop.rs` 是默认 stateful adapter；facts projection、usage budget、stop hook、
  planning reminder 和 cancellation plan repair 都留在这里。
- `src/runtime/tool_execution.rs` 的 `DefaultToolExecutor` 不再持有 `Agent`，只借用 registry、
  workspace、permissions、hooks、session ID 与审批通道。
- compaction 和 session/model commands 继续由 stateful `Agent` 协调，不进入 core API。
- CLI、`Agent::new(...)` 与 `in_memory()` 都从 `Agent::run_turn` 调用同一个公开 core；旧循环
  实现已删除，没有兼容用第二路径。

建议阅读顺序：

```text
src/agent_loop.rs
src/agent_loop/model_call.rs
src/runtime/agent_loop.rs
src/runtime/tool_execution.rs
src/runtime/commands.rs
src/runtime/builder.rs
src/harness.rs
example/pi/packages/agent/src/agent-loop.ts
example/pi/packages/agent/src/agent.ts
```

## 本阶段验收状态

- `Agent::builder_from_provider(...).in_memory()` 仍不创建任何 Onemore 状态目录。
- CLI `Agent::new(...)` 和嵌入入口执行同一个 core loop。
- core loop 的公开输入不包含 `Config`、`SessionBackend`、SQLite、skills discovery 或 TUI。
- planning/compaction/session management 可以不装配，且不在 prompt/tool schema 中留下残余。
- 新增 4 个 `agent_loop::tests`，直接测试四输入 API、工具配对、follow-up 检查点和提交失败
  不推进 transcript。
- 原有 157 个 unit 与 8 个 wire 测试继续通过；新增后共 161 + 8，通过上方完整质量门禁。

## 相关文档

- `docs/runtime-architecture.md`：当前模块边界与可替换组件。
- `docs/prompt-cache.md` / `docs/prompt-cache-cn.md`：稳定 prompt prefix 与 cache key。
- `README.md`：CLI 和嵌入用法。

## 当前阶段：根目录 AGENTS.md 与基础 prompt 微调

- 新增 `src/context/project_instructions.rs`，默认 harness 仅发现 workspace 根目录
  `AGENTS.md`；缺失或空文件不注入，读取失败产生一次非致命 startup notice。
- 项目指令在 builder 构造时冻结，并按 Pi 风格渲染为 `<project_context>` /
  `<project_instructions path="...">`。第一版不支持祖先目录、嵌套作用域、别名、override、
  全局文件或热重载。
- 默认 system section 顺序调整为 instructions、project instructions、skills、environment；
  显式 `context_providers(...)` 继续完整替换默认 context，也不会触发 `AGENTS.md` 扫描。
- 基础 prompt 的 identity 收敛为极简 coding-agent 描述；新增“专用工具优先于 shell”规则，
  plan 规则只强化在有实质进展时保持完整快照更新。其余 Onemore 独有规则未修改。
- 新增 project-instructions 单元测试与 builder 集成测试，覆盖根目录限定、空文件、冻结快照、
  XML path 转义、无效 UTF-8 非致命提示、默认顺序及宿主 context 完整替换。

本阶段完整验收结果：

```text
cargo test --locked
  167 unit tests passed
  8 wire tests passed

cargo clippy --locked --all-targets -- -D warnings
cargo fmt --all --check
$env:RUSTDOCFLAGS='-D warnings'; cargo doc --locked --no-deps
git diff --check
```

## 当前阶段：自动上下文压缩已完成

- 新增公开 `CompactionSettings` 与 Builder/`[compaction]` 配置，默认自动启用，也可关闭；
  阈值按正常输入预算减 `reserve_tokens` 计算，`keep_recent_tokens` 控制保留尾部。
- 手动 `/compact` 与请求前自动触发共用 `runtime::compaction::CompactionRuntime`，没有第二套
  摘要或持久化实现；摘要请求继续是零工具、单条纯文本 user 消息。
- `CompactionRecord` 直接收敛为 summary、tokens_before、retained_messages；模型投影从最新
  摘要、保留尾部和后续事实重建，原始事实不删除、不改写。
- 确定性切分不会从 ToolResult 开始 retained tail；SessionBackend 提交边界再次校验尾部
  ToolUse/ToolResult 闭合。摘要失败、取消或原子提交失败都不追加 Compaction 事实。
- 自动压缩成功后在同一 core loop 内重新投影并执行原有硬预算检查；关闭自动触发时仍保留
  原有工具结果折叠与明确拒绝行为。
- 回归测试已按职责迁到 `src/runtime/tests/compaction.rs`，覆盖成功、纯文本请求、关闭、失败、
  取消、持久化失败、retained tail 和 append-only 行为。

本阶段完整验收结果：

```text
cargo test --locked
  175 unit tests passed
  8 wire tests passed

cargo clippy --locked --all-targets -- -D warnings
cargo fmt --all --check
$env:RUSTDOCFLAGS='-D warnings'; cargo doc --locked --no-deps
git diff --check
```

## 当前阶段：Rust SDK 与 JSONL RPC v1

已实现：

- `src/sdk.rs` 与 `src/sdk/`：稳定 view types、sanitized fact projection、
  `SessionController`、`SessionEvents`、admission receipt、phase、command terminal 和 settled。
- `src/runtime/{inbox,session_runtime,session_events}.rs`：有界 command/event worker、运行中
  prompt busy、steering/follow-up 可观察队列、detached shutdown，以及 approval 原子占用。
- TUI、`--once`、Rust embed 和 RPC 全部使用 `sdk::spawn_session`，旧 `RuntimeHandle` 与旧
  production spawn 已删除。
- `src/rpc.rs` 与 `src/rpc/`：4 MiB LF JSONL framing、hello/version、严格 wire DTO、完整 v1
  command adapter、重复 request ID、EOF/broken-pipe 收尾和 CLI `--rpc`。
- RPC 服务循环保持排空有界事件队列；可能等待 runtime checkpoint 的请求在独立 worker 中做
  admission，避免主 writer 与 event producer 互相阻塞。

关键回归测试：

- snapshot 不泄漏 provider raw、thinking raw、工具原始参数或任意 details。
- `CommandFinished` 先于 `Settled`，运行中第二个 prompt 返回 `busy`。
- steering queue 在 snapshot 中出现并在提交后移除，每个 accepted command 只有一个终态。
- session backend 的 list 错误会唤醒 controller，不会永久等待 Condvar。
- JSONL 覆盖 hello、prompt/events/snapshot、重复 ID、未知字段、busy、审批往返、EOF、
  broken pipe、CRLF、真实 U+2028、半帧 EOF 和超长帧。
- `tests/rpc_wire.rs` 启动真实 `onemore --rpc` 子进程和本地 SSE provider，覆盖 prompt、
  stdout 纯协议、最终 snapshot/settled 与 shutdown。

当前完整验证已通过：

```text
cargo test --locked
  192 unit + 1 RPC subprocess + 8 provider wire tests passed

cargo clippy --locked --all-targets -- -D warnings
cargo fmt --all --check
$env:RUSTDOCFLAGS='-D warnings'; cargo doc --locked --no-deps
```

设计清单仍明确保留的后续覆盖：

1. 慢 reader 的确定性背压测试。
2. RPC 层 allow-session/deny/断连审批矩阵。
3. 子进程层补齐 steer、abort、compact、model 和 session 命令矩阵。

这些是测试覆盖扩展，不需要新增生产执行路径。当前提交前只需检查最终 diff 和工作区状态；
不要创建 commit，除非用户明确要求。

RPC v1 之后再评估轻量插件协议。第一版插件只应覆盖已有真实扩展点：tools、context、hooks、
model registry，不提前引入包管理或另一条 agent 执行路径。
