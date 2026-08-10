# Onemore 下一阶段开发目标

更新日期：2026-08-10

## 总体方向

Onemore 继续遵循：

> **Mechanically strong, cognitively weak**

本阶段的 harness 应该扩充模型的行动能力、反馈质量和任务可观察性，但不替模型规定固定工作流。工具应当提供稳定的语义接口；TUI 应当暴露真实运行状态；行业范式调整应当减少模型与用户的额外认知负担。

## 本阶段范围

### 纳入范围

1. 工具能力与工具契约优化
2. 长任务 TUI 可观察性和交互优化
3. Skill 目录与加载机制向主流 Agent 约定靠拢
4. 高危操作的不可覆盖权限边界

### 明确延期

以下能力保留设计空间，本阶段不实现：

- 工作区语义检索：`glob`、`search`、`repo_state`、`git_diff`
- 网络搜索与抓取：`web_search`、`web_fetch`
- 浏览器控制、页面快照和截图工具

延期不代表否定这些能力，而是先把本地执行、编辑、验证和长任务运行闭环做好。

### 延期设计基线：Web Capability Provider

网络能力后续不应简单实现成一个固定的 `web_search` 工具。Onemore 应把 Web 视为逻辑能力，由 capability resolver 在 session 建立时根据当前 provider、模型能力和本地配置选择具体实现。

```text
Web Capability
    -> Capability Resolver
        -> Provider-hosted native search
        -> Harness-owned function tool
        -> Harness preflight fallback
```

建议的内部绑定类型：

```rust
enum WebCapabilityBinding {
    OpenAiNative,
    AnthropicServer { tool_version: String },
    ProviderNative { provider: String },
    HarnessFunction {
        backend: SearchBackendKind,
        force_tool: bool,
    },
    HarnessPreflight { backend: SearchBackendKind },
    Disabled,
}
```

解析规则：

- OpenAI Responses 原生支持时，provider adapter 注册 `{ "type": "web_search" }`，搜索由 OpenAI 执行。
- Anthropic Messages 原生支持时，provider adapter 注册版本化 server tool，搜索由 Anthropic 执行。
- DeepSeek、xAI 等 provider 原生支持时，使用各自 adapter 和能力声明，不按相同工具名假设协议完全兼容。
- 模型支持 function calling、但没有原生搜索时，向模型暴露 Onemore 的 `web_search` / `web_fetch`，由 harness 调用 Tavily、Brave 等外部服务。
- 模型支持工具但选择能力较弱时，可以在确定需要联网的调用中使用强制 tool choice。
- 完全不支持结构化工具调用的旧模型无法“强制 tool”；只能由 harness preflight 搜索并把受控结果注入上下文，或明确禁用网络能力。

外部搜索服务按执行所有权归类，而不是按算法归类。Tavily、Brave、Exa 即使使用不同的传统、语义或 AI 搜索算法，从 Onemore 角度仍然都是 harness-owned HTTP backend。

native provider tool 不进入普通 `ToolRegistry`，否则 runtime 会误以为需要本地执行。resolver 应在 session 建立时解析一次，并固定本 session 的 binding，不在运行时错误后静默切换 backend 或工具语义，以保持行为、引用和 prompt cache 前缀稳定。

`auto` 是 resolve policy，不是 fallback policy：

```text
auto
  -> capability resolver
  -> OpenAI native / Anthropic server / harness function / preflight
  -> 固定当前 session
```

绑定后的 runtime error 应原样形成 `web_search failed` 一类失败，并带上 capability、provider、backend 和可重试信息。允许对同一 binding 做正常的网络重试，但不应在同一 session 中切换到另一种实现。下一次 session，或用户显式执行 `/reload`、切换 provider/model 所创建的新 capability epoch，才重新 resolver。

不同实现统一投影为 provider-neutral 事件和异构来源记录：

```rust
enum SourceKind {
    WebPage,
    Pdf,
    Document,
    BrowserSnapshot,
    Other(String),
}

struct Source {
    id: String,
    kind: SourceKind,
    locator: Option<String>,
    title: Option<String>,
    content_preview: Option<String>,
    metadata: BTreeMap<String, String>,
}
```

