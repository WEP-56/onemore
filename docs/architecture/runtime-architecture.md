# Runtime 结构与弱 Harness 边界

Onemore 的核心 loop 只协调模型消息、Provider、工具回合和输入队列检查点，由宿主选择
storage、permissions、skills、hooks 与 UI。CLI 和嵌入式 `Agent` 都运行公开的同一条
core loop；stateful runtime 是默认 harness，不是另一套循环。

## 目录职责

```text
src/agent_loop.rs
  公开 run_agent_loop；输入固定为 model / messages / tools / callbacks
src/agent_loop/
  model_call.rs      Provider terminal、流事件转发与“未开播才重试”
  tests.rs           不构造 Agent/Config/SessionBackend 的 core 直接测试
src/compaction.rs
  自动压缩设置、阈值判断与不拆工具配对的确定性切分
src/harness.rs
  ModelRegistry / SessionBackend / ModelPreferences 三个宿主接口
src/harness/
  model.rs           固定单模型 registry
  memory.rs          无文件系统的 session/preferences 实现
src/sdk.rs + src/sdk/
  公开 SessionController、SessionEvents、snapshot/event view 和错误码
src/rpc.rs + src/rpc/
  严格 JSONL v3 framing、wire DTO 和 SessionController adapter
src/runtime.rs
  公开 Agent，并承载唯一 stateful 线程宿主
src/runtime/
  agent_loop.rs       AgentLoopHost adapter：facts、预算、planning、队列、原子提交
  builder.rs          CLI 默认装配和宿主组件注入
  commands.rs         命令分发、模型切换、session 管理与通用事实提交
  compaction.rs       手动/自动共用的纯文本摘要调用与原子 Compaction 提交
  inbox.rs            command ID、一次性 admission ack 与 direct harness 适配
  session_runtime.rs  有界 command/event worker、phase、终态和 settled
  session_events.rs   AgentEvent 到稳定 SDK event/snapshot 的清洗投影
  tool_execution.rs   默认 ToolExecutor：schema、hooks、permissions、并发、取消与超时
  tests.rs            跨域测试夹具
  tests/              按 builder / compaction / permissions / planning / history / queues / concurrency 分组
```

拆分按行为所有权进行，不按任意行数切片。核心生产文件保持在约 160 到 540 行；测试也按
同一职责边界组织，避免重新形成单个巨型模块。

## Core Loop 边界

`run_agent_loop(model, messages, tools, callbacks)` 不引用 `Config`、`SessionBackend`、
SQLite、skills discovery、planning、permissions、hooks、workspace 或 TUI。callback bundle
包含取消标志、事件 sink、轮数与 retry policy；`AgentLoopHost` 提供以下行为：

- 把 messages 变换成 `PromptContext`，可在宿主侧做 system prompt 和预算。
- 执行并提交完整工具回合，成功后返回新的 model-visible messages。
- 在 terminal assistant 提交前返回可选 continuation；默认 harness 在这里实现 stop hook
  和 planning reminder，core 只消费“继续/不继续”的结果。
- 在完整工具批后轮询 steering，在任务将停止时轮询 follow-up。
- 取消时执行宿主清理；默认 harness 在这里修复 in-progress plan。

core 只有在 callback 成功返回后才替换自己的 messages，因此提交失败不会推进 core
transcript。工具 callback 必须闭合整批 ToolUse/ToolResult；默认 adapter 仍在同一事务中
提交 assistant ToolUse、plan effects 和全部 ToolResult。

## 可替换组件

`AgentBuilder` 当前允许宿主替换或追加：

- `ProviderFactory`：启动和后续 provider/model 切换共用同一个 factory。
- `CompactionSettings`：配置或关闭请求前自动压缩，手动 `/compact` 不受开关影响。
- `ModelRegistry`：CLI 配置、固定单模型或宿主动态模型目录使用同一解析接口。
- `ToolRegistry`：可以使用空 registry 或完全由宿主提供工具。
- `ContextProvider` 列表：可完整替换默认 instructions/project instructions/skills/workspace，
  也可追加片段。
