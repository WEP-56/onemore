use super::*;

#[test]
fn builder_defaults_match_legacy_constructor() {
    let root = temp_root("builder-defaults");
    let workspace_root = root.join("workspace");
    std::fs::create_dir_all(&workspace_root).unwrap();
    let direct = Agent::new_with_data_dir(
        multi_model_config(&root),
        Workspace::new(workspace_root.clone()),
        root.join("direct-data"),
    )
    .unwrap();
    let built = Agent::builder(multi_model_config(&root), Workspace::new(workspace_root))
        .data_dir(root.join("builder-data"))
        .build()
        .unwrap();

    assert_eq!(direct.active_selection, built.active_selection);
    assert_eq!(direct.provider_label(), built.provider_label());
    assert_eq!(direct.max_turns, 200);
    assert_eq!(built.max_turns, 200);
    assert_eq!(direct.retry_policy.max_attempts, 8);
    assert_eq!(built.retry_policy.max_attempts, 8);
    assert_eq!(
        direct
            .tools
            .specs()
            .into_iter()
            .map(|spec| spec.name)
            .collect::<Vec<_>>(),
        built
            .tools
            .specs()
            .into_iter()
            .map(|spec| spec.name)
            .collect::<Vec<_>>()
    );
    assert_eq!(
        direct.build_system_prompt().system_sections,
        built.build_system_prompt().system_sections
    );
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn builder_uses_retry_and_turn_limits_from_config() {
    let root = temp_root("builder-config-retry");
    let workspace_root = root.join("workspace");
    std::fs::create_dir_all(&workspace_root).unwrap();
    let config_path = root.join("config.toml");
    std::fs::write(
        &config_path,
        r#"
[agent]
provider = "mock"
max_turns = 41
[retry]
max_attempts = 5
base_delay_ms = 12
max_delay_ms = 120
max_retry_after_ms = 1200
[providers.mock]
api = "responses"
base_url = "http://127.0.0.1:1"
api_key = ""
model = "scripted"
"#,
    )
    .unwrap();
    let agent = Agent::builder(
        Config::load(&config_path).unwrap(),
        Workspace::new(workspace_root),
    )
    .data_dir(root.join("data"))
    .build()
    .unwrap();

    assert_eq!(agent.max_turns, 41);
    assert_eq!(agent.retry_policy.max_attempts, 5);
    assert_eq!(agent.retry_policy.base_delay, Duration::from_millis(12));
    assert_eq!(agent.retry_policy.max_delay, Duration::from_millis(120));
    assert_eq!(
        agent.retry_policy.max_retry_after,
        Duration::from_millis(1200)
    );
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn provider_factory_is_reused_after_model_switch() {
    let root = temp_root("provider-factory");
    let workspace_root = root.join("workspace");
    std::fs::create_dir_all(&workspace_root).unwrap();
    let created_models = Arc::new(Mutex::new(Vec::new()));
    let factory_log = Arc::clone(&created_models);
    let mut agent = Agent::builder(multi_model_config(&root), Workspace::new(workspace_root))
        .data_dir(root.join("data"))
        .provider_factory(move |settings| {
            factory_log.lock().unwrap().push(settings.model);
            Box::new(ScriptedProvider::new(Vec::new()))
        })
        .build()
        .unwrap();

    let mut events = Vec::new();
    agent.handle_command(
        AgentCommand::SelectModel {
            model: "large".into(),
            effort: "high".into(),
        },
        &mut |event| events.push(event),
        &AtomicBool::new(false),
    );

    assert_eq!(*created_models.lock().unwrap(), ["small", "large"]);
    assert_eq!(agent.active_selection.model, "large");
    assert!(events.iter().any(
        |event| matches!(event, AgentEvent::ModelSelectionChanged { model, .. } if model == "large")
    ));
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn builder_injected_components_are_active() {
    let root = temp_root("builder-components");
    let workspace_root = root.join("workspace");
    std::fs::create_dir_all(&workspace_root).unwrap();
    std::fs::write(
        workspace_root.join("AGENTS.md"),
        "this must not leak through an exact context replacement",
    )
    .unwrap();
    let hook_calls = Arc::new(AtomicUsize::new(0));
    let retry_policy = RetryPolicy {
        max_attempts: 7,
        base_delay: Duration::from_millis(11),
        max_delay: Duration::from_millis(22),
        max_retry_after: Duration::from_millis(33),
        jitter_seed: 44,
    };
    let denied = PermissionRules {
        workspace_read: PermissionRule::Deny,
        workspace_write: PermissionRule::Deny,
        outside_workspace: PermissionRule::Deny,
        opaque_side_effect: PermissionRule::Deny,
    };
    let executions = Arc::new(AtomicUsize::new(0));
    let mut agent = Agent::builder(multi_model_config(&root), Workspace::new(workspace_root))
        .data_dir(root.join("data"))
        .tools(ToolRegistry::new(vec![Box::new(CountedTool {
            executions,
            capabilities: ToolCapabilities::READ_ONLY,
            permission: ToolPermissionSpec::default(),
        })]))
        .context_providers(vec![Box::new(HostContext)])
        .hooks(HookRegistry::new(vec![Box::new(PreventStopHook {
            calls: Arc::clone(&hook_calls),
        })]))
        .permissions(PermissionManager::new(denied))
        .retry_policy(retry_policy)
        .build()
        .unwrap();

    assert_eq!(
        agent
            .tools
            .specs()
            .into_iter()
            .map(|spec| spec.name)
            .collect::<Vec<_>>(),
        ["counted"]
    );
    assert_eq!(
        agent.build_system_prompt().system_sections,
        ["host context"]
    );
    let prepared = agent
        .tools
        .prepare("counted", &serde_json::json!({ "path": "file.txt" }))
        .unwrap();
    assert!(matches!(
        agent.permissions.evaluate(&prepared, &agent.workspace),
        PermissionDecision::Deny { .. }
    ));
    let stop = agent
        .hooks
        .run_stop(&ChatMessage::empty_assistant(), agent.sessions.current_id());
    assert_eq!(
        stop.prevent_stop.as_deref(),
        Some("Hook verify_once: run verification")
    );
    assert_eq!(hook_calls.load(Ordering::Relaxed), 1);
    assert_eq!(agent.retry_policy.max_attempts, 7);
    assert_eq!(agent.retry_policy.jitter_seed, 44);
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn default_context_freezes_root_agents_in_pi_style_order() {
    let root = temp_root("agents-context");
    let workspace_root = root.join("workspace");
    std::fs::create_dir_all(workspace_root.join(".onemore/skills/demo")).unwrap();
    std::fs::write(
        workspace_root.join("AGENTS.md"),
        "Keep production paths unique.",
    )
    .unwrap();
    std::fs::write(
        workspace_root.join(".onemore/skills/demo/SKILL.md"),
        "---\nname: demo\ndescription: demo skill\n---\nbody",
    )
    .unwrap();

    let agent = Agent::builder(
        multi_model_config(&root),
        Workspace::new(workspace_root.clone()),
    )
    .data_dir(root.join("data"))
    .system_prompt(Some("host-owned base prompt".into()))
    .build()
    .unwrap();
    let sections = agent.build_system_prompt().system_sections;

    assert_eq!(sections.len(), 4);
    assert_eq!(sections[0], "host-owned base prompt");
    assert!(sections[1].starts_with("<project_context>"));
    assert!(sections[1].contains("Keep production paths unique."));
    assert!(sections[1].contains(&workspace_root.join("AGENTS.md").display().to_string()));
    assert!(sections[2].starts_with("<available_skills>"));
    assert!(sections[3].starts_with("Environment:\n- Working directory:"));
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn invalid_root_agents_is_a_non_fatal_startup_notice() {
    let root = temp_root("agents-warning");
    let workspace_root = root.join("workspace");
    std::fs::create_dir_all(&workspace_root).unwrap();
    std::fs::write(workspace_root.join("AGENTS.md"), [0xff, 0xfe]).unwrap();
    let mut agent = Agent::builder(multi_model_config(&root), Workspace::new(workspace_root))
        .data_dir(root.join("data"))
        .build()
        .unwrap();

    let mut events = Vec::new();
    agent.handle_command(
        AgentCommand::ListSessions { all: false },
        &mut |event| events.push(event),
        &AtomicBool::new(false),
    );
    assert!(matches!(
        events.first(),
        Some(AgentEvent::Notice(message)) if message.contains("AGENTS.md")
    ));
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn direct_in_memory_builder_needs_no_config_state_or_skills_directories() {
    let root = temp_root("in-memory-builder");
    let workspace_root = root.join("workspace");
    let state_root = root.join("must-not-exist");
    std::fs::create_dir_all(&workspace_root).unwrap();
    let mut agent = Agent::builder_from_provider(
        ProviderSettings {
            name: "embedded".into(),
            api: ApiKind::Responses,
            profile: ProviderProfile::OpenAiResponses,
            base_url: "http://127.0.0.1:1".into(),
            api_key: String::new(),
            model: "scripted".into(),
            max_tokens: Some(4096),
            context_window: Some(32_000),
            selected_effort: "medium".into(),
            reasoning_effort: ReasoningEffortPolicy::Omit,
        },
        Workspace::new(workspace_root),
    )
    .data_dir(state_root.clone())
    .in_memory()
    .provider_factory(|_| {
        Box::new(ScriptedProvider::new(vec![ScriptStep::Output(output(
            ChatMessage::empty_assistant(),
            StopReason::EndTurn,
        ))]))
    })
    .build()
    .unwrap();

    assert!(!root.join("config.toml").exists());
    assert!(!state_root.exists());
    assert!(!agent
        .tools
        .specs()
        .iter()
        .any(|spec| spec.name == "load_skill"));
    assert!(!agent
        .build_system_prompt()
        .system_text()
        .contains("load_skill"));

    let mut events = Vec::new();
    agent.handle_command(
        AgentCommand::UserInput("hello".into()),
        &mut |event| events.push(event),
        &AtomicBool::new(false),
    );
    agent.handle_command(
        AgentCommand::ListSessions { all: false },
        &mut |event| events.push(event),
        &AtomicBool::new(false),
    );

    assert!(!events
        .iter()
        .any(|event| matches!(event, AgentEvent::SkillsDiscovered { .. })));
    assert!(events.iter().any(|event| matches!(
        event,
        AgentEvent::SessionsListed { sessions, .. }
            if sessions.len() == 1
                && sessions[0].message_count == 2
                && sessions[0].workspace == root.join("workspace").display().to_string()
    )));
    assert!(!state_root.exists());
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn startup_emits_frozen_skill_catalog_once() {
    let root = temp_root("skills-event");
    let workspace_root = root.join("workspace");
    std::fs::create_dir_all(workspace_root.join(".onemore/skills/demo")).unwrap();
    std::fs::write(
        workspace_root.join(".onemore/skills/demo/SKILL.md"),
        "---\nname: demo\ndescription: demo skill\n---\nbody",
    )
    .unwrap();
    let mut agent = Agent::new_with_data_dir(
        multi_model_config(&root),
        Workspace::new(workspace_root),
        root.join("data"),
    )
    .unwrap();
    let mut events = Vec::new();
    agent.handle_command(
        AgentCommand::ListSessions { all: false },
        &mut |event| events.push(event),
        &AtomicBool::new(false),
    );
    assert!(matches!(
        events.first(),
        Some(AgentEvent::SkillsDiscovered { skills, warnings })
            if skills.iter().any(|skill| skill.name == "demo") && warnings.is_empty()
    ));
    events.clear();
    agent.handle_command(
        AgentCommand::ListSessions { all: false },
        &mut |event| events.push(event),
        &AtomicBool::new(false),
    );
    assert!(!events
        .iter()
        .any(|event| matches!(event, AgentEvent::SkillsDiscovered { .. })));
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn model_selection_updates_budget_records_one_fact_and_persists_effort() {
    let root = temp_root("model-selection");
    let workspace_root = root.join("workspace");
    std::fs::create_dir_all(&workspace_root).unwrap();
    let data_dir = root.join("data");
    let mut agent = Agent::new_with_data_dir(
        multi_model_config(&root),
        Workspace::new(workspace_root.clone()),
        data_dir.clone(),
    )
    .unwrap();
    let mut events = Vec::new();
    agent.handle_command(
        AgentCommand::SelectModel {
            model: "large".into(),
            effort: "high".into(),
        },
        &mut |event| events.push(event),
        &AtomicBool::new(false),
    );

    assert_eq!(agent.active_selection.model, "large");
    assert_eq!(agent.active_selection.effort, "high");
    assert_eq!(agent.budget.context_window, Some(200000));
    assert_eq!(agent.budget.reserve_output, 32000);
    assert_eq!(
        agent
            .entries
            .iter()
            .filter(|entry| matches!(&entry.payload, SessionEntryPayload::ModelChange(_)))
            .count(),
        1
    );
    assert_eq!(
        events
            .iter()
            .filter(|event| matches!(event, AgentEvent::ModelSelectionChanged { .. }))
            .count(),
        1
    );

    let restarted = Agent::new_with_data_dir(
        multi_model_config(&root),
        Workspace::new(workspace_root),
        data_dir,
    )
    .unwrap();
    // 新启动仍从 provider 默认模型开始；偏好是按模型保存的，不会泄漏到 small。
    assert_eq!(restarted.active_selection.model, "small");
    assert_eq!(restarted.active_selection.effort, DEFAULT_REASONING_EFFORT);
    assert_eq!(
        restarted.model_preferences.effort("mock", "large"),
        Some("high")
    );
    let _ = std::fs::remove_dir_all(root);
}
