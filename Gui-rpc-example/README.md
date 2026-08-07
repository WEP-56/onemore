# Onemore RPC GUI 示例项目

> 开发状态（对齐 §11 TODO）：P0/P1 完成；P2 完成；P3 完成；P4 完成（大量 delta
> fixture 除外）；P5 部分完成。前端 `npm run build` / `npm test`（Vitest reducer
> fixtures）、`cargo test`、Clippy、fmt 均通过；完整 quick demo / 长任务实机流程由用户
> 运行验证。

## 1. 项目目标

本目录用于实现一个可运行的 Tauri 桌面程序，演示第三方 GUI 如何通过 Onemore JSONL RPC
驱动同一条生产 Agent 路径。它不是新的 Agent 实现，也不复制 TUI、Provider、工具、审批或
会话逻辑。

首版同时提供两个工作流：

1. **快速示范**：连接 Onemore、发送一个 prompt、显示流式回答、工具进度、审批请求和最终
   snapshot，让读者在几分钟内看懂 RPC 集成。
2. **长任务测试**：运行较长的真实 coding task，观察事件吞吐、队列、token 用量和 revision，
   并能在运行中发送 steering/follow-up 或执行 abort，用于验证 GUI 在长时间运行和大量事件下
   仍保持响应。

协议规范以 [`../docs/rpc-sdk-design.md`](../docs/rpc-sdk-design.md) 为唯一依据。若本文示例与协议
规范冲突，以协议规范和 wire tests 为准。

## 2. 非目标

- 不通过 Tauri 直接调用 Onemore Rust crate；示例必须真实使用 `onemore --rpc` 子进程。
- 不实现 WebSocket、HTTP server、多 session server 或另一套消息协议。
- 不解析或修改 Onemore SQLite、配置文件、provider raw response 或工具内部 details。
- 不在前端执行任意 shell，也不提供绕过 Onemore permissions 的工具入口。
- 不实现营销页、账号系统、云同步、插件市场或复杂富文本编辑器。
- 不自动允许审批。窗口关闭、连接断开或请求失效时一律 fail closed。

## 3. 建议技术栈

- Tauri 2
- Rust stable，负责子进程、stdin/stdout/stderr、生命周期和 RPC framing
- React + TypeScript + Vite，负责界面和本地交互状态
- Zustand 或等价的小型 store；首版不引入 Redux
- Vitest + React Testing Library
- Rust 单元测试用于 JSONL reader、request correlation 和进程收尾
- Playwright 仅用于最终桌面前端的关键流程和截图验证

前端不得直接持有 `Child`、pipe 或操作系统路径能力。Tauri Rust backend 是 RPC transport 的
唯一所有者，通过 typed commands/events 向 WebView 暴露经过校验的 DTO。

## 4. 启动与进程所有权

### 4.1 启动参数

开发模式允许用户在设置中选择 Onemore 可执行文件，默认从 `PATH` 查找：

```text
onemore --rpc --config <config-path>
```

子进程 `current_dir` 必须设置为用户选择的 workspace。不能通过 shell 拼接命令；使用
`std::process::Command` 分别传递 program 和 args。Windows 首版以 npm 全局安装后的
`onemore.cmd`/可解析命令或明确的 `onemore.exe` 路径为主。

### 4.2 生命周期

Tauri backend 必须：

1. 启动子进程并同时接管 stdin、stdout、stderr。
2. 第一时间发送 `{"type":"hello","version":1}`。
3. 只把 stdout 当作 UTF-8 JSONL 协议；stderr 单独进入诊断面板和滚动日志。
4. 为每个 request 生成唯一 ID，并用 pending map 关联乱序 response。
5. 窗口关闭或 transport 出错时先停止接收新请求，关闭 stdin，并等待安全收尾。
6. 超时仍未退出时才终止子进程；不得让 Onemore 在 GUI 退出后继续执行工具。
7. 一个 app window 只拥有一个 RPC 子进程和一个 stdin writer task。

