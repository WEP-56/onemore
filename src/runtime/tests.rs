use super::*;
use crate::config::{
    ApiKind, Config, ProviderProfile, ProviderSettings, ReasoningEffortPolicy,
    DEFAULT_REASONING_EFFORT,
};
use crate::context::{ContextProvider, PromptContext};
use crate::hooks::{Hook, PreToolUseContext, PreToolUseHookResult, StopContext, StopHookResult};
use crate::message::{Block, ChatMessage, Role, StopReason, Usage};
use crate::permission::{
    ApprovalDecision, ApprovalResponse, ApprovalScope, PermissionDecision, PermissionManager,
    PermissionRule, PermissionRules,
};
use crate::plan::{reduce_plan, PlanItem, PlanSnapshot, PlanStatus};
use crate::provider::{FailedTurn, Provider, ProviderError, ProviderEvent, StreamTerminal};
use crate::session::{project_model_messages, SessionEntry, SessionEntryPayload};
use crate::tools::{
    Tool, ToolCapabilities, ToolContext, ToolError, ToolErrorCode, ToolOutput, ToolPermissionSpec,
    ToolRegistry, ToolSpec,
};
use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::atomic::AtomicUsize;
use std::sync::Mutex;

enum ScriptStep {
    Output(crate::provider::TurnOutput),
    Error(ProviderError),
    Cancel,
}

struct ScriptedProvider {
    steps: Mutex<VecDeque<ScriptStep>>,
    /// 每次 stream_turn 收到的消息视图,供测试断言"模型看到了什么"。
    prompts: Arc<Mutex<Vec<Vec<ChatMessage>>>>,
    model: String,
}

struct RuntimeProgressTool;

struct HostContext;

impl ContextProvider for HostContext {
    fn name(&self) -> &'static str {
        "host"
    }

    fn contribute(&self, prompt: &mut PromptContext, _ws: &Workspace) {
        prompt.system_sections.push("host context".into());
    }
}

struct CountedTool {
    executions: Arc<AtomicUsize>,
    capabilities: ToolCapabilities,
    permission: ToolPermissionSpec,
}

impl Tool for CountedTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "counted".into(),
            description: "counted test tool".into(),
            schema: serde_json::json!({
                "type": "object",
                "additionalProperties": false,
                "properties": { "path": { "type": "string" } },
                "required": ["path"]
            }),
            capabilities: self.capabilities,
            permission: self.permission.clone(),
        }
    }

    fn execute(
        &self,
        _args: &serde_json::Value,
        _ctx: &mut ToolContext<'_>,
    ) -> Result<ToolOutput, ToolError> {
        self.executions.fetch_add(1, Ordering::Relaxed);
        Ok(ToolOutput::text("executed"))
    }
}

struct ReplacePathHook {
    calls: Arc<AtomicUsize>,
    replacement: String,
}

struct PreventStopHook {
    calls: Arc<AtomicUsize>,
}

impl Hook for PreventStopHook {
    fn name(&self) -> &str {
        "verify_once"
    }

    fn stop(&mut self, _ctx: &StopContext<'_>) -> anyhow::Result<StopHookResult> {
        self.calls.fetch_add(1, Ordering::Relaxed);
        Ok(StopHookResult::PreventStop("run verification".into()))
    }
}

impl Hook for ReplacePathHook {
    fn name(&self) -> &str {
        "replace_path"
    }

    fn pre_tool_use(
        &mut self,
        _ctx: &PreToolUseContext<'_>,
    ) -> anyhow::Result<PreToolUseHookResult> {
        self.calls.fetch_add(1, Ordering::Relaxed);
        Ok(PreToolUseHookResult::ReplaceArguments(
            serde_json::json!({ "path": self.replacement }),
        ))
    }
}

impl Tool for RuntimeProgressTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "runtime_progress".into(),
            description: "runtime progress test tool".into(),
            schema: serde_json::json!({ "type": "object" }),
            capabilities: ToolCapabilities::READ_ONLY,
            permission: ToolPermissionSpec::default(),
        }
    }

    fn execute(
        &self,
        _args: &serde_json::Value,
        ctx: &mut ToolContext<'_>,
    ) -> Result<ToolOutput, ToolError> {
        ctx.report_progress(ToolOutput {
            model_text: "halfway".into(),
            ui_summary: Some("1/2".into()),
            details: Some(serde_json::json!({ "completed": 1, "total": 2 })),
        });
        Ok(ToolOutput::text("done"))
    }
}

