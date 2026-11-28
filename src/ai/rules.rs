//! Workspace instruction files injected into the AI system prompt.
//!
//! The first existing file wins: `AGENTS.md`, then `CLAUDE.md`. Only the first
//! 16 KiB is read, and the file is capped again when the system prompt is built.

use std::io::Read;
use std::path::{Path, PathBuf};

pub const MAX_RULES_BYTES: u64 = 16 << 10;
pub const RULE_FILES: [&str; 2] = ["AGENTS.md", "CLAUDE.md"];
pub const MEMORY_FILE: &str = "MEMORY.md";

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct WorkspaceRules {
    pub source: Option<PathBuf>,
    pub text: String,
    pub truncated: bool,
    /// Set when a rules file exists but could not be read as UTF-8.
    pub warning: Option<String>,
}

pub fn load(root: &Path) -> WorkspaceRules {
    for name in RULE_FILES {
        let path = root.join(name);
        let is_file = std::fs::metadata(&path)
            .map(|metadata| metadata.is_file())
            .unwrap_or(false);
        if !is_file {
            continue;
        }
        return read(&path);
    }
    WorkspaceRules::default()
}

/// Long-term notes for the workspace. Injected after rules when enabled.
pub fn load_memory(root: &Path) -> WorkspaceRules {
    let path = root.join(MEMORY_FILE);
    let is_file = std::fs::metadata(&path)
        .map(|metadata| metadata.is_file())
        .unwrap_or(false);
    if !is_file {
        return WorkspaceRules::default();
    }
    read(&path)
}

fn read(path: &Path) -> WorkspaceRules {
    let file = match std::fs::File::open(path) {
        Ok(file) => file,
        Err(error) => {
            return WorkspaceRules {
                source: Some(path.to_path_buf()),
                warning: Some(format!("{}: {error}", path.display())),
                ..WorkspaceRules::default()
            };
        }
    };
    let mut bytes = Vec::new();
    if let Err(error) = file.take(MAX_RULES_BYTES + 1).read_to_end(&mut bytes) {
        return WorkspaceRules {
            source: Some(path.to_path_buf()),
            warning: Some(format!("{}: {error}", path.display())),
            ..WorkspaceRules::default()
        };
    }
    let truncated = bytes.len() as u64 > MAX_RULES_BYTES;
    if truncated {
        bytes.truncate(MAX_RULES_BYTES as usize);
    }
    let text = match std::str::from_utf8(&bytes) {
        Ok(text) => text,
        Err(error) if truncated && error.error_len().is_none() => {
            // Only a multibyte character split by the cap is expected; a bad
            // byte in the middle of the file still deserves the warning.
            std::str::from_utf8(&bytes[..error.valid_up_to()]).unwrap_or("")
        }
        Err(_) => {
            return WorkspaceRules {
                source: Some(path.to_path_buf()),
                warning: Some(format!("{} is not valid UTF-8", path.display())),
                ..WorkspaceRules::default()
            };
        }
    };
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    WorkspaceRules {
        source: Some(path.to_path_buf()),
        text: text.trim_end().to_string(),
        truncated,
        warning: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("i3-rules-{name}-{}", std::process::id()));
        std::fs::remove_dir_all(&dir).ok();
        std::fs::create_dir_all(&dir).expect("dir");
        dir
    }

    #[test]
    fn a_bad_byte_inside_a_large_file_still_warns() {
        let dir = temp_dir("bad-byte");
        let mut bytes = vec![b'a'; 32 * 1024];
        bytes[100] = 0xff;
        std::fs::write(dir.join("AGENTS.md"), bytes).expect("write");
        let rules = load(&dir);
        assert!(
            rules
                .warning
                .as_deref()
                .is_some_and(|warning| warning.contains("UTF-8")),
            "{:?}",
            rules.warning
        );
        assert!(rules.text.is_empty());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn agents_wins_over_claude() {
        let dir = temp_dir("priority");
        std::fs::write(dir.join("AGENTS.md"), "agents rules\n").expect("agents");
        std::fs::write(dir.join("CLAUDE.md"), "claude rules\n").expect("claude");
        let rules = load(&dir);
        assert_eq!(rules.text, "agents rules");
        assert!(rules.source.expect("source").ends_with("AGENTS.md"));
        assert!(!rules.truncated);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn claude_is_the_fallback() {
        let dir = temp_dir("fallback");
        std::fs::write(dir.join("CLAUDE.md"), "claude rules\n").expect("claude");
        let rules = load(&dir);
        assert_eq!(rules.text, "claude rules");
        assert!(rules.source.expect("source").ends_with("CLAUDE.md"));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn oversized_rules_are_truncated_without_splitting_a_character() {
        let dir = temp_dir("truncate");
        let mut text = "a".repeat(MAX_RULES_BYTES as usize - 1);
        text.push('é');
        text.push_str("tail");
        std::fs::write(dir.join("AGENTS.md"), text).expect("rules");
        let rules = load(&dir);
        assert!(rules.truncated);
        assert_eq!(rules.text.len(), MAX_RULES_BYTES as usize - 1);
        assert!(rules.warning.is_none());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn memory_is_loaded_independently() {
        let dir = temp_dir("memory");
        std::fs::write(dir.join("AGENTS.md"), "rules\n").expect("rules");
        std::fs::write(dir.join("MEMORY.md"), "remember this\n").expect("memory");
        let memory = load_memory(&dir);
        assert_eq!(memory.text, "remember this");
        assert!(memory.source.expect("source").ends_with("MEMORY.md"));
        assert_eq!(load(&dir).text, "rules");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn invalid_utf8_is_reported_without_text() {
        let dir = temp_dir("invalid");
        std::fs::write(dir.join("AGENTS.md"), [0xff, 0xfe]).expect("rules");
        let rules = load(&dir);
        assert!(rules.text.is_empty());
        assert!(rules.warning.expect("warning").contains("UTF-8"));
        std::fs::remove_dir_all(&dir).ok();
    }
}
