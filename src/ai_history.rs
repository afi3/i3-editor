//! Optional persistent AI transcript and conversation history.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use serde_json::Value;

pub const VERSION: u8 = 1;

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct History {
    #[serde(default = "default_version")]
    pub version: u8,
    #[serde(default)]
    pub entries: Vec<String>,
    #[serde(default)]
    pub conversation: Vec<Value>,
}

fn default_version() -> u8 {
    VERSION
}

pub fn path() -> Option<PathBuf> {
    Some(crate::config::config_dir()?.join("ai_history.json"))
}

pub fn load() -> Option<History> {
    let bytes = std::fs::read(path()?).ok()?;
    decode(&bytes)
}

pub fn save(history: &History) -> Result<(), String> {
    let path = path().ok_or_else(|| "config directory is unavailable".to_string())?;
    let bytes = serde_json::to_vec_pretty(history).map_err(|error| error.to_string())?;
    crate::editor::document::write_atomic(&path, &bytes).map_err(|error| error.to_string())?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600));
    }
    Ok(())
}

pub fn remove() {
    if let Some(path) = path() {
        let _ = std::fs::remove_file(path);
    }
}

fn decode(bytes: &[u8]) -> Option<History> {
    let history: History = serde_json::from_slice(bytes).ok()?;
    (history.version == VERSION).then_some(history)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn history_round_trips_and_rejects_other_versions() {
        let history = History {
            version: VERSION,
            entries: vec!["> hello".to_string(), "answer".to_string()],
            conversation: vec![serde_json::json!({"role": "user", "content": "hello"})],
        };
        let bytes = serde_json::to_vec(&history).expect("encode");
        let loaded = decode(&bytes).expect("decode");
        assert_eq!(loaded.entries, history.entries);
        assert_eq!(loaded.conversation, history.conversation);
        assert!(decode(r#"{"version":99,"entries":[],"conversation":[]}"#.as_bytes()).is_none());
    }
}
