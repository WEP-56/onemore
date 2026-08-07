use super::*;

#[test]
fn terminal_protocol_keeps_final_message_on_error() {
    let provider = ScriptedProvider::new(vec![ScriptStep::Error(ProviderError::fatal("boom"))]);
    let terminal = provider.stream_turn(
        &PromptContext::default(),
        &[],
        &mut |_| {},
        &AtomicBool::new(false),
    );
    match terminal {
        StreamTerminal::Error(failed) => {
            assert!(failed.message.role == Role::Assistant);
            assert_eq!(failed.error.message, "boom");
        }
        other => panic!("应得到 Error 终止，实际为 {:?}", other),
    }
}

#[test]
fn runtime_emits_closed_error_and_abort_turns() {
    let root = temp_root("terminal");
    let mut agent = Agent::new_with_data_dir(
        config(&root),
        Workspace::new(root.clone()),
        root.join("data"),
    )
    .unwrap();
    agent.provider = Box::new(ScriptedProvider::new(vec![ScriptStep::Error(
        ProviderError::fatal("boom"),
    )]));
    let mut events = Vec::new();
    agent.handle_command(
        AgentCommand::UserInput("失败".into()),
        &mut |event| events.push(event),
        &AtomicBool::new(false),
    );
    assert!(events
        .iter()
        .any(|e| matches!(e, AgentEvent::Error(text) if text == "boom")));
    assert!(events
        .iter()
        .any(|e| matches!(e, AgentEvent::TurnFinished { cancelled: false })));

    let mut agent = Agent::new_with_data_dir(
        config(&root),
        Workspace::new(root.clone()),
        root.join("data-abort"),
    )
    .unwrap();
    agent.provider = Box::new(ScriptedProvider::new(vec![ScriptStep::Cancel]));
    let mut events = Vec::new();
    agent.handle_command(
        AgentCommand::UserInput("取消".into()),
        &mut |event| events.push(event),
        &AtomicBool::new(false),
    );
    assert!(events
        .iter()
        .any(|e| matches!(e, AgentEvent::TurnFinished { cancelled: true })));
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn max_tokens_tool_calls_are_returned_as_errors_without_execution() {
    let root = temp_root("length");
    let target = root.join("should-not-exist.txt");
    let first = output(
        ChatMessage {
            role: Role::Assistant,
            blocks: vec![Block::ToolUse {
                id: "call-1".into(),
                name: "write_file".into(),
                input: serde_json::json!({
                    "path": "should-not-exist.txt",
                    "content": "unsafe partial output"
                }),
            }],
        },
        StopReason::MaxTokens,
    );
    let second = output(ChatMessage::empty_assistant(), StopReason::EndTurn);
    let mut agent = Agent::new_with_data_dir(
        config(&root),
        Workspace::new(root.clone()),
        root.join("data"),
    )
    .unwrap();
    agent.provider = Box::new(ScriptedProvider::new(vec![
        ScriptStep::Output(first),
        ScriptStep::Output(second),
    ]));
    let mut events = Vec::new();
    agent.handle_command(
        AgentCommand::UserInput("写文件".into()),
        &mut |event| events.push(event),
        &AtomicBool::new(false),
    );
    assert!(!target.exists(), "length 截断的工具调用不应产生写入副作用");
    assert!(events
        .iter()
        .any(|event| { matches!(event, AgentEvent::ToolCallFinished { error: Some(_), .. }) }));
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn tool_progress_is_mapped_to_runtime_event_before_finish() {
    let root = temp_root("progress");
    let first = output(
        ChatMessage {
            role: Role::Assistant,
            blocks: vec![Block::ToolUse {
                id: "progress-1".into(),
                name: "runtime_progress".into(),
                input: serde_json::json!({}),
            }],
        },
        StopReason::ToolUse,
    );
    let second = output(ChatMessage::empty_assistant(), StopReason::EndTurn);
    let mut agent = Agent::new_with_data_dir(
        config(&root),
        Workspace::new(root.clone()),
        root.join("data"),
    )
    .unwrap();
    agent.tools = ToolRegistry::new(vec![Box::new(RuntimeProgressTool)]);
    agent.provider = Box::new(ScriptedProvider::new(vec![
        ScriptStep::Output(first),
        ScriptStep::Output(second),
    ]));
    let mut events = Vec::new();
    agent.handle_command(
        AgentCommand::UserInput("运行".into()),
        &mut |event| events.push(event),
        &AtomicBool::new(false),
    );

    let updated = events.iter().position(|event| {
        matches!(
            event,
            AgentEvent::ToolCallUpdated { id, name, output }
                if id == "progress-1"
                    && name == "runtime_progress"
                    && output.ui_text() == "1/2"
        )
    });
    let finished = events.iter().position(|event| {
        matches!(
            event,
            AgentEvent::ToolCallFinished { id, .. } if id == "progress-1"
        )
    });
    assert!(updated.is_some(), "应收到结构化工具进度事件: {events:?}");
    assert!(
        updated.unwrap() < finished.expect("应收到工具完成事件"),
        "进度必须先于完成事件"
    );
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn permission_deny_never_executes_and_still_finishes_tool_pair() {
    let root = temp_root("permission-deny");
    let first = tool_turn(vec![("counted", serde_json::json!({ "path": "inside" }))]);
    let second = output(ChatMessage::empty_assistant(), StopReason::EndTurn);
    let mut agent = Agent::new_with_data_dir(
        config(&root),
        Workspace::new(root.clone()),
        root.join("data"),
    )
    .unwrap();
    agent.permissions = PermissionManager::new(PermissionRules {
        workspace_write: PermissionRule::Deny,
        ..PermissionRules::default()
    });
    let executions = install_counted_tool(
        &mut agent,
        ToolCapabilities::MUTATION,
        ToolPermissionSpec::paths(&["path"]),
    );
    agent.provider = Box::new(ScriptedProvider::new(vec![
        ScriptStep::Output(first),
        ScriptStep::Output(second),
    ]));
    let mut events = Vec::new();
    agent.handle_command(
        AgentCommand::UserInput("deny".into()),
        &mut |event| events.push(event),
        &AtomicBool::new(false),
    );

    assert_eq!(executions.load(Ordering::Relaxed), 0);
    assert!(events.iter().any(|event| matches!(
        event,
        AgentEvent::ToolCallFinished {
            error: Some(ToolError {
                code: ToolErrorCode::PermissionDenied,
                ..
            }),
            ..
        }
    )));
    assert!(events
        .iter()
        .any(|event| matches!(event, AgentEvent::TurnFinished { cancelled: false })));
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn approval_rejection_is_a_tool_result_not_a_runtime_failure() {
    let root = temp_root("approval-deny");
    let first = tool_turn(vec![("counted", serde_json::json!({ "path": "opaque" }))]);
    let second = output(ChatMessage::empty_assistant(), StopReason::EndTurn);
    let mut agent = Agent::new_with_data_dir(
        config(&root),
        Workspace::new(root.clone()),
        root.join("data"),
    )
    .unwrap();
    let executions = install_counted_tool(
        &mut agent,
        ToolCapabilities::COMMAND,
        ToolPermissionSpec::opaque_side_effect(&[]),
    );
    agent.provider = Box::new(ScriptedProvider::new(vec![
        ScriptStep::Output(first),
        ScriptStep::Output(second),
    ]));
    let (approval_tx, approval_rx) = std::sync::mpsc::channel();
    agent.approval_rx = Some(approval_rx);
    let mut events = Vec::new();
    agent.handle_command(
        AgentCommand::UserInput("ask".into()),
        &mut |event| {
            if let AgentEvent::PermissionRequested { request } = &event {
                approval_tx
                    .send(ApprovalResponse {
                        request_id: request.request_id.clone(),
                        decision: ApprovalDecision::Deny,
                    })
                    .unwrap();
            }
            events.push(event);
        },
        &AtomicBool::new(false),
    );

    assert_eq!(executions.load(Ordering::Relaxed), 0);
    assert!(events
        .iter()
        .any(|event| matches!(event, AgentEvent::PermissionResolved { allowed: false, .. })));
    assert!(events.iter().any(|event| matches!(
        event,
        AgentEvent::ToolCallFinished {
            error: Some(ToolError {
                code: ToolErrorCode::PermissionDenied,
                ..
            }),
            ..
        }
    )));
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn session_approval_only_skips_identical_following_call() {
    let root = temp_root("approval-session");
    let first = tool_turn(vec![
        ("counted", serde_json::json!({ "path": "same" })),
        ("counted", serde_json::json!({ "path": "same" })),
    ]);
    let second = output(ChatMessage::empty_assistant(), StopReason::EndTurn);
    let mut agent = Agent::new_with_data_dir(
        config(&root),
        Workspace::new(root.clone()),
        root.join("data"),
    )
    .unwrap();
    let executions = install_counted_tool(
        &mut agent,
        ToolCapabilities::COMMAND,
        ToolPermissionSpec::opaque_side_effect(&[]),
    );
    agent.provider = Box::new(ScriptedProvider::new(vec![
        ScriptStep::Output(first),
        ScriptStep::Output(second),
    ]));
    let (approval_tx, approval_rx) = std::sync::mpsc::channel();
    agent.approval_rx = Some(approval_rx);
    let mut requests = 0;
    agent.handle_command(
        AgentCommand::UserInput("session grant".into()),
        &mut |event| {
            if let AgentEvent::PermissionRequested { request } = event {
                requests += 1;
                approval_tx
                    .send(ApprovalResponse {
                        request_id: request.request_id,
                        decision: ApprovalDecision::Allow(ApprovalScope::Session),
                    })
                    .unwrap();
            }
        },
        &AtomicBool::new(false),
    );

    assert_eq!(requests, 1);
    assert_eq!(executions.load(Ordering::Relaxed), 2);
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn cancelling_an_approval_wait_aborts_without_executing() {
    let root = temp_root("approval-cancel");
    let first = tool_turn(vec![("counted", serde_json::json!({ "path": "opaque" }))]);
    let mut agent = Agent::new_with_data_dir(
        config(&root),
        Workspace::new(root.clone()),
        root.join("data"),
    )
    .unwrap();
    let executions = install_counted_tool(
        &mut agent,
        ToolCapabilities::COMMAND,
        ToolPermissionSpec::opaque_side_effect(&[]),
    );
    agent.provider = Box::new(ScriptedProvider::new(vec![ScriptStep::Output(first)]));
    let (_approval_tx, approval_rx) = std::sync::mpsc::channel();
    agent.approval_rx = Some(approval_rx);
    let cancel = AtomicBool::new(false);
    let mut events = Vec::new();
    agent.handle_command(
        AgentCommand::UserInput("cancel approval".into()),
        &mut |event| {
            if matches!(event, AgentEvent::PermissionRequested { .. }) {
                cancel.store(true, Ordering::Relaxed);
            }
            events.push(event);
        },
        &cancel,
    );

    assert_eq!(executions.load(Ordering::Relaxed), 0);
    assert!(events
        .iter()
        .any(|event| matches!(event, AgentEvent::TurnFinished { cancelled: true })));
    assert!(events.iter().any(|event| matches!(
        event,
        AgentEvent::ToolCallFinished {
            error: Some(ToolError {
                code: ToolErrorCode::Aborted,
                ..
            }),
            ..
        }
    )));
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn hook_replacement_is_revalidated_and_rechecked_by_permission() {
    let root = temp_root("hook-recheck");
    let outside = temp_root("hook-outside").join("target.txt");
    let first = tool_turn(vec![("counted", serde_json::json!({ "path": "inside" }))]);
    let second = output(ChatMessage::empty_assistant(), StopReason::EndTurn);
    let mut agent = Agent::new_with_data_dir(
        config(&root),
        Workspace::new(root.clone()),
        root.join("data"),
    )
    .unwrap();
    agent.permissions = PermissionManager::new(PermissionRules {
        outside_workspace: PermissionRule::Deny,
        ..PermissionRules::default()
    });
    let executions = install_counted_tool(
        &mut agent,
        ToolCapabilities::MUTATION,
        ToolPermissionSpec::paths(&["path"]),
    );
    let hook_calls = Arc::new(AtomicUsize::new(0));
    agent.hooks = HookRegistry::new(vec![Box::new(ReplacePathHook {
        calls: Arc::clone(&hook_calls),
        replacement: outside.to_string_lossy().into_owned(),
    })]);
    agent.provider = Box::new(ScriptedProvider::new(vec![
        ScriptStep::Output(first),
        ScriptStep::Output(second),
    ]));
    agent.handle_command(
        AgentCommand::UserInput("hook".into()),
        &mut |_| {},
        &AtomicBool::new(false),
    );

    assert_eq!(hook_calls.load(Ordering::Relaxed), 1);
    assert_eq!(executions.load(Ordering::Relaxed), 0);
    let _ = std::fs::remove_dir_all(root);
}

#[cfg(windows)]
#[test]
fn hard_deny_runs_before_pre_tool_hook() {
    let root = temp_root("hard-deny-hook");
    let first = tool_turn(vec![("counted", serde_json::json!({ "path": "NUL.txt" }))]);
    let second = output(ChatMessage::empty_assistant(), StopReason::EndTurn);
    let mut agent = Agent::new_with_data_dir(
        config(&root),
        Workspace::new(root.clone()),
        root.join("data"),
    )
    .unwrap();
    let executions = install_counted_tool(
        &mut agent,
        ToolCapabilities::MUTATION,
        ToolPermissionSpec::paths(&["path"]),
    );
    let hook_calls = Arc::new(AtomicUsize::new(0));
    agent.hooks = HookRegistry::new(vec![Box::new(ReplacePathHook {
        calls: Arc::clone(&hook_calls),
        replacement: "safe.txt".into(),
    })]);
    agent.provider = Box::new(ScriptedProvider::new(vec![
        ScriptStep::Output(first),
        ScriptStep::Output(second),
    ]));
    agent.handle_command(
        AgentCommand::UserInput("hard deny".into()),
        &mut |_| {},
        &AtomicBool::new(false),
    );

    assert_eq!(hook_calls.load(Ordering::Relaxed), 0);
    assert_eq!(executions.load(Ordering::Relaxed), 0);
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn stop_hook_can_prevent_stop_only_once_per_run() {
    let root = temp_root("stop-hook");
    let first = output(
        ChatMessage {
            role: Role::Assistant,
            blocks: vec![Block::Text("first".into())],
        },
        StopReason::EndTurn,
    );
    let second = output(
        ChatMessage {
            role: Role::Assistant,
            blocks: vec![Block::Text("verified".into())],
        },
        StopReason::EndTurn,
    );
    let mut agent = Agent::new_with_data_dir(
        config(&root),
        Workspace::new(root.clone()),
        root.join("data"),
    )
    .unwrap();
    let calls = Arc::new(AtomicUsize::new(0));
    agent.hooks = HookRegistry::new(vec![Box::new(PreventStopHook {
        calls: Arc::clone(&calls),
    })]);
    agent.provider = Box::new(ScriptedProvider::new(vec![
        ScriptStep::Output(first),
        ScriptStep::Output(second),
    ]));
    let mut events = Vec::new();
    agent.handle_command(
        AgentCommand::UserInput("verify".into()),
        &mut |event| events.push(event),
        &AtomicBool::new(false),
    );

    assert_eq!(calls.load(Ordering::Relaxed), 1);
    assert!(events
        .iter()
        .any(|event| matches!(event, AgentEvent::AssistantMessage(text) if text == "verified")));
    assert!(events
        .iter()
        .any(|event| matches!(event, AgentEvent::TurnFinished { cancelled: false })));
    let _ = std::fs::remove_dir_all(root);
}
