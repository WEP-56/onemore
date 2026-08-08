use super::*;

// ---- 阶段 4:事实日志、模型视图与预算 ----

#[test]
fn assistant_usage_is_recorded_as_fact_and_seeds_baseline() {
    let root = temp_root("usage-fact");
    let mut agent = Agent::new_with_data_dir(
        config(&root),
        Workspace::new(root.clone()),
        root.join("data"),
    )
    .unwrap();
    agent.provider = Box::new(ScriptedProvider::new(vec![ScriptStep::Output(
        crate::provider::TurnOutput {
            message: ChatMessage {
                role: Role::Assistant,
                blocks: vec![Block::Text("答复".into())],
            },
            usage: Usage {
                input_tokens: 1200,
                output_tokens: 34,
                cache: None,
            },
            stop: StopReason::EndTurn,
            prompt_fingerprint: Some("sha256:test".into()),
        },
    )]));
    agent.handle_command(
        AgentCommand::UserInput("问题".into()),
        &mut |_| {},
        &AtomicBool::new(false),
    );

    let assistant = agent
        .entries
        .iter()
        .find_map(|entry| match &entry.payload {
            SessionEntryPayload::Message(record) if record.message.role == Role::Assistant => {
                Some(record.clone())
            }
            _ => None,
        })
        .expect("assistant 应成为 Message 事实");
    assert_eq!(
        assistant.usage,
        Some(Usage {
            input_tokens: 1200,
            output_tokens: 34,
            cache: None,
        }),
        "事实必须携带该次调用的真实 usage"
    );
    assert_eq!(assistant.prompt_fingerprint.as_deref(), Some("sha256:test"));
    let projection = project_model_messages(&agent.entries);
    assert_eq!(projection.known_token_baseline, Some(1234));
    assert_eq!(projection.tail_chars_after_baseline, 0);
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn over_budget_refuses_to_call_provider() {
    let root = temp_root("budget-refuse");
    let mut agent = Agent::new_with_data_dir(
        config(&root),
        Workspace::new(root.clone()),
        root.join("data"),
    )
    .unwrap();
    // 空脚本:任何 provider 调用都会 panic("script exhausted"),
    // 由此证明拒绝发生在请求发出之前。
    agent.provider = Box::new(ScriptedProvider::new(Vec::new()));
    agent.budget = ContextBudget {
        context_window: Some(100),
        reserve_output: 50,
    };
    agent.compaction_settings.enabled = false;
    let mut events = Vec::new();
    agent.handle_command(
        AgentCommand::UserInput("长输入".repeat(2000)),
        &mut |event| events.push(event),
        &AtomicBool::new(false),
    );

    assert!(events
        .iter()
        .any(|event| matches!(event, AgentEvent::Error(text) if text.contains("/compact"))));
    assert!(events
        .iter()
        .any(|event| matches!(event, AgentEvent::TurnFinished { cancelled: false })));
    // 用户消息仍然是事实(拒绝的是本次请求,不是用户输入)。
    assert_eq!(agent.entries.len(), 1);
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn compact_appends_fact_and_shrinks_model_view() {
    let root = temp_root("compact");
    let mut agent = Agent::new_with_data_dir(
        config(&root),
        Workspace::new(root.clone()),
        root.join("data"),
    )
    .unwrap();
    // 先跑一轮正常对话形成历史。
    agent.provider = Box::new(ScriptedProvider::new(vec![ScriptStep::Output(output(
        ChatMessage {
            role: Role::Assistant,
            blocks: vec![Block::Text("旧回答".into())],
        },
        StopReason::EndTurn,
    ))]));
    agent.handle_command(
        AgentCommand::UserInput("旧问题".into()),
        &mut |_| {},
        &AtomicBool::new(false),
    );
    let facts_before = agent.entries.len();

    // /compact:模型返回摘要。
    agent.provider = Box::new(ScriptedProvider::new(vec![ScriptStep::Output(output(
        ChatMessage {
            role: Role::Assistant,
            blocks: vec![Block::Text("摘要:讨论了旧问题".into())],
        },
        StopReason::EndTurn,
    ))]));
    let mut events = Vec::new();
    agent.handle_command(
        AgentCommand::Compact,
        &mut |event| events.push(event),
        &AtomicBool::new(false),
    );

    // 压缩后事实条数不减少(只增加 Compaction)。
    assert_eq!(agent.entries.len(), facts_before + 1);
    assert!(matches!(
        agent.entries.last().unwrap().payload,
        SessionEntryPayload::Compaction(_)
    ));
    // 模型视图缩小为"摘要"一条。
    let projection = project_model_messages(&agent.entries);
    assert_eq!(projection.messages.len(), 1);
    assert!(projection.messages[0].text().contains("摘要:讨论了旧问题"));
    // 压缩期间不把摘要当对话正文流出。
    assert!(!events
        .iter()
        .any(|event| matches!(event, AgentEvent::AssistantDelta(_))));
    assert!(events
        .iter()
        .any(|event| matches!(event, AgentEvent::Notice(text) if text.contains("历史已压缩"))));
    let _ = std::fs::remove_dir_all(root);
}

/// /compact 的请求形状回归:历史里有工具往返时,压缩请求必须是
/// 纯文本单条 user 消息——零工具请求携带 ToolUse/ToolResult 块在
/// Anthropic 上是 400,在 OpenAI 兼容网关上常表现为 502。
#[test]
fn compact_request_is_plain_text_without_tool_blocks() {
    let root = temp_root("compact-plain");
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
        ScriptStep::Output(output(
            ChatMessage {
                role: Role::Assistant,
                blocks: vec![Block::Text("做完了".into())],
            },
            StopReason::EndTurn,
        )),
    ]));
    agent.handle_command(
        AgentCommand::UserInput("做点事".into()),
        &mut |_| {},
        &AtomicBool::new(false),
    );

    let prompts = Arc::new(Mutex::new(Vec::new()));
    agent.provider = Box::new(ScriptedProvider::with_prompt_log(
        vec![ScriptStep::Output(output(
            ChatMessage {
                role: Role::Assistant,
                blocks: vec![Block::Text("摘要".into())],
            },
            StopReason::EndTurn,
        ))],
        Arc::clone(&prompts),
    ));
    agent.handle_command(AgentCommand::Compact, &mut |_| {}, &AtomicBool::new(false));

    let prompts = prompts.lock().unwrap();
    assert_eq!(prompts.len(), 1);
    let request = &prompts[0];
    assert_eq!(request.len(), 1, "压缩请求应是单条消息: {request:?}");
    assert_eq!(request[0].role, Role::User);
    assert!(
        request[0]
            .blocks
            .iter()
            .all(|block| matches!(block, Block::Text(_))),
        "压缩请求不得携带结构化工具/思考块: {request:?}"
    );
    let text = request[0].text();
    assert!(text.contains("counted"), "工具调用应以文字保留: {text}");
    assert!(text.contains("executed"), "工具结果应以文字保留: {text}");
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn session_restore_returns_full_facts_including_ui_only() {
    let root = temp_root("restore-facts");
    let data_dir = root.join("data");
    let session_id;
    {
        let mut agent = Agent::new_with_data_dir(
            config(&root),
            Workspace::new(root.clone()),
            data_dir.clone(),
        )
        .unwrap();
        session_id = agent.session_id().to_string();
        // MaxTokens 且无工具调用 → assistant 事实 + UI-only Notice 事实同批落库。
        agent.provider = Box::new(ScriptedProvider::new(vec![ScriptStep::Output(output(
            ChatMessage {
                role: Role::Assistant,
                blocks: vec![Block::Text("被截断的回答".into())],
            },
            StopReason::MaxTokens,
        ))]));
        agent.handle_command(
            AgentCommand::UserInput("问题".into()),
            &mut |_| {},
            &AtomicBool::new(false),
        );
    }

    let mut second =
        Agent::new_with_data_dir(config(&root), Workspace::new(root.clone()), data_dir).unwrap();
    let mut events = Vec::new();
    second.handle_command(
        AgentCommand::LoadSession(session_id.clone()),
        &mut |event| events.push(event),
        &AtomicBool::new(false),
    );
    let entries = events
        .iter()
        .find_map(|event| match event {
            AgentEvent::SessionLoaded { id, entries, .. } if *id == session_id => {
                Some(entries.clone())
            }
            _ => None,
        })
        .expect("应恢复目标会话");
    assert_eq!(entries.len(), 3, "user + assistant + notice 三条事实");
    assert!(entries
        .iter()
        .any(|entry| matches!(entry.payload, SessionEntryPayload::Notice(_))));
    // UI-only 事实不进模型视图。
    let projection = project_model_messages(&entries);
    assert_eq!(projection.messages.len(), 2);
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn commit_failure_stops_memory_advance_and_reports() {
    let root = temp_root("commit-fail");
    let mut agent = Agent::new_with_data_dir(
        config(&root),
        Workspace::new(root.clone()),
        root.join("data"),
    )
    .unwrap();
    // 半批工具事实(ToolUse 无配对结果)会被提交边界拒绝;
    // Runtime 的 commit 必须返回 false、发 Error,并且不推进内存镜像。
    let orphan = SessionEntryPayload::message(
        ChatMessage {
            role: Role::Assistant,
            blocks: vec![Block::ToolUse {
                id: "orphan".into(),
                name: "read_file".into(),
                input: serde_json::json!({}),
            }],
        },
        None,
    );
    let mut events = Vec::new();
    let committed = agent.commit(vec![orphan], &mut |event| events.push(event));

    assert!(!committed);
    assert!(events
        .iter()
        .any(|event| matches!(event, AgentEvent::Error(text) if text.contains("保存会话失败"))));
    // 内存镜像为空；lazy session 在首次成功提交前不物化空数据库。
    assert!(agent.entries.is_empty());
    let id = agent.session_id().to_string();
    assert!(agent.sessions.load(&id).is_err());
    assert!(!root
        .join("data")
        .join("sessions")
        .join(format!("{id}.db"))
        .exists());
    let _ = std::fs::remove_dir_all(root);
}
