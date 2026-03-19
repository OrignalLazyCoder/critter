pub struct TalkTurnInput<'a> {
    pub pet_name: &'a str,
    pub user_name: &'a str,
    pub peer_name: &'a str,
    pub topic: &'a str,
    pub content: &'a str,
    pub allow_jokes: bool,
    pub allow_random: bool,
    pub active_app: &'a str,
    pub idle_secs: u64,
    pub hunger: u16,
    pub social: u16,
    pub focus: u16,
    pub inbound: Option<&'a str>,
    pub last_sent: Option<&'a str>,
    pub seed: u64,
}

pub fn system_prompt() -> &'static str {
    "You are a playful terminal pet chatting with another pet. \
Output exactly one short conversational line (max 22 words). \
No bullet points, no quotes, no emojis, no speaker name prefix, no timestamps. \
Do not repeat previous wording. Keep it human and varied: gossip, jokes, random observations, light banter."
}

pub fn build_turn_prompt(input: &TalkTurnInput<'_>) -> String {
    let mut lines =
        vec![
        format!("You are {}. You are chatting with pet {}.", input.pet_name, input.peer_name),
        format!("Human owner name: {}.", input.user_name),
        format!(
            "Preferences: jokes={}, random={}.",
            input.allow_jokes, input.allow_random
        ),
        format!(
            "Stats snapshot: hunger={}, social={}, focus={}, idle_secs={}, active_app={}.",
            input.hunger, input.social, input.focus, input.idle_secs, input.active_app
        ),
        format!("Topic hint: {}.", input.topic.trim()),
        format!("Custom content hint: {}.", input.content.trim()),
        format!("Random seed: {}.", input.seed),
        "Style requirements: avoid status-report tone; ask a question sometimes; keep it natural."
            .to_string(),
    ];
    if let Some(inbound) = input.inbound {
        lines.push(format!(
            "Peer just said: {}. Reply to the same thread naturally without quoting or paraphrasing it literally.",
            inbound
        ));
    } else {
        lines.push("Start a fresh conversational line, not a system update.".to_string());
    }
    if let Some(last_sent) = input.last_sent {
        lines.push(format!(
            "Your previous line was: {}. Do NOT repeat similar wording.",
            last_sent
        ));
    }
    lines.join("\n")
}

pub fn normalize_turn(raw: &str) -> String {
    let one_line = raw
        .replace('\n', " ")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    let mut out = one_line
        .trim_matches('"')
        .trim_matches('\'')
        .trim()
        .to_string();
    if out.len() > 180 {
        out.truncate(180);
        out = out.trim().to_string();
    }
    if out.is_empty() {
        "what is your human doing right now?".to_string()
    } else {
        out
    }
}