`locator` 可以是 URL、`document_id`、`tab_id/snapshot_id` 或 provider 侧的不透明定位符；`id` 应在 provider/backend 作用域内稳定。`metadata` 适合保存 published time、provider、mime type、citation label 等扩展属性。实现上优先使用 `BTreeMap` 或自定义稳定 map，避免事件、快照和 prompt cache 中出现不确定的键顺序。

SDK/TUI 只消费 `CapabilityStarted`、`SourcesUpdated`、`CapabilityFinished` 等统一事件，不直接依赖 OpenAI `web_search_call`、Anthropic `server_tool_use` 或第三方原始 JSON。会话持久化最终回答和规范化 sources，不持久化无法跨 provider 重放的原始 server-tool block。

后续实施顺序：

1. 先建立 `WebCapability`、resolver、统一 source/citation 和 backend trait。
2. 第一版实现 Tavily、Brave 两个 `HarnessFunction` backend；同一时刻只向模型暴露一个选定实现。
3. 再增加 OpenAI Responses、Anthropic Messages、DeepSeek 和 xAI 的 native binding。
4. 最后评估 Exa、通用 `web_fetch`、抓取缓存和浏览器降级。

建议配置形态：

```toml
[web]
mode = "auto" # auto | native | external | disabled
external_backends = ["tavily", "brave"]
```

`auto` 必须产生可观察、确定的选择结果；启动事件和 TUI 应显示当前 Web backend。runtime 失败时报告失败，不在同一 session 内静默切换 backend。

## 一、工具扩充与契约

### 1. 结构化局部编辑（P0）

当前 `write_file` 适合新建或整文件重写，`edit_file` 需要继续增强为模型友好的局部修改接口。目标是提供一个等价于 `apply_patch` 的能力，而不是再包一层 Bash。

目标行为：

- 支持一个或多个局部 patch/hunk
- 原文上下文不匹配时拒绝应用，不静默覆盖
- 一次调用返回修改文件、hunk 数量、增删行和冲突位置
- 修改结果可直接生成 diff，便于 TUI 展示和模型复核
- 新文件、整文件重写和局部修改分别保持清晰用途
- 失败结果包含路径、匹配上下文和可修复建议

建议的模型可见结果保持短小，详细 diff 放入结构化 `details`，避免把完整文件重新传回上下文。

验收标准：

- 正常 patch 一次完成，模型不需要先读取整文件再用 Bash 重写
- patch 冲突不会损坏文件
- 并行修改后能明确报告冲突，而不是覆盖用户修改
- 现有工具配对、权限和事实日志不回退

### 2. 长进程生命周期工具（P0）

短命命令继续使用 `run_command`；常驻服务、开发服务器和长测试需要由 harness 管理进程，而不是依赖模型自行拼接后台 shell 语法。

建议的最小能力：

```text
process_start   启动进程，返回 process_id
process_read    读取自上次读取后的 stdout/stderr 增量
process_write   向 stdin 写入内容
process_wait    等待退出或等待新的输出
process_stop    请求终止并返回最终状态
```

过程状态至少包括：`starting`、`running`、`exited`、`failed`、`stopped`、`timed_out`。

工具协议必须保证：

- `process_id` 在一次 Agent 会话内稳定
- 输出支持增量读取和上限控制
- 返回退出码、运行时长、最近输出和是否仍存活
- 取消 Agent 任务时可以传播到关联进程
- 进程不会因为一次模型请求结束而丢失状态
- `run_command` 与进程工具的职责、返回格式和错误码保持一致

验收标准：

- Agent 可以启动 dev server，继续执行其他工作，再读取日志并最终停止它
- 长命令不会阻塞模型直到超时才返回
- 进程异常退出能在下一次读取时被发现
- stdout/stderr 不会无限增长或重复注入上下文

### 3. 结构化诊断与验证结果（P1）

本阶段先不实现完整 LSP。优先为编译、测试和检查命令提供统一的结果抽取层，减少模型从长日志中寻找有效错误的成本。

建议提供统一结果模型：

