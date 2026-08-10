# Onemore 下一阶段接力文档

更新日期：2026-08-10
工作区：`E:\onemore-cli`
功能代码基线：`8fc3df4 添加web_search工具`

## 开始前必须知道

用户已经确定本阶段顺序：

1. 高危操作强制审批。
2. Skills 迁移到 `.agents/skills` 并实现 `/reload`，不考虑旧配置或旧目录迁移。
3. 工作区工具与 Web 工具。

以上三项目前均已完成可交付实现。Web 的外部 Tavily、Brave Search、Exa、Serper backend
也已接入；Web 专用 RPC DTO 仍是后续任务。用户明确允许 RPC 层先只写 TODO，暂不实现协议。

`docs/next-phase-goals.md` 是本阶段最初路线图，不是实时完成状态。其中“明确延期”的工作区
搜索和 Web 搜索后来已经提前完成；后续会话必须以本文的完成状态为准，不要重复实现。

其他明确决定：

- Workspace skill：`<workspace>/.agents/skills/<skill-name>/SKILL.md`。
- User skill：`<user-agent-root>/.agents/skills/<skill-name>/SKILL.md`。
- 帮助用户安装 skill 时，内置提示词必须先询问安装作用域，并提示安装后执行 `/reload`。
- 工具参数、描述、输出清洗、截断与结构化 `details` 应参考成熟 coding agent，而不是只给
  shell 命令换名字。
- 本机是 `rustc 1.95.0`；仓库仍使用 Rust 2021 edition。没有必要为少量语法升级 edition。

## 已完成：高危操作审批

核心文件：

- `src/permission/command_risk.rs`
- `src/permission/mod.rs`
- `src/runtime/tool_execution.rs`
- `src/event.rs`
- `src/sdk/view.rs`
- `Gui-rpc-example/onemoreGui/src/components/ApprovalDialog.tsx`

当前行为：

- 删除、清理、权限/所有者修改、磁盘操作等高风险命令即使权限配置为 `allow`，仍强制一次审批。
- 识别 PowerShell、cmd、Git Bash 的命令链、管道和嵌套 shell；无法可靠解析时 fail closed。
- 审批请求携带规范化命令、cwd、目标范围和风险原因。
- 用户拒绝或取消后命令不会启动，但 ToolUse 仍得到配对 ToolResult。
- session grant 只匹配完全相同的已审批调用，不能泛化成永久绕过。
- SDK/RPC/GUI 使用同一审批事实；RPC 审批往返和重复响应已有回归测试。

## 已完成：Skills 与 `/reload`

核心文件：

- `src/skills.rs`
- `src/runtime/builder.rs`
- `src/runtime/commands.rs`
- `src/tools/load_skill.rs`
- `src/context/instructions.rs`
- `src/tui/command.rs`

当前行为：

- 只发现规范 `.agents/skills`，不读取旧 `.onemore/skills`。
- Repo 与 User 两级 catalog 在启动时冻结；Repo 同名优先，顺序确定，单个坏 skill 只产生 warning。
- `load_skill(name)` 只加载冻结 catalog 中的路径；路径或 hash 改变会返回 stale，不静默读取新内容。
- `/reload` 重新读取配置、`AGENTS.md`、skill catalog、工具声明和 provider/Web binding，同时保留
  当前 session facts。
- 默认提示词只在 skills harness 可用时注入 skill 指令，且安装路径说明只出现一次。
- README 中用户级路径已经与实际代码统一为 `<data-root>/.agents/skills`。

## 已完成：工作区工具

核心文件：

- `src/tools/workspace_walk.rs`
- `src/tools/glob.rs`
- `src/tools/search.rs`
- `src/tools/git.rs`
- `src/tools/mod.rs`
- `src/workspace.rs`

### `glob`

- 基于 ripgrep 同源的 `ignore`、`globset` crate，不依赖系统安装 `rg`。
- 遵循 `.gitignore`，不跟随目录符号链接，跳过 `.git`、`target`、`node_modules` 等生成目录。
- 结果稳定排序，`limit` 有硬上限并明确报告截断。
- 模型可见路径统一使用 `/`，避免 Windows 反斜杠被复制进 JSON 参数后产生转义歧义。

### `search`

- 行级 Rust regex，支持 `path`、`include`、`case_sensitive`、`limit`。
- 新增 `context = 0..10`，返回匹配前后行；正文采用接近 `grep -C` 的 `path-line-` / `path:line:`
  表示，`details.matches[].before/after` 保留结构化行号与文本。
- 仍然是**逐行匹配**，不支持跨换行 regex；tool description 已明确该边界。
- 跳过大文件、非 UTF-8 和不可读文件，并分别计数；模型可见路径统一使用 `/`。

### `repo_state`