前端刷新不应隐式生成第二个子进程。开发环境发生 WebView hot reload 时，backend 继续拥有原
transport，并向新前端发布最新 snapshot。

## 5. RPC 客户端状态机

```text
disconnected
  -> spawning
  -> handshaking
  -> idle
  -> running | compacting | waiting_approval
  -> idle
  -> shutting_down
  -> disconnected

任意状态 -> error -> shutting_down/disconnected
```

最低限度的 backend API：

```rust
#[tauri::command]
async fn rpc_start(options: StartOptions) -> Result<ServerInfo, GuiError>;

#[tauri::command]
async fn rpc_request(request: GuiRequest) -> Result<GuiResponse, GuiError>;

#[tauri::command]
async fn rpc_stop() -> Result<(), GuiError>;

#[tauri::command]
async fn rpc_diagnostics_tail(limit: usize) -> Result<Vec<String>, GuiError>;
```

backend 向前端发送单一事件流，例如 `onemore://rpc-event`。payload 使用和协议一致的稳定 view，
但 transport error、进程退出码和 stderr 诊断使用 GUI 自己的封闭 DTO，不能伪装成
`SessionEvent`。

### 5.1 客户端事实规则

- `response.ok=true` 只表示 request 已成功处理；mutation 的业务完成状态看
  `command_finished`。
- `command_id` 关联 mutation 与后续终态，request `id` 只关联一次 response。
- `session_snapshot` 是权威值。progress 可以即时渲染，但 snapshot 到达时必须纠正本地组装状态。
- `settled` 只用于表示当前 session 已回到稳定边界，不能代替具体 command terminal。
- GUI 不缓存 provider raw、thinking raw、工具原始参数或任意 details。
- 未知 version、tag、必填字段缺失和重复 response 都进入明确 transport error，不静默忽略。

## 6. 应暴露和显示的数据

### 6.1 顶部状态栏

- 连接状态和 Onemore server ID 短值
- workspace
- 当前 provider / model / effort label
- session phase
-累计 input/output/cache token
- 当前 snapshot revision

### 6.2 Transcript

- user message
- assistant text/thinking 展示块
- tool name、human-readable summary、状态和清洗后的 output
- notice level/text

不得显示或存储：provider raw payload、thinking raw、工具原始 input JSON、tool details、API key、
system prompt、compaction prompt、SQLite 路径。

### 6.3 Live run

- 当前 command ID 短值和 elapsed time
- streaming text/thinking
- tool started/updated/finished
- steering/follow-up queues
- pending approval summary/reason/scopes
-最近一次错误的稳定 code/message

## 7. 界面结构

这是工作型桌面工具，不制作 landing page。首屏直接进入可操作界面：

```text
+-----------------------------------------------------------------------+
| Onemore RPC | workspace | model / effort | phase | usage | connect    |
+----------------------+------------------------------------------------+
| Sessions / modes     | Transcript                                     |
|                      |                                                |
| Quick Demo           | user / assistant / tools / notices             |
| Long Task            |                                                |
| Diagnostics          |                                                |
|                      +------------------------------------------------+
|                      | prompt input                      send / abort   |
+----------------------+------------------------------------------------+
| Queue / approval / current command / revision / elapsed                |
+-----------------------------------------------------------------------+
```

交互约束：

- `Quick Demo` 和 `Long Task` 使用 tabs 或左侧模式选择，不用两套页面和两套 transport。
- 连接、发送、abort 等明确命令使用 icon + text；重复工具操作使用 icon button 和 tooltip。
- 运行中主输入默认发送 `steer`；`follow_up` 通过明确菜单或独立队列按钮提交。
- 审批使用阻塞式 modal，展示 tool、summary、reason 和 Once/Session/Deny 三种明确选择。
- transcript 是主视觉区域，不使用卡片套卡片；工具调用作为紧凑的重复行展示。
- Diagnostics 使用独立 tab，stderr 与 protocol error 分栏，默认隐藏完整日志。
- 所有长 ID 默认显示前 8-12 位，支持复制完整值。

## 8. 快速示范模式

### 8.1 默认流程

