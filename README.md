# Onemore

## Design Documents

- [Runtime 结构与弱 Harness 边界](docs/runtime-architecture.md)
- [Rust SDK 与 JSONL RPC v1](docs/rpc-sdk-design.md)
- [提示词缓存设计](docs/prompt-cache.md)
- [API 兼容性与 Chat Completions 删除](docs/api-compatibility.md)
- [Reasoning effort 配置与 TUI 行为](docs/reason-effort.md)
- [CacheBoard 测试项目需求书](docs/cacheboard-test-project-cn.md)
- [Workspace and Web tools](docs/workspace-and-web-tools.md)

Onemore 是从 [Zerone](https://github.com/WEP-56/zerone) 教学基线迁移出的独立 coding agent 工程

## 运行

```powershell
cargo run
cargo run -- --once "你好"
cargo run -- --rpc
cargo run -- -p deepseek
```

首次运行会在平台数据目录生成 `config.toml`(Windows 默认 `%APPDATA%\onemore`). 也可以设置 `ONEMORE_HOME` 将配置和会话
放到独立目录:

```powershell
$env:ONEMORE_HOME = "D:\onemore-data"
cargo run
```

配置样例见 `config.example.toml`. 本地 `config.toml` 可能包含 API key,已被 Git 忽略。

## 作为库嵌入

`Agent::new` 保留 CLI 的完整默认装配；宿主也可以从 `Agent::builder` 只替换需要接管的
组件。Provider 使用持久 factory，而不是一次性的实例，因此 `/provider` 和 `/model`
切换后仍会走宿主实现。

```rust
use onemore::config::ProviderSettings;
use onemore::context::ContextProvider;
use onemore::provider::Provider;
use onemore::runtime::{Agent, RetryPolicy};
use onemore::tools::ToolRegistry;
use onemore::workspace::Workspace;

fn embedded_agent(
    settings: ProviderSettings,
    workspace: Workspace,
    provider_factory: impl Fn(ProviderSettings) -> Box<dyn Provider> + Send + Sync + 'static,
    tools: ToolRegistry,
    context: Vec<Box<dyn ContextProvider>>,
) -> anyhow::Result<Agent> {
    Agent::builder_from_provider(settings, workspace)
        .in_memory()
        .provider_factory(provider_factory)
        .tools(tools)
        .context_providers(context)
        .retry_policy(RetryPolicy::default())
        .build()
}
```

`in_memory()` 不创建 SQLite、workspace 偏好或 skills 目录，也不会把 `load_skill` 及其
提示词装进 Agent。宿主还可以分别注入 `ModelRegistry`、`SessionBackend`、
`ModelPreferences` 或冻结的 `SkillCatalog`。

需要完整 stateful harness 的嵌入方通过 `SessionController` 提交命令，并从有界
`SessionEvents` 流消费进度和权威 snapshot。成功返回 `CommandReceipt` 表示 Runtime 已接纳，
最终状态由 `CommandFinished` 报告，`Settled` 总是在相关终态之后：

```rust
use std::time::Duration;

use onemore::runtime::Agent;
use onemore::sdk::{spawn_session, SessionEvent, SessionPhase, SessionSnapshot};

fn run_prompt(agent: Agent) -> Result<SessionSnapshot, Box<dyn std::error::Error>> {
    let mut session = spawn_session(agent);
    let receipt = session.controller.prompt("检查当前项目")?;

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
    let _ = session.controller.shutdown();
    Ok(snapshot)
}
```

## JSONL RPC

`--rpc` 在 stdin/stdout 上提供 UTF-8 JSONL v1。第一帧必须是 hello；stdout 只包含协议帧，
诊断写 stderr。request response 可乱序返回，客户端应使用 request `id` 和 Runtime
`command_id` 关联结果与事件。

```powershell
@(
  '{"type":"hello","version":1}'
  '{"type":"request","id":"snapshot","request":{"command":"get_snapshot"}}'
  '{"type":"request","id":"shutdown","request":{"command":"shutdown"}}'
) | cargo run --quiet -- --rpc
```

帧限制、公开数据白名单、完整命令集和审批示例见
[Rust SDK 与 JSONL RPC v1](docs/rpc-sdk-design.md)。

不需要 Onemore stateful harness 的宿主可以直接调用同一条生产 core loop：

```rust
use std::sync::atomic::AtomicBool;

use onemore::agent_loop::{
    run_agent_loop, AgentLoopCallbacks, AgentLoopHost, AgentLoopOutcome,
};
use onemore::event::AgentEvent;
use onemore::message::ChatMessage;
use onemore::provider::Provider;
use onemore::tools::ToolSpec;

fn run_core(
    model: &dyn Provider,
    messages: Vec<ChatMessage>,
    tools: &[ToolSpec],
    host: &mut dyn AgentLoopHost,
) -> AgentLoopOutcome {
    let cancel = AtomicBool::new(false);
    let mut emit = |_event: AgentEvent| {};
    run_agent_loop(
        model,
        messages,
        tools,
        AgentLoopCallbacks::new(host, &mut emit, &cancel),
    )
}
```

`AgentLoopHost` callbacks 决定 prompt 变换、工具执行、提交、steering/follow-up 和收尾；
默认 `Agent` adapter 才理解 facts、planning reminder、permissions/hooks 与 session。
compaction 和 session commands 不属于 core loop。具体边界见
[Runtime 结构与弱 Harness 边界](docs/runtime-architecture.md)。

TUI 内常用操作:

| 操作 | 说明 |
|---|---|
| 运行中输入并回车 | steering:在当前一批工具全部完成并提交后注入,修正方向 |
| `/queue <内容>` | follow-up:排队后续任务,当前任务将停止时才注入，在agent运行中直接发送消息也可以“插嘴” |
| `/compact` | 调用模型生成摘要作为 Compaction 事实;模型视图缩小,事实日志不减少 |
| `Esc` | 取消当前轮(丢弃半截流式输出;未执行的工具调用补取消结果;清空排队输入) |
| `/session [ID\|all]` | 默认列出/恢复当前 workspace 会话；`all` 只做跨 workspace 发现，恢复时重建全部事实(含 UI-only 提示) |
| `/provider` | 只切换 provider，使用其默认模型，历史保留 |
| `/model` | 只列出当前 provider 的模型；选模型后再确认思考程度，可以在配置文件中自定义思考程度名称 |
| `/reasoning` (`/effort`) | 调整当前模型的思考程度 |
| `/clear` | 新建会话 |


## 与 Zerone 的区别

Zerone 是刻意压低复杂度的可运行基线;Onemore 在同一架构骨架上补齐了工程化契约。
未变的部分:统一消息模型 `ChatMessage/Block`、两种 API(Messages / Responses)适配边界、`AgentCommand/AgentEvent` 事件流与双前端
(TUI + `--once`)、工具必须经 `Workspace` 访问文件、一会话一 SQLite 库。

### 1. Provider:从"可能静默成功"到终止完备协议(阶段 1)

- Zerone:`stream_turn` 返回 `Result<Option<TurnOutput>>`,取消是 `Ok(None)`,
  EOF 后可能把半截流当正常回答。
- Onemore:每次调用必然终止于 `StreamTerminal::{Done, Error, Aborted}` 之一;
  EOF 而无终止事件一律是错误;失败路径也携带可消费的 final assistant
  (`FailedTurn`)。两个适配器都有 EOF 断流 wire 测试锁定该行为。
- 重试收敛为 `RetryPolicy` 纯函数:指数退避 + 确定性 jitter + 上限,
  解析 `retry-after-ms`/`retry-after`,服务器要求等待超过 60s 直接放弃;
  "只有未产生任何流事件的失败才重试"的幂等前提不变。等待和重新发起分别投影为
  `retry_scheduled` / `retry_started`，snapshot phase 同步切换 `retrying` / `running`。

### 2. 工具:从字符串到类型化管线(阶段 2)

- Zerone:`execute() -> Result<String, String>`,Registry 统一 24K 中间截断,
  模型正文、UI 展示、诊断混在一根字符串里。
- Onemore:`ToolOutput { model_text, ui_summary, details }` 与
  `ToolError { code, retryable, details }` 分离;稳定错误码(`not_found` /
  `conflict` / `timeout` / `permission_denied` …)供 UI、指标与策略消费,
  工具失败仍是模型可见 Observation。
- 参数走 `prepare_arguments(兼容转换) → JSON Schema 校验 → 执行` 管线,
  校验失败一定不会到达 execute;`length` 截断的 assistant 里所有工具调用
  一律不执行(截断参数可能"语法合法但语义不完整")。
- 工具可上报结构化进度(`ToolCallUpdated`);settle 之后的迟到进度被忽略。

### 3. 权限与 Hook:副作用有了安全门(阶段 3)

- Zerone:没有权限层,`Workspace` 允许任意绝对路径。
- Onemore:`PermissionManager` 按 `workspace_read / workspace_write /
  outside_workspace / commands` 四条规则(allow | ask | deny)评估**已校验参数**;
  设备路径等 hard deny 不可被任何配置或 Hook 覆盖;审批走独立通道,
  支持 Once / Session 两种作用域,等待审批期间可取消。
- 四个 Hook 扩展点(UserPromptSubmit / PreToolUse / PostToolUse / Stop)。
  hard deny 先于 Hook 运行;Hook 改写参数后会重新 preflight 并重新过权限,
  因此 Hook 无法绕过安全策略。

### 4. 会话与上下文:事实日志 ≠ 模型视图(阶段 4)

- Zerone:屏幕历史 = 运行历史 = 持久历史 = 模型上下文,四者是同一个
  `Vec<ChatMessage>` 全量发送;SQLite 只存最终模型消息。
- Onemore:持久层是 append-only 的事实日志(schema v4):
  `SessionEntry { id, parent_id, kind, payload }`,payload 分
  `Message(含该次真实 usage) / Notice / Compaction / ModelChange / Artifact /
  PlanUpdated / PlanReminder`。
  entry、链尾(leaf)与统计在同一事务提交;带 ToolUse 的消息批在提交边界
  被强制配对完整,提交失败则内存镜像不推进、本轮立即终止——内存与磁盘
  永不分叉。旧版线性库在打开时单事务自动迁移,失败回滚保留原库。
- 模型看到什么由**单向投影**决定:UI-only 事实不进 Provider;投影时对旧库
  损坏数据做防御性工具配对修复并发出诊断。`/session` 恢复的是完整事实。
- 上下文预算(配置 `context_window` 后启用):优先用最近一条 assistant 的
  真实 usage 作基线、只估算其后尾部。达到可配置阈值时自动复用 `/compact` 的唯一
  生产路径，摘要旧前缀并原样保留最近消息；切分不拆 ToolUse/ToolResult，事实日志只追加。
  自动压缩关闭时，硬预算仍会先折叠旧 ToolResult，仍超限则明确拒绝发请求。

### 5. 运行时:ActiveRun 与两个输入队列(阶段 5)

- Zerone:输入只能阻塞等待,turn 进行中的命令靠 mpsc 排队时机隐式决定。
- Onemore:一次运行是一个显式 ActiveRun。运行期间到达的输入在检查点被
  显式分类:steering 只在**完整工具批提交后**注入(不打断执行中的工具,
  紧急停止走 Esc);follow-up 只在**当前任务将停止时**注入;两者都是
  one-at-a-time。`/clear`、`/provider` 等命令延迟到本轮结束执行;
  运行中收到退出请求会先取消当前轮再退出;取消清空全部排队输入并提示。

### 6. 受控并发与资源锁(阶段 6)

- Zerone:工具严格串行。
- Onemore:preflight(校验/权限/审批)按源顺序;全批都是 ParallelSafe 且
  多于一个才并发(上限 4),任一 Sequential 工具使整批退回串行。
  `ToolCallFinished` 按完成顺序发出(UI 及时),历史 ToolResult 始终按
  ToolUse 源顺序写入(相同输入产生相同 prompt)。取消传播到每个调用的
  组合标志,未启动的调用直接补取消结果——每个 ToolUse 无论如何都有配对。
- 第二道资源锁:`write_file`/`edit_file` 的完整 read-modify-write 在同
  canonical path 的 mutation 锁内进行,即使调度层允许并发也不会交错。
- 可配置单工具超时(`[agent] tool_timeout_secs`):逾期置组合取消标志,
  因此中止的结果报 `timeout`;工具无视标志坚持完成的保留真实结果。

### 7. 结构化计划与长任务纪律

- 固定 `update_plan` 工具使用稳定 ID 的增量 patch；revision 由服务端生成，条目有稳定 ID、
  `pending / in_progress / completed` 三种状态，运行时强制最多一个 `in_progress`，
  并限制条目数与文本长度。过期 revision 和非法快照不会落库。
- 工具通过独立 harness effect 返回状态变化；`ToolUse + PlanUpdated + ToolResult`
  在同一 SQLite 事务提交，`ToolCallFinished` 与 `PlanUpdated` 事件只在成功后发出。
- TUI/headless 显示真实快照；恢复由 facts reducer 重建。压缩只携带活动项和完成
  计数。取消把 `in_progress` 退回 `pending` 并追加新 revision，不伪造完成。
- 模型准备结束但计划仍有活动项时，Runtime 最多追加一次继续提醒；之后允许结束，
  不会形成无限 completion gate。steering/follow-up 的优先级高于自动提醒。

### 8. Skills

- 启动时扫描当前 workspace 的 `.agents/skills/**/SKILL.md` 与平台 user-agent root 下的
  `.agents/skills/**/SKILL.md`（Windows 默认 `%APPDATA%\onemore\.agents\skills`，也可用
  `ONEMORE_HOME` 覆盖），目录快照在
  当前 Runtime 内冻结。Repo 技能覆盖 User 同名技能，单个坏文件只产生警告。
- `SKILL.md` 需要 YAML frontmatter 中的 `name` 与 `description`；system prompt
  只包含稳定 metadata catalog，正文按需由模型调用 `load_skill({"name": "..."})`。
- 技能正文和它引导的工具调用仍受原有权限、审批和工具能力策略控制；文件变化会被
  识别为 stale catalog，下一次启动才重新发现。

### 9. Project instructions

- 默认 harness 在启动时读取 workspace 根目录的 `AGENTS.md`；缺失或空文件不增加
  prompt，读取失败只产生一次非致命提示。
- 文件路径与正文使用 `<project_context>` / `<project_instructions path="...">` 边界
  注入 system prompt。快照在 Agent 生命周期内冻结，修改后需要重新启动 Agent 才会生效。
- 第一版不扫描 workspace 外的祖先目录，也不加载嵌套目录、`CLAUDE.md`、override 或
  全局指令文件。宿主通过 `context_providers(...)` 完整替换默认 context 时不会扫描
  `AGENTS.md`。

### 尚未实现

MCP、持久 Background/Task 系统、子代理、树形会话的 move/fork。

## 配置增量

相对 Zerone 新增的配置项:

```toml
[agent]
max_turns = 200                # 单个用户任务最多连续调用模型的次数
tool_timeout_secs = 300        # 可选:单工具执行超时,默认不限制

[retry]
max_attempts = 8               # 包含首次请求
base_delay_ms = 1000
max_delay_ms = 10000
max_retry_after_ms = 60000     # 服务端要求更长等待时直接失败

[compaction]
enabled = true                 # 可关闭自动触发；手动 /compact 始终可用
reserve_tokens = 16384         # 在正常输入硬预算前触发
keep_recent_tokens = 20000     # 压缩后原样保留的最近消息估算量

[permissions]                  # allow | ask | deny
workspace_read = "allow"
workspace_write = "allow"
outside_workspace = "ask"
commands = "ask"

[providers.xxx]
profile = "openai"             # openai | anthropic | deepseek-responses | deepseek-messages
default_model = "gpt-5"

[providers.xxx.models."gpt-5"]
context_window = 400000        # 每个模型独立配置上下文窗口
max_tokens = 128000
# 省略 efforts 时使用 profile 标准列表；非空数组完整覆盖；空数组不发送 effort。
efforts = ["low", "high", "max"]
# 可选；省略时优先 medium，不存在则用 efforts 第一项。
default_effort = "high"
```

`profile = "openai"` 的标准列表是 `none/minimal/low/medium/high/xhigh/max`；
`profile = "anthropic"` 是 `low/medium/high/xhigh/max`。模型选择按
workspace/provider/model 保存；切回该模型的 `default_effort` 时删除偏好覆盖。

## 存储

```text
%APPDATA%/romaing/onemore/  # Windows 默认；其他平台使用 XDG 数据目录
  config.toml
  sessions/
    <session-id>.db            # schema v4:事实日志 + 严格计划 reducer + token/cache 用量
  workspaces/
    <workspace-hash>.json      # 仅保存偏离各模型 default_effort 的思考程度
  .agents/
    skills/
      <skill-name>/SKILL.md  # 用户级技能；工作区技能位于当前项目 .agents/skills/
```

Onemore 不读取 `~/.zerone`,也不识别 `ZERONE_HOME`,因此两个程序的配置、密钥和会话
互不污染。每个会话仍使用独立 SQLite 数据库,并按 workspace 隔离。`/session all` 可枚举
全部 workspace 并显示所属路径，但当前 Runtime 只能加载其启动 workspace 内的会话；跨 workspace
恢复应从对应目录重新启动。v1(线性
messages 表)数据库在打开时自动迁移到当前 schema,迁移失败回滚、原库保持可用。

## npm 包

默认 npm 包名是 `onemore-agent`,安装后命令是 `onemore`:

```powershell
.\scripts\package-npm.ps1 -Pack
npm install --global .\dist\npm\onemore-agent-0.5.0.tgz
onemore --help
```

项目也上传了npm包，但不保证除windows外的可用性
```
npm install -g onemore-agent
```

本地打包只包含当前平台二进制。跨平台组包可通过 `-ArtifactsDir` 提供对应产物。

## 验证

```powershell
cargo fmt --check
cargo test --locked          # 单元测试 + wire 协议测试
cargo build --release --locked
.\scripts\package-npm.ps1 -Pack
```

## License

MIT