- 一次返回 repository root、branch、HEAD、staged/unstaged/untracked 统计和有界变更列表。
- Git 直接通过 `Command` 调用，不经过 shell；禁用 hooks、fsmonitor、color 和 optional locks。
- 进程有 5 秒超时、取消传播和 stdout/stderr 上限；路径统一使用 `/`。

### `git_diff`

- `path` 现在具有真实过滤语义：先通过 `git rev-parse --show-prefix` 转换为仓库相对 pathspec，
  再传入 `git diff -- <pathspec>`。默认过滤到 workspace root；workspace 正好是 repo root 时查看全仓库。
- patch 前先运行有界 `git diff --numstat -z`，输出逐文件 `+新增 -删除 path` 概览；二进制和 rename
  有独立结构化表示。
- `details` 包含 `pathspec`、`changed_files`、总增删行、binary 数量、summary/patch 独立截断状态。
- 文件概览最多占 6K 模型字符，总 diff 模型正文预算为 20K；截断时提示使用 `path` 深入目录。
- `base` 与 `staged` 互斥；仍支持默认 worktree/index diff、staged diff 和 revision/worktree diff。
- Git diff 不包含 untracked 文件，这是 Git 本身的语义；先用 `repo_state` 查看 untracked 清单。

工具统一返回 `ToolOutput { model_text, ui_summary, details }`，模型文本和 UI 摘要经过 ANSI/C0
控制字符清洗及 24K 总上限，失败保留稳定 `ToolErrorCode`。

## 已完成：Web capability、OpenAI native 与四家 external backend

核心文件：

- `src/web.rs`
- `src/web/tavily.rs`
- `src/web/brave.rs`
- `src/web/exa.rs`
- `src/web/serper.rs`
- `src/web/http_client.rs`
- `src/tools/web_search.rs`
- `src/config.rs`
- `src/provider/openai_responses.rs`
- `src/provider/mod.rs`
- `config.example.toml`
- `docs/workspace-and-web-tools.md`

当前架构：

```text
[web] config
  -> capability resolver
  -> frozen WebCapabilityBinding
      -> OpenAI hosted tool -> provider request
      -> HarnessFunction -> local web_search tool
  -> prompt/cache identity（不含密钥）
```

当前行为：

- `[web].mode`：`auto | native | external | disabled`。
- OpenAI Responses 的 `auto/native` 解析为 hosted `{ "type": "web_search" }`；hosted tool 不进入
  本地 `ToolRegistry`。
- 可选参数：`context_size = low|medium|high`、最多 100 个 `allowed_domains`、近似
  `country/region/city/timezone`。配置在 provider 构造时冻结，不由模型临时控制。
- Web binding 和参数进入 prompt fingerprint 与 OpenAI `prompt_cache_key`，避免 capability 变化后
  错误复用缓存。
- OpenAI `url_citation` 被投影为 provider-neutral `Source`：只接受有界 HTTP(S) URL，去除 URL
  credentials 与 fragment，清洗/压平/截断标题，URL 去重，最多 20 个 source。
- Agent 启动、`/reload` 和 provider/model 切换均发出当前 Web capability Notice。
- `external_backends` 固定支持 `tavily | brave | exa | serper`，按配置顺序选择首个有凭据的 backend。
- 每个 backend 的密钥与 LLM provider 使用相同规则：在 `[web.backends.<name>]` 二选一配置
  `api_key` 或 `api_key_env`；两者都省略时读取对应标准环境变量。解析后的密钥在 capability
  epoch 构建时冻结，不进入 prompt identity、工具输出或持久化事实。
- harness-owned `web_search` 复用普通工具生命周期、强制审批、取消、超时、清洗和有界输出；
  runtime error 不会触发 session 内 backend 切换。

尚未实现：

- Anthropic/DeepSeek/xAI native Web adapters。
- Web started/completed/failed、sources/provenance 的稳定 SDK/RPC DTO 与一等持久化。
- 通用 `web_fetch`、抓取缓存和浏览器控制。用户当前判断 `run_command + curl` 已能覆盖偶发抓取，
  `web_fetch` 必要性不强，明确暂缓；只有反复出现 HTML 正文清洗、SSRF/重定向约束、统一来源元数据
  或跨 shell 可移植性问题时再评估窄范围只读实现。

RPC 延期项已记录在 `docs/workspace-and-web-tools.md`。可以先设计 provider-neutral runtime/SDK
source 事件，但在用户改变决定前，不要为 Web 修改 RPC 协议。

## 路线图实时状态

| `next-phase-goals.md` 里程碑 | 当前状态 | 下一步 |
| --- | --- | --- |
| A：可观察的运行基础 | 已完成 | compaction 生命周期、固定计划面板、可展开工具详情、实时工具进度和长任务回归均已贯通 |
| B：进程与编辑能力 | 已取消 | 用户决定取消该里程碑，不实现结构化 patch/edit 或 `process_start/read/write/wait/stop` |
| C：Skill 范式与 reload | 已完成 | 只做回归维护，不再设计旧目录兼容 |
| D：诊断与回归 | 部分完成 | workspace/Web 已完成；结构化编译/测试诊断和真实 provider 的 usage/compaction/retry 验证仍未完成 |

