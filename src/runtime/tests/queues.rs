use super::*;

// ---- 阶段 5:RetryPolicy 与 steering/follow-up ----

/// 轮询组合取消标志直到被置位,再以 Aborted 收尾。
/// 配合"emit 侧看到 ToolCallStarted 就置全局取消"模拟用户在工具执行中按 Esc:
/// 协调线程会把全局取消传播到每个调用的组合标志。
struct WaitForCancelTool;

impl Tool for WaitForCancelTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "wait_for_cancel".into(),
            description: "waits until cancelled".into(),
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

fn inbox_with(commands: Vec<AgentCommand>) -> Receiver<AgentCommand> {
    let (tx, rx) = std::sync::mpsc::channel();
    for command in commands {
        tx.send(command).unwrap();
    }
    rx
}

fn user_texts(entries: &[SessionEntry]) -> Vec<String> {
    entries
        .iter()
        .filter_map(|entry| match &entry.payload {
            SessionEntryPayload::Message(record) if record.message.role == Role::User => {
                let text = record.message.text();
                (!text.is_empty()).then_some(text)
            }
            _ => None,
        })
        .collect()
}

#[test]
fn retry_policy_is_deterministic_and_bounded() {
    let policy = RetryPolicy {
        max_attempts: 3,
        base_delay: Duration::from_secs(2),
        max_delay: Duration::from_secs(30),
        max_retry_after: Duration::from_secs(60),
        jitter_seed: 42,
    };
    // 尝试耗尽 → 不再重试。
    assert_eq!(policy.delay_for(3, None), None);
    // 服务器等待请求超上限 → 拒绝重试。
    assert_eq!(policy.delay_for(1, Some(Duration::from_secs(120))), None);
    // 合理的 Retry-After 原样生效(不加 jitter)。
    assert_eq!(
        policy.delay_for(1, Some(Duration::from_millis(2500))),
        Some(Duration::from_millis(2500))
    );
    // 指数退避 + jitter ∈ [0,25%),且同参数结果恒定。
    let first = policy.delay_for(1, None).unwrap();
    assert!(first >= Duration::from_secs(2) && first < Duration::from_millis(2500));
    assert_eq!(policy.delay_for(1, None), Some(first));
    let second = policy.delay_for(2, None).unwrap();
    assert!(second >= Duration::from_secs(4) && second < Duration::from_secs(5));
    // 退避不越过 max_delay。
    let capped = RetryPolicy {
        base_delay: Duration::from_secs(20),
        max_delay: Duration::from_secs(30),
        ..policy
    };
    assert!(capped.delay_for(2, None).unwrap() <= Duration::from_secs(30));
}

