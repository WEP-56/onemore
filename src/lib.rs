//! Onemore:面向可靠性与实用性的 Coding Agent。
//!
//! 阅读顺序建议(每个模块头部都有详细注释):
//! 1. [`message`] —— 统一消息模型,全项目的"通用语言"
//! 2. [`workspace`] + [`tools`] —— 工具系统
//! 3. [`context`] —— 上下文组装
//! 4. [`provider`] —— 两种 API 的流式适配
//! 5. [`agent_loop`] + [`harness`] + [`runtime`] —— 核心循环、可注入状态与命令
//! 6. [`storage`] —— 默认的数据路径、SQLite session 与 JSON 偏好适配器
//! 7. [`tui`] —— 只是事件流的一个消费者
//!
//! Runtime 模块边界见 `docs/runtime-architecture.md`，其余设计文档见 `docs/`。

pub mod agent_loop;
pub mod compaction;
pub mod config;
pub mod context;
pub mod event;
pub mod harness;
pub mod hooks;
pub mod mcp;
pub mod message;
pub mod permission;
pub mod plan;
pub(crate) mod process;
pub mod provider;
pub mod rpc;
pub mod runtime;
pub mod sdk;
pub mod session;
pub mod skills;
pub mod storage;
pub mod tools;
pub mod tui;
pub mod util;
pub mod web;
pub mod workspace;
