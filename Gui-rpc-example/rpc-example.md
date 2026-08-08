# Onemore JSONL RPC 集成教学

以 `Gui-rpc-example`（Tauri 2 桌面客户端）为参考实现，教你从零接入 Onemore 的
`--rpc` 子进程协议。协议本身的权威定义见 [`rpc-sdk-design.md`](./rpc-sdk-design.md)，
本文是**面向集成者的教学路线**：先动手感受协议，再讲客户端工程要点，最后排错。

> 阅读前提：你打算写一个自己的 GUI / 脚本 / 服务来驱动 Onemore agent。你不需要懂
> Onemore 内部实现——RPC 是唯一需要掌握的边界。

---

## 0. 一句话概括

```
你的程序 = 一个子进程管理员 + 一个 JSONL 读写器 + 一个状态投影器
```

- 你用 `std::process::Command` 启动 `onemore --rpc`（不经 shell）；
- 向它的 **stdin** 写 `{"type":...}` JSON 行（**必须以 LF 结尾**）；
- 从它的 **stdout** 逐行读 JSON（协议帧），**stderr** 只当诊断日志；
- 协议帮你区分「命令已接纳」「命令已结束」「session 已稳定空闲」三种状态；
- 永远不要在你的客户端里复制 agent 逻辑、不要碰 SQLite、不要绕过权限。

---

## 1. 先动手：三行命令感受协议

下面的命令在你的机器上真实可跑（假设 `onemore` 在 PATH）。

### 1.1 Hello——第一次握手

```bash
printf '{"type":"hello","version":1}\n' | onemore --rpc
```

你会看到一帧 `hello`：`server`（进程实例 ID、协议版本、能力位、模型目录）+ `snapshot`
（session ID、revision、workspace、phase、usage、空 transcript……）。

这就是「连接成功」的定义：**hello 返回了，你才拥有一个可对话的 session**。

### 1.2 Request / Response——问一个查询

```bash
printf '{"type":"hello","version":1}\n{"type":"request","id":"req-1","request":{"command":"get_snapshot"}}\n' \
  | onemore --rpc
```

末尾会看到：

```json
{"type":"response","id":"req-1","ok":true,"result":{"command":"get_snapshot","snapshot":{...}}}
```

注意：query 命令（`get_snapshot`/`list_models`/`list_sessions`）在 response 里**直接返回数据，
没有 `command_id`**；mutation 命令（`prompt`/`steer`/…）返回 `command_id`，完成状态靠事件报告。

### 1.3 完整的一次对话（真实模型调用）

```bash
printf '%s\n' \
  '{"type":"hello","version":1}' \
  '{"type":"request","id":"req-1","request":{"command":"prompt","text":"请只读检查当前工作区，不要修改文件"}}' \
  | onemore --rpc
```

stdout 依次会出现：hello → response（`command_id`）→ 一串 event（`run_started`、
`assistant_delta`、可能的 `tool_started`/`tool_finished`、`command_finished`、`settled`）。

**这是理解整个协议的关键画面**：一条消息序列 = 一个 response + 一组事件，
request ID 关联「这一个请求的应答」，command ID 关联「这一条命令的整个生命周期」。

---

## 2. 语义模型：三种状态不能混用

| 状态 | 含义 | 你会看到什么 |
|---|---|---|
| accepted | 命令已被 Runtime 接纳，别重发 | `response.ok=true` 且 `result.command_id=...` |
| finished | 命令已有明确终态（成功/失败/取消） | `command_finished { command_id, status, error }` |
| settled | session 回到稳定边界，没有运行/队列/审批 | `settled { revision }` |

规则：

1. `response.ok=true` **只表示接纳成功**，业务是否做完看 `command_finished`。
2. 每个 accepted mutation 恰好一个 `command_finished`；`settled` 不早于相关 terminal、不重复。
3. `settled` 不能代替 `command_finished`——settled 是 session 级，terminal 是命令级。
4. 忙时发 `prompt` 会得到 `busy` 错误；运行中请改用 `steer`（工具批之间注入方向）或
   `follow_up`（当前任务将停止时追加工作）。

---

## 3. 命令集速查（v1）

