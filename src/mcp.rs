//! # MCP 客户端(stdio,dual-era)
//!
//! Onemore 作为 MCP host,把外部 stdio server 的工具接进 [`crate::tools`] 平台。
//! 设计与范围见 `docs/planning/mcp-client-plan.md`,要点:
//!
//! - 只支持 stdio transport 与 tools 能力;resources/prompts/sampling/HTTP 不做。
//! - dual-era:`server/discover` 探测 2026-07-28(modern,无握手),失败回退
//!   legacy `initialize`(2025-06-18 基线)。
//! - server 是不受信的第三方插件进程:声明与输出都经清洗限额,权限固定走
//!   "未声明目标的副作用 → Ask",annotations 不参与任何决策。
//! - registry 在一个 capability epoch 内冻结;server 死亡不自动重启不自动重试,
//!   恢复靠 `/reload`。
//!
//! 手写同步实现,无异步运行时:写端互斥、每 server 一个 reader 线程、
//! 条件变量等待,取消靠标志轮询与杀进程,与项目其余部分同构。

use std::sync::Arc;

pub(crate) mod client;
pub(crate) mod import;
pub(crate) mod protocol;
pub(crate) mod transport;

use client::{ConnectResult, McpClient};
use import::{import_tools, ImportedTool};

use crate::config::McpServerConfig;

/// 一个待注册工具:代理层(`tools::mcp_proxy`)据此构造 `McpTool`。
/// mcp 模块自身不依赖 tools,装配由 runtime builder 完成。
pub(crate) struct McpToolSeed {
    pub client: Arc<McpClient>,
    pub tool: ImportedTool,
    /// 来自该 server 配置的收紧开关:true 时逐次审批(Once),不可 session 授权。
    pub always_ask: bool,
}

pub(crate) struct McpStartOutcome {
    pub host: McpHost,
    pub seeds: Vec<McpToolSeed>,
    /// 启动阶段的全部人类可读事件:就绪、降级、拒绝,逐条发 Notice。
    pub notices: Vec<String>,
}

enum HostedOutcome {
    Ready {
        client: Arc<McpClient>,
        tools: usize,
        instructions: Option<String>,
    },
    Failed {
        error: String,
    },
}

struct HostedServer {
    name: String,
    outcome: HostedOutcome,
}

/// 全部已配置 server 的持有者。随 Agent 生命周期存在,`/reload` 整体重建
/// (唯一的 capability epoch 边界)。
pub struct McpHost {
    servers: Vec<HostedServer>,
}

impl McpHost {
    pub(crate) fn empty() -> McpHost {
        McpHost {
            servers: Vec::new(),
        }
    }

    /// 并行启动全部 enabled server;单个失败只降级自身,绝不阻止 Agent 构建。
    /// `name_taken` 查询与现有 registry 的公开名冲突。
    pub(crate) fn start(
        configs: &[McpServerConfig],
        name_taken: &dyn Fn(&str) -> bool,
    ) -> McpStartOutcome {
        let enabled: Vec<&McpServerConfig> =
            configs.iter().filter(|config| config.enabled).collect();
        let mut results: Vec<Result<ConnectResult, String>> = Vec::new();
        std::thread::scope(|scope| {
            let handles: Vec<_> = enabled
                .iter()
                .map(|config| scope.spawn(move || McpClient::connect(config)))
                .collect();
            results = handles
                .into_iter()
                .map(|handle| {
                    handle
                        .join()
                        .unwrap_or_else(|_| Err("连接线程 panic".into()))
                })
                .collect();
        });

        let mut servers = Vec::new();
        let mut seeds: Vec<McpToolSeed> = Vec::new();
        let mut notices = Vec::new();
        for (config, result) in enabled.iter().zip(results) {
            match result {
                Ok(connected) => {
                    let client = Arc::new(connected.client);
                    for warning in &connected.warnings {
                        notices.push(format!("MCP server {}: {}", config.name, warning));
                    }
                    let mut taken = |name: &str| {
                        name_taken(name) || seeds.iter().any(|seed| seed.tool.public_name == name)
                    };
                    let report = import_tools(
                        &config.name,
                        connected.tools,
                        config.include_tools.as_deref(),
                        &config.exclude_tools,
                        &mut taken,
                    );
                    for warning in report.warnings {
                        notices.push(format!("MCP {}", warning));
                    }
                    let count = report.tools.len();
                    notices.push(format!(
                        "MCP server {} 就绪({},{} 个工具{})",
                        config.name,
                        client.era_label(),
                        count,
                        client
                            .server_label
                            .as_deref()
                            .map(|label| format!(",{label}"))
                            .unwrap_or_default()
                    ));
                    seeds.extend(report.tools.into_iter().map(|tool| McpToolSeed {
                        client: Arc::clone(&client),
                        tool,
                        always_ask: config.always_ask,
                    }));
                    servers.push(HostedServer {
                        name: config.name.clone(),
                        outcome: HostedOutcome::Ready {
                            client,
                            tools: count,
                            instructions: connected.instructions,
                        },
                    });
                }
                Err(error) => {
                    notices.push(format!(
                        "MCP server {} 不可用,已跳过: {}",
                        config.name, error
                    ));
                    servers.push(HostedServer {
                        name: config.name.clone(),
                        outcome: HostedOutcome::Failed { error },
                    });
                }
            }
        }
        McpStartOutcome {
            host: McpHost { servers },
            seeds,
            notices,
        }
    }