```json
{
  "status": "failed",
  "passed": 41,
  "failed": 2,
  "failures": [
    {
      "file": "src/foo.rs",
      "line": 83,
      "column": 12,
      "message": "expected 400, got 500"
    }
  ]
}
```

原始输出仍可按需查看，但默认返回应优先给出失败数量、定位信息和最后的关键错误。

### 4. 现有工具的模型体验修正（P0/P1）

在增加新工具前，先统一现有工具的契约：

- `read_file`：支持明确的行范围、字符上限、截断元数据
- `list_dir`：支持深度、过滤和稳定排序
- `edit_file`：错误路径、未匹配文本和多重匹配必须分别报告
- `run_command`：明确 cwd 不持久、stdin 行为、超时和输出截断
- 所有工具：区分模型正文、UI 摘要和结构化 `details`
- 所有路径参数：返回规范化路径和失败原因，减少弱模型的路径猜测

新工具的准入标准是：它必须提供 Bash 不具备的语义、状态、原子性或生命周期管理，或者显著压缩模型需要处理的输出。仅仅把一条 Bash 命令改名为工具不纳入本阶段。

## 二、权限与高危操作

### 1. 强制审批边界（P0）

权限配置中的 `allow` 只能作为普通操作的默认授权，不能覆盖 harness 对高危副作用的硬边界。即使用户把所有权限配置为 `allow`，以下操作仍必须经过一次明确的用户确认：

- 删除文件或目录：`rm`、`rm -rf`、`del`、`rmdir`、PowerShell `Remove-Item`
- 大范围或不可逆清理：`git clean`、`git reset --hard`、格式化和磁盘清理命令
- 修改权限、所有者或系统级配置的命令
- 工具声明为 `always_ask` 的远端、设备或其他不可静态判断的副作用

这条边界由 harness 强制执行，不能通过配置、prompt、skill 或模型传入的参数关闭。

### 2. 命令风险识别

高危判断不能只依赖简单的字符串包含，否则容易被引号、路径名、管道、别名或不同 shell 语法绕过。建议按当前实际 shell 做最小解析，并至少识别：

- 命令链、管道、重定向和子命令中的高危操作
- Git Bash、PowerShell、cmd 三种 shell 的等价删除/清理语义
- 相对路径、绝对路径、通配符和递归参数的实际作用范围
- 一次命令中同时包含普通操作和高危操作的情况

无法可靠解析时，应升级为审批，而不是降级为允许执行。

### 3. 审批体验与审计

审批请求应让用户知道“将执行什么”和“影响什么”，而不是只显示工具名：

- 显示规范化后的命令、cwd、目标路径和风险原因
- 对递归、通配符和工作区外路径给出明确范围提示
- 支持允许一次、拒绝一次；本阶段不提供绕过高危边界的永久 allow
- 拒绝后向模型返回稳定错误码和简洁原因
- 记录审批请求、用户决定、实际执行命令和结果，便于会话审计

验收标准：

- 所有配置均为 `allow` 时，`rm` 等高危命令仍弹出审批
- 用户拒绝后命令不会启动，且 ToolUse 仍得到配对的 ToolResult
- 高危命令嵌在管道、链式命令或子 shell 中时仍能触发审批
- 无法解析的命令默认请求审批，不出现静默放行
- TUI、SDK 和 RPC 对审批状态使用同一套事件与终态

## 三、TUI 优化

长任务测试记录见 [`长任务测试观察.md`](../长任务测试观察.md)。本阶段 TUI 的目标不是展示所有内部细节，而是让用户在任务运行中回答四个问题：模型现在在做什么、哪个工具正在运行、是否遇到阻塞或重试、距离上下文压缩还有多远。

### 1. 流式运行状态（P0）

现有事件模型已经包含 `AssistantDelta`、`ThinkingDelta`、`ToolCallPending`、`ToolCallStarted`、`ToolCallUpdated`、`ToolCallFinished` 和 retry 事件。TUI 应优先修复这些事件的消费、刷新和生命周期绑定。

目标表现：

