use std::io::{self, Write};

use rusqlite::{Connection, params};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LlmProvider {
    Local,
    OpenAi,
}

#[derive(Debug, Clone)]
pub struct UserProfile {
    pub user_name: String,
    pub pet_name: String,
    pub llm_provider: LlmProvider,
    pub openai_api_key: Option<String>,
    pub text_model: String,
}

const OPENAI_FALLBACK_MODELS: &[&str] = &[
    "gpt-5.2",
    "gpt-5.1",
    "gpt-5",
    "gpt-5-mini",
    "gpt-5-nano",
    "gpt-4.1",
    "gpt-4.1-mini",
    "gpt-4.1-nano",
];
const LOCAL_MODEL_NAME: &str = "Qwen2.5-0.5B-Instruct-Q4_K_M";

pub fn load_or_init_profile_interactive() -> Result<UserProfile, String> {
    let db = open_db()?;
    init_schema(&db)?;

    if let Some(profile) = load_profile(&db)? {
        return Ok(profile);
    }

    run_setup_interactive()
}

pub fn run_setup_interactive() -> Result<UserProfile, String> {
    let mut db = open_db()?;
    init_schema(&db)?;

    println!("Critter setup:");
    let user_name = prompt_no_space("user name (no spaces)")?;
    let pet_name = prompt_no_space("pet name (no spaces)")?;
    let llm_provider = prompt_provider()?;
    let (openai_api_key, text_model) = match llm_provider {
        LlmProvider::Local => (None, LOCAL_MODEL_NAME.to_string()),
        LlmProvider::OpenAi => {
            let key = prompt_non_empty("OpenAI API key")?;
            let model = prompt_model_choice(&key)?;
            (Some(key), model)
        }
    };

    let profile = UserProfile {
        user_name,
        pet_name,
        llm_provider,
        openai_api_key,
        text_model,
    };
    save_profile(&mut db, &profile)?;
    Ok(profile)
}

#[cfg_attr(not(feature = "web"), allow(dead_code))]
pub fn save_profile_noninteractive(profile: &UserProfile) -> Result<(), String> {
    let mut db = open_db()?;
    init_schema(&db)?;
    save_profile(&mut db, profile)
}

fn open_db() -> Result<Connection, String> {
    let db_dir = crate::system::paths::data_dir()?;
    let db_path = db_dir.join("config.sqlite3");
    Connection::open(&db_path)
        .map_err(|e| format!("failed to open sqlite db {}: {e}", db_path.display()))
}

fn init_schema(db: &Connection) -> Result<(), String> {
    db.execute_batch(
        "
        CREATE TABLE IF NOT EXISTS user_profile (
            id INTEGER PRIMARY KEY CHECK (id = 1),
            user_name TEXT NOT NULL,
            pet_name TEXT NOT NULL,
            llm_provider TEXT NOT NULL,
            openai_api_key TEXT,
            text_model TEXT NOT NULL
        );
        ",
    )
    .map_err(|e| format!("failed to initialize profile schema: {e}"))
}

fn load_profile(db: &Connection) -> Result<Option<UserProfile>, String> {
    let mut stmt = db
        .prepare(
            "SELECT user_name, pet_name, llm_provider, openai_api_key, text_model FROM user_profile WHERE id = 1",
        )
        .map_err(|e| format!("failed to prepare profile query: {e}"))?;

    let mut rows = stmt
        .query([])
        .map_err(|e| format!("failed to query profile: {e}"))?;
    let Some(row) = rows
        .next()
        .map_err(|e| format!("failed to fetch profile row: {e}"))?
    else {
        return Ok(None);
    };

    let llm_provider_raw: String = row
        .get(2)
        .map_err(|e| format!("invalid llm_provider in profile: {e}"))?;
    let llm_provider = match llm_provider_raw.as_str() {
        "local" => LlmProvider::Local,
        "openai" => LlmProvider::OpenAi,
        other => return Err(format!("unknown llm_provider in profile: {other}")),
    };

    let profile = UserProfile {
        user_name: row
            .get(0)
            .map_err(|e| format!("invalid user_name in profile: {e}"))?,
        pet_name: row
            .get(1)
            .map_err(|e| format!("invalid pet_name in profile: {e}"))?,
        llm_provider,
        openai_api_key: row
            .get(3)
            .map_err(|e| format!("invalid openai_api_key in profile: {e}"))?,
        text_model: row
            .get(4)
            .map_err(|e| format!("invalid text_model in profile: {e}"))?,
    };

    Ok(Some(profile))
}