    /// `/mcp` 的状态报告,每 server 一段。
    pub(crate) fn status_lines(&self) -> Vec<String> {
        if self.servers.is_empty() {
            return vec!["未配置 MCP server;在 config.toml 的 [[mcp_servers]] 中添加".into()];
        }
        let mut lines = Vec::new();
        for server in &self.servers {
            match &server.outcome {
                HostedOutcome::Ready {
                    client,
                    tools,
                    instructions,
                } => {
                    let mut line =
                        format!("{}: {}({} 个工具", server.name, client.era_label(), tools);
                    if let Some(label) = &client.server_label {
                        line.push_str(&format!(",{label}"));
                    }
                    line.push(')');
                    if let Some(reason) = client.failure() {
                        line.push_str(&format!(" — 已故障: {reason},/reload 可恢复"));
                    }
                    lines.push(line);
                    if let Some(instructions) = instructions {
                        lines.push(format!(
                            "  instructions: {}",
                            crate::util::ellipsis(instructions, 200)
                        ));
                    }
                    for stderr in client.stderr_tail().iter().rev().take(3).rev() {
                        lines.push(format!("  stderr: {}", crate::util::ellipsis(stderr, 200)));
                    }
                }
                HostedOutcome::Failed { error } => {
                    lines.push(format!("{}: 启动失败 — {}", server.name, error));
                }
            }
        }
        lines
    }

    /// 取走各 server 的 list_changed 标志,返回需要提醒的 server 名。
    pub(crate) fn take_list_changed(&self) -> Vec<String> {
        self.servers
            .iter()
            .filter_map(|server| match &server.outcome {
                HostedOutcome::Ready { client, .. } if client.take_list_changed() => {
                    Some(server.name.clone())
                }
                _ => None,
            })
            .collect()
    }

    /// 关停全部 server(关 stdin → 有界等待 → 强杀进程树)。幂等。
    pub(crate) fn shutdown(&self) {
        for server in &self.servers {
            if let HostedOutcome::Ready { client, .. } = &server.outcome {
                client.shutdown();
            }
        }
    }
}

impl Drop for McpHost {
    fn drop(&mut self) {
        self.shutdown();
    }
}

/// 标准字母表 base64 解码(接受无填充与内嵌空白)。MCP image 内容块专用,
/// 不值得为此引入依赖。
pub(crate) fn decode_base64(input: &str) -> Result<Vec<u8>, String> {
    fn sextet(byte: u8) -> Option<u32> {
        match byte {
            b'A'..=b'Z' => Some(u32::from(byte - b'A')),
            b'a'..=b'z' => Some(u32::from(byte - b'a') + 26),
            b'0'..=b'9' => Some(u32::from(byte - b'0') + 52),
            b'+' => Some(62),
            b'/' => Some(63),
            _ => None,
        }
    }
    let mut out = Vec::with_capacity(input.len() / 4 * 3);
    let mut acc: u32 = 0;
    let mut bits: u32 = 0;
    let mut padded = false;
    for &byte in input.as_bytes() {
        match byte {
            b'\r' | b'\n' | b' ' | b'\t' => continue,
            b'=' => {
                padded = true;
                continue;
            }
            _ => {}
        }
        if padded {
            return Err("填充符之后出现数据".into());
        }
        let Some(value) = sextet(byte) else {
            return Err(format!("非法 base64 字符 0x{byte:02x}"));
        };
        acc = (acc << 6) | value;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits) as u8);
        }
    }
    if bits >= 6 {
        return Err("base64 数据长度非法".into());
    }
    Ok(out)
}

#[cfg(test)]
mod tests;
