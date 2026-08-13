# Onemore 阶段交付记录

更新日期：2026-08-12

## 状态

本阶段工作已完成，暂无待办事项。

## 已完成

### RPC v3 与 SDK

- 对齐 JSONL RPC v3、SDK view 和 runtime 事件投影。
- 补齐全部公开 `ProgressEvent` 的序列化与 GUI reducer 处理。
- 审批投影包含 `command`、`cwd`、`targets`。
- 失败工具正文统一由公开 `output` 携带，不暴露内部 `details` 或额外 transcript error 字段。
- 队列输入和 session load 保持 snapshot-only 语义。
- MCP 工具继续通过通用 tool、approval 和 notice 路径呈现，不扩张 RPC 协议。

### 桌面 GUI

- Tauri 后端按 `connection_id` 管理独立 RPC 子进程和请求状态。
- 切换工作区或会话不再终止后台 loop；退出应用时统一关闭全部受管进程。
- 前端按连接缓存独立 reducer 状态，后台事件不会覆盖当前任务。
- 历史会话和刚创建但尚未落盘的任务均可从侧栏切换。
- 侧栏展示运行、等待审批、失败和完成状态。
- GUI 示例已完整消费 RPC v3 progress，并以 snapshot 做权威校正。

### 可视化配置

- 配置编辑器已对齐最新 `config.example.toml`。
- 覆盖 agent、retry、compaction、permissions、providers 和 models。
- 覆盖 `[web]`、`[web.location]`、`[web.backends.*]`。
- 覆盖 `[[mcp_servers]]` 的命令、参数、环境变量、工作目录、启用状态、超时、审批和工具过滤。
- `api_key` 与 `api_key_env` 在 UI 和写入层保持互斥。
- 使用 `toml_edit` 增量更新，保留注释和未管理字段。
- 配置保存仅影响之后新建或重启的 RPC 任务。

### 文档

- 已同步更新 `docs/protocol/rpc-sdk-design.md`。
- 已同步更新 `docs/architecture/runtime-architecture.md`。
- 已更新 GUI README、RPC 集成示例和功能清单。

## 验证结果

- `cargo fmt --all -- --check` 通过。
- `cargo check --workspace` 通过。
- `cargo test --workspace --no-fail-fast` 通过：315 passed，1 ignored。
- GUI Tauri 测试通过：5 passed。
- GUI `npm run build` 通过。
- `git diff --check` 通过。

忽略项是依赖外部 Playwright MCP 环境的端到端测试，不影响本阶段交付。

## 发布产物

- CLI/npm：`onemore-agent` 版本 `0.8.0`，仅包含 Windows x64 二进制。
  - `dist/npm/onemore-agent-0.8.0.tgz`
- 桌面端：`OnemoreGui` 版本 `0.2.0`，Windows x64 NSIS Setup。
  - `Gui-rpc-example/onemoreGui/src-tauri/target/release/bundle/nsis/OnemoreGui_0.2.0_x64-setup.exe`

## 保持约束

1. RPC 协议版本保持 v3，不增加 MCP 专属 DTO、事件或命令。
2. Snapshot 是 GUI 的权威状态，progress 仅用于实时增量展示。
3. ToolUse 必须配对 ToolResult，持久化提交保持原子性。
4. 审批与断连继续 fail closed。
5. 工具输出保持有界，内部结构化 `details` 不进入公开 RPC 正文。
6. `config.example.toml` 与 `Config::EXAMPLE_CONFIG` 保持一致。