fn save_profile(db: &mut Connection, profile: &UserProfile) -> Result<(), String> {
    let provider = match profile.llm_provider {
        LlmProvider::Local => "local",
        LlmProvider::OpenAi => "openai",
    };
    db.execute(
        "INSERT OR REPLACE INTO user_profile(id, user_name, pet_name, llm_provider, openai_api_key, text_model)
         VALUES(1, ?1, ?2, ?3, ?4, ?5)",
        params![
            profile.user_name,
            profile.pet_name,
            provider,
            profile.openai_api_key,
            profile.text_model,
        ],
    )
    .map_err(|e| format!("failed to save profile: {e}"))?;
    Ok(())
}

fn prompt_no_space(label: &str) -> Result<String, String> {
    loop {
        let value = prompt_non_empty(label)?;
        if value.chars().any(char::is_whitespace) {
            println!("Input must not contain spaces. Try again.");
            continue;
        }
        return Ok(value);
    }
}

fn prompt_non_empty(label: &str) -> Result<String, String> {
    loop {
        print!("{label}: ");
        io::stdout()
            .flush()
            .map_err(|e| format!("failed to flush stdout: {e}"))?;
        let mut s = String::new();
        io::stdin()
            .read_line(&mut s)
            .map_err(|e| format!("failed to read stdin: {e}"))?;
        let v = s.trim().to_string();
        if v.is_empty() {
            println!("Input cannot be empty. Try again.");
            continue;
        }
        return Ok(v);
    }
}

fn prompt_provider() -> Result<LlmProvider, String> {
    loop {
        println!("Choose LLM provider:");
        println!("  1) local ({LOCAL_MODEL_NAME})");
        println!("  2) OpenAI");
        let v = prompt_non_empty("provider number")?;
        match v.as_str() {
            "1" => return Ok(LlmProvider::Local),
            "2" => return Ok(LlmProvider::OpenAi),
            _ => println!("Invalid choice. Enter 1 or 2."),
        }
    }
}

fn prompt_model_choice(api_key: &str) -> Result<String, String> {
    let mut models = fetch_openai_text_models(api_key).unwrap_or_else(|_| {
        OPENAI_FALLBACK_MODELS
            .iter()
            .map(|m| (*m).to_string())
            .collect()
    });
    if models.is_empty() {
        models = OPENAI_FALLBACK_MODELS
            .iter()
            .map(|m| (*m).to_string())
            .collect();
    }
    loop {
        println!("Choose OpenAI text model:");
        for (idx, model) in models.iter().enumerate() {
            println!("  {}) {}", idx + 1, model);
        }
        let v = prompt_non_empty("model number")?;
        if let Ok(n) = v.parse::<usize>()
            && (1..=models.len()).contains(&n)
        {
            return Ok(models[n - 1].to_string());
        }
        println!("Invalid model choice.");
    }
}

fn fetch_openai_text_models(api_key: &str) -> Result<Vec<String>, String> {
    let client = reqwest::blocking::Client::builder()
        .timeout(std::time::Duration::from_secs(15))
        .build()
        .map_err(|e| format!("openai client init failed: {e}"))?;
    let resp = client
        .get("https://api.openai.com/v1/models")
        .bearer_auth(api_key)
        .send()
        .map_err(|e| format!("openai models request failed: {e}"))?;
    if !resp.status().is_success() {
        return Err(format!("openai models endpoint returned {}", resp.status()));
    }
    let value: serde_json::Value = resp
        .json()
        .map_err(|e| format!("openai models parse failed: {e}"))?;
    let mut ids: Vec<String> = value["data"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|it| it["id"].as_str().map(|s| s.to_string()))
        .filter(|id| id.starts_with("gpt-"))
        .filter(|id| {
            !id.contains("audio")
                && !id.contains("realtime")
                && !id.contains("transcribe")
                && !id.contains("tts")
                && !id.contains("image")
                && !id.contains("search")
        })
        .collect();
    ids.sort();
    ids.dedup();

    let mut ordered = Vec::new();
    for preferred in OPENAI_FALLBACK_MODELS {
        if let Some(pos) = ids.iter().position(|m| m == preferred) {
            ordered.push(ids.remove(pos));
        }
    }
    ordered.extend(ids);
    Ok(ordered.into_iter().take(24).collect())
}
