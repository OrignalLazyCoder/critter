use chrono::Utc;
use rusqlite::{Connection, params};

use crate::network::codec::MoodPacket;

#[derive(Debug, Clone)]
pub(crate) struct StoredPeer {
    pub node_id: String,
    pub first_seen_epoch: i64,
    pub last_seen_epoch: i64,
    pub status: String,
    pub last_packet: Option<MoodPacket>,
}

impl StoredPeer {
    fn new(node_id: String) -> Self {
        let now = Utc::now().timestamp();
        Self {
            node_id,
            first_seen_epoch: now,
            last_seen_epoch: now,
            status: "online".to_string(),
            last_packet: None,
        }
    }
}

pub(crate) struct PeerRegistry {
    conn: Connection,
}

impl PeerRegistry {
    pub(crate) fn open_default() -> Result<Self, String> {
        let db_dir = crate::system::paths::data_dir()?;
        let path = db_dir.join("social.sqlite3");

        let conn = Connection::open(&path)
            .map_err(|e| format!("failed to open sqlite db {}: {e}", path.display()))?;
        conn.execute_batch(
            "
            PRAGMA journal_mode = WAL;
            PRAGMA synchronous = NORMAL;
            CREATE TABLE IF NOT EXISTS peers (
                node_id TEXT PRIMARY KEY,
                first_seen_epoch INTEGER NOT NULL,
                last_seen_epoch INTEGER NOT NULL,
                status TEXT NOT NULL,
                last_packet BLOB
            );
            CREATE INDEX IF NOT EXISTS idx_peers_last_seen ON peers(last_seen_epoch DESC);
            ",
        )
        .map_err(|e| format!("failed to initialize peer registry schema: {e}"))?;

        Ok(Self { conn })
    }

    pub(crate) fn touch_discovered(&mut self, node_id: &str) -> Result<(), String> {
        let now = Utc::now().timestamp();
        let peer = self.get_or_new(node_id)?;
        self.upsert_peer(&StoredPeer {
            node_id: peer.node_id,
            first_seen_epoch: peer.first_seen_epoch,
            last_seen_epoch: now,
            status: "online".to_string(),
            last_packet: peer.last_packet,
        })
    }

    pub(crate) fn touch_expired(&mut self, node_id: &str) -> Result<(), String> {
        let now = Utc::now().timestamp();
        let peer = self.get_or_new(node_id)?;
        self.upsert_peer(&StoredPeer {
            node_id: peer.node_id,
            first_seen_epoch: peer.first_seen_epoch,
            last_seen_epoch: now,
            status: "offline".to_string(),
            last_packet: peer.last_packet,
        })
    }

    pub(crate) fn apply_packet(
        &mut self,
        node_id: &str,
        packet: &MoodPacket,
    ) -> Result<(), String> {
        let now = Utc::now().timestamp();
        let peer = self.get_or_new(node_id)?;
        self.upsert_peer(&StoredPeer {
            node_id: peer.node_id,
            first_seen_epoch: peer.first_seen_epoch,
            last_seen_epoch: now,
            status: "online".to_string(),
            last_packet: Some(packet.clone()),
        })
    }

    pub(crate) fn peers(&self) -> Vec<StoredPeer> {
        let mut stmt = match self.conn.prepare(
            "
            SELECT node_id, first_seen_epoch, last_seen_epoch, status, last_packet
            FROM peers
            ORDER BY last_seen_epoch DESC
            ",
        ) {
            Ok(stmt) => stmt,
            Err(_) => return Vec::new(),
        };

        let rows = match stmt.query_map([], |row| {
            let last_packet_blob: Option<Vec<u8>> = row.get(4)?;
            let last_packet =
                last_packet_blob.and_then(|blob| bincode::deserialize::<MoodPacket>(&blob).ok());
            Ok(StoredPeer {
                node_id: row.get(0)?,
                first_seen_epoch: row.get(1)?,
                last_seen_epoch: row.get(2)?,
                status: row.get(3)?,
                last_packet,
            })
        }) {
            Ok(rows) => rows,
            Err(_) => return Vec::new(),
        };

        rows.filter_map(Result::ok).collect()
    }

    fn get_or_new(&self, node_id: &str) -> Result<StoredPeer, String> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT node_id, first_seen_epoch, last_seen_epoch, status, last_packet FROM peers WHERE node_id = ?1",
            )
            .map_err(|e| format!("failed to prepare peer lookup query: {e}"))?;
        let mut rows = stmt
            .query(params![node_id])
            .map_err(|e| format!("failed to query peer {node_id}: {e}"))?;

        if let Some(row) = rows
            .next()
            .map_err(|e| format!("failed to read peer row for {node_id}: {e}"))?
        {
            let last_packet_blob: Option<Vec<u8>> = row
                .get(4)
                .map_err(|e| format!("invalid last_packet for {node_id}: {e}"))?;
            let last_packet =
                last_packet_blob.and_then(|blob| bincode::deserialize::<MoodPacket>(&blob).ok());
            return Ok(StoredPeer {
                node_id: row
                    .get(0)
                    .map_err(|e| format!("invalid node_id for {node_id}: {e}"))?,
                first_seen_epoch: row
                    .get(1)
                    .map_err(|e| format!("invalid first_seen for {node_id}: {e}"))?,
                last_seen_epoch: row
                    .get(2)
                    .map_err(|e| format!("invalid last_seen for {node_id}: {e}"))?,
                status: row
                    .get(3)
                    .map_err(|e| format!("invalid status for {node_id}: {e}"))?,
                last_packet,
            });
        }

        Ok(StoredPeer::new(node_id.to_string()))
    }

    fn upsert_peer(&mut self, peer: &StoredPeer) -> Result<(), String> {
        let packet_blob = match &peer.last_packet {
            Some(packet) => Some(
                bincode::serialize(packet)
                    .map_err(|e| format!("failed to encode mood packet for db: {e}"))?,
            ),
            None => None,
        };
        self.conn
            .execute(
                "
                INSERT INTO peers(node_id, first_seen_epoch, last_seen_epoch, status, last_packet)
                VALUES(?1, ?2, ?3, ?4, ?5)
                ON CONFLICT(node_id) DO UPDATE SET
                    first_seen_epoch = excluded.first_seen_epoch,
                    last_seen_epoch = excluded.last_seen_epoch,
                    status = excluded.status,
                    last_packet = excluded.last_packet
                ",
                params![
                    &peer.node_id,
                    peer.first_seen_epoch,
                    peer.last_seen_epoch,
                    &peer.status,
                    packet_blob,
                ],
            )
            .map_err(|e| format!("failed to upsert peer {}: {e}", peer.node_id))?;
        Ok(())
    }
}
