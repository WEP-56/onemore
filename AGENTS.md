# Onemore 开发约束

- 保持中文交流
- 保持弱 harness、极简公开接口和唯一生产路径，不复制备用实现。
- 不考虑旧数据或旧配置迁移，可以直接收敛最终接口。
- 不得破坏 ToolUse/ToolResult 配对、原子提交和 append-only 事实日志。
- 按职责拆分模块，控制单文件规模，保持目录和命名可读。
- 优先复用现有抽象；新增接口必须由 CLI 或嵌入生产路径真实使用。
- 工作时参考 `example/pi`，必要时对照 `example/grok-build`，避免架构偏离。
- 改动完成后运行完整测试、Clippy、fmt、rustdoc 和 `git diff --check`。
