use std::collections::BTreeMap;

use chrono::Utc;
use rusqlite::{Connection, params};

#[derive(Debug, Clone)]
pub struct FriendRecord {
    pub node_id: String,
    pub display_name: String,
    pub since_epoch: i64,
}

#[derive(Debug, Clone)]
pub struct FriendRequest {
    pub node_id: String,
    pub display_name: String,
    pub direction: RequestDirection,
    pub ts_epoch: i64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RequestDirection {
    Incoming,
    Outgoing,
}

impl RequestDirection {
    fn as_db(self) -> &'static str {
        match self {
            RequestDirection::Incoming => "in",
            RequestDirection::Outgoing => "out",
        }
    }

    fn from_db(v: &str) -> Option<Self> {
        match v {
            "in" => Some(RequestDirection::Incoming),
            "out" => Some(RequestDirection::Outgoing),
            _ => None,
        }
    }
}

#[derive(Debug)]
pub struct FriendManager {
    conn: Connection,
    friends: BTreeMap<String, FriendRecord>,
    requests: BTreeMap<String, FriendRequest>,
}

impl FriendManager {
    pub fn in_memory() -> Result<Self, String> {
        let conn = Connection::open_in_memory()
            .map_err(|e| format!("failed to open in-memory friends db: {e}"))?;
        conn.execute_batch(
            "
            CREATE TABLE IF NOT EXISTS friends (
                node_id TEXT PRIMARY KEY,
                display_name TEXT NOT NULL,
                since_epoch INTEGER NOT NULL
            );
            CREATE TABLE IF NOT EXISTS friend_requests (
                node_id TEXT PRIMARY KEY,
                display_name TEXT NOT NULL,
                direction TEXT NOT NULL,
                ts_epoch INTEGER NOT NULL
            );
            ",
        )
        .map_err(|e| format!("failed to initialize in-memory friends schema: {e}"))?;
        Ok(Self {
            conn,
            friends: BTreeMap::new(),
            requests: BTreeMap::new(),
        })
    }

    pub fn open_default() -> Result<Self, String> {
        let db_dir = crate::system::paths::data_dir()?;
        let db_path = db_dir.join("social.sqlite3");
        let conn = Connection::open(&db_path)
            .map_err(|e| format!("failed to open sqlite db {}: {e}", db_path.display()))?;
        conn.execute_batch(
            "
            PRAGMA journal_mode = WAL;
            PRAGMA synchronous = NORMAL;
            CREATE TABLE IF NOT EXISTS friends (
                node_id TEXT PRIMARY KEY,
                display_name TEXT NOT NULL,
                since_epoch INTEGER NOT NULL
            );
            CREATE TABLE IF NOT EXISTS friend_requests (
                node_id TEXT PRIMARY KEY,
                display_name TEXT NOT NULL,
                direction TEXT NOT NULL,
                ts_epoch INTEGER NOT NULL
            );
            CREATE INDEX IF NOT EXISTS idx_friend_requests_ts ON friend_requests(ts_epoch DESC);
            ",
        )
        .map_err(|e| format!("failed to initialize friends schema: {e}"))?;

        let mut out = Self {
            conn,
            friends: BTreeMap::new(),
            requests: BTreeMap::new(),
        };
        out.load()?;
        Ok(out)
    }

    pub fn is_friend(&self, node_id: &str) -> bool {
        self.friends.contains_key(node_id)
    }

    pub fn friends(&self) -> impl Iterator<Item = &FriendRecord> {
        self.friends.values()
    }

    pub fn incoming_requests(&self) -> impl Iterator<Item = &FriendRequest> {
        self.requests
            .values()
            .filter(|r| r.direction == RequestDirection::Incoming)
    }

    pub fn mark_request_sent(&mut self, node_id: &str, display_name: &str) -> Result<(), String> {
        self.upsert_request(node_id, display_name, RequestDirection::Outgoing)
    }

    pub fn mark_request_received(
        &mut self,
        node_id: &str,
        display_name: &str,
    ) -> Result<(), String> {
        if self.is_friend(node_id) {
            return Ok(());
        }
        self.upsert_request(node_id, display_name, RequestDirection::Incoming)
    }