- 助手增量文本在生成期间可见，完成事件用于最终校正
- 思考内容默认以低干扰状态显示；是否展示正文由界面策略决定
- 工具调用在开始、运行中、完成和失败时都可见
- 同一工具调用按 `id` 更新，不重复追加大量行
- 长任务期间有明确的当前阶段和最后活动时间
- 任务结束后仍保留可回看的运行摘要

### 2. 工具调用详情（P0）

针对长 Bash 命令当前只能看到状态的问题：

- 默认显示一行可读摘要
- 支持展开查看完整命令、cwd、运行时长和退出码
- 输出默认显示首尾片段和截断标记
- 失败时优先显示错误摘要，完整输出按需展开
- 工具参数不应因为 UI 截断而影响模型收到的 ToolResult

### 3. 固定计划面板（P0）

计划应当在任务运行期间持续悬挂，而不是只在某次刷新或任务结束后出现。

至少显示：

- 当前活动项
- 已完成/总数
- 待处理项
- 最近一次计划变更

面板应支持收起，但不能因为聊天区滚动而永久丢失。计划状态仍以 runtime snapshot 为准，TUI 不自行推断状态。

### 4. Retry 与网络错误可观察性（P1）

长任务中的网络失败不能只表现为最终错误。TUI 应展示：

- 当前第几次重试和最大次数
- 下次重试倒计时或等待时长
- 最近一次错误摘要
- 当前是否已经恢复
- 最终失败时的完整失败链路

本阶段重点是可观察性；重试次数和退避策略沿用 runtime 配置，除非测试证明策略本身不足。

### 5. 自动压缩显示（P0）

自动压缩是一次用户不可见的模型调用，必须在 TUI 中成为明确的阶段，而不是看起来像卡顿。

目标表现：

- 开始时显示：`正在自动压缩历史`
- 显示触发原因和估算值，例如 `128k / 151k tokens`
- 压缩完成时显示：摘要长度、保留消息数量和压缩前估算
- 压缩失败、取消或持久化失败时显示明确错误，说明历史是否改变
- 状态栏持续显示当前估算上下文、可用预算和压缩状态
- 手动 `/compact` 与自动压缩使用相同的视觉组件，但标注触发来源

建议在 SDK/RPC 事件层增加稳定的 `CompactionStarted`、`CompactionFinished`、`CompactionFailed` 事件，或扩充现有 Notice 的结构化字段。TUI 不应通过匹配中文提示文本判断压缩状态。

### 6. `/reload`（P1）

增加一个面向用户的 `/reload` 命令，默认只在当前轮结束后执行。

建议语义：

- 重新读取配置、项目指令和 `.agent` skill catalog
- 重建工具声明与 context providers
- 保留当前 session facts 和模型对话历史
- 当前轮运行中提交时返回 busy，并提示在任务结束后重试
- reload 成功或失败都生成明确事件

如果某些组件只能在 Agent 构造时冻结，应采用“以当前 facts 构造新 Agent”的方式，不在每轮请求前隐式重扫目录。

## 四、Skill 与行业目录范式

### 1. 目录约定（P0）

将当前 Onemore 专有的 `.onemore/skills` 约定调整为 Codex/Agent Skills 生态使用的 `.agents/skills`：

```text
workspace/
└── .agents/
    └── skills/
        └── skill-name/
            └── SKILL.md
```

用户级技能使用对应的用户目录：

```text
<user-agent-root>/.agents/skills/<skill-name>/SKILL.md
```

其中 `<user-agent-root>` 由平台数据目录决定；仓库内可按目录层级向上发现 `.agents/skills`，使子目录可以拥有局部技能。

`AGENTS.md` 继续承担项目指令职责，不与 skill 包目录混用：

- `AGENTS.md`：项目级工作约束和说明
- `.agents/skills/`：可按需加载的能力包

### 2. `AGENTS.md` 的职责（P0）

`AGENTS.md` 本身不需要迁移到 `.agent` 或 `.agents`。它已经是主流 coding agent 使用的项目指令约定，建议采用分层文件而不是一个不断膨胀的全局 prompt：

