use super::*;
use crate::plan::{PlanItem, PlanSnapshot, PlanStatus};
use crate::session::{NoticeLevel, NoticeRecord};

fn temp_root(name: &str) -> PathBuf {
    std::env::temp_dir().join(format!(
        "onemore-storage-{}-{}-{}",
        name,
        std::process::id(),
        uuid::Uuid::new_v4()
    ))
}

fn message_payload(message: ChatMessage) -> SessionEntryPayload {
    SessionEntryPayload::message(message, None)
}

#[test]
fn workspace_reasoning_preferences_only_store_model_overrides() {
    let root = temp_root("reasoning-preferences");
    let workspaces = root.join("preferences");
    let workspace = root.join("workspace-a");
    let other_workspace = root.join("workspace-b");
    std::fs::create_dir_all(&workspace).unwrap();
    std::fs::create_dir_all(&other_workspace).unwrap();

    let mut preferences = WorkspacePreferences::load(&workspaces, &workspace).unwrap();
    assert_eq!(preferences.effort("openai", "gpt-5"), None);
    preferences
        .set_effort("openai", "gpt-5", "high", "low")
        .unwrap();
    assert!(preferences.path.exists());

    let reloaded = WorkspacePreferences::load(&workspaces, &workspace).unwrap();
    assert_eq!(reloaded.effort("openai", "gpt-5"), Some("high"));
    let other = WorkspacePreferences::load(&workspaces, &other_workspace).unwrap();
    assert_eq!(other.effort("openai", "gpt-5"), None);

    preferences
        .set_effort("openai", "gpt-5", "low", "low")
        .unwrap();
    assert_eq!(preferences.effort("openai", "gpt-5"), None);
    assert!(!preferences.path.exists(), "切回模型默认值应删除空偏好文件");
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn persists_lists_loads_and_clears_session() {
    let root = temp_root("roundtrip");
    let workspace = root.join("workspace");
    std::fs::create_dir_all(&workspace).unwrap();
    let sessions = root.join("sessions");
    let mut manager = SessionManager::create(sessions.clone(), &workspace).unwrap();
    let id = manager.current_id().to_string();
    let appended = manager
        .append_payloads(
            vec![
                message_payload(ChatMessage::user_text("第一条问题")),
                SessionEntryPayload::message_with_prompt(
                    ChatMessage {
                        role: Role::Assistant,
                        blocks: vec![Block::Text("回答".into())],
                    },
                    Usage::default(),
                    Some("sha256:test".into()),
                ),
                SessionEntryPayload::Notice(NoticeRecord {
                    text: "仅 UI 可见".into(),
                    level: NoticeLevel::Info,
                }),
            ],
            Usage {
                input_tokens: 12,
                output_tokens: 7,
                cache: Some(CacheUsage {
                    read_tokens: 8,
                    write_tokens: 3,
                }),
            },
        )
        .unwrap();
    // parent 链:第一条无 parent,之后逐条相连。
    assert_eq!(appended[0].parent_id, None);
    assert_eq!(appended[1].parent_id, Some(appended[0].id.clone()));
    assert_eq!(appended[2].parent_id, Some(appended[1].id.clone()));

    let listed = manager.list().unwrap();
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].title, "第一条问题");
    assert_eq!(listed[0].message_count, 2, "Notice 不计入消息数");

    let mut other = SessionManager::create(sessions, &workspace).unwrap();
    let (loaded, usage) = other.load(&id[..8]).unwrap();
    assert_eq!(loaded.len(), 3);
    assert!(matches!(loaded[2].payload, SessionEntryPayload::Notice(_)));
    assert_eq!(usage.input_tokens, 12);
    assert_eq!(usage.cache.unwrap().read_tokens, 8);
    let SessionEntryPayload::Message(assistant) = &loaded[1].payload else {
        panic!("第二条事实应是 assistant message");
    };
    assert_eq!(assistant.prompt_fingerprint.as_deref(), Some("sha256:test"));
    other.clear().unwrap();
    assert!(other.load(&id).unwrap().0.is_empty());
    // 清空后可以继续追加(leaf 已复位)。
    other
        .append_payloads(
            vec![message_payload(ChatMessage::user_text("再来"))],
            Usage::default(),
        )
        .unwrap();

    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn sessions_are_scoped_to_workspace() {
    let root = temp_root("workspace");
    let first = root.join("first");
    let second = root.join("second");
    std::fs::create_dir_all(&first).unwrap();
    std::fs::create_dir_all(&second).unwrap();
    let sessions = root.join("sessions");
    let first_manager = SessionManager::create(sessions.clone(), &first).unwrap();
    let first_id = first_manager.current_id().to_string();
    let mut second_manager = SessionManager::create(sessions, &second).unwrap();
    assert_eq!(second_manager.list().unwrap().len(), 1);
    assert!(second_manager.load(&first_id).is_err());
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn preserves_tool_messages_as_provider_neutral_json() {
    let root = temp_root("tools");
    let workspace = root.join("workspace");
    std::fs::create_dir_all(&workspace).unwrap();
    let sessions = root.join("sessions");
    let mut manager = SessionManager::create(sessions.clone(), &workspace).unwrap();
    let id = manager.current_id().to_string();
    manager
        .append_payloads(
            vec![
                message_payload(ChatMessage::user_text("读取文件")),
                message_payload(ChatMessage {
                    role: Role::Assistant,
                    blocks: vec![
                        Block::Thinking {
                            text: "先读取".into(),
                            provider_kind: Some("responses".into()),
                            raw: Some(serde_json::json!({"id": "reasoning-1"})),
                        },
                        Block::ToolUse {
                            id: "call-1".into(),
                            name: "read_file".into(),
                            input: serde_json::json!({"path": "README.md"}),
                        },
                    ],
                }),
                message_payload(ChatMessage {
                    role: Role::User,
                    blocks: vec![Block::ToolResult {
                        tool_use_id: "call-1".into(),
                        content: "file body".into(),
                        is_error: false,
                    }],
                }),
            ],
            Usage::default(),
        )
        .unwrap();

    let mut reopened = SessionManager::create(sessions, &workspace).unwrap();
    let (loaded, _) = reopened.load(&id).unwrap();
    assert_eq!(loaded.len(), 3);
    let SessionEntryPayload::Message(assistant) = &loaded[1].payload else {
        panic!("应为 Message 事实");
    };
    assert_eq!(assistant.message.tool_uses()[0].0, "call-1");
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn half_tool_batches_are_rejected_at_commit() {
    let root = temp_root("halfbatch");
    let workspace = root.join("workspace");
    std::fs::create_dir_all(&workspace).unwrap();
    let mut manager = SessionManager::create(root.join("sessions"), &workspace).unwrap();
    let orphan = message_payload(ChatMessage {
        role: Role::Assistant,
        blocks: vec![Block::ToolUse {
            id: "call-1".into(),
            name: "read_file".into(),
            input: serde_json::json!({}),
        }],
    });
    assert!(manager
        .append_payloads(vec![orphan], Usage::default())
        .is_err());
    // 拒绝后日志与 leaf 未变化,正常批仍可提交。
    let appended = manager
        .append_payloads(
            vec![message_payload(ChatMessage::user_text("ok"))],
            Usage::default(),
        )
        .unwrap();
    assert_eq!(appended[0].parent_id, None);
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn plan_facts_are_validated_atomically_at_commit() {
    let root = temp_root("plan-atomic");
    let workspace = root.join("workspace");
    std::fs::create_dir_all(&workspace).unwrap();
    let sessions = root.join("sessions");
    let mut manager = SessionManager::create(sessions.clone(), &workspace).unwrap();
    let invalid = SessionEntryPayload::PlanUpdated(PlanSnapshot {
        revision: 2,
        items: vec![PlanItem {
            id: "inspect".into(),
            text: "Inspect the code".into(),
            status: PlanStatus::InProgress,
        }],
        explanation: None,
    });
    assert!(manager
        .append_payloads(
            vec![
                message_payload(ChatMessage::user_text("before")),
                invalid,
                message_payload(ChatMessage::user_text("after")),
            ],
            Usage::default(),
        )
        .is_err());

    let valid = SessionEntryPayload::PlanUpdated(PlanSnapshot {
        revision: 1,
        items: Vec::new(),
        explanation: Some("clear".into()),
    });
    let appended = manager
        .append_payloads(vec![valid], Usage::default())
        .unwrap();
    assert_eq!(appended.len(), 1, "rejected batch must leave no entries");
    assert_eq!(
        appended[0].parent_id, None,
        "leaf must not advance on failure"
    );

    let mut reopened = SessionManager::create(sessions, &workspace).unwrap();
    let (loaded, _) = reopened.load(manager.current_id()).unwrap();
    assert_eq!(loaded.len(), 1);
    assert!(matches!(
        loaded[0].payload,
        SessionEntryPayload::PlanUpdated(PlanSnapshot { revision: 1, .. })
    ));
    let _ = std::fs::remove_dir_all(root);
}

/// 手工构造一个 v1 库(线性 messages 表,user_version=0)。
fn create_v1_database(sessions: &Path, workspace: &Path, payloads: &[&str]) -> (String, PathBuf) {
    std::fs::create_dir_all(sessions).unwrap();
    let id = uuid::Uuid::new_v4().to_string();
    let path = sessions.join(format!("{}.db", id));
    let connection = Connection::open(&path).unwrap();
    connection
        .execute_batch(
            "CREATE TABLE session (
                     id TEXT PRIMARY KEY,
                     workspace TEXT NOT NULL,
                     title TEXT NOT NULL,
                     created_at INTEGER NOT NULL,
                     updated_at INTEGER NOT NULL,
                     input_tokens INTEGER NOT NULL,
                     output_tokens INTEGER NOT NULL
                 );
                 CREATE TABLE messages (
                     sequence INTEGER PRIMARY KEY AUTOINCREMENT,
                     payload TEXT NOT NULL
                 );",
        )
        .unwrap();
    connection
        .execute(
            "INSERT INTO session VALUES (?1, ?2, '旧标题', 100, 200, 3, 4)",
            params![id, workspace_key(workspace)],
        )
        .unwrap();
    for payload in payloads {
        connection
            .execute("INSERT INTO messages (payload) VALUES (?1)", [payload])
            .unwrap();
    }
    (id, path)
}

#[test]
fn v1_databases_migrate_to_entries_preserving_order() {
    let root = temp_root("migrate-ok");
    let workspace = root.join("workspace");
    std::fs::create_dir_all(&workspace).unwrap();
    let sessions = root.join("sessions");
    let user = serde_json::to_string(&ChatMessage::user_text("旧问题")).unwrap();
    let assistant = serde_json::to_string(&ChatMessage {
        role: Role::Assistant,
        blocks: vec![Block::Text("旧回答".into())],
    })
    .unwrap();
    let (id, _path) = create_v1_database(&sessions, &workspace, &[&user, &assistant]);

    let mut manager = SessionManager::create(sessions, &workspace).unwrap();
    let (entries, usage) = manager.load(&id).unwrap();
    assert_eq!(entries.len(), 2);
    assert_eq!(entries[0].parent_id, None);
    assert_eq!(entries[1].parent_id, Some(entries[0].id.clone()));
    let SessionEntryPayload::Message(first) = &entries[0].payload else {
        panic!("应迁移成 Message 事实");
    };
    assert_eq!(first.message.text(), "旧问题");
    assert_eq!(first.usage, None, "v1 无逐消息 usage");
    assert_eq!(usage.input_tokens, 3);
    // 迁移后可以继续追加。
    manager
        .append_payloads(
            vec![message_payload(ChatMessage::user_text("新问题"))],
            usage,
        )
        .unwrap();
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn v2_schema_gains_nullable_cache_usage_columns() {
    let mut connection = Connection::open_in_memory().unwrap();
    connection
        .execute_batch(
            "CREATE TABLE session (
                     id TEXT PRIMARY KEY,
                     workspace TEXT NOT NULL,
                     title TEXT NOT NULL,
                     created_at INTEGER NOT NULL,
                     updated_at INTEGER NOT NULL,
                     input_tokens INTEGER NOT NULL,
                     output_tokens INTEGER NOT NULL,
                     leaf_id TEXT
                 );
                 CREATE TABLE entries (
                     sequence INTEGER PRIMARY KEY AUTOINCREMENT,
                     id TEXT NOT NULL UNIQUE,
                     parent_id TEXT,
                     kind TEXT NOT NULL,
                     payload TEXT NOT NULL,
                     created_at INTEGER NOT NULL
                 );
                 PRAGMA user_version = 2;",
        )
        .unwrap();

    initialize(&mut connection).unwrap();
    assert_eq!(schema_version(&connection).unwrap(), SCHEMA_VERSION);
    assert!(column_exists(&connection, "session", "cache_read_tokens").unwrap());
    assert!(column_exists(&connection, "session", "cache_write_tokens").unwrap());
}

#[test]
fn failed_migration_rolls_back_and_keeps_v1_database() {
    let root = temp_root("migrate-fail");
    let workspace = root.join("workspace");
    std::fs::create_dir_all(&workspace).unwrap();
    let sessions = root.join("sessions");
    let good = serde_json::to_string(&ChatMessage::user_text("好消息")).unwrap();
    let (id, path) = create_v1_database(&sessions, &workspace, &[&good, "{not json"]);

    let mut manager = SessionManager::create(sessions, &workspace).unwrap();
    let error = manager.load(&id).unwrap_err();
    assert!(
        format!("{:#}", error).contains("迁移中止"),
        "错误应说明迁移失败: {:#}",
        error
    );

    // 原库应保持 v1:messages 表仍在,user_version 仍为 0,数据完整。
    let connection = Connection::open(&path).unwrap();
    assert_eq!(schema_version(&connection).unwrap(), 0);
    let count: i64 = connection
        .query_row("SELECT COUNT(*) FROM messages", [], |row| row.get(0))
        .unwrap();
    assert_eq!(count, 2);
    let _ = std::fs::remove_dir_all(root);
}
