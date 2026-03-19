use std::{
    collections::{HashMap, HashSet},
    time::{Duration, Instant},
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PresenceState {
    Online,
    Away,
    Offline,
}

#[derive(Debug, Clone)]
pub struct PresencePolicy {
    pub online_window: Duration,
    pub away_window: Duration,
}

impl Default for PresencePolicy {
    fn default() -> Self {
        Self {
            online_window: Duration::from_secs(45),
            away_window: Duration::from_secs(180),
        }
    }
}

#[derive(Debug, Default)]
pub struct PresenceTracker {
    policy: PresencePolicy,
    last_seen: HashMap<String, Instant>,
    forced_offline: HashSet<String>,
}

impl PresenceTracker {
    pub fn new(policy: PresencePolicy) -> Self {
        Self {
            policy,
            last_seen: HashMap::new(),
            forced_offline: HashSet::new(),
        }
    }

    pub fn touch(&mut self, peer_id: &str) {
        self.last_seen.insert(peer_id.to_string(), Instant::now());
        self.forced_offline.remove(peer_id);
    }

    pub fn mark_offline(&mut self, peer_id: &str) {
        self.forced_offline.insert(peer_id.to_string());
    }

    pub fn status(&self, peer_id: &str) -> PresenceState {
        if self.forced_offline.contains(peer_id) {
            return PresenceState::Offline;
        }
        let Some(last_seen) = self.last_seen.get(peer_id) else {
            return PresenceState::Offline;
        };
        let elapsed = last_seen.elapsed();
        if elapsed <= self.policy.online_window {
            PresenceState::Online
        } else if elapsed <= self.policy.away_window {
            PresenceState::Away
        } else {
            PresenceState::Offline
        }
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::{PresencePolicy, PresenceState, PresenceTracker};

    #[test]
    fn touch_sets_online() {
        let mut tracker = PresenceTracker::new(PresencePolicy::default());
        tracker.touch("p1");
        assert_eq!(tracker.status("p1"), PresenceState::Online);
    }

    #[test]
    fn mark_offline_overrides_seen_state() {
        let mut tracker = PresenceTracker::new(PresencePolicy::default());
        tracker.touch("p1");
        tracker.mark_offline("p1");
        assert_eq!(tracker.status("p1"), PresenceState::Offline);
    }

    #[test]
    fn custom_policy_maps_to_away() {
        let mut tracker = PresenceTracker::new(PresencePolicy {
            online_window: Duration::from_millis(5),
            away_window: Duration::from_millis(50),
        });
        tracker.touch("p1");
        std::thread::sleep(Duration::from_millis(10));
        assert_eq!(tracker.status("p1"), PresenceState::Away);
    }
}
