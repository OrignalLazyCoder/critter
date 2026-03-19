use std::{
    collections::HashMap,
    time::{Duration, Instant},
};

#[derive(Debug, Clone)]
pub struct DialoguePolicy {
    pub min_interval: Duration,
    pub max_turns: u8,
}

impl Default for DialoguePolicy {
    fn default() -> Self {
        Self {
            min_interval: Duration::from_secs(5 * 60),
            max_turns: 4,
        }
    }
}

#[derive(Debug, Clone)]
struct SessionState {
    started_at: Instant,
    last_turn_at: Instant,
    turns: u8,
}

#[derive(Debug)]
pub struct DialogueEngine {
    policy: DialoguePolicy,
    sessions: HashMap<String, SessionState>,
}

impl DialogueEngine {
    pub fn new(policy: DialoguePolicy) -> Self {
        Self {
            policy,
            sessions: HashMap::new(),
        }
    }

    pub fn can_initiate(&self, peer_id: &str) -> bool {
        let Some(state) = self.sessions.get(peer_id) else {
            return true;
        };
        if state.turns < self.policy.max_turns {
            return true;
        }
        state.last_turn_at.elapsed() >= self.policy.min_interval
    }

    pub fn start_or_continue(&mut self, peer_id: &str) -> bool {
        let now = Instant::now();
        let state = self
            .sessions
            .entry(peer_id.to_string())
            .or_insert(SessionState {
                started_at: now,
                last_turn_at: now,
                turns: 0,
            });

        if state.turns >= self.policy.max_turns
            && state.last_turn_at.elapsed() >= self.policy.min_interval
        {
            state.started_at = now;
            state.turns = 0;
        }

        if state.turns >= self.policy.max_turns {
            return false;
        }

        state.turns = state.turns.saturating_add(1);
        state.last_turn_at = now;
        true
    }

    pub fn end_if_stale(&mut self, max_idle: Duration) {
        self.sessions
            .retain(|_, state| state.last_turn_at.elapsed() <= max_idle);
    }

    pub fn turns_for(&self, peer_id: &str) -> u8 {
        self.sessions.get(peer_id).map(|s| s.turns).unwrap_or(0)
    }

    pub fn cooldown_remaining_for(&self, peer_id: &str) -> Duration {
        let Some(state) = self.sessions.get(peer_id) else {
            return Duration::ZERO;
        };
        if state.turns < self.policy.max_turns {
            return Duration::ZERO;
        }
        self.policy
            .min_interval
            .saturating_sub(state.last_turn_at.elapsed())
    }

    pub fn started_elapsed_for(&self, peer_id: &str) -> Option<Duration> {
        self.sessions.get(peer_id).map(|s| s.started_at.elapsed())
    }

    pub fn set_policy(&mut self, policy: DialoguePolicy) {
        self.policy = policy;
    }
}

#[cfg(test)]
mod tests {
    use super::{DialogueEngine, DialoguePolicy};

    #[test]
    fn turn_cap_is_enforced() {
        let mut engine = DialogueEngine::new(DialoguePolicy::default());
        let peer = "peer-a";

        assert!(engine.start_or_continue(peer));
        assert!(engine.start_or_continue(peer));
        assert!(engine.start_or_continue(peer));
        assert!(engine.start_or_continue(peer));
        assert!(!engine.start_or_continue(peer));
        assert_eq!(engine.turns_for(peer), 4);
    }

    #[test]
    fn initiation_allowed_for_new_peer() {
        let engine = DialogueEngine::new(DialoguePolicy::default());
        assert!(engine.can_initiate("peer-new"));
    }
}