    pub fn accept(&mut self, node_id: &str, display_name: &str) -> Result<(), String> {
        let now = Utc::now().timestamp();
        let display = if display_name.trim().is_empty() {
            node_id.to_string()
        } else {
            display_name.trim().to_string()
        };
        self.conn
            .execute(
                "
                INSERT INTO friends(node_id, display_name, since_epoch)
                VALUES(?1, ?2, ?3)
                ON CONFLICT(node_id) DO UPDATE SET
                    display_name = excluded.display_name
                ",
                params![node_id, display, now],
            )
            .map_err(|e| format!("failed to save friend {node_id}: {e}"))?;
        self.conn
            .execute(
                "DELETE FROM friend_requests WHERE node_id = ?1",
                params![node_id],
            )
            .map_err(|e| format!("failed to clear friend request for {node_id}: {e}"))?;
        self.friends.insert(
            node_id.to_string(),
            FriendRecord {
                node_id: node_id.to_string(),
                display_name: display,
                since_epoch: now,
            },
        );
        self.requests.remove(node_id);
        Ok(())
    }

    pub fn rename_friend(&mut self, node_id: &str, display_name: &str) -> Result<(), String> {
        let display = display_name.trim();
        if display.is_empty() {
            return Err("friend display name cannot be empty".to_string());
        }
        self.conn
            .execute(
                "UPDATE friends SET display_name = ?2 WHERE node_id = ?1",
                params![node_id, display],
            )
            .map_err(|e| format!("failed to rename friend {node_id}: {e}"))?;
        if let Some(friend) = self.friends.get_mut(node_id) {
            friend.display_name = display.to_string();
        }
        Ok(())
    }

    fn upsert_request(
        &mut self,
        node_id: &str,
        display_name: &str,
        direction: RequestDirection,
    ) -> Result<(), String> {
        let now = Utc::now().timestamp();
        let display = if display_name.trim().is_empty() {
            node_id.to_string()
        } else {
            display_name.trim().to_string()
        };
        self.conn
            .execute(
                "
                INSERT INTO friend_requests(node_id, display_name, direction, ts_epoch)
                VALUES(?1, ?2, ?3, ?4)
                ON CONFLICT(node_id) DO UPDATE SET
                    display_name = excluded.display_name,
                    direction = excluded.direction,
                    ts_epoch = excluded.ts_epoch
                ",
                params![node_id, display, direction.as_db(), now],
            )
            .map_err(|e| format!("failed to upsert friend request for {node_id}: {e}"))?;
        self.requests.insert(
            node_id.to_string(),
            FriendRequest {
                node_id: node_id.to_string(),
                display_name: display,
                direction,
                ts_epoch: now,
            },
        );
        Ok(())
    }

    fn load(&mut self) -> Result<(), String> {
        {
            let mut stmt = self
                .conn
                .prepare("SELECT node_id, display_name, since_epoch FROM friends")
                .map_err(|e| format!("failed to prepare friends query: {e}"))?;
            let rows = stmt
                .query_map([], |row| {
                    Ok(FriendRecord {
                        node_id: row.get(0)?,
                        display_name: row.get(1)?,
                        since_epoch: row.get(2)?,
                    })
                })
                .map_err(|e| format!("failed to query friends: {e}"))?;
            for row in rows {
                let rec = row.map_err(|e| format!("failed to parse friend row: {e}"))?;
                self.friends.insert(rec.node_id.clone(), rec);
            }
        }

        {
            let mut stmt = self
                .conn
                .prepare("SELECT node_id, display_name, direction, ts_epoch FROM friend_requests")
                .map_err(|e| format!("failed to prepare friend requests query: {e}"))?;
            let rows = stmt
                .query_map([], |row| {
                    let dir_raw: String = row.get(2)?;
                    let direction =
                        RequestDirection::from_db(&dir_raw).unwrap_or(RequestDirection::Incoming);
                    Ok(FriendRequest {
                        node_id: row.get(0)?,
                        display_name: row.get(1)?,
                        direction,
                        ts_epoch: row.get(3)?,
                    })
                })
                .map_err(|e| format!("failed to query friend requests: {e}"))?;
            for row in rows {
                let req = row.map_err(|e| format!("failed to parse friend request row: {e}"))?;
                self.requests.insert(req.node_id.clone(), req);
            }
        }
        Ok(())
    }
}
