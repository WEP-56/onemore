//! Session 列表：扫描 roaming/onemore/sessions/*.db，读取 session 表。
//! RPC 协议的 list_sessions 只返回当前 workspace 的会话，
//! 这里直接读 SQLite 以便在未连接或跨 workspace 时展示历史。

use std::fs;

use rusqlite::Connection;
use serde::Serialize;

use crate::config::sessions_dir;
use crate::error::GuiError;

#[derive(Debug, Clone, Serialize)]
pub struct SessionEntry {
    pub id: String,
    pub workspace: String,
    pub title: String,
    pub created_at: u64,
    pub updated_at: u64,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub message_count: u64,
}

/// 列出所有 session DB 中的会话，按 updated_at 降序。
pub fn list_all_sessions() -> Result<Vec<SessionEntry>, GuiError> {
    let dir = sessions_dir()?;
    if !dir.exists() {
        return Ok(Vec::new());
    }

    let mut entries = Vec::new();

    let db_files: Vec<_> = fs::read_dir(&dir)
        .map_err(|e| GuiError::new("list_sessions", e.to_string()))?
        .filter_map(|e| e.ok())
        .filter(|e| {
            e.file_name()
                .to_str()
                .map(|n| n.ends_with(".db"))
                .unwrap_or(false)
        })
        .collect();

    for db_file in db_files {
        let path = db_file.path();
        let conn = match Connection::open_with_flags(
            &path,
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX,
        ) {
            Ok(c) => c,
            Err(_) => continue,
        };

        let mut session_rows: Vec<SessionEntry> = Vec::new();

        {
            let mut stmt = match conn.prepare(
                "SELECT id, workspace, title, created_at, updated_at, input_tokens, output_tokens FROM session",
            ) {
                Ok(s) => s,
                Err(_) => continue,
            };
            let rows = stmt.query_map([], |row| {
                Ok(SessionEntry {
                    id: row.get(0)?,
                    workspace: row.get(1)?,
                    title: row.get(2)?,
                    created_at: row.get(3)?,
                    updated_at: row.get(4)?,
                    input_tokens: row.get(5)?,
                    output_tokens: row.get(6)?,
                    message_count: 0,
                })
            });
            if let Ok(iter) = rows {
                for r in iter {
                    if let Ok(s) = r {
                        session_rows.push(s);
                    }
                }
            }
        }

        for s in &mut session_rows {
            if let Ok(count) = conn.query_row::<u64, _, _>(
                "SELECT COUNT(*) FROM entries WHERE kind = 'message'",
                [],
                |row| row.get(0),
            ) {
                s.message_count = count;
            }
        }
        entries.extend(session_rows);
    }

    entries.sort_by(|a, b| b.updated_at.cmp(&a.updated_at));
    Ok(entries)
}

/// 定位 session 所在的 DB 文件(sessions 目录下任一 .db)。
fn find_session_db(session_id: &str) -> Result<std::path::PathBuf, GuiError> {
    let dir = sessions_dir()?;
    if !dir.exists() {
        return Err(GuiError::new("session_not_found", "会话不存在"));
    }
    for entry in fs::read_dir(&dir).map_err(|e| GuiError::new("list_sessions", e.to_string()))? {
        let entry = entry.map_err(|e| GuiError::new("list_sessions", e.to_string()))?;
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("db") {
            continue;
        }
        let conn = match Connection::open_with_flags(
            &path,
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX,
        ) {
            Ok(c) => c,
            Err(_) => continue,
        };
        let found: bool = conn
            .query_row("SELECT EXISTS(SELECT 1 FROM session WHERE id = ?1)", [session_id], |r| {
                r.get(0)
            })
            .unwrap_or(false);
        if found {
            return Ok(path);
        }
    }
    Err(GuiError::new("session_not_found", "会话不存在"))
}

/// 重命名会话。
pub fn rename_session(session_id: &str, title: &str) -> Result<(), GuiError> {
    let path = find_session_db(session_id)?;
    let conn = Connection::open(&path).map_err(|e| GuiError::new("rename_session", e.to_string()))?;
    conn.execute(
        "UPDATE session SET title = ?1, updated_at = ?2 WHERE id = ?3",
        rusqlite::params![title, now_secs(), session_id],
    )
    .map_err(|e| GuiError::new("rename_session", e.to_string()))?;
    Ok(())
}

/// 删除会话:删除 session 行及其 leaf 链上的全部 entries。
pub fn delete_session(session_id: &str) -> Result<(), GuiError> {
    let path = find_session_db(session_id)?;
    let mut conn = Connection::open(&path).map_err(|e| GuiError::new("delete_session", e.to_string()))?;
    let tx = conn
        .transaction()
        .map_err(|e| GuiError::new("delete_session", e.to_string()))?;

    // 沿 parent_id 从 leaf 回溯收集全部 entry id。
    let leaf_id: Option<String> = tx
        .query_row("SELECT leaf_id FROM session WHERE id = ?1", [session_id], |r| r.get(0))
        .map_err(|e| GuiError::new("delete_session", e.to_string()))?;

    let mut chain: Vec<String> = Vec::new();
    let mut current: Option<String> = leaf_id;
    let mut guard = 0usize;
    while let Some(id) = current {
        if guard > 100_000 {
            break;
        }
        guard += 1;
        chain.push(id.clone());
        current = tx
            .query_row(
                "SELECT parent_id FROM entries WHERE id = ?1",
                [&id],
                |r| r.get::<_, Option<String>>(0),
            )
            .map_err(|e| GuiError::new("delete_session", e.to_string()))?;
    }

    for id in &chain {
        tx.execute("DELETE FROM entries WHERE id = ?1", [id])
            .map_err(|e| GuiError::new("delete_session", e.to_string()))?;
    }
    tx.execute("DELETE FROM session WHERE id = ?1", [session_id])
        .map_err(|e| GuiError::new("delete_session", e.to_string()))?;
    tx.commit()
        .map_err(|e| GuiError::new("delete_session", e.to_string()))?;
    Ok(())
}

fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}
