# Runtime 结构与弱 Harness 边界

Onemore 的目标不是把所有能力塞进 Agent，而是让核心 loop 只协调消息、Provider 和工具，
由宿主选择 storage、permissions、skills、hooks 与 UI。当前实现已经建立可组合入口，但还
处于从完整 CLI runtime 向 Pi 风格弱 harness 迁移的中间阶段。

## 目录职责

```text
src/runtime.rs
  公开 Agent / RetryPolicy / RuntimeHandle，并承载线程宿主
src/runtime/
  agent_loop.rs       模型循环、steering/follow-up、provider terminal 与 retry
  builder.rs          CLI 默认装配和宿主组件注入
  commands.rs         命令分发、模型切换、事实提交、上下文投影与预算
  compaction.rs       纯文本压缩请求和 transcript 渲染
  tool_execution.rs   schema preflight、hooks、permissions、并发、取消与超时
  tests.rs            跨域测试夹具
  tests/              按 builder / permissions / planning / history / queues / concurrency 分组
```

拆分按行为所有权进行，不按任意行数切片。核心生产文件保持在约 160 到 540 行；测试也按
同一职责边界组织，避免重新形成单个巨型模块。

## 可替换组件

`AgentBuilder` 当前允许宿主替换或追加：

- `ProviderFactory`：启动和后续 provider/model 切换共用同一个 factory。
- `ToolRegistry`：可以使用空 registry 或完全由宿主提供工具。
- `ContextProvider` 列表：可完整替换默认 instructions/workspace/skills，也可追加片段。
- `HookRegistry`、`PermissionManager` 与 `RetryPolicy`。
- data directory：测试和嵌入场景无需接触平台默认数据目录。

没有注入的组件继续采用 CLI 默认值，因此 `Agent::new` 与
`Agent::new_with_data_dir` 的行为保持不变。

## 稳定前缀

Builder 在一次 Agent 构造时冻结 context provider 顺序、skill catalog 与 tool registry；
工具声明在请求前按名称排序。动态计划、工具结果、notice 和 compaction 都进入事实/消息
层，不修改稳定 system prefix。Provider 再根据规范化后的 system、tools 与模型选择生成
稳定 `prompt_cache_key`。

宿主若替换 context 或 tools，应在一个 Agent 生命周期内保持其稳定；需要接受新目录时应
创建新 Agent，而不是在每轮请求前重扫。

## 仍需解除的耦合

当前 `Agent` 仍直接持有 `SessionManager`、`WorkspacePreferences` 和冻结的
`SkillCatalog`。也就是说，builder 是 stateful harness 的组合边界，还不是像 Pi
`agent-loop` 那样只依赖消息、model、tools 和 callbacks 的纯核心。

下一层拆分应先定义被生产路径真实使用的 session/fact store 接口，再把 persistence、
planning reminder、compaction command 和 skill discovery 留在默认 harness；不能复制一套
未被 CLI 使用的第二 loop。完成后，内存宿主应能在不创建 SQLite 或 skills 目录的情况下
运行同一套核心 loop。
