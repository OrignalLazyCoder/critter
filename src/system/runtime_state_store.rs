use chrono::Utc;
use rusqlite::{Connection, params};

use crate::core::shared_state::SharedState;

pub struct RuntimeStateStore {
    conn: Connection,
}

impl RuntimeStateStore {
    pub fn open_default() -> Result<Self, String> {
        let db_dir = crate::system::paths::data_dir()?;
        let db_path = db_dir.join("state.sqlite3");
        let conn = Connection::open(&db_path)
            .map_err(|e| format!("failed to open sqlite db {}: {e}", db_path.display()))?;
        conn.execute_batch(
            "
            PRAGMA journal_mode = WAL;
            PRAGMA synchronous = NORMAL;
            CREATE TABLE IF NOT EXISTS runtime_state (
                id INTEGER PRIMARY KEY CHECK (id = 1),
                payload TEXT NOT NULL,
                updated_epoch INTEGER NOT NULL
            );
            ",
        )
        .map_err(|e| format!("failed to initialize runtime_state schema: {e}"))?;
        Ok(Self { conn })
    }

    pub fn save(&mut self, state: &SharedState) -> Result<(), String> {
        let payload = serde_json::to_string(state)
            .map_err(|e| format!("failed to encode runtime_state payload: {e}"))?;
        let now = Utc::now().timestamp();
        self.conn
            .execute(
                "
                INSERT INTO runtime_state(id, payload, updated_epoch)
                VALUES(1, ?1, ?2)
                ON CONFLICT(id) DO UPDATE SET
                    payload = excluded.payload,
                    updated_epoch = excluded.updated_epoch
                ",
                params![payload, now],
            )
            .map_err(|e| format!("failed to save runtime_state: {e}"))?;
        Ok(())
    }

    pub fn load(&self) -> Result<Option<SharedState>, String> {
        let mut stmt = self
            .conn
            .prepare("SELECT payload FROM runtime_state WHERE id = 1")
            .map_err(|e| format!("failed to prepare runtime_state query: {e}"))?;
        let mut rows = stmt
            .query([])
            .map_err(|e| format!("failed to query runtime_state: {e}"))?;
        let Some(row) = rows
            .next()
            .map_err(|e| format!("failed to fetch runtime_state row: {e}"))?
        else {
            return Ok(None);
        };
        let raw: String = row
            .get(0)
            .map_err(|e| format!("failed to parse runtime_state payload: {e}"))?;
        let state: SharedState = serde_json::from_str(&raw)
            .map_err(|e| format!("failed to decode runtime_state payload: {e}"))?;
        Ok(Some(state))
    }
}