| 命令 | 参数 | 类型 | 说明 |
|---|---|---|---|
| `prompt` | `text` | mutation | 仅 idle 接纳；忙时返回 busy |
| `steer` | `text` | mutation | 运行中注入方向修正；空闲时等价新 run |
| `follow_up` | `text` | mutation | 当前任务将停止时追加工作 |
| `abort` | — | mutation | 设置取消信号，等每个 accepted 命令的 terminal |
| `compact` | — | mutation | 触发压缩 |
| `set_model` | `provider, model, effort` | mutation | 一次性提交，无半切换状态 |
| `clear_conversation` | — | mutation | 仅 idle |
| `list_sessions` | — | query | 返回 `sessions` |
| `load_session` | `session_id` | mutation | 仅 idle；成功后清理 session 级授权 |
| `list_models` | — | query | 返回 `models`（不含 API key / base URL） |
| `get_snapshot` | — | query | 权威快照，不改运行状态 |
| `approval_response` | `request_id, decision` | mutation | decision ∈ allow_once / allow_session / deny |
| `shutdown` | — | mutation | 成功 response flush 后进程退出 |

**事件**（`{"type":"event","event":{...}}`）：

- `session_snapshot { snapshot }` —— 权威值，前端重建画面的唯一来源
- `progress { progress }` —— 瞬时流式：`run_started` / `assistant_delta` /
  `assistant_finished` / `tool_started` / `tool_updated` / `tool_finished` /
  `approval_requested` / `approval_resolved` / `notice` / `user_message` / `error` …
- `command_finished { command_id, status, error }`
- `settled { revision }`

---

## 4. 客户端工程要点

下面每一条都是 Gui-rpc-example 踩过/实现过的真实做法，按重要性排序。

### 4.1 进程生命周期与所有权（最重要）

- 用 `std::process::Command` 分别传 program 和 args，**不要拼接 shell 字符串**；
- `current_dir` 设为用户的 workspace；
- 启动后**立刻接管 stdin/stdout/stderr**；
- 一个窗口/客户端**至多一个 RPC 子进程、一个 stdin writer**；
- GUI 退出、窗口关闭、transport 出错时：先停止接收新请求 → 关 stdin（EOF）→ 等进程退出 →
  超时才 kill。**绝不能让 onemore 在 GUI 退出后继续执行工具**。

### 4.2 Framing：LF 是命根子

```
stdin 发送： 每帧 JSON + '\n'（只写 LF，接受端容忍 CRLF）
stdout 读取：只以 0x0A 分帧，U+2028/U+2029 不是分隔符
```

**真实事故**：发送 hello 帧时漏了 `\n`，服务端立刻回
`{"type":"hello_error","error":{"code":"unterminated_frame","message":"RPC frame must end with LF"}}`。
症状是客户端永远卡在"握手"。修复方式：让 **writer 是唯一写 stdin 的地方，统一补 LF**，
不要在每一处调用点手工拼。

### 4.3 增量 reader 与严格 DTO

- stdout 用**增量** JSONL reader：半帧 EOF、超长帧、无效 UTF-8 都要是明确错误，
  不能静默跳过（`onemoreGui/src-tauri/src/rpc/reader.rs`）；
- 服务端对所有 object 用 `deny_unknown_fields`，你收到的帧如果解析失败，
  按 transport error 处理并终止连接——**不要宽容**；
- 未知 tag / 重复 request ID / malformed JSON 都是协议错误，不是可恢复噪音。

### 4.4 Request 关联：pending map

- 每个 request 由客户端分配唯一 ID（如 `req-1`），服务端**每个 request 恰好一个 response**；
- 用一个 `HashMap<id, sender>` 把 response 送回等待方；乱序到达是常态，不要假设顺序；
- 重复/未知 ID 的 response → 明确 transport error（`duplicate_or_unknown_response`）。

### 4.5 stderr 与诊断分离

- **stdout 只允许协议帧**；日志、panic、内部错误全走 stderr；
- stderr 单独一个线程读，有界保留最近 N 行（如 400），进诊断面板；
- transport error、进程退出码、stderr 行用**客户端自己的封闭 DTO**，不要伪装成 `SessionEvent`。

### 4.6 安全 shutdown 与 fail closed

- stdin EOF 等价于 transport shutdown：服务端会取消运行、deny 审批、安全退出；
- 审批在断连时**一律按 deny 收尾**（fail closed），客户端不要"断连自动允许"；
- 客户端收到 `settled` 不代表连接可以关——显式发 `shutdown` 或关 stdin 并等退出。

---

## 5. 前端状态投影：snapshot 权威，progress 瞬时

GUI 最容易犯的错：拿 progress 事件自己攒 transcript，攒乱了。

正确姿势（Gui-rpc-example 的 reducer 就是这么写的）：

```
snapshot（权威）      progress（瞬时增量）
    │                     │
    ├─ transcript         ├─ assistant_delta → 流式块（按 message_id 归并）
    ├─ phase              ├─ tool_started/updated/finished → 工具行
    ├─ usage              ├─ approval_requested → 审批 modal
    ├─ queues             └─ ...
    └─ pending_approval
```

三条规则：

