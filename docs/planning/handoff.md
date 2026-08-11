# Onemore 开发接力

更新日期：2026-08-10

## 当前目标

当前有两个开发目标：

1. P0：修复 TUI 中 `Esc` 不能及时取消当前步骤的问题。
2. P1：浏览器控制，范围、实现顺序和验收标准见[当前开发目标](next-phase-goals.md)。

现有 Web 搜索能力按当前契约维护，不再借浏览器控制扩展其他 Web 协议或抓取能力。

TUI 交互和渲染优先参考同技术栈实现 `example/tui`，但接入时仍以 Onemore 的
`SessionController`、`SessionEvents` 和权威 snapshot 为准，不能引入第二套运行状态。

## P0：`Esc` 及时取消

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

开始实现前建议依次阅读：

1. [当前开发目标](next-phase-goals.md)
2. [Runtime 结构与弱 Harness 边界](../architecture/runtime-architecture.md)
3. [Workspace 与 Web 工具](../architecture/workspace-and-web-tools.md)
4. `src/tui/mod.rs`、`src/sdk/controller.rs`、`src/runtime/session_runtime.rs`
5. `src/provider/mod.rs`、`src/provider/sse.rs`、两个 provider adapter
6. `src/runtime/tool_execution.rs`、`src/tools/run_command.rs`、`src/web/http_client.rs`
7. `src/runtime/tests/session_runtime.rs`、`src/runtime/tests/concurrency.rs`
8. `example/tui`

代码变更至少执行：

```powershell
cargo fmt --all -- --check
cargo check --workspace
cargo test --workspace
git diff --check
```
