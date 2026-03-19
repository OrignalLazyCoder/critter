use std::collections::BTreeMap;

#[derive(Debug, Clone, Default)]
pub struct NotificationCenter {
    missed_by_channel: BTreeMap<String, usize>,
}

impl NotificationCenter {
    pub fn mark_missed(&mut self, channel: &str) {
        let key = normalize_channel(channel);
        let entry = self.missed_by_channel.entry(key).or_insert(0);
        *entry = entry.saturating_add(1);
    }

    pub fn clear(&mut self, channel: &str) {
        let key = normalize_channel(channel);
        self.missed_by_channel.insert(key, 0);
    }

    pub fn count(&self, channel: &str) -> usize {
        let key = normalize_channel(channel);
        self.missed_by_channel.get(&key).copied().unwrap_or(0)
    }

    pub fn total(&self) -> usize {
        self.missed_by_channel
            .values()
            .copied()
            .fold(0usize, usize::saturating_add)
    }
}

fn normalize_channel(channel: &str) -> String {
    channel.trim().to_ascii_lowercase()
}
