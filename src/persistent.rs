//! Small JSON stores in the editor config directory.

use serde::Serialize;
use serde::de::DeserializeOwned;

macro_rules! store {
    ($load:ident, $save:ident, $name:literal) => {
        pub fn $load<T: DeserializeOwned>() -> Option<T> {
            let path = crate::config::config_dir()?.join($name);
            let bytes = std::fs::read(path).ok()?;
            serde_json::from_slice(&bytes).ok()
        }

        pub fn $save<T: Serialize>(value: &T) -> Result<(), String> {
            let path = crate::config::config_dir()
                .ok_or_else(|| "config directory is unavailable".to_string())?
                .join($name);
            let bytes = serde_json::to_vec_pretty(value).map_err(|error| error.to_string())?;
            crate::editor::document::write_atomic(&path, &bytes)
                .map_err(|error| error.to_string())?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600));
            }
            Ok(())
        }
    };
}

store!(load_objectives, save_objectives, "agent_objectives.json");
store!(load_context, save_context, "context_index.json");
