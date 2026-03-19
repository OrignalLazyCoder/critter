use std::collections::HashMap;

#[derive(Debug, Clone, Default)]
pub struct DmSession {
    pub peer_label: String,
    pub unread: usize,
}

#[derive(Debug, Default)]
pub struct DmManager {
    sessions: HashMap<String, DmSession>,
}

impl DmManager {
    pub fn touch(&mut self, peer_label: &str) {
        self.sessions
            .entry(peer_label.to_string())
            .or_insert_with(|| DmSession {
                peer_label: peer_label.to_string(),
                unread: 0,
            });
    }

    pub fn mark_unread(&mut self, peer_label: &str) {
        let session = self
            .sessions
            .entry(peer_label.to_string())
            .or_insert_with(|| DmSession {
                peer_label: peer_label.to_string(),
                unread: 0,
            });
        session.unread = session.unread.saturating_add(1);
    }

    pub fn clear_unread(&mut self, peer_label: &str) {
        if let Some(session) = self.sessions.get_mut(peer_label) {
            session.unread = 0;
        }
    }

    pub fn unread_for(&self, peer_label: &str) -> usize {
        self.sessions.get(peer_label).map(|s| s.unread).unwrap_or(0)
    }
}

#[cfg(test)]
mod tests {
    use super::DmManager;

    #[test]
    fn unread_counter_roundtrip() {
        let mut dm = DmManager::default();
        dm.touch("@ rintaro");
        dm.mark_unread("@ rintaro");
        dm.mark_unread("@ rintaro");
        assert_eq!(dm.unread_for("@ rintaro"), 2);
        dm.clear_unread("@ rintaro");
        assert_eq!(dm.unread_for("@ rintaro"), 0);
    }
}
