# Onemore 开发接力

更新日期：2026-08-10

## 当前目标

当前只保留浏览器控制这一项产品开发目标。范围、实现顺序和验收标准见
[当前开发目标](next-phase-goals.md)。现有 Web 搜索能力按当前契约维护，不再借浏览器控制扩展
其他 Web 协议或抓取能力。

TUI 交互和渲染优先参考同技术栈实现 `example/tui`，但接入时仍以 Onemore 的
`SessionController`、`SessionEvents` 和权威 snapshot 为准，不能引入第二套运行状态。

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
4. `src/event.rs`、`src/sdk/view.rs`、`src/runtime/session_events.rs`
5. `src/tui/mod.rs`、`src/tui/transcript.rs`
6. `example/tui`

代码变更至少执行：

```powershell
cargo fmt --all -- --check
cargo check --workspace
cargo test --workspace
git diff --check
```

