//! Persistent background-agent objectives.

use crate::persistent;
use serde::{Deserialize, Serialize};

pub const VERSION: u8 = 1;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Objective {
    pub id: u64,
    pub text: String,
    pub status: String,
    #[serde(default)]
    pub depends_on: Vec<u64>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Store {
    #[serde(default = "default_version")]
    pub version: u8,
    #[serde(default)]
    pub next_id: u64,
    #[serde(default)]
    pub objectives: Vec<Objective>,
    #[serde(default)]
    pub queue_active: bool,
}

fn default_version() -> u8 {
    VERSION
}

impl Default for Store {
    fn default() -> Self {
        Self {
            version: VERSION,
            next_id: 0,
            objectives: Vec::new(),
            queue_active: false,
        }
    }
}

impl Store {
    pub fn add(&mut self, text: String) -> u64 {
        self.next_id = self.next_id.max(1);
        let id = self.next_id;
        self.next_id += 1;
        self.objectives.push(Objective {
            id,
            text,
            status: "pending".to_string(),
            depends_on: Vec::new(),
        });
        id
    }

    pub fn reset_running(&mut self) -> bool {
        let mut changed = false;
        for objective in &mut self.objectives {
            if objective.status == "running" {
                objective.status = "pending".to_string();
                changed = true;
            }
        }
        changed
    }

    pub fn set_status(&mut self, id: u64, status: &str) {
        if let Some(objective) = self
            .objectives
            .iter_mut()
            .find(|objective| objective.id == id)
        {
            objective.status = status.to_string();
        }
    }

    pub fn get(&self, id: u64) -> Option<&Objective> {
        self.objectives.iter().find(|objective| objective.id == id)
    }

    pub fn has_cycle(&self, id: u64, dependencies: &[u64]) -> bool {
        let mut stack = dependencies.to_vec();
        let mut seen = std::collections::HashSet::new();
        while let Some(dependency) = stack.pop() {
            if dependency == id {
                return true;
            }
            if !seen.insert(dependency) {
                continue;
            }
            if let Some(objective) = self.get(dependency) {
                stack.extend(objective.depends_on.iter().copied());
            }
        }
        false
    }
}

pub fn load() -> Store {
    persistent::load_objectives::<Store>()
        .filter(|store| store.version == VERSION)
        .unwrap_or_default()
}

pub fn save(store: &Store) -> Result<(), String> {
    persistent::save_objectives(store)
}

#[cfg(test)]
fn decode(bytes: &[u8]) -> Option<Store> {
    let store: Store = serde_json::from_slice(bytes).ok()?;
    (store.version == VERSION).then_some(store)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn objectives_round_trip_and_default_to_pending() {
        let mut store = Store::default();
        let id = store.add("inspect the project".to_string());
        assert_eq!(id, 1);
        assert_eq!(store.get(id).expect("objective").status, "pending");
        store.set_status(id, "running");
        assert!(store.reset_running());
        assert_eq!(store.get(id).expect("objective").status, "pending");
        store.set_status(id, "done");
        store.queue_active = true;
        let bytes = serde_json::to_vec(&store).expect("encode");
        let decoded = decode(&bytes).expect("decode");
        assert_eq!(decoded.get(id).expect("objective").status, "done");
        assert!(decoded.queue_active);
        assert!(decode(r#"{"version":99,"next_id":1,"objectives":[]}"#.as_bytes()).is_none());
    }
}
