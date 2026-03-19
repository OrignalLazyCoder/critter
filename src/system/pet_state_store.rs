use chrono::Utc;
use rusqlite::{Connection, params};

#[derive(Debug, Clone, Copy)]
pub struct PersistedPetState {
    pub hunger: u16,
    pub energy: u16,
    pub social: u16,
    pub focus: u16,
}

pub struct PetStateStore {
    conn: Connection,
}

impl PetStateStore {
    pub fn open_default() -> Result<Self, String> {
        let db_dir = crate::system::paths::data_dir()?;
        let db_path = db_dir.join("state.sqlite3");
        let conn = Connection::open(&db_path)
            .map_err(|e| format!("failed to open sqlite db {}: {e}", db_path.display()))?;

        conn.execute_batch(
            "
            PRAGMA journal_mode = WAL;
            PRAGMA synchronous = NORMAL;
            CREATE TABLE IF NOT EXISTS pet_state (
                id INTEGER PRIMARY KEY CHECK (id = 1),
                hunger INTEGER NOT NULL,
                energy INTEGER NOT NULL,
                social INTEGER NOT NULL,
                focus INTEGER NOT NULL,
                updated_epoch INTEGER NOT NULL
            );
            ",
        )
        .map_err(|e| format!("failed to initialize pet_state schema: {e}"))?;

        Ok(Self { conn })
    }

    pub fn load(&self) -> Result<Option<PersistedPetState>, String> {
        let mut stmt = self
            .conn
            .prepare("SELECT hunger, energy, social, focus FROM pet_state WHERE id = 1")
            .map_err(|e| format!("failed to prepare pet_state query: {e}"))?;
        let mut rows = stmt
            .query([])
            .map_err(|e| format!("failed to query pet_state: {e}"))?;
        let Some(row) = rows
            .next()
            .map_err(|e| format!("failed to read pet_state row: {e}"))?
        else {
            return Ok(None);
        };

        let s = PersistedPetState {
            hunger: clamp_pct(
                row.get::<_, i64>(0)
                    .map_err(|e| format!("invalid hunger value: {e}"))?,
            ),
            energy: clamp_pct(
                row.get::<_, i64>(1)
                    .map_err(|e| format!("invalid energy value: {e}"))?,
            ),
            social: clamp_pct(
                row.get::<_, i64>(2)
                    .map_err(|e| format!("invalid social value: {e}"))?,
            ),
            focus: clamp_pct(
                row.get::<_, i64>(3)
                    .map_err(|e| format!("invalid focus value: {e}"))?,
            ),
        };
        Ok(Some(s))
    }

    pub fn save(
        &mut self,
        hunger: u16,
        energy: u16,
        social: u16,
        focus: u16,
    ) -> Result<(), String> {
        let now = Utc::now().timestamp();
        self.conn
            .execute(
                "
                INSERT INTO pet_state(id, hunger, energy, social, focus, updated_epoch)
                VALUES(1, ?1, ?2, ?3, ?4, ?5)
                ON CONFLICT(id) DO UPDATE SET
                    hunger = excluded.hunger,
                    energy = excluded.energy,
                    social = excluded.social,
                    focus = excluded.focus,
                    updated_epoch = excluded.updated_epoch
                ",
                params![
                    hunger as i64,
                    energy as i64,
                    social as i64,
                    focus as i64,
                    now
                ],
            )
            .map_err(|e| format!("failed to save pet_state: {e}"))?;
        Ok(())
    }
}

fn clamp_pct(v: i64) -> u16 {
    v.clamp(0, 100) as u16
}