1. 选择 workspace、config 和 Onemore executable。
2. Connect，完成 hello 并显示初始 snapshot。
3. 使用预置 prompt 或输入自定义 prompt。
4. 展示 prompt response、progress、command terminal 和 settled 的关联关系。
5. 运行结束后自动调用 `get_snapshot` 做一次权威校正。

建议预置 prompt：

```text
请只读检查当前 workspace：概括项目用途，指出三个关键模块，并说明你会先运行哪项验证。
不要修改文件。
```

该 prompt 默认不产生写操作，适合作为首次连接演示。若模型请求需要审批的工具，仍按正常审批
流程展示，不做特判。

### 8.2 最小成功标准

- 用户能看见 hello/server info 和 idle snapshot。
- prompt response 的 command ID 能与 `run_started`、`command_finished` 对应。
- streaming delta 不重复；最终 snapshot 能恢复完整 transcript。
- shutdown response flush 后进程退出，GUI 回到 disconnected。

## 9. 长任务测试模式

长任务模式不是另一种协议，只是在同一客户端上增加可观察性和测试控制。

### 9.1 配置

- 可编辑长任务 prompt
- 最大观察时长，仅限制 GUI 测试，不改变 Onemore model/tool timeout
- 是否在 settled 后自动请求 snapshot
- 可选的 steering/follow-up 定时发送脚本；默认关闭
- 事件日志导出路径，只保存公开 DTO 和 GUI 时间戳

建议预置 prompt：

```text
请调研当前项目并完成一个中等规模、可验证的改进。先建立计划，再实现、运行完整相关测试，
最后总结改动、验证结果和剩余风险。遵守项目 AGENTS.md，不要跳过审批。
```

### 9.2 运行中控制

- Send steering：在当前完整工具批后注入方向修正
- Queue follow-up：在当前任务将停止时追加工作
- Abort：立即请求取消，并等待每个 accepted command 的 terminal
- Approval：Allow Once / Allow Session / Deny
- Snapshot now：发送 `get_snapshot`，不修改运行状态

### 9.3 观测指标

- 总 elapsed、idle/running/waiting approval 各阶段时长
- 收到的 session/progress/terminal/settled 数量
- assistant delta 字符数
- tool started/finished 数和未闭合 tool call 数；正常结束时必须为 0
- accepted command 数与 terminal 数；正常结束时必须相等
- 最大 queue 长度、最后 revision、token usage
- transport write/read error、invalid frame、stderr 尾部

首版指标只保存在内存，允许导出一个 JSON 报告；不引入数据库。

## 10. 目录规划

```text
Gui-rpc-example/
  README.md
  package.json
  vite.config.ts
  src/
    main.tsx
    app/
      App.tsx
      store.ts
      types.ts
      util.ts
    components/
      TopBar.tsx
      ConnectPanel.tsx
      ModePanel.tsx
      Transcript.tsx
      Composer.tsx
      RunMonitor.tsx
      ApprovalDialog.tsx
      Diagnostics.tsx
      CopyId.tsx
    rpc/
      client.ts
      reducer.ts
      protocol.ts
    styles/
      app.css
  src-tauri/
    Cargo.toml
    tauri.conf.json
    capabilities/
      default.json
    src/
      main.rs
      rpc/
        mod.rs
        process.rs
        reader.rs
        writer.rs
        types.rs
        events.rs
      state.rs
      error.rs
  tests/
    fixtures/
    quick-demo.spec.ts
    long-task.spec.ts
```

按职责拆分，但不要为了目录形式创建空抽象。RPC process、reader、writer 可以共享一个 state，
不能各自启动子进程或拥有第二份 pending request map。

## 11. 施工 TODO

### P0：脚手架

- [x] 使用官方 Tauri 2 + React + TypeScript 模板建立项目。
- [x] 固定 Node、Rust、Tauri 版本并提交 lockfiles。
- [x] 建立最小 capabilities，仅开放实际需要的 Tauri commands/events。
- [x] 配置 Windows 开发和构建命令。

### P1：Rust RPC transport

