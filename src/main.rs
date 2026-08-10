//! 可执行入口:参数解析、配置引导、装配 Agent,然后二选一:
//! - 默认:Runtime 进工作线程 + TUI 前端;
//! - `--once "提示词"`:headless 单轮模式,直接在当前线程跑完打印退出。
//! - `--rpc`:严格 JSONL stdin/stdout 子进程协议。
//!
//! headless 模式的存在不只是为了方便调试:它和 TUI 消费**同一条事件流**,
//! 证明 Runtime 与前端确实解耦(未来接 GUI/Web 就是再写一个消费者)。

use std::io::Write;
use std::path::PathBuf;

use anyhow::{bail, Result};

use onemore::config::{Config, EXAMPLE_CONFIG};
use onemore::runtime::Agent;
use onemore::sdk::{
    ApprovalDecisionView, ApprovalResponseView, CommandStatus, ProgressEvent, SessionEvent,
};
use onemore::storage::AppPaths;
use onemore::workspace::Workspace;

const VERSION: &str = env!("CARGO_PKG_VERSION");

const HELP: &str = "Onemore —— 可靠、实用的 Coding Agent

用法:
  onemore                     启动 TUI
  onemore --once <提示词...>   无界面跑一轮(方便调试/脚本化)
  onemore --rpc               启动 JSONL RPC

选项:
  -v, --version          显示版本
  -c, --config <路径>    配置文件(默认平台数据目录/config.toml,不存在会生成模板)
  -p, --provider <名字>  覆盖 [agent].provider
  -h, --help             显示本帮助
";

struct Args {
    config: PathBuf,
    provider: Option<String>,
    once: Option<String>,
    rpc: bool,
    version: bool,
}

fn parse_args(default_config: PathBuf) -> Result<Option<Args>> {
    let mut config = default_config;
    let mut provider = None;
    let mut once = None;
    let mut rpc = false;
    let mut version = false;
    let mut it = std::env::args().skip(1);
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "-h" | "--help" => return Ok(None),
            "-v" | "--version" => version = true,
            "-c" | "--config" => {
                config = PathBuf::from(
                    it.next()
                        .ok_or_else(|| anyhow::anyhow!("--config 缺参数"))?,
                )
            }
            "-p" | "--provider" => {
                provider = Some(
                    it.next()
                        .ok_or_else(|| anyhow::anyhow!("--provider 缺参数"))?,
                )
            }
            "--once" => {
                // 其后所有参数拼成提示词
                let rest: Vec<String> = it.by_ref().collect();
                if rest.is_empty() {
                    bail!("--once 需要提示词");
                }
                once = Some(rest.join(" "));
            }
            "--rpc" => rpc = true,
            other => bail!("未知参数 {:?},-h 查看用法", other),
        }
    }
    if rpc && once.is_some() {
        bail!("--rpc 与 --once 不能同时使用");
    }
    Ok(Some(Args {
        config,
        provider,
        once,
        rpc,
        version,
    }))
}

fn main() -> Result<()> {
    let paths = AppPaths::discover()?;
    let Some(args) = parse_args(paths.config.clone())? else {
        print!("{}", HELP);
        return Ok(());
    };
    if args.version {
        println!("onemore {}", VERSION);
        return Ok(());
    }
    paths.ensure()?;

    // 首次运行:生成配置模板后退出,提示用户填 key
    if !args.config.exists() {
        if let Some(parent) = args.config.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(&args.config, EXAMPLE_CONFIG)?;
        let message = format!(
            "已生成 Onemore 配置模板 {}。\n填好 API key(或设置对应环境变量)后重新运行 onemore。",
            args.config.display()
        );
        if args.rpc {
            eprintln!("{}", message);
        } else {
            println!("{}", message);
        }
        return Ok(());
    }

    let mut cfg = Config::load(&args.config)?;
    if let Some(p) = args.provider {
        // 提前校验,给出可用列表
        cfg.resolve_provider(&p)?;
        cfg.active_provider = p;
    }

    let workspace = Workspace::new(std::env::current_dir()?);
    let agent = Agent::new(cfg, workspace)?;

    match (args.rpc, args.once) {
        (true, None) => onemore::rpc::run(agent),
        (false, Some(prompt)) => run_once(agent, prompt),
        (false, None) => onemore::tui::run(onemore::sdk::spawn_session(agent)),
        (true, Some(_)) => unreachable!("argument parser rejects --rpc with --once"),
    }
}

