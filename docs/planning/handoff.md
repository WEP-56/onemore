# Onemore 开发接力

更新日期：2026-08-11

## 当前目标

本轮处理四个紧密相关的目标：

1. P0：按最新 SDK 实现修补 JSONL RPC v3 契约、投影与回归测试。
2. P0：同步补齐 `Gui-rpc-example` 对完整 RPC v3 的接收、reducer 和展示能力。
3. P0：桌面端按受管任务维护独立 RPC 连接，切换工作区/会话不终止后台 loop。
4. P0：按最新 `config.example.toml` 补齐 Web 与 MCP servers 可视化配置。

`docs/protocol/rpc-sdk-design.md` 与 `docs/architecture/runtime-architecture.md` 中现有的
2026-08-11 改动是本轮对账后的目标规格，必须保留，不能回退。

边界很窄的 stdio MCP 客户端已经接入并完成测试。本轮不继续扩张 MCP：不增加 MCP 专属 RPC
DTO、capability、命令或持久化；远端工具继续通过既有 tool progress/transcript、审批、notice
和清洗后的 `content` 呈现。

`Esc` 及时取消和浏览器控制仍是后续事项，但明确不在下一会话实现。浏览器范围继续见
[当前开发目标](next-phase-goals.md)。

## P0：RPC v3 与 GUI 示例同步

### 已完成的文档对账

- `ProgressEvent` 文档原先只列出 14 个事件，现已按代码补齐遗漏的 10 个：
  `user_message`、`assistant_finished`、`tool_call_pending`、`error`、`plan_updated`、
  `skills_discovered`、`usage`、`conversation_cleared`、`model_selection_changed`、
  `sessions_listed`。排队输入和 session 载入不发专用 progress，只反映在 snapshot。
- `ApprovalRequestView` 已补 `command`、`cwd`、`targets`，§7.4 wire 示例也已改为当前形状。
- §3 已按代码修正 SDK 签名：`list_models()` 返回 `Vec<ModelMetadata>`，补列
  `server_info()`，`shutdown()` 返回 `CommandReceipt`，审批接收 `ApprovalResponseView`，
  `SessionEvents::recv` 使用实际 channel error 类型。
- §5.3/§5.6 已统一工具输出语义：不暴露任意 `details` 原始 JSON；清洗限长后的
  `ToolOutput.model_text` 就是公开 `content`。transcript tool 没有独立 `error` 字段，失败正文
  由 `output` 携带。
- Runtime 架构图已补 `src/mcp`、`src/process`，更新 builder/commands 职责、
  `mcp_servers()` 注入点及 `/reload` 重建 MCP server epoch 的语义。

### 施工范围（已完成）

1. 以两份已修改文档为规格，对账 `src/sdk/view.rs`、`src/sdk/controller.rs`、
   `src/runtime/session_events.rs`、`src/rpc.rs` 与 `src/rpc/`；只修真实存在的 RPC v3 偏差。
2. 补强 wire tests，覆盖完整 progress 集合、审批结构化字段、失败工具 transcript/output、
   snapshot-only 的队列与 session load，以及 MCP 通过通用 tool/notice 路径呈现。
3. 同步 `Gui-rpc-example/onemoreGui/src/rpc/protocol.ts`、`reducer.ts`、app store 和相关组件。
   每个公开事件都要么明确更新临时状态，要么明确交给后续 snapshot 校正，不能静默漏掉。
4. `assistant_finished` 应校正流式正文；plan、skills、usage、clear、model selection 和 session list
   应与 snapshot 保持一致；审批界面展示 `command/cwd/targets`；失败工具从 `output` 读取正文。
5. 更新 `Gui-rpc-example/rpc-example.md`、README 或 feature list 中仍与 wire 不一致的示例。
6. Tauri 后端以 `connection_id` 管理多个 RPC handle，前端按连接缓存独立 reducer 状态并显示
   后台任务的运行、等待审批、失败和完成状态。
7. config DTO、`toml_edit` 增量更新器和可视化表单已覆盖 `[web]`、`[web.location]`、
   `[web.backends.*]` 与 `[[mcp_servers]]`；配置保存只影响之后新建或重启的 RPC 任务。

### 明确不做

- 不提升 RPC 版本，不增加 v3 必填字段，不把内部 `AgentCommand` 直接暴露为 wire DTO。
- 不为 `/reload`、`/mcp` 增加 RPC 命令或 MCP 运行时管理页；RPC 客户端需要新 epoch 时仍重启子进程。
- 不增加 MCP 专属事件、来源 DTO、结构化 capability 或 GUI 管理页面。
- 不顺带实现 `Esc` 取消、浏览器控制、Web runtime/protocol 扩展或 GUI 大规模视觉重构。
- 不重构 SDK/runtime 所有权边界；补丁以文档和当前实现对齐为止。

## 后续：`Esc` 及时取消

### 现象

TUI 运行中按 `Esc` 后会立即显示“取消中…”，但经常要等当前模型请求、流读取或工具步骤自然
返回后，任务才真正进入 cancelled 终态。长时间没有 SSE 事件或 I/O 阻塞时尤其明显。

