use chrono::Utc;
use rusqlite::{Connection, params};

use crate::system::observe_loop::{TrackerConfig, TrackerToggles, TrackingMode};

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
struct TrackerConfigPersist {
    mode: String,
    custom: TrackerTogglesPersist,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
struct TrackerTogglesPersist {
    hardware: bool,
    network: bool,
    process: bool,
    input: bool,
    session: bool,
    audio: bool,
    storage: bool,
    calendar: bool,
    environment: bool,
    accessibility: bool,
    peripherals: bool,
}

impl From<TrackerConfig> for TrackerConfigPersist {
    fn from(value: TrackerConfig) -> Self {
        Self {
            mode: value.mode.as_str().to_string(),
            custom: TrackerTogglesPersist {
                hardware: value.custom.hardware,
                network: value.custom.network,
                process: value.custom.process,
                input: value.custom.input,
                session: value.custom.session,
                audio: value.custom.audio,
                storage: value.custom.storage,
                calendar: value.custom.calendar,
                environment: value.custom.environment,
                accessibility: value.custom.accessibility,
                peripherals: value.custom.peripherals,
            },
        }
    }
}

impl From<TrackerConfigPersist> for TrackerConfig {
    fn from(value: TrackerConfigPersist) -> Self {
        Self {
            mode: TrackingMode::from_str(&value.mode),
            custom: TrackerToggles {
                hardware: value.custom.hardware,
                network: value.custom.network,
                process: value.custom.process,
                input: value.custom.input,
                session: value.custom.session,
                audio: value.custom.audio,
                storage: value.custom.storage,
                calendar: value.custom.calendar,
                environment: value.custom.environment,
                accessibility: value.custom.accessibility,
                peripherals: value.custom.peripherals,
            },
        }
    }
}

pub fn load_default() -> Result<Option<TrackerConfig>, String> {
    let db_path = crate::system::paths::data_dir()?.join("settings.sqlite3");
    let conn = Connection::open(&db_path)
        .map_err(|e| format!("failed to open settings sqlite {}: {e}", db_path.display()))?;
    init_schema(&conn)?;
    let mut stmt = conn
        .prepare("SELECT payload FROM tracker_settings WHERE id = 1")
        .map_err(|e| format!("failed to prepare tracker_settings query: {e}"))?;
    let mut rows = stmt
        .query([])
        .map_err(|e| format!("failed to query tracker_settings: {e}"))?;
    let Some(row) = rows
        .next()
        .map_err(|e| format!("failed to fetch tracker_settings row: {e}"))?
    else {
        return Ok(None);
    };
    let payload: String = row
        .get(0)
        .map_err(|e| format!("invalid tracker_settings payload: {e}"))?;
    let parsed: TrackerConfigPersist = serde_json::from_str(&payload)
        .map_err(|e| format!("invalid tracker_settings payload json: {e}"))?;
    Ok(Some(parsed.into()))
}

pub fn save_default(config: &TrackerConfig) -> Result<(), String> {
    let db_path = crate::system::paths::data_dir()?.join("settings.sqlite3");
    let conn = Connection::open(&db_path)
        .map_err(|e| format!("failed to open settings sqlite {}: {e}", db_path.display()))?;
    init_schema(&conn)?;
    let payload = serde_json::to_string(&TrackerConfigPersist::from(config.clone()))
        .map_err(|e| format!("failed to encode tracker settings payload: {e}"))?;
    let now = Utc::now().timestamp();
    conn.execute(
        "
        INSERT INTO tracker_settings(id, payload, updated_epoch)
        VALUES(1, ?1, ?2)
        ON CONFLICT(id) DO UPDATE SET
          payload = excluded.payload,
          updated_epoch = excluded.updated_epoch
        ",
        params![payload, now],
    )
    .map_err(|e| format!("failed to save tracker settings: {e}"))?;
    Ok(())
}

fn init_schema(conn: &Connection) -> Result<(), String> {
    conn.execute_batch(
        "
        PRAGMA journal_mode = WAL;
        PRAGMA synchronous = NORMAL;
        CREATE TABLE IF NOT EXISTS tracker_settings (
            id INTEGER PRIMARY KEY CHECK (id = 1),
            payload TEXT NOT NULL,
            updated_epoch INTEGER NOT NULL
        );
        ",
    )
    .map_err(|e| format!("failed to initialize tracker_settings schema: {e}"))
}
