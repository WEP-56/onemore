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
