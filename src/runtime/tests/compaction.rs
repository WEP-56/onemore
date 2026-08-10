use super::*;

struct RejectCompactionBackend {
    inner: crate::harness::MemorySessionBackend,
}

impl RejectCompactionBackend {
    fn new() -> Self {
        RejectCompactionBackend {
            inner: crate::harness::MemorySessionBackend::new(),
        }
    }
}

impl crate::harness::SessionBackend for RejectCompactionBackend {
    fn current_id(&self) -> &str {
        self.inner.current_id()
    }

    fn append_payloads(
        &mut self,
        payloads: Vec<SessionEntryPayload>,
        usage: Usage,
    ) -> anyhow::Result<Vec<SessionEntry>> {
        if payloads
            .iter()
            .any(|payload| matches!(payload, SessionEntryPayload::Compaction(_)))
        {
            anyhow::bail!("injected compaction persistence failure");
        }
        self.inner.append_payloads(payloads, usage)
    }
}

#[test]
fn automatic_compaction_uses_manual_path_and_retains_recent_messages() {
    let root = temp_root("auto-compact");
    let mut agent = Agent::builder(config(&root), Workspace::new(root.clone()))
        .data_dir(root.join("data"))
        .compaction(crate::compaction::CompactionSettings {
            enabled: true,
            reserve_tokens: 100,
            keep_recent_tokens: 10,
        })
        .build()
        .unwrap();
    agent.tools = ToolRegistry::new(Vec::new());
    agent.extra_context.clear();
    agent.budget = ContextBudget {
        context_window: Some(1_000),
        reserve_output: 100,
    };
    assert!(agent.commit(
        vec![
            SessionEntryPayload::message(ChatMessage::user_text("x".repeat(4_000)), None),
            SessionEntryPayload::message(
                ChatMessage {
                    role: Role::Assistant,
                    blocks: vec![Block::Text("recent".into())],
                },
                None,
            ),
        ],
        &mut |_| {},
    ));
    let facts_before = agent.entries.len();
    let prompts = Arc::new(Mutex::new(Vec::new()));
    agent.provider = Box::new(ScriptedProvider::with_prompt_log(
        vec![
            ScriptStep::Output(output(
                ChatMessage {
                    role: Role::Assistant,
                    blocks: vec![Block::Text("old work summary".into())],
                },
                StopReason::EndTurn,
            )),
            ScriptStep::Output(output(
                ChatMessage {
                    role: Role::Assistant,
                    blocks: vec![Block::Text("continued".into())],
                },
                StopReason::EndTurn,
            )),
        ],
        Arc::clone(&prompts),
    ));

    let mut events = Vec::new();
    agent.handle_command(
        AgentCommand::UserInput("next".into()),
        &mut |event| events.push(event),
        &AtomicBool::new(false),
    );

    let compaction = agent
        .entries
        .iter()
        .find_map(|entry| match &entry.payload {
            SessionEntryPayload::Compaction(record) => Some(record),
            _ => None,
        })
        .expect("自动压缩应追加 Compaction 事实");
    assert_eq!(compaction.summary, "old work summary");
    assert_eq!(compaction.retained_messages.len(), 2);
    assert_eq!(compaction.retained_messages[0].text(), "recent");
    assert_eq!(compaction.retained_messages[1].text(), "next");
    assert_eq!(agent.entries.len(), facts_before + 3);
    let started = events
        .iter()
        .position(|event| {
            matches!(
                event,
                AgentEvent::CompactionStarted {
                    trigger: crate::event::CompactionTrigger::Automatic,
                    estimated_tokens,
                    available_tokens: Some(900),
                    ..
                } if *estimated_tokens > 900
            )
        })
        .expect("自动压缩应发出 started 事件");
    let finished = events
        .iter()
        .position(|event| {
            matches!(
                event,
                AgentEvent::CompactionFinished {
                    trigger: crate::event::CompactionTrigger::Automatic,
                    summary_chars: 16,
                    retained_messages: 2,
                    ..
                }
            )
        })
        .expect("自动压缩应发出 finished 事件");
    assert!(started < finished);

    let prompts = prompts.lock().unwrap();
    assert_eq!(prompts.len(), 2, "一次摘要调用 + 一次正常模型调用");
    assert_eq!(prompts[0].len(), 1);
    assert!(prompts[0][0]
        .blocks
        .iter()
        .all(|block| matches!(block, Block::Text(_))));
    assert!(prompts[1][0].text().contains("old work summary"));
    assert_eq!(prompts[1][1].text(), "recent");
    assert_eq!(prompts[1][2].text(), "next");
    let old_marker = "x".repeat(100);
    assert!(!prompts[1]
        .iter()
        .any(|message| message.text().contains(old_marker.as_str())));
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn automatic_compaction_failure_and_cancel_do_not_append_a_summary() {
    for (name, step, cancelled) in [
        (
            "failure",
            ScriptStep::Error(ProviderError {
                message: "summary failed".into(),
                retryable: false,
                retry_after: None,
            }),
            false,
        ),
        ("cancel", ScriptStep::Cancel, true),
    ] {
        let root = temp_root(&format!("auto-compact-{name}"));
        let mut agent = Agent::new_with_data_dir(
            config(&root),
            Workspace::new(root.clone()),
            root.join("data"),
        )
        .unwrap();
        agent.budget = ContextBudget {
            context_window: Some(1_000),
            reserve_output: 100,
        };
        agent.compaction_settings = crate::compaction::CompactionSettings {
            enabled: true,
            reserve_tokens: 100,
            keep_recent_tokens: 10,
        };
        assert!(agent.commit(
            vec![SessionEntryPayload::message(
                ChatMessage::user_text("x".repeat(4_000)),
                None,
            )],
            &mut |_| {},
        ));
        let facts_before = agent.entries.len();
        agent.provider = Box::new(ScriptedProvider::new(vec![step]));

        let mut events = Vec::new();
        agent.handle_command(
            AgentCommand::UserInput("next".into()),
            &mut |event| events.push(event),
            &AtomicBool::new(false),
        );

        assert_eq!(
            agent.entries.len(),
            facts_before + 1,
            "只保留已提交的用户输入"
        );
        assert!(!agent
            .entries
            .iter()
            .any(|entry| matches!(entry.payload, SessionEntryPayload::Compaction(_))));
        assert!(events.iter().any(|event| matches!(
            event,
            AgentEvent::CompactionFailed {
                trigger: crate::event::CompactionTrigger::Automatic,
                cancelled: actual,
                history_changed: false,
                ..
            } if *actual == cancelled
        )));
        assert!(events.iter().any(|event| matches!(
            event,
            AgentEvent::TurnFinished { cancelled: actual } if *actual == cancelled
        )));
        let _ = std::fs::remove_dir_all(root);
    }
}

#[test]
fn automatic_compaction_persistence_failure_does_not_advance_memory_facts() {
    let root = temp_root("auto-compact-persist-fail");
    let mut agent = Agent::new_with_data_dir(
        config(&root),
        Workspace::new(root.clone()),
        root.join("data"),
    )
    .unwrap();
    agent.sessions = Box::new(RejectCompactionBackend::new());
    agent.entries.clear();
    agent.budget = ContextBudget {
        context_window: Some(1_000),
        reserve_output: 100,
    };
    agent.compaction_settings = crate::compaction::CompactionSettings {
        enabled: true,
        reserve_tokens: 100,
        keep_recent_tokens: 10,
    };
    assert!(agent.commit(
        vec![SessionEntryPayload::message(
            ChatMessage::user_text("x".repeat(4_000)),
            None,
        )],
        &mut |_| {},
    ));
    let facts_before = agent.entries.len();
    agent.provider = Box::new(ScriptedProvider::new(vec![ScriptStep::Output(output(
        ChatMessage {
            role: Role::Assistant,
            blocks: vec![Block::Text("summary that cannot persist".into())],
        },
        StopReason::EndTurn,
    ))]));

    let mut events = Vec::new();
    agent.handle_command(
        AgentCommand::UserInput("next".into()),
        &mut |event| events.push(event),
        &AtomicBool::new(false),
    );

    assert_eq!(agent.entries.len(), facts_before + 1);
    assert!(!agent
        .entries
        .iter()
        .any(|entry| matches!(entry.payload, SessionEntryPayload::Compaction(_))));
    assert!(events
        .iter()
        .any(|event| matches!(event, AgentEvent::CompactionFailed { error, history_changed: false, .. } if error.contains("压缩事实未写入"))));
    let _ = std::fs::remove_dir_all(root);
}