#[test]
fn oversized_retry_after_fails_without_waiting() {
    let root = temp_root("retry-after-cap");
    let mut agent = Agent::new_with_data_dir(
        config(&root),
        Workspace::new(root.clone()),
        root.join("data"),
    )
    .unwrap();
    // 只有一个步骤:若发生重试,脚本耗尽会 panic。
    agent.provider = Box::new(ScriptedProvider::new(vec![ScriptStep::Error(
        ProviderError {
            message: "overloaded".into(),
            retryable: true,
            retry_after: Some(Duration::from_secs(300)),
        },
    )]));
    let started = std::time::Instant::now();
    let mut events = Vec::new();
    agent.handle_command(
        AgentCommand::UserInput("hi".into()),
        &mut |event| events.push(event),
        &AtomicBool::new(false),
    );
    assert!(
        started.elapsed() < Duration::from_secs(2),
        "不应等待超长 Retry-After"
    );
    assert!(events
        .iter()
        .any(|event| matches!(event, AgentEvent::Error(text) if text.contains("overloaded"))));
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn retry_emits_structured_schedule_and_start_events() {
    let root = temp_root("structured-retry");
    let mut agent = Agent::new_with_data_dir(
        config(&root),
        Workspace::new(root.clone()),
        root.join("data"),
    )
    .unwrap();
    agent.retry_policy = RetryPolicy {
        max_attempts: 2,
        base_delay: Duration::from_millis(1),
        max_delay: Duration::from_millis(1),
        max_retry_after: Duration::from_secs(1),
        jitter_seed: 0,
    };
    agent.provider = Box::new(ScriptedProvider::new(vec![
        ScriptStep::Error(ProviderError::retryable("connection reset")),
        ScriptStep::Output(output(
            ChatMessage {
                role: Role::Assistant,
                blocks: vec![Block::Text("recovered".into())],
            },
            StopReason::EndTurn,
        )),
    ]));
    let mut events = Vec::new();
    agent.handle_command(
        AgentCommand::UserInput("hi".into()),
        &mut |event| events.push(event),
        &AtomicBool::new(false),
    );
    assert!(events.iter().any(|event| matches!(
        event,
        AgentEvent::RetryScheduled {
            attempt: 1,
            max_retries: 1,
            error,
            ..
        } if error.contains("connection reset")
    )));
    assert!(events.iter().any(|event| matches!(
        event,
        AgentEvent::RetryStarted {
            attempt: 1,
            max_retries: 1
        }
    )));
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn steering_is_injected_only_after_full_tool_batch() {
    let root = temp_root("steering");
    let mut agent = Agent::new_with_data_dir(
        config(&root),
        Workspace::new(root.clone()),
        root.join("data"),
    )
    .unwrap();
    install_counted_tool(
        &mut agent,
        ToolCapabilities::READ_ONLY,
        ToolPermissionSpec::default(),
    );
    let prompts = Arc::new(Mutex::new(Vec::new()));
    agent.provider = Box::new(ScriptedProvider::with_prompt_log(
        vec![
            ScriptStep::Output(tool_turn(vec![
                ("counted", serde_json::json!({ "path": "a" })),
                ("counted", serde_json::json!({ "path": "b" })),
            ])),
            ScriptStep::Output(output(
                ChatMessage {
                    role: Role::Assistant,
                    blocks: vec![Block::Text("changed course".into())],
                },
                StopReason::EndTurn,
            )),
        ],
        Arc::clone(&prompts),
    ));
    let inbox = inbox_with(vec![AgentCommand::Steer("换个方向".into())]);
    let mut events = Vec::new();
    agent.handle_command_with_inbox(
        AgentCommand::UserInput("开始".into()),
        &mut |event| events.push(event),
        &AtomicBool::new(false),
        Some(&inbox),
    );

    // steering 出现在完整工具批(assistant + results)之后。
    let kinds: Vec<String> = agent
        .entries
        .iter()
        .map(|entry| match &entry.payload {
            SessionEntryPayload::Message(record) => {
                let has_results = record
                    .message
                    .blocks
                    .iter()
                    .any(|block| matches!(block, Block::ToolResult { .. }));
                if record.message.role == Role::Assistant {
                    "assistant".into()
                } else if has_results {
                    "results".into()
                } else {
                    format!("user:{}", record.message.text())
                }
            }
            other => other.kind().to_string(),
        })
        .collect();
    assert_eq!(
        kinds,
        vec![
            "user:开始".to_string(),
            "assistant".into(),
            "results".into(),
            "user:换个方向".into(),
            "assistant".into(),
        ]
    );
    // 第二次模型调用看到了 steering。
    let prompts = prompts.lock().unwrap();
    assert_eq!(prompts.len(), 2);
    assert_eq!(prompts[1].last().unwrap().text(), "换个方向");
    // 整个运行只有一对 TurnStarted/TurnFinished(没有重入)。
    assert_eq!(
        events
            .iter()
            .filter(|event| matches!(event, AgentEvent::TurnStarted))
            .count(),
        1
    );
    assert_eq!(
        events
            .iter()
            .filter(|event| matches!(event, AgentEvent::TurnFinished { .. }))
            .count(),
        1
    );
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn user_input_during_active_run_is_classified_as_steering() {
    let root = temp_root("classify");
    let mut agent = Agent::new_with_data_dir(
        config(&root),
        Workspace::new(root.clone()),
        root.join("data"),
    )
    .unwrap();
    install_counted_tool(
        &mut agent,
        ToolCapabilities::READ_ONLY,
        ToolPermissionSpec::default(),
    );
    agent.provider = Box::new(ScriptedProvider::new(vec![
        ScriptStep::Output(tool_turn(vec![(
            "counted",
            serde_json::json!({ "path": "a" }),
        )])),
        ScriptStep::Output(output(ChatMessage::empty_assistant(), StopReason::EndTurn)),
    ]));
    let inbox = inbox_with(vec![AgentCommand::UserInput("第二条".into())]);
    let mut events = Vec::new();
    agent.handle_command_with_inbox(
        AgentCommand::UserInput("第一条".into()),
        &mut |event| events.push(event),
        &AtomicBool::new(false),
        Some(&inbox),
    );

    assert!(events
        .iter()
        .any(|event| matches!(event, AgentEvent::Notice(text) if text.contains("steering"))));
    assert_eq!(
        events
            .iter()
            .filter(|event| matches!(event, AgentEvent::TurnStarted))
            .count(),
        1,
        "不允许重入开第二个运行"
    );
    assert!(user_texts(&agent.entries).contains(&"第二条".to_string()));
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn follow_up_runs_only_after_current_task_would_stop() {
    let root = temp_root("follow-up");
    let mut agent = Agent::new_with_data_dir(
        config(&root),
        Workspace::new(root.clone()),
        root.join("data"),
    )
    .unwrap();
    let prompts = Arc::new(Mutex::new(Vec::new()));
    agent.provider = Box::new(ScriptedProvider::with_prompt_log(
        vec![
            ScriptStep::Output(output(
                ChatMessage {
                    role: Role::Assistant,
                    blocks: vec![Block::Text("第一件事完成".into())],
                },
                StopReason::EndTurn,
            )),
            ScriptStep::Output(output(
                ChatMessage {
                    role: Role::Assistant,
                    blocks: vec![Block::Text("第二件事完成".into())],
                },
                StopReason::EndTurn,
            )),
        ],
        Arc::clone(&prompts),
    ));
    let inbox = inbox_with(vec![AgentCommand::FollowUp("下一件事".into())]);
    let mut events = Vec::new();
    agent.handle_command_with_inbox(
        AgentCommand::UserInput("第一件事".into()),
        &mut |event| events.push(event),
        &AtomicBool::new(false),
        Some(&inbox),
    );

    // follow-up 在第一件事的 assistant 之后注入。
    assert_eq!(
        user_texts(&agent.entries),
        vec!["第一件事".to_string(), "下一件事".into()]
    );
    let prompts = prompts.lock().unwrap();
    assert_eq!(prompts.len(), 2);
    assert_eq!(prompts[1].last().unwrap().text(), "下一件事");
    assert_eq!(
        events
            .iter()
            .filter(|event| matches!(event, AgentEvent::TurnFinished { .. }))
            .count(),
        1
    );
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn queued_inputs_are_injected_one_at_a_time() {
    let root = temp_root("one-at-a-time");
    let mut agent = Agent::new_with_data_dir(
        config_with_max_turns(&root, 5),
        Workspace::new(root.clone()),
        root.join("data"),
    )
    .unwrap();
    install_counted_tool(
        &mut agent,
        ToolCapabilities::READ_ONLY,
        ToolPermissionSpec::default(),
    );
    let prompts = Arc::new(Mutex::new(Vec::new()));
    agent.provider = Box::new(ScriptedProvider::with_prompt_log(
        vec![
            ScriptStep::Output(tool_turn(vec![(
                "counted",
                serde_json::json!({ "path": "a" }),
            )])),
            ScriptStep::Output(tool_turn(vec![(
                "counted",
                serde_json::json!({ "path": "b" }),
            )])),
            ScriptStep::Output(output(ChatMessage::empty_assistant(), StopReason::EndTurn)),
        ],
        Arc::clone(&prompts),
    ));
    let inbox = inbox_with(vec![
        AgentCommand::Steer("s1".into()),
        AgentCommand::Steer("s2".into()),
    ]);
    agent.handle_command_with_inbox(
        AgentCommand::UserInput("go".into()),
        &mut |_| {},
        &AtomicBool::new(false),
        Some(&inbox),
    );

    let prompts = prompts.lock().unwrap();
    assert_eq!(prompts.len(), 3);
    // 第二次调用只注入了最老的一条;s2 等到下一个检查点。
    let second: Vec<String> = prompts[1].iter().map(|m| m.text()).collect();
    assert!(second.contains(&"s1".to_string()));
    assert!(!second.contains(&"s2".to_string()));
    let third: Vec<String> = prompts[2].iter().map(|m| m.text()).collect();
    assert!(third.contains(&"s2".to_string()));
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn cancel_drops_queued_inputs_with_notice() {
    let root = temp_root("cancel-queues");
    let mut agent = Agent::new_with_data_dir(
        config(&root),
        Workspace::new(root.clone()),
        root.join("data"),
    )
    .unwrap();
    agent.tools = ToolRegistry::new(vec![Box::new(WaitForCancelTool)]);
    agent.provider = Box::new(ScriptedProvider::new(vec![ScriptStep::Output(tool_turn(
        vec![("wait_for_cancel", serde_json::json!({}))],
    ))]));
    let inbox = inbox_with(vec![AgentCommand::Steer("不该注入".into())]);
    let cancel = AtomicBool::new(false);
    let mut events = Vec::new();
    agent.handle_command_with_inbox(
        AgentCommand::UserInput("go".into()),
        &mut |event| {
            // 模拟用户在工具刚开始执行时按 Esc。
            if matches!(event, AgentEvent::ToolCallStarted { .. }) {
                cancel.store(true, Ordering::Relaxed);
            }
            events.push(event);
        },
        &cancel,
        Some(&inbox),
    );

    assert!(events
        .iter()
        .any(|event| matches!(event, AgentEvent::TurnFinished { cancelled: true })));
    assert!(events
        .iter()
        .any(|event| matches!(event, AgentEvent::Notice(text) if text.contains("丢弃"))));
    assert!(!user_texts(&agent.entries).contains(&"不该注入".to_string()));
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn shutdown_during_run_cancels_and_defers_exit() {
    let root = temp_root("shutdown-mid-run");
    let mut agent = Agent::new_with_data_dir(
        config(&root),
        Workspace::new(root.clone()),
        root.join("data"),
    )
    .unwrap();
    install_counted_tool(
        &mut agent,
        ToolCapabilities::READ_ONLY,
        ToolPermissionSpec::default(),
    );
    agent.provider = Box::new(ScriptedProvider::new(vec![ScriptStep::Output(tool_turn(
        vec![("counted", serde_json::json!({ "path": "a" }))],
    ))]));
    let inbox = inbox_with(vec![AgentCommand::Shutdown]);
    let cancel = AtomicBool::new(false);
    let mut events = Vec::new();
    let keep_running = agent.handle_command_with_inbox(
        AgentCommand::UserInput("go".into()),
        &mut |event| events.push(event),
        &cancel,
        Some(&inbox),
    );

    // 当前命令本身处理完毕(true),Shutdown 延迟到下一轮宿主循环。
    assert!(keep_running);
    assert!(cancel.load(Ordering::Relaxed), "Shutdown 应请求取消当前轮");
    assert!(matches!(
        agent.take_deferred(),
        Some(AgentCommand::Shutdown)
    ));
    assert!(events
        .iter()
        .any(|event| matches!(event, AgentEvent::TurnFinished { cancelled: true })));
    let _ = std::fs::remove_dir_all(root);
}
