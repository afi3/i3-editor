//! `.editorconfig` indentation plus per-language tab defaults.

use std::path::Path;

use globset::{GlobBuilder, GlobMatcher};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct IndentSettings {
    pub tab_width: usize,
    pub insert_spaces: bool,
}

impl Default for IndentSettings {
    fn default() -> Self {
        Self {
            tab_width: 4,
            insert_spaces: true,
        }
    }
}

/// Resolve the settings for `path` from per-language defaults, then the
/// workspace-root `.editorconfig` sections in order. Unknown keys are ignored.
pub fn resolve(root: &Path, path: &Path, base: IndentSettings) -> IndentSettings {
    let mut settings = language_defaults(path, base);
    let canonical_root = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());
    let canonical_path = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
    let Ok(text) = std::fs::read_to_string(canonical_root.join(".editorconfig")) else {
        return settings;
    };
    let relative = canonical_path
        .strip_prefix(&canonical_root)
        .or_else(|_| path.strip_prefix(root))
        .unwrap_or(canonical_path.as_path())
        .to_string_lossy()
        .replace('\\', "/");
    for section in parse(&text) {
        // A section whose glob did not compile must not apply to every file.
        if section
            .matcher
            .as_ref()
            .is_some_and(|matcher| matcher.is_match(&relative))
        {
            section.props.apply(&mut settings);
        }
    }
    settings
}

fn language_defaults(path: &Path, base: IndentSettings) -> IndentSettings {
    let name = path
        .file_name()
        .map(|name| name.to_string_lossy().to_ascii_lowercase())
        .unwrap_or_default();
    let extension = path
        .extension()
        .map(|extension| extension.to_string_lossy().to_ascii_lowercase())
        .unwrap_or_default();
    if matches!(name.as_str(), "makefile" | "gnumakefile") || extension == "mk" {
        return IndentSettings {
            tab_width: 8,
            insert_spaces: false,
        };
    }
    if extension == "go" {
        return IndentSettings {
            tab_width: 8,
            insert_spaces: false,
        };
    }
    base
}

struct Section {
    matcher: Option<GlobMatcher>,
    props: Props,
}

#[derive(Default)]
struct Props {
    indent_style: Option<bool>,
    indent_size: Option<usize>,
    tab_width: Option<usize>,
}

impl Props {
    fn set(&mut self, key: &str, value: &str) {
        match key.to_ascii_lowercase().as_str() {
            "indent_style" => match value.to_ascii_lowercase().as_str() {
                "space" => self.indent_style = Some(true),
                "tab" => self.indent_style = Some(false),
                _ => {}
            },
            "indent_size" => {
                if !value.eq_ignore_ascii_case("tab")
                    && let Ok(size) = value.parse::<usize>()
                {
                    self.indent_size = Some(size);
                }
            }
            "tab_width" => {
                if let Ok(width) = value.parse::<usize>() {
                    self.tab_width = Some(width);
                }
            }
            _ => {}
        }
    }

    fn apply(self, settings: &mut IndentSettings) {
        if let Some(insert_spaces) = self.indent_style {
            settings.insert_spaces = insert_spaces;
        }
        if let Some(size) = self.indent_size.filter(|size| *size > 0) {
            settings.tab_width = size;
        }
        if let Some(width) = self.tab_width.filter(|width| *width > 0) {
            settings.tab_width = width;
        }
        settings.tab_width = settings.tab_width.clamp(1, 32);
    }
}

fn parse(text: &str) -> Vec<Section> {
    let mut sections = Vec::new();
    let mut current: Option<Section> = None;
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') || line.starts_with(';') {
            continue;
        }
        if line.starts_with('[') && line.ends_with(']') {
            if let Some(section) = current.take() {
                sections.push(section);
            }
            let pattern = &line[1..line.len() - 1];
            let matcher = GlobBuilder::new(pattern)
                .build()
                .ok()
                .map(|glob| glob.compile_matcher());
            current = Some(Section {
                matcher,
                props: Props::default(),
            });
            continue;
        }
        if let Some((key, value)) = line.split_once('=')
            && let Some(section) = current.as_mut()
        {
            section.props.set(key.trim(), value.trim());
        }
    }
    if let Some(section) = current {
        sections.push(section);
    }
    sections
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_section_with_a_broken_glob_applies_to_nothing() {
        let dir = temp_dir("broken-glob");
        std::fs::write(
            dir.join(".editorconfig"),
            "[*.{rs,ts]\nindent_size = 8\n\n[*.md]\nindent_size = 2\n",
        )
        .expect("write");
        let base = IndentSettings::default();
        assert_eq!(
            resolve(&dir, &dir.join("a.rs"), base),
            base,
            "a malformed glob must not apply to every file"
        );
        assert_eq!(
            resolve(&dir, &dir.join("a.md"), base).tab_width,
            2,
            "a valid section after the broken one still applies"
        );
        std::fs::remove_dir_all(&dir).ok();
    }
    use std::path::PathBuf;

    fn temp_dir(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("i3-editorconfig-{}-{name}", std::process::id()));
        std::fs::remove_dir_all(&dir).ok();
        std::fs::create_dir_all(&dir).expect("dir");
        dir
    }

    #[test]
    fn language_defaults_use_tabs_for_go_and_make() {
        let base = IndentSettings {
            tab_width: 4,
            insert_spaces: true,
        };
        assert_eq!(
            resolve(Path::new("/nonexistent"), Path::new("/tmp/main.go"), base),
            IndentSettings {
                tab_width: 8,
                insert_spaces: false
            }
        );
        assert_eq!(
            resolve(Path::new("/nonexistent"), Path::new("/tmp/Makefile"), base),
            IndentSettings {
                tab_width: 8,
                insert_spaces: false
            }
        );
    }

    #[test]
    fn editorconfig_sections_override_in_order() {
        let dir = temp_dir("sections");
        std::fs::write(
            dir.join(".editorconfig"),
            "root = true\n[*]\nindent_style = space\nindent_size = 2\n\
             [*.go]\nindent_style = tab\ntab_width = 4\n\
             [Makefile]\nindent_style = tab\n\
             [src/*.rs]\nindent_size = 8\n",
        )
        .expect("editorconfig");
        let base = IndentSettings {
            tab_width: 4,
            insert_spaces: true,
        };
        assert_eq!(
            resolve(&dir, &dir.join("main.rs"), base),
            IndentSettings {
                tab_width: 2,
                insert_spaces: true
            }
        );
        assert_eq!(
            resolve(&dir, &dir.join("main.go"), base),
            IndentSettings {
                tab_width: 4,
                insert_spaces: false
            }
        );
        assert_eq!(
            resolve(&dir, &dir.join("Makefile"), base),
            IndentSettings {
                tab_width: 2,
                insert_spaces: false
            }
        );
        assert_eq!(
            resolve(&dir, &dir.join("src").join("main.rs"), base),
            IndentSettings {
                tab_width: 8,
                insert_spaces: true
            }
        );
        std::fs::remove_dir_all(&dir).ok();
    }
}