### 已定位原因

- `src/tui/mod.rs` 的按键路径没有排队问题：`Esc` 直接调用
  `SessionController::cancel_now()`，共享 `AtomicBool` 会立即置位。
- Provider 使用同步阻塞的 `ureq`。`post_sse()` 的建连/发送和 `SseReader::next_event()` 内部的
  `BufRead::read_until()` 都不能被取消标志唤醒。
- OpenAI 与 Anthropic adapter 只在两次 SSE 事件之间检查取消；Provider read timeout 当前为
  300 秒。因此没有新事件时，`Esc` 只能等待阻塞读返回。
- 工具取消同样是协作式的。调度器会在约 25ms 内传播每个工具的取消标志，但
  `std::thread::scope` 必须等 worker 退出；不轮询标志或阻塞在不可中断 I/O 的工具仍会拖住终态。
- `run_command` 已约每 40ms 检查取消并终止进程树，不是主要问题。外部 Web HTTP 目前只在
  阻塞请求前后检查取消，也存在相同缺口。
- 当前取消测试使用会主动轮询 `AtomicBool` 的 mock provider/tool，所以只能证明信号传播正确，
  没有覆盖“建连阻塞”“首个 SSE 事件前阻塞”和“相邻事件之间阻塞”。

### 修复边界

- 需要让 Provider HTTP/SSE transport 本身可中断；仅缩短 read timeout、增加更多
  `cancel.load()` 或让 TUI 重复发送 Abort 都不能根治。
- 优先建立统一的可等待取消句柄，并让建连、请求发送和 stream read 能在取消到达时关闭底层
  请求。若引入异步 transport，应把运行时限制在 provider/HTTP 边界，避免把整个 Agent loop
  改写成第二套异步实现。
- 取消后不得继续发送 provider 增量、触发 retry 或提交半截 assistant；仍要发布一次
  `Aborted`、配对已有 ToolUse/ToolResult，并最终进入权威 settled snapshot。
- 工具层继续允许协作式取消，但所有内置阻塞 I/O 都必须提供可中断路径；无法安全中断的第三方
  in-process tool 需要明确契约或进程隔离，不能让 runtime 假装已经停止。

### 回归要求

- mock server 接受连接但不返回响应头时，`Esc` 后应在有界短时间内结束。
- 已建立 SSE、首个事件迟迟不到，以及两个事件之间停顿时，`Esc` 都应及时结束。
- 取消瞬间已有部分文本或半截工具参数时，不提交半截 assistant，也不执行不完整工具调用。
- `run_command`、Git/Web 请求、并行工具批和 compaction 分别覆盖执行中取消。
- TUI、`--once`、SDK 和 RPC 使用同一取消路径；测试同时断言 cancelled 终态、settled 顺序和
  不发生取消后重试。

## 必须保持的可靠性约束

1. Provider 每次调用必须终止于 `Done / Error / Aborted`；裸 EOF 或裸 `[DONE]` 不是成功。
2. 自动 retry 只允许发生在尚未产生任何流事件时，避免重复副作用或重复文本。
3. ToolUse 必须恰好配对一个 ToolResult；半截参数不得执行。
4. assistant ToolUse、harness effects 与全部 ToolResult 必须原子提交。
5. session append 失败时，内存 facts 不得推进。
6. 权限顺序保持 `prepare/schema -> hard deny -> pre-hook -> re-prepare -> permission`。
7. 高危审批边界不能被 config、prompt、skill 或模型参数关闭。
8. 并发工具 UI 事件可按完成顺序，持久化 ToolResult 必须按源顺序。
9. provider、model 或 Web binding 改变后，不得复用不匹配的 usage baseline 或 prompt identity。
10. hosted Web tool 不进入本地 registry；runtime error 不得触发 session 内 backend fallback。
11. 工具输出必须有界；`details` 用于结构化数据，不能成为绕过模型上下文限制的第二正文。
12. `config.example.toml` 必须与 `Config::EXAMPLE_CONFIG` 完全一致。

## 工作入口

下一会话建议依次阅读：

1. [Rust SDK 与 JSONL RPC v3](../protocol/rpc-sdk-design.md)
2. [Runtime 结构与弱 Harness 边界](../architecture/runtime-architecture.md)
3. `src/sdk/view.rs`、`src/sdk/controller.rs`、`src/runtime/session_events.rs`
4. `src/rpc.rs`、`src/rpc/`、`src/rpc/tests.rs`
5. `Gui-rpc-example/onemoreGui/src/rpc/protocol.ts` 与 `reducer.ts`
6. `Gui-rpc-example/onemoreGui/src/app/store.ts` 和相关展示组件
7. `Gui-rpc-example/rpc-example.md`、`Gui-rpc-example/README.md`

代码变更至少执行：

```powershell
cargo fmt --all -- --check
cargo check --workspace
cargo test --workspace
Push-Location Gui-rpc-example/onemoreGui
npm run build
Pop-Location
git diff --check
```