impl ScriptedProvider {
    fn new(steps: Vec<ScriptStep>) -> Self {
        ScriptedProvider {
            steps: Mutex::new(steps.into()),
            prompts: Arc::new(Mutex::new(Vec::new())),
            model: "scripted".into(),
        }
    }

    fn with_prompt_log(steps: Vec<ScriptStep>, prompts: Arc<Mutex<Vec<Vec<ChatMessage>>>>) -> Self {
        ScriptedProvider {
            steps: Mutex::new(steps.into()),
            prompts,
            model: "scripted".into(),
        }
    }
}

impl Provider for ScriptedProvider {
    fn label(&self) -> String {
        format!("scripted / {}", self.model)
    }

    fn model(&self) -> &str {
        &self.model
    }

    fn stream_turn(
        &self,
        prompt: &PromptContext,
        _tools: &[ToolSpec],
        _on_event: &mut dyn FnMut(ProviderEvent),
        _cancel: &AtomicBool,
    ) -> StreamTerminal {
        self.prompts.lock().unwrap().push(prompt.messages.clone());
        match self
            .steps
            .lock()
            .unwrap()
            .pop_front()
            .expect("script exhausted")
        {
            ScriptStep::Output(output) => StreamTerminal::Done(output),
            ScriptStep::Error(error) => StreamTerminal::Error(FailedTurn::from_error(error)),
            ScriptStep::Cancel => StreamTerminal::Aborted(FailedTurn::aborted()),
        }
    }
}

fn config(root: &std::path::Path) -> Config {
    config_with_max_turns(root, 2)
}

fn config_with_max_turns(root: &std::path::Path, max_turns: u32) -> Config {
    let path = root.join("config.toml");
    std::fs::write(
        &path,
        format!(
            r#"
[agent]
provider = "mock"
max_turns = {}

[providers.mock]
api = "responses"
base_url = "http://127.0.0.1:1"
model = "scripted"
api_key = ""
"#,
            max_turns
        ),
    )
    .unwrap();
    Config::load(&path).unwrap()
}

fn temp_root(name: &str) -> PathBuf {
    let root = std::env::temp_dir().join(format!(
        "onemore-runtime-{}-{}-{}",
        name,
        std::process::id(),
        uuid::Uuid::new_v4()
    ));
    std::fs::create_dir_all(&root).unwrap();
    root
}

fn multi_model_config(root: &std::path::Path) -> Config {
    let path = root.join("multi-model-config.toml");
    std::fs::write(
        &path,
        r#"
[agent]
provider = "mock"

[providers.mock]
api = "responses"
profile = "openai"
base_url = "http://127.0.0.1:1"
api_key = ""
default_model = "small"

[providers.mock.models.small]
context_window = 32000
max_tokens = 4096

[providers.mock.models.large]
context_window = 200000
max_tokens = 32000
efforts = ["low", "medium", "high"]
"#,
    )
    .unwrap();
    Config::load(&path).unwrap()
}

fn output(message: ChatMessage, stop: StopReason) -> crate::provider::TurnOutput {
    crate::provider::TurnOutput {
        message,
        usage: Usage::default(),
        stop,
        prompt_fingerprint: None,
    }
}

fn tool_turn(calls: Vec<(&str, serde_json::Value)>) -> crate::provider::TurnOutput {
    output(
        ChatMessage {
            role: Role::Assistant,
            blocks: calls
                .into_iter()
                .enumerate()
                .map(|(index, (name, input))| Block::ToolUse {
                    id: format!("call-{}", index + 1),
                    name: name.into(),
                    input,
                })
                .collect(),
        },
        StopReason::ToolUse,
    )
}

fn update_plan_turn(
    expected_revision: u64,
    items: serde_json::Value,
) -> crate::provider::TurnOutput {
    tool_turn(vec![(
        "update_plan",
        serde_json::json!({
            "expected_revision": expected_revision,
            "plan": items,
        }),
    )])
}

fn install_counted_tool(
    agent: &mut Agent,
    capabilities: ToolCapabilities,
    permission: ToolPermissionSpec,
) -> Arc<AtomicUsize> {
    let executions = Arc::new(AtomicUsize::new(0));
    agent.tools = ToolRegistry::new(vec![Box::new(CountedTool {
        executions: Arc::clone(&executions),
        capabilities,
        permission,
    })]);
    executions
}

mod builder;
mod compaction;
mod concurrency;
mod history;
mod permissions;
mod planning;
mod queues;