- [x] 实现不经 shell 的 Onemore 子进程启动和路径校验。
- [x] 实现 LF JSONL 增量 reader、最大帧限制和严格 DTO。
- [x] 实现单 writer task、唯一 request ID 和 pending response map。
- [x] 完成 hello/version，拒绝不兼容版本。
- [x] stderr 独立读取并有界保留最近日志。
- [x] 实现 stdin EOF、broken pipe、窗口关闭和超时后的安全 shutdown。

### P2：前端状态投影

- [x] 为 server/snapshot/event/response 建立 TypeScript discriminated unions。
- [x] 实现以 snapshot 为权威、progress 为瞬时增量的 reducer。
- [x] 保证 delta、tool、terminal 和 settled 重复/乱序输入不会破坏 UI。
- [x] 实现连接、phase、queue、approval 和 diagnostics 状态。

### P3：快速示范

- [x] 完成连接表单、顶部状态栏、transcript 和 composer。
- [x] 完成 prompt、get_snapshot、model list/set。
- [x] 完成 session list/load 和 clear_conversation。
- [x] 完成工具进度和审批 modal。
- [x] 提供只读预置 prompt 和一键清空本地 UI 日志。

### P4：长任务测试

- [x] 完成长任务 preset、elapsed time 和事件统计。
- [x] 完成 steer、follow_up、abort 和 snapshot-now 控制。
- [x] 检查 accepted/terminal、tool started/finished 和 revision 不变量。
- [x] 实现公开 DTO 的 JSON 测试报告导出。
- [ ] 用大量 delta 和慢渲染 fixture 验证界面不冻结、内存有界。

### P5：测试与交付

- [x] Rust 单元测试覆盖半帧 EOF、超长帧、无效 UTF-8、malformed JSON、干净 EOF。
- [x] Rust 单元测试覆盖重复 request ID、response 关联和 fail-all-pending。
- [ ] Rust 单元测试覆盖 broken pipe（writer write error 路径）。
- [x] reducer fixture 覆盖 response/event 交错、最终 snapshot 校正和重复 terminal（Vitest，`npm test`）。
- [ ] 使用真实 `onemore --rpc` 跑 quick demo 集成测试（冒烟已验证 hello/get_snapshot，完整流程由用户运行）。
- [ ] 使用可控 mock RPC sidecar 跑确定性的长任务/审批/abort 测试。
- [ ] Playwright 验证桌面和窄窗口布局，无文本重叠或空白主视图。
- [x] 运行前端 typecheck/build、`npm test`、`cargo test`、Clippy 和 fmt。

## 12. 验收标准

项目完成必须同时满足：

1. Quick Demo 能从真实 Onemore hello 运行到 prompt terminal/settled，并在 snapshot 中看到回答。
2. Long Task 运行期间 GUI 始终可操作，可提交 steer/follow-up、处理审批并 abort。
3. 每个 accepted mutation 恰好显示一个 terminal；settled 不早于相关 terminal。
4. snapshot 可在任意 progress 交错后完整纠正 transcript、queue、phase、usage 和 approval。
5. GUI 退出后不存在遗留 Onemore 子进程；断连审批 fail closed。
6. stdout 只按 JSONL 解析，stderr 不污染协议；未知协议对象明确报错。
7. GUI 不暴露规范禁止的数据，也不提供绕过 ToolRegistry/permissions 的执行入口。
8. 全部自动化检查通过，并有 Windows 实机 quick demo 截图和长任务报告样例。

## 13. 建议给 Onemore 的首个施工指令

```text
阅读 Gui-rpc-example/README.md 和 docs/rpc-sdk-design.md。先完成 P0 与 P1：建立 Tauri 2 +
React/TypeScript 项目，实现由 Tauri Rust backend 独占的 onemore --rpc 子进程、严格 hello、
单 writer、增量 stdout JSONL reader、独立 stderr、request correlation 和安全 shutdown。
不要先做复杂 UI，不要复制 Agent 行为。完成后运行 Rust/前端测试、Clippy、fmt，并更新本文 TODO。
```
