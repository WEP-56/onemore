use super::*;

#[test]
fn plan_events_and_success_finish_are_emitted_after_commit() {
    let root = temp_root("plan-commit-event");
    let data_dir = root.join("data");
    let mut agent = Agent::new_with_data_dir(
        config(&root),
        Workspace::new(root.clone()),
        data_dir.clone(),
    )
    .unwrap();
    agent.provider = Box::new(ScriptedProvider::new(vec![
        ScriptStep::Output(update_plan_turn(serde_json::json!([
            {"id": "done", "text": "Already complete", "status": "completed"}
        ]))),
        ScriptStep::Output(output(
            ChatMessage {
                role: Role::Assistant,
                blocks: vec![Block::Text("finished".into())],
            },
            StopReason::EndTurn,
        )),
    ]));
    let database = data_dir
        .join("sessions")
        .join(format!("{}.db", agent.session_id()));
    let mut events = Vec::new();
    agent.handle_command(
        AgentCommand::UserInput("make a plan".into()),
        &mut |event| {
            if matches!(
                &event,
                AgentEvent::ToolCallFinished { name, error: None, .. }
                    if name == "update_plan"
            ) || matches!(&event, AgentEvent::PlanUpdated { .. })
            {
                let connection = rusqlite::Connection::open(&database).unwrap();
                let count: i64 = connection
                    .query_row(
                        "SELECT COUNT(*) FROM entries WHERE kind = 'plan_updated'",
                        [],
                        |row| row.get(0),
                    )
                    .unwrap();
                assert_eq!(count, 1, "success events must observe committed state");
            }
            events.push(event);
        },
        &AtomicBool::new(false),
    );

    let fact_index = agent
        .entries
        .iter()
        .position(|entry| matches!(entry.payload, SessionEntryPayload::PlanUpdated(_)))
        .unwrap();
    assert!(matches!(
        agent.entries[fact_index - 1].payload,
        SessionEntryPayload::Message(_)
    ));
    assert!(matches!(
        agent.entries[fact_index + 1].payload,
        SessionEntryPayload::Message(_)
    ));
    let finished = events
            .iter()
            .position(|event| matches!(event, AgentEvent::ToolCallFinished { name, .. } if name == "update_plan"))
            .unwrap();
    let updated = events
        .iter()
        .position(|event| matches!(event, AgentEvent::PlanUpdated { revision: 1, .. }))
        .unwrap();
    assert!(finished < updated);
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn active_plan_triggers_only_one_continuation_reminder() {
    let root = temp_root("plan-reminder");
    let prompts = Arc::new(Mutex::new(Vec::new()));
    let mut agent = Agent::new_with_data_dir(
        config_with_max_turns(&root, 4),
        Workspace::new(root.clone()),
        root.join("data"),
    )
    .unwrap();
    agent.provider = Box::new(ScriptedProvider::with_prompt_log(
        vec![
            ScriptStep::Output(update_plan_turn(serde_json::json!([
                {"id": "work", "text": "Do the work", "status": "in_progress"}
            ]))),
            ScriptStep::Output(output(
                ChatMessage {
                    role: Role::Assistant,
                    blocks: vec![Block::Text("stopping early".into())],
                },
                StopReason::EndTurn,
            )),
            ScriptStep::Output(output(
                ChatMessage {
                    role: Role::Assistant,
                    blocks: vec![Block::Text("still stopping".into())],
                },
                StopReason::EndTurn,
            )),
        ],
        Arc::clone(&prompts),
    ));
    let mut events = Vec::new();
    agent.handle_command(
        AgentCommand::UserInput("long task".into()),
        &mut |event| events.push(event),
        &AtomicBool::new(false),
    );

    assert_eq!(prompts.lock().unwrap().len(), 3);
    assert_eq!(
        agent
            .entries
            .iter()
            .filter(|entry| matches!(entry.payload, SessionEntryPayload::PlanReminder(_)))
            .count(),
        1
    );
    assert_eq!(
            events
                .iter()
                .filter(|event| matches!(event, AgentEvent::Notice(text) if text.contains("要求模型继续一次")))
                .count(),
            1
        );
    assert_eq!(reduce_plan(&agent.entries).snapshot.revision, 1);
    assert!(events
        .iter()
        .any(|event| matches!(event, AgentEvent::TurnFinished { cancelled: false })));
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn cancellation_returns_in_progress_item_to_pending_as_a_new_fact() {
    let root = temp_root("plan-cancel");
    let mut agent = Agent::new_with_data_dir(
        config(&root),
        Workspace::new(root.clone()),
        root.join("data"),
    )
    .unwrap();
    agent.provider = Box::new(ScriptedProvider::new(vec![
        ScriptStep::Output(update_plan_turn(serde_json::json!([
            {"id": "work", "text": "Do the work", "status": "in_progress"}
        ]))),
        ScriptStep::Cancel,
    ]));
    let mut events = Vec::new();
    agent.handle_command(
        AgentCommand::UserInput("cancel me".into()),
        &mut |event| events.push(event),
        &AtomicBool::new(false),
    );

    let plan = reduce_plan(&agent.entries).snapshot;
    assert_eq!(plan.revision, 2);
    assert_eq!(plan.items[0].status, PlanStatus::Pending);
    assert_eq!(
        agent
            .entries
            .iter()
            .filter(|entry| matches!(entry.payload, SessionEntryPayload::PlanUpdated(_)))
            .count(),
        2
    );
    assert!(events
        .iter()
        .any(|event| matches!(event, AgentEvent::PlanUpdated { revision: 2, .. })));
    assert!(events
        .iter()
        .any(|event| matches!(event, AgentEvent::TurnFinished { cancelled: true })));
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn compaction_keeps_only_active_plan_items_and_completed_count() {
    let root = temp_root("compact-plan");
    let mut agent = Agent::new_with_data_dir(
        config(&root),
        Workspace::new(root.clone()),
        root.join("data"),
    )
    .unwrap();
    assert!(agent.commit(
        vec![
            SessionEntryPayload::message(ChatMessage::user_text("task"), None),
            SessionEntryPayload::PlanUpdated(PlanSnapshot {
                revision: 1,
                items: vec![
                    PlanItem {
                        id: "done".into(),
                        text: "Completed detail must be folded".into(),
                        status: PlanStatus::Completed,
                    },
                    PlanItem {
                        id: "active".into(),
                        text: "Active detail must remain".into(),
                        status: PlanStatus::Pending,
                    },
                ],
                explanation: None,
            }),
        ],
        &mut |_| {},
    ));
    agent.provider = Box::new(ScriptedProvider::new(vec![ScriptStep::Output(output(
        ChatMessage {
            role: Role::Assistant,
            blocks: vec![Block::Text("base summary".into())],
        },
        StopReason::EndTurn,
    ))]));
    agent.handle_command(AgentCommand::Compact, &mut |_| {}, &AtomicBool::new(false));

    let summary = agent
        .entries
        .iter()
        .rev()
        .find_map(|entry| match &entry.payload {
            SessionEntryPayload::Compaction(compaction) => Some(compaction.summary.as_str()),
            _ => None,
        })
        .unwrap();
    assert!(summary.contains("Active plan revision 1; 1 completed"));
    assert!(summary.contains("Active detail must remain"));
    assert!(!summary.contains("Completed detail must be folded"));
    let _ = std::fs::remove_dir_all(root);
}
