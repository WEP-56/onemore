use super::*;

// ---- 阶段 6:受控并发、超时与资源锁 ----

/// 可配置执行模式的睡眠工具:分片睡眠,轮询组合取消标志。
struct SleepTool {
    name: &'static str,
    millis: u64,
    mode: crate::tools::ToolExecutionMode,
}

impl Tool for SleepTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: self.name.into(),
            description: "sleeps".into(),
            schema: serde_json::json!({ "type": "object" }),
            capabilities: ToolCapabilities {
                read_only: true,
                destructive: false,
                execution_mode: self.mode,
                supports_background: false,
            },
            permission: ToolPermissionSpec::default(),
        }
    }

    fn execute(
        &self,
        _args: &serde_json::Value,
        ctx: &mut ToolContext<'_>,
    ) -> Result<ToolOutput, ToolError> {
        let deadline = std::time::Instant::now() + Duration::from_millis(self.millis);
        while std::time::Instant::now() < deadline {
            if ctx.cancel.load(Ordering::Relaxed) {
                return Err(ToolError::new(ToolErrorCode::Aborted, "已中止"));
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        Ok(ToolOutput::text(format!("{} 完成", self.name)))
    }
}

/// 先报一次进度,然后等待取消(用于验证执行中取消的传播与配对)。
struct ProgressThenWaitCancelTool;

impl Tool for ProgressThenWaitCancelTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "progress_then_wait".into(),
            description: "emits progress then waits for cancel".into(),
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
        ctx.report_progress(ToolOutput::text("已启动"));
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        while !ctx.cancel.load(Ordering::Relaxed) {
            if std::time::Instant::now() > deadline {
                return Err(ToolError::execution("测试超时:未收到取消传播"));
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        Err(ToolError::new(ToolErrorCode::Aborted, "已取消"))
    }
}

fn finished_order(events: &[AgentEvent]) -> Vec<String> {
    events
        .iter()
        .filter_map(|event| match event {
            AgentEvent::ToolCallFinished { id, .. } => Some(id.clone()),
            _ => None,
        })
        .collect()
}

fn result_block_order(entries: &[SessionEntry]) -> Vec<String> {
    entries
        .iter()
        .filter_map(|entry| match &entry.payload {
            SessionEntryPayload::Message(record) if record.message.role == Role::User => {
                let ids: Vec<String> = record
                    .message
                    .blocks
                    .iter()
                    .filter_map(|block| match block {
                        Block::ToolResult { tool_use_id, .. } => Some(tool_use_id.clone()),
                        _ => None,
                    })
                    .collect();
                (!ids.is_empty()).then_some(ids)
            }
            _ => None,
        })
        .flatten()
        .collect()
}

#[test]
fn parallel_batch_finishes_by_completion_but_history_keeps_source_order() {
    let root = temp_root("parallel-order");
    let mut agent = Agent::new_with_data_dir(
        config(&root),
        Workspace::new(root.clone()),
        root.join("data"),
    )
    .unwrap();
    agent.tools = ToolRegistry::new(vec![
        Box::new(SleepTool {
            name: "slow",
            millis: 300,
            mode: crate::tools::ToolExecutionMode::ParallelSafe,
        }),
        Box::new(SleepTool {
            name: "fast",
            millis: 10,
            mode: crate::tools::ToolExecutionMode::ParallelSafe,
        }),
    ]);
    agent.provider = Box::new(ScriptedProvider::new(vec![
        // call-1 = slow, call-2 = fast(按源顺序)。
        ScriptStep::Output(tool_turn(vec![
            ("slow", serde_json::json!({})),
            ("fast", serde_json::json!({})),
        ])),
        ScriptStep::Output(output(ChatMessage::empty_assistant(), StopReason::EndTurn)),
    ]));
    let started = std::time::Instant::now();
    let mut events = Vec::new();
    agent.handle_command(
        AgentCommand::UserInput("go".into()),
        &mut |event| events.push(event),
        &AtomicBool::new(false),
    );

    // UI 完成事件:快的先到。
    assert_eq!(
        finished_order(&events),
        vec!["call-2".to_string(), "call-1".into()],
        "完成顺序应是 fast 先"
    );
    // 历史 ToolResult:仍按 ToolUse 源顺序。
    assert_eq!(
        result_block_order(&agent.entries),
        vec!["call-1".to_string(), "call-2".into()]
    );
    // 并发生效:总时长应接近 slow(300ms)而不是显著串行。
    assert!(
        started.elapsed() < Duration::from_millis(1500),
        "批执行不应退化成显著串行"
    );
    // 所有 Started 在任何 Finished 之前(preflight 按源顺序先行)。
    let first_finished = events
        .iter()
        .position(|event| matches!(event, AgentEvent::ToolCallFinished { .. }))
        .unwrap();
    let started_count_before = events[..first_finished]
        .iter()
        .filter(|event| matches!(event, AgentEvent::ToolCallStarted { .. }))
        .count();
    assert_eq!(started_count_before, 2);
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn any_sequential_tool_forces_whole_batch_sequential() {
    let root = temp_root("sequential-forced");
    let mut agent = Agent::new_with_data_dir(
        config(&root),
        Workspace::new(root.clone()),
        root.join("data"),
    )
    .unwrap();
    agent.tools = ToolRegistry::new(vec![
        Box::new(SleepTool {
            name: "slow_seq",
            millis: 150,
            mode: crate::tools::ToolExecutionMode::Sequential,
        }),
        Box::new(SleepTool {
            name: "fast_par",
            millis: 5,
            mode: crate::tools::ToolExecutionMode::ParallelSafe,
        }),
    ]);
    agent.provider = Box::new(ScriptedProvider::new(vec![
        ScriptStep::Output(tool_turn(vec![
            ("slow_seq", serde_json::json!({})),
            ("fast_par", serde_json::json!({})),
        ])),
        ScriptStep::Output(output(ChatMessage::empty_assistant(), StopReason::EndTurn)),
    ]));
    let mut events = Vec::new();
    agent.handle_command(
        AgentCommand::UserInput("go".into()),
        &mut |event| events.push(event),
        &AtomicBool::new(false),
    );

    // 整批退回串行:尽管 fast 更快,完成顺序仍是源顺序。
    assert_eq!(
        finished_order(&events),
        vec!["call-1".to_string(), "call-2".into()]
    );
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn cancel_during_parallel_execution_still_pairs_every_call() {
    let root = temp_root("parallel-cancel");
    let mut agent = Agent::new_with_data_dir(
        config(&root),
        Workspace::new(root.clone()),
        root.join("data"),
    )
    .unwrap();
    agent.tools = ToolRegistry::new(vec![
        Box::new(ProgressThenWaitCancelTool),
        Box::new(SleepTool {
            name: "long_sleep",
            millis: 5_000,
            mode: crate::tools::ToolExecutionMode::ParallelSafe,
        }),
    ]);
    agent.provider = Box::new(ScriptedProvider::new(vec![ScriptStep::Output(tool_turn(
        vec![
            ("progress_then_wait", serde_json::json!({})),
            ("long_sleep", serde_json::json!({})),
        ],
    ))]));
    let cancel = AtomicBool::new(false);
    let started = std::time::Instant::now();
    let mut events = Vec::new();
    agent.handle_command(
        AgentCommand::UserInput("go".into()),
        &mut |event| {
            // 第一个进度事件说明工具确实已在执行中,此刻模拟 Esc。
            if matches!(event, AgentEvent::ToolCallUpdated { .. }) {
                cancel.store(true, Ordering::Relaxed);
            }
            events.push(event);
        },
        &cancel,
    );

    assert!(
        started.elapsed() < Duration::from_secs(4),
        "取消应中断执行,而不是等工具睡满"
    );
    // 每个 ToolUse 都有配对结果,历史仍按源顺序。
    assert_eq!(
        result_block_order(&agent.entries),
        vec!["call-1".to_string(), "call-2".into()]
    );
    assert!(events
        .iter()
        .any(|event| matches!(event, AgentEvent::TurnFinished { cancelled: true })));
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn tool_timeout_rewrites_obedient_abort_to_timeout() {
    let root = temp_root("tool-timeout");
    let mut agent = Agent::new_with_data_dir(
        config(&root),
        Workspace::new(root.clone()),
        root.join("data"),
    )
    .unwrap();
    agent.config.tool_timeout = Some(Duration::from_millis(120));
    agent.tools = ToolRegistry::new(vec![Box::new(SleepTool {
        name: "sleepy",
        millis: 10_000,
        mode: crate::tools::ToolExecutionMode::ParallelSafe,
    })]);
    agent.provider = Box::new(ScriptedProvider::new(vec![
        ScriptStep::Output(tool_turn(vec![("sleepy", serde_json::json!({}))])),
        ScriptStep::Output(output(ChatMessage::empty_assistant(), StopReason::EndTurn)),
    ]));
    let started = std::time::Instant::now();
    let mut events = Vec::new();
    agent.handle_command(
        AgentCommand::UserInput("go".into()),
        &mut |event| events.push(event),
        &AtomicBool::new(false),
    );

    assert!(
        started.elapsed() < Duration::from_secs(5),
        "超时应中止工具,而不是等它睡满 10s"
    );
    assert!(
        events.iter().any(|event| matches!(
            event,
            AgentEvent::ToolCallFinished {
                error: Some(ToolError {
                    code: ToolErrorCode::Timeout,
                    ..
                }),
                ..
            }
        )),
        "超时中止应报 Timeout 而不是 Aborted: {events:?}"
    );
    // 超时不是用户取消,本轮正常结束(模型会看到错误结果并自行调整)。
    assert!(events
        .iter()
        .any(|event| matches!(event, AgentEvent::TurnFinished { cancelled: false })));
    let _ = std::fs::remove_dir_all(root);
}