1. **progress 可以即时渲染，但 `session_snapshot` 到达时，用它整体纠正本地组装**；
   delta、工具、terminal、settled 的重复/乱序输入不得破坏 UI。
2. **snapshot 到达时清理已提交的 live 增量**，只保留还没被提交的流式消息/工具调用。
3. `settled` 只用于"回到稳定边界"的展示，不能替代具体命令的 terminal。

Gui-rpc-example 的 reducer 有完整 Vitest fixture：流式累积、snapshot 纠正、
未提交增量跨 snapshot 保留、重复 terminal 不炸。

---

## 6. 运行中控制

| 动作 | 命令 | 注意 |
|---|---|---|
| 打断当前方向 | `steer {text}` | 在完整工具批后注入 |
| 排队追加工作 | `follow_up {text}` | 当前任务将停止时执行 |
| 立即取消 | `abort` | 等每个 accepted 命令的 terminal，不要立刻关窗口 |
| 权威校正 | `get_snapshot` | 不修改运行状态，随时可发 |
| 审批 | `approval_response {request_id, decision}` | Once / Session / Deny |

snapshot 的 `queues.steering` / `queues.follow_up` 是队列的权威视图；
不要在前端乐观地自己拼队列。

---

## 7. 常见错误速查

| 症状 | 原因 | 修复 |
|---|---|---|
| 卡在握手 / `unterminated_frame: RPC frame must end with LF` | 发送帧没以 LF 结尾 | writer 统一补 `\n` |
| `invalid_handshake: first frame must be hello` | 第一帧发了 request | 严格先 hello |
| `version_mismatch` | hello 的 version ≠ 1 | 精确版本协商，不降级 |
| `busy` | running 时发 prompt | 改用 steer / follow_up |
| `duplicate_request_id` | 复用了 request ID | 单调递增，不重用 |
| 进程残留、审批静默通过 | 退出没关 stdin / 断连自动允许 | EOF → 等退出 → 超时 kill；fail closed |
| transcript 乱/重复 | 前端拿 progress 当权威 | snapshot 校正，progress 只做瞬时增量 |
| 把 stderr 当协议解析 | stdout 污染 | stdout 只读协议帧，stderr 独立进诊断 |

---

## 8. 最小集成 Checklist

- [ ] 不经 shell 启动子进程，`current_dir` = workspace
- [ ] 启动后立即接管三路流，先发 `hello`（带 LF）
- [ ] 等待 hello 成功，校验 version == 1，拒绝降级
- [ ] stdout 用增量 JSONL reader，stderr 独立有界保留
- [ ] 单 writer task 独占 stdin，统一补 LF
- [ ] request ID 唯一，pending map 关联 response
- [ ] mutation 只信 `command_id`，完成看 `command_finished`，稳定看 `settled`
- [ ] 前端以 snapshot 为权威，progress 为瞬时增量，到达时纠正
- [ ] 断连/退出：EOF → 等退出 → 超时 kill；审批 fail closed
- [ ] 不缓存/显示 provider raw、thinking raw、工具原始参数、API key

---

## 9. 参考代码索引（onemoreGui）

| 职责 | 文件 |
|---|---|
| 协议 view types（TS discriminated unions） | `onemoreGui/src/rpc/protocol.ts` |
| Tauri invoke / 事件订阅（唯一 transport 入口） | `onemoreGui/src/rpc/client.ts` |
| snapshot 权威 + progress 增量 reducer | `onemoreGui/src/rpc/reducer.ts` |
| 状态 store（连接/phase/queue/approval/指标） | `onemoreGui/src/app/store.ts` |
| 子进程生命周期 + pending map + 安全 shutdown | `onemoreGui/src-tauri/src/rpc/process.rs` |
| LF 单 writer | `onemoreGui/src-tauri/src/rpc/writer.rs` |
| 增量 JSONL reader（半帧/超长/UTF-8） | `onemoreGui/src-tauri/src/rpc/reader.rs` |
| 严格入站 DTO | `onemoreGui/src-tauri/src/rpc/types.rs` |
| GUI 事件 DTO（封闭，不伪装协议事件） | `onemoreGui/src-tauri/src/rpc/events.rs` |
| 审批 modal / transcript / composer | `onemoreGui/src/components/*` |

---

## 附：Windows 上手动实验的提示

- 上面的 `printf` 示例在 Git Bash 下可用；PowerShell 请改用
  `"{\"type\":\"hello\",\"version\":1}" | onemore --rpc`（注意编码为 UTF-8）。
- 想看完整 prompt 流时，先让窗口保持打开：管道输入 EOF 会立即触发安全关闭，
  事件只输出到 EOF 前——这本身就是"EOF = shutdown"的直观演示。