TUI 已消费 `ToolCallStarted`、retry、compaction 和计划事件；工具详情使用稳定 `tool_call_id`，
主 transcript 只显示有界头尾预览，完整内容通过 `Ctrl+T` 或 `/tools` overlay 查看。进入后续工作前
仍需区分 runtime/SDK 缺少事实、适配层未投影和 TUI 未展示三类问题。

## 必须保持的可靠性约束

1. Provider 每次调用必须终止于 `Done / Error / Aborted`；裸 EOF 或裸 `[DONE]` 不是成功。
2. 自动 retry 只允许发生在尚未产生任何流事件时，避免重复副作用或重复文本。
3. ToolUse 必须恰好配对一个 ToolResult；半截参数不得执行。
4. assistant ToolUse、harness effects 与全部 ToolResult 必须原子提交。
5. session append 失败时，内存 facts 不得推进。
6. 权限顺序保持 `prepare/schema -> hard deny -> pre-hook -> re-prepare -> permission`。
7. 高危审批边界不能被 config、prompt、skill 或模型参数关闭。
8. 并发工具 UI 事件可按完成顺序，持久化 ToolResult 必须按源顺序。
9. provider/model/Web binding 改变后不得复用不匹配的 usage baseline 或 prompt identity。
10. hosted Web tool 不进入本地 registry；runtime error 不得触发 session 内 backend fallback。
11. 工具输出必须有界；`details` 用于结构化数据，不能成为绕过模型上下文限制的第二正文。
12. `config.example.toml` 必须与 `Config::EXAMPLE_CONFIG` 完全一致，已有测试锁定。

## 验证状态

2026-08-10 的最终完整回归已通过：

- 270 个 unit tests。
- 1 个 RPC subprocess test。
- 8 个 provider wire tests。
- 0 failed、0 ignored。

验证命令如下；任何后续代码改动都应重新执行：

```powershell
cargo fmt --all -- --check
cargo check --workspace
cargo test --workspace
git diff --check
```

Cargo 在当前 E: 盘文件系统上会提示 incremental cache 无法 hard link 并自动复制。这是环境 warning，
不是 Rust lint 或测试失败。测试内临时 Git 仓库还可能打印 LF/CRLF warning，同样不影响结果。

GUI `npm run build` 与 Tauri Rust `cargo check` 均已通过；工具输出 v3 DTO 同步到 GUI reducer。

外部 Web live smoke：Tavily、Exa、Serper 已用免费计划真实接口成功验证；Brave Search 按用户要求
只做离线 wire fixture。OpenAI native Web 尚未使用真实 API key 做 live smoke。测试密钥没有写入仓库，
配置回归同时覆盖 `[web.backends.<name>].api_key` 直写和凭据不进入 binding identity。

## 下一会话建议顺序

1. 先读本文、`docs/workspace-and-web-tools.md` 和 `docs/next-phase-goals.md`。
2. 里程碑 A 已完成；后续只需维护 `AgentEvent -> ProgressEvent/SessionSnapshot -> TUI` 链路，
   保持稳定 ID、快照权威和有界输出约束。
3. 不进入已取消的里程碑 B；后续优先做结构化编译/测试诊断及真实 provider
   usage/retry/compaction 验证。
4. Web 后续仅考虑 provider-native adapter 或 provider-neutral source 事件；`web_fetch` 暂缓，
   Web RPC DTO 继续保留 TODO，除非用户明确改变优先级。

## 推荐阅读顺序

```text
docs/handoff.md
docs/workspace-and-web-tools.md
docs/next-phase-goals.md
src/event.rs
src/sdk/view.rs
src/runtime/session_events.rs
src/runtime/compaction.rs
src/tui/mod.rs
src/tui/transcript.rs
src/plan.rs
src/tools/edit_file.rs
src/tools/run_command.rs
src/tools/mod.rs
```

开始 Web 后续工作时再读 `src/web.rs`、`src/tools/web_search.rs`、`src/config.rs` 和对应 adapter；
开始权限回归时再读 `src/permission/command_risk.rs` 与 `src/runtime/tool_execution.rs`。

开源方案调研记录在 `docs/open-source-agent-research.md`。本机还保留了用于只读调研的 Codex 浅克隆：

```text
C:\Users\14844\AppData\Local\Temp\onemore-source-research\codex
commit 8cabf5a
```

仓库内另有 `example/grok-build`。两者只用于参考参数、描述、生命周期和清洗范式，不应直接复制
不适合 Onemore 当前 runtime 边界的完整架构。