/// headless 前端:事件流直接打到终端。
/// 助手正文走 stdout(方便管道),过程信息走 stderr。
fn run_once(agent: Agent, prompt: String) -> Result<()> {
    let mut session = onemore::sdk::spawn_session(agent);
    let receipt = session.controller.prompt(prompt)?;
    let mut failed = false;
    let mut streamed = false;
    loop {
        match session.events.recv()? {
            SessionEvent::Progress { progress } => match progress {
                ProgressEvent::UserMessage { text } => eprintln!("❯ {}", text),
                ProgressEvent::CompactionStarted {
                    trigger,
                    estimated_tokens,
                    available_tokens,
                    ..
                } => {
                    let source = match trigger {
                        onemore::sdk::CompactionTriggerView::Automatic => "自动",
                        onemore::sdk::CompactionTriggerView::Manual => "手动",
                    };
                    match available_tokens {
                        Some(available) => eprintln!(
                            "◐ 正在{}压缩历史(约 {} / {} tokens)",
                            source,
                            onemore::util::fmt_tokens(estimated_tokens),
                            onemore::util::fmt_tokens(available)
                        ),
                        None => eprintln!(
                            "◐ 正在{}压缩历史(约 {} tokens)",
                            source,
                            onemore::util::fmt_tokens(estimated_tokens)
                        ),
                    }
                }
                ProgressEvent::CompactionFinished {
                    tokens_before,
                    summary_chars,
                    retained_messages,
                    ..
                } => eprintln!(
                    "✓ 压缩完成:压缩前约 {} tokens,摘要 {} 字符,保留 {} 条消息",
                    onemore::util::fmt_tokens(tokens_before),
                    summary_chars,
                    retained_messages
                ),
                ProgressEvent::CompactionFailed {
                    error,
                    cancelled,
                    history_changed,
                    ..
                } => eprintln!(
                    "{} 压缩{}:{}({})",
                    if cancelled { "■" } else { "✖" },
                    if cancelled { "已取消" } else { "失败" },
                    error,
                    if history_changed {
                        "历史已改变"
                    } else {
                        "历史未改变"
                    }
                ),
                ProgressEvent::AssistantDelta { kind, delta, .. } if kind == "text" => {
                    streamed = true;
                    print!("{}", delta);
                    let _ = std::io::stdout().flush();
                }
                ProgressEvent::AssistantFinished { text: _, .. } if streamed => {
                    println!();
                    streamed = false;
                }
                ProgressEvent::AssistantFinished { text, .. } => println!("{}", text),
                ProgressEvent::ToolStarted { name, summary, .. } => {
                    eprintln!("● {}({})", name, summary);
                }
                ProgressEvent::ToolUpdated { output, .. } => {
                    eprintln!("  … {}", onemore::util::ellipsis(&output.summary, 120));
                }
                ProgressEvent::ToolFinished { output, error, .. } => {
                    let shown = output.content.as_str();
                    let first = shown.lines().next().unwrap_or("");
                    let more = shown.lines().count().saturating_sub(1);
                    eprintln!(
                        "  └ {}{}{}",
                        if error.is_some() { "✖ " } else { "" },
                        onemore::util::ellipsis(first, 120),
                        if more > 0 {
                            format!(" (+{} 行)", more)
                        } else {
                            String::new()
                        }
                    );
                }
                ProgressEvent::ApprovalRequested { request } => {
                    eprintln!(
                        "? {}({}) 需要审批: {}",
                        request.tool, request.summary, request.reason
                    );
                    session
                        .controller
                        .respond_to_approval(ApprovalResponseView {
                            request_id: request.request_id,
                            decision: ApprovalDecisionView::Deny,
                        })?;
                }
                ProgressEvent::ApprovalResolved { allowed, .. } => {
                    eprintln!("  {}", if allowed { "已允许" } else { "未允许" });
                }
                ProgressEvent::PlanUpdated { plan } => {
                    eprintln!("· 计划 #{}", plan.revision);
                    if let Some(explanation) = plan.explanation {
                        eprintln!("  {}", explanation);
                    }
                    for item in plan.items {
                        eprintln!("  [{}] {}: {}", item.status.as_str(), item.id, item.text);
                    }
                }
                ProgressEvent::SkillsDiscovered { skills, warnings } => {
                    if !skills.is_empty() {
                        eprintln!("· 已发现 {} 个可用技能", skills.len());
                    }
                    for warning in warnings {
                        eprintln!("· 技能发现警告: {}", warning);
                    }
                }
                ProgressEvent::Usage { usage } => {
                    if usage.cache_read_tokens.is_some() || usage.cache_write_tokens.is_some() {
                        eprintln!(
                            "· 用量 ↑{} ↓{} · cache read {} write {}",
                            usage.input_tokens,
                            usage.output_tokens,
                            usage.cache_read_tokens.unwrap_or(0),
                            usage.cache_write_tokens.unwrap_or(0)
                        );
                    } else {
                        eprintln!("· 用量 ↑{} ↓{}", usage.input_tokens, usage.output_tokens);
                    }
                }
                ProgressEvent::Notice { text, .. } => eprintln!("· {}", text),
                ProgressEvent::Error { error } => {
                    failed = true;
                    eprintln!("✖ {}", error.message);
                }
                _ => {}
            },
            SessionEvent::CommandFinished {
                command_id, status, ..
            } if command_id == receipt.command_id => {
                failed |= status == CommandStatus::Failed;
            }
            SessionEvent::Settled { .. } => break,
            SessionEvent::SessionSnapshot { .. } | SessionEvent::CommandFinished { .. } => {}
        }
    }
    let _ = session.controller.shutdown();
    if failed {
        bail!("本轮出现错误");
    }
    Ok(())
}
