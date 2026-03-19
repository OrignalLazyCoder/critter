use std::{
    fs,
    path::{Path, PathBuf},
};

use chrono::Local;
use rusqlite::{Connection, params};

pub(crate) struct ChatStore {
    conn: Connection,
    max_messages: usize,
}

impl ChatStore {
    pub(crate) fn open(path: &Path, max_messages: usize) -> Result<Self, String> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(|e| {
                format!("failed to create chat store dir {}: {e}", parent.display())
            })?;
        }
        let conn = Connection::open(path)
            .map_err(|e| format!("failed to open chat sqlite {}: {e}", path.display()))?;
        conn.execute_batch(
            "
            PRAGMA journal_mode = WAL;
            PRAGMA synchronous = NORMAL;
            CREATE TABLE IF NOT EXISTS chat_messages (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                ts_epoch INTEGER NOT NULL,
                body TEXT NOT NULL
            );
            CREATE INDEX IF NOT EXISTS idx_chat_messages_ts ON chat_messages(ts_epoch);
            ",
        )
        .map_err(|e| format!("failed to initialize chat store schema: {e}"))?;

        Ok(Self { conn, max_messages })
    }

    pub(crate) fn append(&mut self, body: &str) -> Result<(), String> {
        let now = Local::now().timestamp();
        self.conn
            .execute(
                "INSERT INTO chat_messages(ts_epoch, body) VALUES(?1, ?2)",
                params![now, body],
            )
            .map_err(|e| format!("failed to append chat message: {e}"))?;
        self.prune_excess()
    }

    pub(crate) fn load_recent(&mut self, limit: usize) -> Result<Vec<String>, String> {
        let mut stmt = self
            .conn
            .prepare(
                "
                SELECT body
                FROM chat_messages
                ORDER BY id DESC
                LIMIT ?1
                ",
            )
            .map_err(|e| format!("failed to prepare chat history query: {e}"))?;

        let rows = stmt
            .query_map(params![limit as i64], |row| row.get::<_, String>(0))
            .map_err(|e| format!("failed to query chat history: {e}"))?;

        let mut out = rows.filter_map(Result::ok).collect::<Vec<_>>();
        out.reverse();
        Ok(out)
    }

    fn prune_excess(&mut self) -> Result<(), String> {
        if self.max_messages == 0 {
            return Ok(());
        }
        self.conn
            .execute(
                "
                DELETE FROM chat_messages
                WHERE id NOT IN (
                    SELECT id
                    FROM chat_messages
                    ORDER BY id DESC
                    LIMIT ?1
                )
                ",
                params![self.max_messages as i64],
            )
            .map_err(|e| format!("failed to prune chat history: {e}"))?;
        Ok(())
    }
}

pub(crate) fn resolve_store_path(raw: &str) -> Result<PathBuf, String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err("chat persistence path is empty".to_string());
    }
    if let Some(rest) = trimmed.strip_prefix("~/") {
        let home = std::env::var("HOME").map_err(|_| "HOME env var is not set".to_string())?;
        return Ok(PathBuf::from(home).join(rest));
    }

    let path = PathBuf::from(trimmed);
    if path.is_absolute() {
        return Ok(path);
    }

    let base = crate::system::paths::data_dir()?;
    Ok(base.join(path))
}