- `HookRegistry`、`PermissionManager` 与 `RetryPolicy`。
- `SessionBackend` 与 `ModelPreferences`：默认分别是 SQLite 和 workspace JSON，也可替换。
- skills：可使用本地发现、宿主提供的冻结 `SkillCatalog`，或完全关闭。
- `in_memory()`：使用内存 facts/preferences 并关闭 skills，不解析或创建 Onemore 数据目录。
- data directory：使用默认持久化组件时可覆盖平台目录。

没有注入的组件继续采用 CLI 默认值，因此 `Agent::new` 与
`Agent::new_with_data_dir` 的行为保持不变。

## 稳定前缀

Builder 在一次 Agent 构造时冻结 context provider 顺序、workspace 根目录 `AGENTS.md`、
skill catalog 与 tool registry；默认 system section 顺序是 instructions、project
instructions、skills、environment，把每个 workspace 不同的环境信息放在尾部。工具声明在
请求前按名称排序。动态计划、工具结果、notice 和 compaction 都进入事实/消息层，不修改
稳定 system prefix。Provider 再根据规范化后的 system、tools 与模型选择生成稳定
`prompt_cache_key`。

宿主若替换 context 或 tools，应在一个 capability epoch 内保持其稳定；默认 harness 通过
`/reload` 显式重建配置、context、skills、tools 与 Web binding，而不是在每轮请求前重扫。

## Stateful Harness

`Agent` 已不再依赖 SQLite、偏好文件或 `SkillCatalog`；skills 和项目指令只存在于默认
context/tool 装配和一次性 startup discovery 中。内存宿主运行同一条生产 loop 时不会创建
任何 Onemore 状态目录，但默认 context 仍可读取 workspace 根目录的 `AGENTS.md`；显式替换
context providers 可整体关闭该行为。`Agent::builder_from_provider` 也可以直接接受已经解析的
`ProviderSettings`，完全绕过文件 `Config`；多模型宿主可实现 `ModelRegistry`。

provider/model 切换、planning reminder、compaction command 和 session commands 有意保留
在 stateful `Agent` 外壳内。它们可通过不装配 `Agent` 而整体省略，也不会出现在 core 的
输入类型、prompt 或 tool schema 中。

默认 adapter 从 facts 投影 model messages，在每次 callback commit 后重新投影并携带真实
usage baseline；model change、compaction 和损坏历史修复的语义仍由 session/harness 层拥有。
请求前达到阈值时，adapter 调用与手动 `/compact` 相同的摘要和提交函数；成功后以摘要、
安全切分的 retained tail 和后续 facts 重新投影，失败或取消不会追加 Compaction 事实。
`Agent::new(...)`、`Agent::builder_from_provider(...).in_memory()` 与 CLI 线程宿主最终都从
`Agent::run_turn` 调用公开 `run_agent_loop`，不存在兼容用的第二套 loop。

## SessionController 与前端

`sdk::spawn_session(agent)` 是 TUI、`--once`、Rust 嵌入和 JSONL RPC 的唯一 stateful
生产入口。命令通道和事件通道都有固定容量；mutation 携带 Runtime 生成的 `command_id` 和
一次性 admission ack。成功 receipt 只表示已接纳，完成状态由 `CommandFinished` 报告；
Runtime 发布最终权威 snapshot 后才发送一次 `Settled`。`wait_until_settled` 使用共享状态和
Condvar，不会与前端争抢事件流。

活动运行中，`prompt` 在 runtime checkpoint 明确返回 `busy`；`steer` 与 `follow_up` 分别进入
可观察的队列并保留 command ID。排队输入在提交为用户事实后从 snapshot 队列移除，取消时每项
都会得到 `cancelled` 终态。审批继续走独立响应通道，并在共享状态中原子占用 request ID，
重复或过期响应不会进入工具执行器。

SDK view 只从 committed facts 和 runtime live state 单向投影。它不序列化 `AgentCommand`、
`AgentEvent` 或 `SessionEntryPayload`，也不暴露 provider raw reasoning、原始工具参数、任意
tool details、system/compaction prompt 或存储位置。RPC 只是这个 SDK 的严格 JSONL adapter，
没有第二套命令执行实现。