- 仓库根目录 `AGENTS.md`：项目级构建、测试、架构和协作规则
- 子目录 `AGENTS.md`：只描述该目录的额外约束，离当前文件更近的规则优先
- `.agents/skills/`：可发现、可按需加载的专项工作流

根 `AGENTS.md` 应保持短而稳定，放入模型无法从代码和工具结果可靠推断的事实；具体工作流、长篇参考资料和可执行脚本应进入 skill 包。

### 3. Skill 包格式（P0）

继续使用 `SKILL.md` 作为入口，但将格式定义为稳定公共契约：

```markdown
---
name: skill-name
description: One-line description used in the catalog
---

Skill instructions loaded on demand.
```

要求：

- `name` 使用小写字母、数字和连字符
- `description` 是短描述，只进入启动 catalog
- 正文按需通过 `load_skill` 加载，不预先进入完整 system prompt
- 一个目录只包含一个主 `SKILL.md`
- 可选资源放在同一 skill 目录内，并由正文使用相对路径引用
- discovery、排序、同名覆盖和 stale hash 检查保持确定性

### 4. 加载与刷新生命周期（P1）

- Agent 启动时扫描 `.agents/skills`
- 启动 prompt 只列出 name/description
- 模型调用 `load_skill(name)` 后才读取正文
- 文件发生变化时，旧 catalog 明确报告 stale，不静默读取新内容
- `/reload` 重新建立 catalog
- skill discovery 的 warning 通过启动事件和 TUI 显示

### 5. 迁移策略

本项目允许直接调整最终接口，因此本阶段可以直接采用 `.agents/skills` 作为规范路径。是否暂时读取 `.onemore/skills` 作为兼容路径，应单独决定；如果保留兼容，必须明确优先级、冲突规则和弃用提示，不能让两个目录静默产生不确定覆盖。

## 五、建议实施顺序

### 里程碑 A：可观察的运行基础

- 先完成高危操作的强制审批边界和命令风险识别
- 修复 TUI 对现有流式、工具和 retry 事件的消费
- 增加自动压缩开始/完成/失败的结构化状态
- 工具输出统一摘要、正文和 details
- 补齐长命令详情和固定计划面板

完成标志：一次包含多个工具、长输出和 retry 的长任务，用户可以在不中断任务的情况下判断当前状态。

### 里程碑 B：进程与编辑能力

- 完成局部 patch 工具
- 完成长进程生命周期工具
- 为进程启动、增量输出、退出和取消补齐事件与测试
- 让 TUI 展示运行中的进程和最终退出结果

完成标志：Agent 可以启动一个常驻服务，继续修改代码，读取服务输出，运行检查并停止服务。

### 里程碑 C：Skill 范式与 reload

- 切换 `.agents/skills` discovery
- 固化 `SKILL.md` 公共格式和目录契约
- 实现 `/reload`
- 增加启动、刷新、stale 和冲突场景测试

完成标志：不重启整个程序即可重新加载配置、项目指令和 skill catalog，同时保持当前会话事实不变。

### 里程碑 D：诊断与回归

- 增加结构化测试/编译诊断结果
- 补充工具契约和长任务集成测试
- 用真实 provider 记录验证 usage、自动压缩和 retry 展示
- 确认下一阶段按既定 Web Capability Provider 设计进入 Tavily/Brave 实现
- 再评估工作区搜索和浏览器工具的优先级

## 六、阶段验收标准

本阶段完成前，至少满足：

- 工具失败不会静默成功，输出不会无界增长
- 高危删除、清理和系统修改命令不能被 `allow` 配置绕过
- 审批请求展示实际命令、目标范围和风险原因，并留下可追踪结果
- 局部编辑冲突不会覆盖文件
- 常驻进程可以被观察、交互、取消和回收
- 长任务中助手增量、工具状态、计划、retry 和压缩状态可见
- 自动压缩与手动 `/compact` 的状态表现一致且可区分来源
- `/reload` 不破坏已提交 session facts
- `.agents/skills` 的 discovery、加载、冲突和 stale 行为有测试
- TUI、`--once`、SDK 和 RPC 继续使用同一 runtime 事实与事件源
