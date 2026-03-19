use std::collections::{BTreeMap, BTreeSet};

use rusqlite::{Connection, params};

#[derive(Debug, Clone)]
pub struct Group {
    pub name: String,
    pub code: String,
    pub members: BTreeSet<String>,
}

#[derive(Debug)]
pub struct GroupManager {
    groups_by_code: BTreeMap<String, Group>,
    active_code: Option<String>,
    conn: Option<Connection>,
}

impl Default for GroupManager {
    fn default() -> Self {
        Self {
            groups_by_code: BTreeMap::new(),
            active_code: None,
            conn: None,
        }
    }
}

impl GroupManager {
    pub fn open_default() -> Result<Self, String> {
        let db_dir = crate::system::paths::data_dir()?;
        let db_path = db_dir.join("social.sqlite3");
        let conn = Connection::open(&db_path)
            .map_err(|e| format!("failed to open sqlite db {}: {e}", db_path.display()))?;

        conn.execute_batch(
            "
            PRAGMA journal_mode = WAL;
            PRAGMA synchronous = NORMAL;
            CREATE TABLE IF NOT EXISTS groups (
                code TEXT PRIMARY KEY,
                name TEXT NOT NULL,
                active INTEGER NOT NULL DEFAULT 0
            );
            CREATE TABLE IF NOT EXISTS group_members (
                code TEXT NOT NULL,
                member TEXT NOT NULL,
                PRIMARY KEY(code, member)
            );
            CREATE INDEX IF NOT EXISTS idx_group_members_code ON group_members(code);
            ",
        )
        .map_err(|e| format!("failed to initialize groups schema: {e}"))?;

        let mut manager = Self {
            groups_by_code: BTreeMap::new(),
            active_code: None,
            conn: Some(conn),
        };
        manager.load_from_db()?;
        Ok(manager)
    }

    pub fn groups(&self) -> impl Iterator<Item = &Group> {
        self.groups_by_code.values()
    }

    pub fn create_group(&mut self, name: &str, creator: &str) -> Group {
        let code = make_code(name, self.groups_by_code.len() as u64 + 1);
        let mut members = BTreeSet::new();
        members.insert(creator.to_string());
        let group = Group {
            name: name.to_string(),
            code: code.clone(),
            members,
        };
        self.groups_by_code.insert(code.clone(), group.clone());
        self.active_code = Some(code);
        let _ = self.persist();
        group
    }

    pub fn join_group(&mut self, code: &str, member: &str) -> Option<Group> {
        let group = self.groups_by_code.get_mut(code)?;
        group.members.insert(member.to_string());
        self.active_code = Some(code.to_string());
        let out = group.clone();
        let _ = self.persist();
        Some(out)
    }

    pub fn leave_active(&mut self, member: &str) -> Option<Group> {
        let code = self.active_code.clone()?;
        let mut remove_group = false;
        let out = if let Some(group) = self.groups_by_code.get_mut(&code) {
            group.members.remove(member);
            if group.members.is_empty() {
                remove_group = true;
            }
            Some(group.clone())
        } else {
            None
        };
        if remove_group {
            self.groups_by_code.remove(&code);
        }
        self.active_code = None;
        let _ = self.persist();
        out
    }

    pub fn invite_to_active(&self, target: &str) -> Option<(String, String)> {
        let code = self.active_code.as_ref()?;
        let group = self.groups_by_code.get(code)?;
        Some((target.to_string(), group.code.clone()))
    }

    pub fn active_group(&self) -> Option<&Group> {
        let code = self.active_code.as_ref()?;
        self.groups_by_code.get(code)
    }

    fn load_from_db(&mut self) -> Result<(), String> {
        let Some(conn) = self.conn.as_ref() else {
            return Ok(());
        };

        let mut groups_stmt = conn
            .prepare("SELECT code, name, active FROM groups ORDER BY name ASC")
            .map_err(|e| format!("failed to prepare groups query: {e}"))?;
        let mut groups_rows = groups_stmt
            .query([])
            .map_err(|e| format!("failed to query groups: {e}"))?;

        while let Some(row) = groups_rows
            .next()
            .map_err(|e| format!("failed to read group row: {e}"))?
        {
            let code: String = row.get(0).map_err(|e| format!("invalid group code: {e}"))?;
            let name: String = row.get(1).map_err(|e| format!("invalid group name: {e}"))?;
            let active: i64 = row
                .get(2)
                .map_err(|e| format!("invalid group active: {e}"))?;
            if active != 0 {
                self.active_code = Some(code.clone());
            }
            self.groups_by_code.insert(
                code.clone(),
                Group {
                    name,
                    code,
                    members: BTreeSet::new(),
                },
            );
        }

        let mut member_stmt = conn
            .prepare("SELECT code, member FROM group_members")
            .map_err(|e| format!("failed to prepare members query: {e}"))?;
        let mut member_rows = member_stmt
            .query([])
            .map_err(|e| format!("failed to query group members: {e}"))?;

        while let Some(row) = member_rows
            .next()
            .map_err(|e| format!("failed to read member row: {e}"))?
        {
            let code: String = row
                .get(0)
                .map_err(|e| format!("invalid member code: {e}"))?;
            let member: String = row
                .get(1)
                .map_err(|e| format!("invalid member name: {e}"))?;
            if let Some(group) = self.groups_by_code.get_mut(&code) {
                group.members.insert(member);
            }
        }

        Ok(())
    }

    fn persist(&mut self) -> Result<(), String> {
        let Some(conn) = self.conn.as_mut() else {
            return Ok(());
        };

        let tx = conn
            .transaction()
            .map_err(|e| format!("failed to start groups transaction: {e}"))?;
        tx.execute("DELETE FROM group_members", [])
            .map_err(|e| format!("failed to clear group members: {e}"))?;
        tx.execute("DELETE FROM groups", [])
            .map_err(|e| format!("failed to clear groups: {e}"))?;

        for (code, group) in &self.groups_by_code {
            let active = self
                .active_code
                .as_ref()
                .is_some_and(|active_code| active_code == code) as i64;
            tx.execute(
                "INSERT INTO groups(code, name, active) VALUES(?1, ?2, ?3)",
                params![code, &group.name, active],
            )
            .map_err(|e| format!("failed to save group {code}: {e}"))?;

            for member in &group.members {
                tx.execute(
                    "INSERT INTO group_members(code, member) VALUES(?1, ?2)",
                    params![code, member],
                )
                .map_err(|e| format!("failed to save group member for {code}: {e}"))?;
            }
        }

        tx.commit()
            .map_err(|e| format!("failed to commit groups transaction: {e}"))
    }
}

fn make_code(name: &str, salt: u64) -> String {
    let base = name.trim().trim_start_matches('#').to_ascii_uppercase();
    let prefix: String = base
        .chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .take(6)
        .collect();
    let suffix = format!("{:04}", (salt % 10_000));
    format!("CRITTER-{prefix}-{suffix}")
}

#[cfg(test)]
mod tests {
    use super::GroupManager;

    #[test]
    fn create_join_leave_flow() {
        let mut gm = GroupManager::default();
        let g = gm.create_group("#grind", "blob");
        assert!(g.code.starts_with("CRITTER-GRIND-"));
        let j = gm.join_group(&g.code, "peer-1").expect("join existing");
        assert!(j.members.contains("peer-1"));
        let left = gm.leave_active("blob").expect("leave active");
        assert_eq!(left.name, "#grind");
    }
}
