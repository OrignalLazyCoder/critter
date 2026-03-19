use chrono::{Local, TimeZone};
use rusqlite::{Connection, params};

const RETENTION_SECS: i64 = 24 * 60 * 60;
const PRUNE_EVERY_WRITES: u32 = 32;

pub(crate) struct EventStore {
    conn: Connection,
    writes_since_prune: u32,
}

impl EventStore {
    pub(crate) fn open_default() -> Result<Self, String> {
        let db_dir = crate::system::paths::data_dir()?;
        let db_path = db_dir.join("events.sqlite3");

        let conn = Connection::open(&db_path)
            .map_err(|e| format!("failed to open sqlite db {}: {e}", db_path.display()))?;

        conn.execute_batch(
            "
            PRAGMA journal_mode = WAL;
            PRAGMA synchronous = NORMAL;
            CREATE TABLE IF NOT EXISTS events (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                ts_epoch INTEGER NOT NULL,
                kind TEXT NOT NULL,
                label TEXT NOT NULL,
                detail TEXT NOT NULL
            );
            CREATE INDEX IF NOT EXISTS idx_events_ts ON events(ts_epoch);
            CREATE INDEX IF NOT EXISTS idx_events_kind_ts ON events(kind, ts_epoch);
            ",
        )
        .map_err(|e| format!("failed to init sqlite schema: {e}"))?;

        let mut store = Self {
            conn,
            writes_since_prune: 0,
        };
        store.prune_old()?;
        Ok(store)
    }

    pub(crate) fn record(&mut self, kind: &str, label: &str, detail: &str) -> Result<(), String> {
        let now = Local::now().timestamp();
        self.conn
            .execute(
                "INSERT INTO events(ts_epoch, kind, label, detail) VALUES (?1, ?2, ?3, ?4)",
                params![now, kind, label, detail],
            )
            .map_err(|e| format!("failed to insert event: {e}"))?;

        self.writes_since_prune = self.writes_since_prune.saturating_add(1);
        if self.writes_since_prune >= PRUNE_EVERY_WRITES {
            self.writes_since_prune = 0;
            self.prune_old()?;
        }
        Ok(())
    }

    pub(crate) fn recent_lines(&mut self, limit: usize) -> Result<Vec<String>, String> {
        self.prune_old()?;
        let mut stmt = self
            .conn
            .prepare(
                "
                SELECT ts_epoch, kind, label, detail
                FROM events
                ORDER BY ts_epoch DESC, id DESC
                LIMIT ?1
                ",
            )
            .map_err(|e| format!("failed to prepare recent query: {e}"))?;

        let mut rows = stmt
            .query(params![limit as i64])
            .map_err(|e| format!("failed to run recent query: {e}"))?;

        let mut out = Vec::new();
        while let Some(row) = rows
            .next()
            .map_err(|e| format!("failed to fetch row: {e}"))?
        {
            let ts: i64 = row.get(0).map_err(|e| format!("bad row ts_epoch: {e}"))?;
            let kind: String = row.get(1).map_err(|e| format!("bad row kind: {e}"))?;
            let label: String = row.get(2).map_err(|e| format!("bad row label: {e}"))?;
            let detail: String = row.get(3).map_err(|e| format!("bad row detail: {e}"))?;
            let ts_text = Local
                .timestamp_opt(ts, 0)
                .single()
                .map(|dt| dt.format("%H:%M").to_string())
                .unwrap_or_else(|| "??:??".to_string());

            let mut line = format!("[{ts_text}] {kind}: {label}");
            if !detail.trim().is_empty() {
                line.push_str(" - ");
                line.push_str(detail.trim());
            }
            out.push(line);
        }

        Ok(out)
    }

    pub(crate) fn recent_matching_lines(
        &mut self,
        keywords: &[String],
        limit: usize,
    ) -> Result<Vec<String>, String> {
        self.prune_old()?;
        if keywords.is_empty() {
            return self.recent_lines(limit);
        }

        let mut stmt = self
            .conn
            .prepare(
                "
                SELECT ts_epoch, kind, label, detail
                FROM events
                WHERE lower(label) LIKE ?1 OR lower(detail) LIKE ?1
                ORDER BY ts_epoch DESC, id DESC
                LIMIT ?2
                ",
            )
            .map_err(|e| format!("failed to prepare matching query: {e}"))?;

        let mut gathered = Vec::new();
        for key in keywords {
            let pattern = format!("%{}%", key.to_lowercase());
            let mut rows = stmt
                .query(params![pattern, limit as i64])
                .map_err(|e| format!("failed to run matching query: {e}"))?;

            while let Some(row) = rows
                .next()
                .map_err(|e| format!("failed to fetch row: {e}"))?
            {
                let ts: i64 = row.get(0).map_err(|e| format!("bad row ts_epoch: {e}"))?;
                let kind: String = row.get(1).map_err(|e| format!("bad row kind: {e}"))?;
                let label: String = row.get(2).map_err(|e| format!("bad row label: {e}"))?;
                let detail: String = row.get(3).map_err(|e| format!("bad row detail: {e}"))?;
                let ts_text = Local
                    .timestamp_opt(ts, 0)
                    .single()
                    .map(|dt| dt.format("%H:%M").to_string())
                    .unwrap_or_else(|| "??:??".to_string());
                let mut line = format!("[{ts_text}] {kind}: {label}");
                if !detail.trim().is_empty() {
                    line.push_str(" - ");
                    line.push_str(detail.trim());
                }
                gathered.push((ts, line));
            }
        }

        gathered.sort_by(|a, b| b.0.cmp(&a.0));
        gathered.dedup_by(|a, b| a.1 == b.1);
        Ok(gathered
            .into_iter()
            .take(limit)
            .map(|(_, line)| line)
            .collect())
    }

    fn prune_old(&mut self) -> Result<(), String> {
        let cutoff = Local::now() - chrono::Duration::seconds(RETENTION_SECS);
        let cutoff_epoch = cutoff.timestamp();
        self.conn
            .execute(
                "DELETE FROM events WHERE ts_epoch < ?1",
                params![cutoff_epoch],
            )
            .map_err(|e| format!("failed to prune old events: {e}"))?;
        Ok(())
    }
}
