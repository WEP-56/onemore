# MCP 客户端接入实现计划

更新日期：2026-08-11

## 实施状态(2026-08-11)

四个阶段已全部落地并通过验收,详见 `src/mcp/` 与 `src/tools/mcp_proxy.rs`。
真实验收:`@playwright/mcp` 0.0.79 经 era 探测正确回退为 legacy 2025-06-18,
24 个工具全部导入,导航 + 快照工作流通过,关停无进程遗留(`cargo test --lib
mcp:: -- --ignored` 可复跑)。与原计划的两处实现偏差:

- schema 校验模式落在 `Tool` trait 的默认方法上(`schema_validation()`),而非
  `ToolSpec` 字段——对现有工具零改动,语义相同。
- 协议测试 fixture 采用 `std::io::pipe` 内存管道 + 脚本化 server 线程;真实进程
  生命周期(EOF 优雅退出、强杀、stderr 捕获)单独用 PowerShell 子进程覆盖。

以下为原计划正文,作为契约与 v2 议题的依据保留。

## 定位

Onemore 作为 MCP host/client 接入 stdio MCP server，目标是不重编译即可扩展工具能力，
首个实用场景是通过成熟 browser server（如 chrome-devtools-mcp、playwright-mcp）获得
浏览器控制。

权威规范是 [modelcontextprotocol.io 的版本化规范](https://modelcontextprotocol.io/specification/2026-07-28)，
当前修订为 2026-07-28（上一修订 2025-11-25）。本计划只采用其中自 2024-11-05 以来最
稳定的核心面：stdio transport 上的 tools 能力。规范中"重"的部分（HTTP transport 与
OAuth、resources、prompts、subscriptions）v1 全部不做；sampling、roots、logging 已被
2026-07-28 正式弃用，本项目永不实现。

设计基线：

- 不引入异步运行时。stdio MCP 是子进程标准流上的换行分隔 JSON-RPC 2.0，手写同步
  客户端（写端加锁 + 每 server 一个 reader 线程），不使用 rmcp/tokio。
- MCP server 是不受信的第三方插件进程。它提供的名称、描述、schema、annotations、
  serverInfo 与全部输出都按不受信输入处理。
- 未配置任何 `[[mcp_servers]]` 时，行为与当前版本完全一致：不 spawn 进程、不改
  工具声明、不改 prompt identity。

## 与现有目标的关系

- P0（Esc 取消）：无硬依赖。MCP 的阻塞读发生在 reader 线程，调用线程等待响应使用
  条件变量 + 有界超时片轮询取消标志，因此取消及时性不依赖可中断 HTTP transport；
  杀进程即可解除 reader 阻塞。但 MCP 等待路径必须满足与 P0 相同的验收：取消后有界
  时间内进入终态。
- P1（浏览器控制）：本计划落地后，先用现成 browser MCP server 跑真实工作流，再评估
  原生 P1 的必要范围。通用 MCP 客户端无法实现 P1 要求的 URL 白名单、元素引用失效
  语义与按交互类型分级审批，这些边界在工具内部；本计划不修改 next-phase-goals.md。

## 协议范围（v1）

### 采用

- **stdio transport**：换行分隔 JSON-RPC 2.0，UTF-8，单条消息不含内嵌换行。reader
  对单行长度设上限（8 MiB），超限视为协议故障并终止该 server。stderr 由专职线程
  读入有界环形缓冲（诊断用），永不进入模型上下文。
- **双时代（dual-era）客户端**，按规范的 stdio 探测机制检测 server 时代，结果按
  server 进程缓存：
  - 启动后先发 `server/discover`（`_meta` 携带 `io.modelcontextprotocol/protocolVersion`
    = "2026-07-28"、`clientInfo`、`clientCapabilities` = `{}`）。
  - 返回 `DiscoverResult` → modern，从 `supportedVersions` 选择版本；返回
    `UnsupportedProtocolVersionError`（-32022）→ modern，从其 `supported` 列表选择，
    不回退；其他任何错误或探测超时 → legacy，回退 `initialize` 握手。
  - modern：每个请求在 `_meta` 携带协议版本、`clientInfo` 与空 capabilities；结果的
    `resultType` 缺省按 `"complete"` 处理；`"input_required"`（MRTR）v1 不支持，
    映射为稳定工具错误。modern server 不会向 client 发起 JSON-RPC 请求。
  - legacy：`initialize`（请求版本 2025-06-18，接受 server 协商出的
    2024-11-05 / 2025-03-26 / 2025-06-18 / 2025-11-25）→ `notifications/initialized`
    → 正常请求。server→client 请求：`ping` 回空结果，其余一律回 -32601；自发通知
    （如 `notifications/tools/list_changed`）容忍并按下文处理。
- **tools/list**：`cursor` 分页循环取完；容忍并忽略 `ttlMs` / `cacheScope` / `icons`
  等未消费字段。
- **tools/call**：结果处理规则——
  - `content` 中 text 块按顺序以空行拼接为 `model_text`（registry 层现有
    `sanitize_and_bound` 继续兜底 24k 上限）。
  - image 块落盘到会话数据目录（单图 ≤ 8 MiB、单次调用 ≤ 4 张，超限丢弃），
    `model_text` 附一行文件路径；audio / resource_link / embedded resource v1 丢弃。
    所有被丢弃的块在 `details` 记录类型与大小。
  - `structuredContent` 放入 `details`（序列化 ≤ 64 KiB，超限丢弃并注明）。
  - `isError: true` → `ToolErrorCode::ExecutionFailed`，text 内容作为错误信息进入
    observation（规范要求把执行错误交给模型自纠）。
  - JSON-RPC 协议错误：-32602 → `InvalidArguments`，其余 → `ExecutionFailed`；
    错误对象有界后进入 observation。
- **notifications/cancelled**：取消标志置位后立即发送并停止等待，有界时间内返回
  `Aborted`；迟到的响应由 reader 按已放弃的 id 丢弃。工具调用超时（配置）同样发送
  cancelled 通知后返回 `Timeout`。
- **关停**：关 stdin → 有界等待退出 → 强制终止进程树（Windows 复用 run_command 的
  Job Objects 基建；规范明确认可该机制）。Agent 关闭与 `/reload` 都走此路径。

### 不做（v1）

- Streamable HTTP transport 与整个授权层（需要远程 server 时再议）。
- resources、prompts、completion、`subscriptions/listen`。
- sampling、roots、logging（规范已弃用，永不采用）。
- tasks 扩展、extensions 协商、MRTR 应答（`input_required` 报稳定错误）。
- `progressToken` / 进度订阅（v1 不请求，未知通知一律忽略）。
- 信任 `annotations`（`readOnlyHint` 等不得影响权限；规范自身也要求视为不受信）。
- `instructions` 注入 system prompt（违反稳定前缀与不受信文本边界；仅在 `/mcp`
  详情展示）。
- server 异常退出后的自动重启。规范建议 SHOULD restart 并重试丢失请求，此处刻意
  偏离：绝不自动重试可能已产生副作用的 tools/call；进行中的调用以稳定错误结束，
  恢复靠 `/reload`。"仅为后续调用重启进程"列为 v2 议题。

## 模块与代码落点

```text
src/mcp.rs + src/mcp/
  transport.rs   子进程 spawn、行框架读写、reader 线程与 id 路由、stderr 环形
                 缓冲、行上限、关停与强杀
  protocol.rs    请求/响应/通知 DTO、_meta 构造、错误码映射、两时代报文差异
  client.rs      era 探测与缓存、legacy 握手、tools/list 分页、tools/call、
                 cancelled 通知、每 server 单请求互斥
  import.rs      工具导入卫生：命名、清洗、上限、冲突拒绝、启动 notice 汇总
  tests.rs       脚本化 fixture 下的单元测试
src/tools/mcp_proxy.rs
  McpTool：实现 Tool，spec 来自导入结果，execute 转发 client 并处理取消/超时
src/config.rs
  [[mcp_servers]] 解析与校验；config.example.toml 同步（约束 12）
src/runtime/builder.rs
  装配 McpHost：并行启动全部 enabled server（各自启动超时），失败降级并汇总
  notice；导入工具追加进 ToolRegistry
src/runtime/commands.rs
  /reload：关停旧 McpHost → 重建 → registry generation 递增（现有机制使旧
  PreparedToolCall 失效）；新增 /mcp 状态命令
```

连接由 `McpHost` 持有，`McpTool` 经 `Arc` 共享对应 server 连接；Host 随 Agent 生命
周期关停。工具执行仍走现有 `prepare → 权限 → execute` 链路与 `ToolCall*` 事件，TUI
不新增第二条状态源。

需要的两处核心类型扩展（保持最小）：

1. `ToolSpec` 增加 schema 校验模式：内置工具维持现有子集严格校验；MCP 工具标记为
   server 权威——本地仅要求参数为 object，不用子集校验器误杀 2020-12 / draft-07 /
   `$ref` 构造（规范已放开到完整 JSON Schema 2020-12）。schema 原样透传给 provider。
2. `ToolPermissionSpec` 增加 session grant 粒度声明：现有键为"工具名 + 完整参数"，
   浏览器类工具每次参数都不同将导致审批疲劳；MCP 工具声明按工具名授权（Session 范围
   内首次审批后同名工具放行）。`always_ask` 高危边界语义不变（仍只有 Once）。

## 导入卫生与上限

- 工具改名为 `mcp__{server}__{tool}`；`{server}` 取 config 中的 server 名（须匹配
  `^[a-z0-9][a-z0-9_-]{0,31}$`），不使用 serverInfo（规范注明其不保证唯一、仅供
  展示）。完整名超 64 字符（Anthropic 工具名上限）→ 拒绝该工具。
- 原始工具名须匹配规范字符集（字母、数字、`_`、`-`、`.`，1–128 字符），`.` 在改名时
  替换为 `_`；不合规则拒绝。
- 与内置工具或先注册 server 的工具冲突 → 拒绝后到者。
- description 经 `sanitize` 清洗控制字符并截断至 1k；title 同理。声明文本进入工具
  声明即进入提示注入面，UI `details` 始终标注来源 server。
- schema 序列化 > 64 KiB → 拒绝该工具。
- 每 server 工具数上限默认 64（按名称排序后确定性截断）；config 可用
  `include_tools` / `exclude_tools`（精确名列表）先行过滤。
- 一切拒绝与截断都以启动 notice 逐条列出，不允许静默缺席。

## 配置

```toml
# [[mcp_servers]]
# name = "browser"                  # 必填，^[a-z0-9][a-z0-9_-]{0,31}$，用作工具前缀
# command = "cmd"                   # 必填，可执行文件，不经 shell 解析
# args = ["/c", "npx", "-y", "chrome-devtools-mcp@latest"]
# # Windows 上 npm 系 server 需经 cmd /c 启动；npx 首次运行会下载包，
# # 启动超时请留足或预装。
# # env = { KEY = "value" }         # 叠加在继承环境之上；环境中的敏感变量对 server 可见
# # cwd = "..."                     # 缺省继承 Onemore 工作目录
# # enabled = true
# # startup_timeout_ms = 30000     # 含 era 探测与 tools/list
# # call_timeout_ms = 60000
# # always_ask = true              # 只可收紧：该 server 全部工具逐次审批（Once）
# # include_tools = ["click", "navigate_page"]
# # exclude_tools = []
```

`config.example.toml` 与 `Config::EXAMPLE_CONFIG` 保持一致（约束 12）。

## 权限与安全

- MCP 工具 capabilities 固定为非只读、destructive、`Sequential`、不支持后台；不声明
  路径参数。按现有权限判定（`src/permission/mod.rs` 的"未声明目标的副作用"分支）
  落入 Ask，授权范围 Once / Session（Session 按工具名，见上文扩展）。
- 权限只可经 config 收紧（`always_ask`），不可放宽：annotations、server 声明或模型
  参数都不能把 MCP 工具降为只读或免审批（约束 7 的延伸）。
- 子进程环境 = 继承父环境 + config `env` 叠加；文档明示环境变量对 server 可见。
  Onemore 的 provider 密钥存于 config.toml，不经环境泄露。
- 输出边界：model_text / ui_summary 走现有 `sanitize_and_bound`（约束 11 自动成立）；
  details 中的结构化数据按上文各自限额。
- 违反协议的 server（stdout 噪声行、超长行、非法 JSON、响应未知 id）：噪声行计数
  容忍（有限次后判故障），超限与非法框架直接判故障；故障 server 的工具全部转为
  稳定错误。

## 生命周期与失败语义

- 启动：builder 内并行 spawn 全部 enabled server，各自 startup timeout；任何 server
  失败只影响自身（降级缺席 + notice），Agent 照常构建。
- 会话中死亡：该 server 全部工具返回稳定 `ToolError`（提示可 `/reload` 恢复）；不
  自动重启、不自动重试（呼应约束 10 的"不在 session 内做 backend fallback"）。
- registry 在一个 capability epoch 内冻结：v1 不开订阅，modern server 的列表变更
  不会到达；legacy server 自发的 `list_changed` 通知仅转为一条建议 `/reload` 的
  notice。`/reload` 是唯一 epoch 边界，prompt identity 随之改变（约束 9 由现有
  reload 语义承接）。
- 取消与关停如上文协议范围所述；工具调用无论成功、失败、超时、取消，恰好产生一个
  ToolResult（约束 3）。

## 实施顺序

每阶段合入前执行 `cargo fmt --all -- --check`、`cargo check --workspace`、
`cargo test --workspace`、`git diff --check`。

1. **transport + protocol**：子进程框架读写、reader 线程与 id 路由、stderr 缓冲、
   行上限、关停强杀；两时代 DTO 与 era 探测。测试用脚本化 fixture server（复用测试
   可执行文件的环境变量模式或独立小型 fixture 程序，可编排任意应答序列），覆盖：
   modern / legacy / 版本不匹配三种探测结局、探测超时回退、乱序响应路由、噪声行
   容忍与超限故障、调用中取消（cancelled 通知 + 有界返回）、调用超时、server 中途
   崩溃、关停回收（不遗留进程）。
2. **client + import + 集成**：tools/list 分页、tools/call 结果映射（text / image
   落盘 / isError / structuredContent / input_required）、导入卫生全套（命名、清洗、
   上限、冲突、notice）、`[[mcp_servers]]` 解析、builder 装配与降级、`/reload` 重建
   与 generation 失效、ToolSpec 校验模式与 NameOnly session grant 两处核心扩展及其
   权限测试。
3. **TUI 与可观察性**：`/mcp` 状态命令（era 与协商版本、工具数、最近错误、stderr
   尾部）、启动/拒绝/死亡 notice 接入现有事件链路、details 标注来源 server。
4. **真实 server 端到端**：`@modelcontextprotocol/server-everything` 做协议健全性
   检查（多内容类型、isError）；browser server（chrome-devtools-mcp 或
   playwright-mcp）跑完整工作流：导航 → 快照 → 点击/输入 → 截图落盘 → 关闭，全程
   经 Ask → Session 授权。沉淀 Windows 启动写法与推荐配置到文档。

## 验收标准

- 配置两个 server 且其一启动失败：Agent 正常可用，notice 说明缺席原因；未配置
  MCP 时无任何行为与 prompt identity 变化。
- modern 与 legacy fixture server 均可完成 list 与 call；版本全不兼容时给出列明
  双方版本的稳定错误。
- 浏览器工作流端到端通过；首次调用弹审批，Session 授权后同名工具不再重复弹。
- 运行中 Esc：有界时间内该调用以 `Aborted` 终态结束，cancelled 通知已发出，迟到
  响应被丢弃，ToolUse/ToolResult 配对完整，无取消后重试。
- 敌意 server（超长行、控制字符名称/描述、重名、非法 schema、巨型结果、stdout
  噪声）全部有界处理，无 panic、无上下文污染、无静默缺席。
- 会话中杀死 server 进程：后续调用稳定报错，`/reload` 后恢复；reload 前已 prepare
  的调用因 generation 失效得到 Conflict。
- 任务结束与 Agent 退出不遗留 server 进程（含 npx 派生的子进程树）。
- 全部现有测试与 12 条可靠性约束保持通过。

## v2 议题

按需求排序：仅为后续调用的进程自动重启；同 server 并发多路复用（JSON-RPC id 已
支持）；`progressToken` → `report_progress` 转发；resource / audio 内容块落盘；
Streamable HTTP transport 与授权；resources / prompts；tasks 扩展与 MRTR。

## 参考

- 规范总览与 changelog：`/specification/2026-07-28`、`/specification/2026-07-28/changelog`
- stdio binding 与回退探测：`/specification/2026-07-28/basic/transports/stdio`
- 版本协商与双时代兼容矩阵：`/specification/2026-07-28/basic/versioning`
- tools：`/specification/2026-07-28/server/tools`
