//! Configuration: TOML at `~/.config/i3-editor/config.toml`, with defaults for
//! every missing section.
//!
//! A broken section is replaced by its default and reported, so a typo never
//! prevents the editor from starting.

use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(default)]
pub struct AppConfig {
    pub editor: EditorConfig,
    pub explorer: ExplorerConfig,
    pub appearance: AppearanceConfig,
    pub terminal: TerminalConfig,
    pub keys: KeysConfig,
    pub lsp: LspConfig,
    pub mcp: McpConfig,
    pub ai: AiConfig,
    pub git: GitConfig,
    pub lua: LuaConfig,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(default)]
pub struct LuaConfig {
    pub enabled: bool,
    /// Load Lua's io library and expose `fs.write_file`.
    pub allow_file: bool,
    /// Load Lua's os library and expose `process.run`.
    pub allow_process: bool,
    /// Per-callback instruction-time budget in milliseconds.
    pub time_budget_ms: u64,
}

impl Default for LuaConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            allow_file: false,
            allow_process: false,
            time_budget_ms: 100,
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(default)]
pub struct GitConfig {
    /// Git executable used for every command.
    pub binary: String,
}

impl Default for GitConfig {
    fn default() -> Self {
        Self {
            binary: "git".to_string(),
        }
    }
}

/// Language servers, keyed by a short name (`rust`, `python`, ...). The key is
/// also the default language match when `extensions` is empty.
#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(default)]
pub struct LspConfig {
    #[serde(flatten)]
    pub servers: BTreeMap<String, LspServerConfig>,
}

/// Match a configured filename against an exact name or a simple `*` prefix/suffix
/// pattern (`Dockerfile.*`, `*.Dockerfile`).
pub fn filename_matches(pattern: &str, name: &str) -> bool {
    let pattern = pattern.to_ascii_lowercase();
    let name = name.to_ascii_lowercase();
    if let Some(prefix) = pattern.strip_suffix('*') {
        return name.starts_with(prefix);
    }
    if let Some(suffix) = pattern.strip_prefix('*') {
        return name.ends_with(suffix);
    }
    pattern == name
}

#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(default)]
pub struct LspServerConfig {
    pub command: String,
    #[serde(default)]
    pub args: Vec<String>,
    /// File extensions that select this server; empty uses the table key.
    #[serde(default)]
    pub extensions: Vec<String>,
    /// Exact file names for servers such as Dockerfile.
    #[serde(default)]
    pub filenames: Vec<String>,
}

/// MCP servers share the same installer/config shape as language servers.
#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(default)]
pub struct McpConfig {
    #[serde(flatten)]
    pub servers: BTreeMap<String, crate::mcp::McpServerConfig>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(default)]
pub struct AiConfig {
    /// OpenAI-compatible API root, without `/chat/completions`.
    pub base_url: String,
    pub model: String,
    /// Environment variable holding the API key; empty means unauthenticated.
    pub api_key_env: String,
    /// Optional direct key for local configs; prefer `api_key_env`.
    pub api_key: String,
    pub timeout_secs: u64,
    /// Maximum estimated prompt tokens sent to the provider.
    pub max_context_tokens: usize,
    /// Maximum characters kept from one MCP tool result.
    pub max_tool_output_chars: usize,
    /// Maximum prior user turns retained in the request.
    pub max_history_turns: usize,
    /// Attach matching indexed files to a prompt automatically.
    pub auto_context: bool,
    /// Maximum files attached by auto context.
    pub auto_context_files: usize,
    /// Maximum characters attached by auto context across all files.
    pub auto_context_chars: usize,
    /// Stop the harness after this many actual session tokens; 0 is unlimited.
    pub max_session_tokens: usize,
    /// Stop the harness after this estimated session cost; 0 is unlimited.
    pub max_session_cost_usd: f64,
    /// Optional `server/tool` allowlist; empty means every tool.
    pub allowed_tools: Vec<String>,
    /// `server/tool` entries that are never offered or called.
    pub denied_tools: Vec<String>,
    /// Additional attempts after a retryable request failure.
    pub retry_attempts: usize,
    /// Base backoff delay; doubles per attempt.
    pub retry_base_ms: u64,
    /// Maximum backoff delay.
    pub retry_max_ms: u64,
    /// AI panel width as a percentage of the terminal width.
    pub panel_width: u16,
    /// Request SSE streaming from the provider.
    pub stream: bool,
    /// Keep the transcript and conversation across restarts.
    pub persist_history: bool,
    /// Allow the explicit background-agent command.
    pub background_agents: bool,
    /// Run pending objectives sequentially after ai_queue_objectives.
    pub agent_auto_queue: bool,
    /// Optional input price in USD per million prompt tokens.
    pub input_cost_per_million: f64,
    /// Optional output price in USD per million completion tokens.
    pub output_cost_per_million: f64,
    /// OS keyring service name; empty disables keyring lookup.
    pub keyring_service: String,
    /// OS keyring account name.
    pub keyring_account: String,
    /// Tools auto-approved for background agents; empty keeps confirmation.
    pub background_auto_tools: Vec<String>,
}

impl Default for AiConfig {
    fn default() -> Self {
        Self {
            base_url: "https://openrouter.ai/api/v1".to_string(),
            model: "openai/gpt-4o-mini".to_string(),
            api_key_env: "OPENROUTER_API_KEY".to_string(),
            api_key: String::new(),
            timeout_secs: 60,
            max_context_tokens: 16_000,
            max_tool_output_chars: 20_000,
            max_history_turns: 24,
            auto_context: false,
            auto_context_files: 4,
            auto_context_chars: 12_000,
            max_session_tokens: 0,
            max_session_cost_usd: 0.0,
            allowed_tools: Vec::new(),
            denied_tools: Vec::new(),
            retry_attempts: 2,
            retry_base_ms: 500,
            retry_max_ms: 8_000,
            panel_width: 20,
            stream: true,
            persist_history: false,
            background_agents: false,
            agent_auto_queue: false,
            input_cost_per_million: 0.0,
            output_cost_per_million: 0.0,
            keyring_service: String::new(),
            keyring_account: String::new(),
            background_auto_tools: Vec::new(),
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(default)]
pub struct TerminalConfig {
    /// Height of the bottom terminal panel in rows.
    pub height: u16,
    /// Shell executable; empty or missing uses `$SHELL`, then `/bin/sh`.
    pub shell: Option<String>,
    /// Lines kept in the panel's scrollback.
    pub scrollback: usize,
}

impl Default for TerminalConfig {
    fn default() -> Self {
        Self {
            height: 12,
            shell: None,
            scrollback: 2000,
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(default)]
pub struct EditorConfig {
    pub tab_size: usize,
    pub insert_spaces: bool,
    pub line_numbers: bool,
    pub auto_indent: bool,
    pub word_wrap: bool,
}

impl Default for EditorConfig {
    fn default() -> Self {
        Self {
            tab_size: 4,
            insert_spaces: true,
            line_numbers: true,
            auto_indent: true,
            word_wrap: false,
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(default)]
pub struct ExplorerConfig {
    pub visible: bool,
    /// Percentage of the terminal width, clamped to `5..=50`.
    pub width: u16,
    /// Show dotfiles and entries ignored by `.gitignore` (for example `.env`).
    pub show_hidden: bool,
}

impl Default for ExplorerConfig {
    fn default() -> Self {
        Self {
            visible: true,
            width: 20,
            show_hidden: false,
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(default)]
pub struct AppearanceConfig {
    pub theme: String,
    pub show_status_bar: bool,
    pub show_document_tabs: bool,
    pub show_workspace_tabs: bool,
}

impl Default for AppearanceConfig {
    fn default() -> Self {
        Self {
            theme: "default".to_string(),
            show_status_bar: true,
            show_document_tabs: true,
            show_workspace_tabs: true,
        }
    }
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(default)]
pub struct KeysConfig {
    pub preset: Option<String>,
    pub global: BTreeMap<String, String>,
    pub editing: BTreeMap<String, String>,
    pub explorer: BTreeMap<String, String>,
    /// Where the section was read from, for line-numbered binding warnings.
    #[serde(skip)]
    pub source: Option<ConfigSource>,
}

#[derive(Clone, Debug)]
pub struct ConfigSource {
    pub path: PathBuf,
    pub text: String,
}

impl ConfigSource {
    /// `path:line:` prefix for a section header or a key inside it.
    pub fn prefix(&self, section: &str, key: Option<&str>) -> String {
        match self.line_of(section, key) {
            Some(line) => format!("{}:{line}: ", self.path.display()),
            None => format!("{}: ", self.path.display()),
        }
    }

    fn line_of(&self, section: &str, key: Option<&str>) -> Option<usize> {
        let mut inside = false;
        let mut fallback = None;
        for (index, line) in self.text.lines().enumerate() {
            let trimmed = line.trim();
            if let Some(name) = trimmed
                .strip_prefix('[')
                .and_then(|rest| rest.strip_suffix(']'))
            {
                inside = name.trim() == section;
                if inside && key.is_none() {
                    return Some(index + 1);
                }
                continue;
            }
            let Some(key) = key else {
                continue;
            };
            let found = trimmed
                .split('=')
                .next()
                .unwrap_or("")
                .trim()
                .trim_matches('"');
            if found == key {
                if inside {
                    return Some(index + 1);
                }
                // Nested tables such as `[keys.global]` live below the parent
                // section; use the first matching key there as a fallback.
                fallback.get_or_insert(index + 1);
            }
        }
        fallback
    }
}

impl KeysConfig {
    /// Prefix for a binding warning, with the config path and line when known.
    pub fn warning_prefix(&self, scope: &str, action: &str) -> String {
        match &self.source {
            Some(source) => source.prefix(&format!("keys.{scope}"), Some(action)),
            None => format!("[keys.{scope}] "),
        }
    }
}

pub fn config_dir() -> Option<PathBuf> {
    if let Some(xdg) = std::env::var_os("XDG_CONFIG_HOME")
        && !xdg.is_empty()
    {
        return Some(PathBuf::from(xdg).join("i3-editor"));
    }
    #[cfg(windows)]
    if let Some(appdata) = std::env::var_os("APPDATA")
        && !appdata.is_empty()
    {
        return Some(PathBuf::from(appdata).join("i3-editor"));
    }
    std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".config").join("i3-editor"))
}

pub fn themes_dir() -> Option<PathBuf> {
    config_dir().map(|dir| dir.join("themes"))
}

pub fn lsp_dir() -> Option<PathBuf> {
    config_dir().map(|dir| dir.join("lsp"))
}

pub fn managed_lsp_path() -> Option<PathBuf> {
    config_dir().map(|dir| dir.join("lsp.toml"))
}

pub fn load_managed_lsp() -> (LspConfig, Vec<String>) {
    let Some(path) = managed_lsp_path() else {
        return (LspConfig::default(), Vec::new());
    };
    let text = match std::fs::read_to_string(&path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return (LspConfig::default(), Vec::new());
        }
        Err(error) => {
            return (
                LspConfig::default(),
                vec![format!("cannot read {}: {error}", path.display())],
            );
        }
    };
    match toml::from_str::<LspConfig>(&text) {
        Ok(config) => (config, Vec::new()),
        Err(error) => (
            LspConfig::default(),
            vec![format!("{} is not valid LSP TOML: {error}", path.display())],
        ),
    }
}

/// Merge installer-managed servers. An explicit `[lsp.<name>]` in the user
/// config always wins, so an install never silently overrides a hand-written
/// command.
pub fn merge_managed_lsp(config: &mut AppConfig) -> Vec<String> {
    let (managed, warnings) = load_managed_lsp();
    merge_lsp_config(config, managed);
    warnings
}

fn merge_lsp_config(config: &mut AppConfig, managed: LspConfig) {
    for (key, server) in managed.servers {
        config.lsp.servers.entry(key).or_insert(server);
    }
}

pub fn persist_lsp_server(key: &str, server: &LspServerConfig) -> std::io::Result<()> {
    let (mut managed, _) = load_managed_lsp();
    managed.servers.insert(key.to_string(), server.clone());
    write_managed_lsp(&managed)
}

/// Remove one installer-managed server and return its old command config.
pub fn take_managed_lsp_server(key: &str) -> std::io::Result<Option<LspServerConfig>> {
    let (mut managed, _) = load_managed_lsp();
    let Some(server) = managed.servers.remove(key) else {
        return Ok(None);
    };
    write_managed_lsp(&managed)?;
    Ok(Some(server))
}

fn write_managed_lsp(managed: &LspConfig) -> std::io::Result<()> {
    let Some(path) = managed_lsp_path() else {
        return Err(std::io::Error::other("no config directory"));
    };
    let text = toml::to_string(managed).map_err(std::io::Error::other)?;
    if let Some(parent) = path.parent()
        && !parent.as_os_str().is_empty()
    {
        std::fs::create_dir_all(parent)?;
    }
    let temporary = path.with_extension("toml.tmp");
    std::fs::write(&temporary, text)?;
    std::fs::rename(&temporary, path)
}

#[derive(Default, Deserialize, Serialize)]
struct McpTrustFile {
    #[serde(default)]
    trusted: Vec<String>,
}

pub fn mcp_trust_path() -> Option<PathBuf> {
    config_dir().map(|dir| dir.join("mcp_trust.toml"))
}

pub fn load_mcp_trust() -> (HashSet<String>, Vec<String>) {
    let Some(path) = mcp_trust_path() else {
        return (HashSet::new(), Vec::new());
    };
    let text = match std::fs::read_to_string(&path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return (HashSet::new(), Vec::new());
        }
        Err(error) => {
            return (
                HashSet::new(),
                vec![format!("cannot read {}: {error}", path.display())],
            );
        }
    };
    match toml::from_str::<McpTrustFile>(&text) {
        Ok(file) => (file.trusted.into_iter().collect(), Vec::new()),
        Err(error) => (
            HashSet::new(),
            vec![format!(
                "{} is not valid MCP trust TOML: {error}",
                path.display()
            )],
        ),
    }
}

fn render_mcp_trust(trusted: &HashSet<String>) -> String {
    let mut values: Vec<String> = trusted.iter().cloned().collect();
    values.sort_unstable();
    toml::to_string(&McpTrustFile { trusted: values }).unwrap_or_default()
}

pub fn save_mcp_trust(trusted: &HashSet<String>) -> std::io::Result<()> {
    let Some(path) = mcp_trust_path() else {
        return Err(std::io::Error::other("no config directory"));
    };
    let text = render_mcp_trust(trusted);
    if let Some(parent) = path.parent()
        && !parent.as_os_str().is_empty()
    {
        std::fs::create_dir_all(parent)?;
    }
    let temporary = path.with_extension("toml.tmp");
    std::fs::write(&temporary, text)?;
    std::fs::rename(&temporary, path)
}

pub fn managed_mcp_path() -> Option<PathBuf> {
    config_dir().map(|dir| dir.join("mcp.toml"))
}

pub fn load_managed_mcp() -> (McpConfig, Vec<String>) {
    let Some(path) = managed_mcp_path() else {
        return (McpConfig::default(), Vec::new());
    };
    let text = match std::fs::read_to_string(&path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return (McpConfig::default(), Vec::new());
        }
        Err(error) => {
            return (
                McpConfig::default(),
                vec![format!("cannot read {}: {error}", path.display())],
            );
        }
    };
    match toml::from_str::<McpConfig>(&text) {
        Ok(config) => (config, Vec::new()),
        Err(error) => (
            McpConfig::default(),
            vec![format!("{} is not valid MCP TOML: {error}", path.display())],
        ),
    }
}

pub fn merge_managed_mcp(config: &mut AppConfig) -> Vec<String> {
    let (managed, warnings) = load_managed_mcp();
    merge_mcp_config(config, managed);
    warnings
}

fn merge_mcp_config(config: &mut AppConfig, managed: McpConfig) {
    for (key, server) in managed.servers {
        config.mcp.servers.entry(key).or_insert(server);
    }
}

fn write_managed_mcp(managed: &McpConfig) -> std::io::Result<()> {
    let Some(path) = managed_mcp_path() else {
        return Err(std::io::Error::other("no config directory"));
    };
    let text = toml::to_string(managed).map_err(std::io::Error::other)?;
    if let Some(parent) = path.parent()
        && !parent.as_os_str().is_empty()
    {
        std::fs::create_dir_all(parent)?;
    }
    let temporary = path.with_extension("toml.tmp");
    std::fs::write(&temporary, text)?;
    std::fs::rename(&temporary, path)
}

pub fn persist_mcp_server(key: &str, server: &crate::mcp::McpServerConfig) -> std::io::Result<()> {
    let (mut managed, _) = load_managed_mcp();
    managed.servers.insert(key.to_string(), server.clone());
    write_managed_mcp(&managed)
}

#[derive(Default, Deserialize, Serialize)]
struct McpRecentFile {
    #[serde(flatten)]
    servers: BTreeMap<String, Vec<String>>,
}

pub fn mcp_recent_path() -> Option<PathBuf> {
    config_dir().map(|dir| dir.join("mcp_recent.toml"))
}

pub fn load_mcp_recent() -> (BTreeMap<String, Vec<String>>, Vec<String>) {
    let Some(path) = mcp_recent_path() else {
        return (BTreeMap::new(), Vec::new());
    };
    let text = match std::fs::read_to_string(&path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return (BTreeMap::new(), Vec::new());
        }
        Err(error) => {
            return (
                BTreeMap::new(),
                vec![format!("cannot read {}: {error}", path.display())],
            );
        }
    };
    match toml::from_str::<McpRecentFile>(&text) {
        Ok(file) => (file.servers, Vec::new()),
        Err(error) => (
            BTreeMap::new(),
            vec![format!(
                "{} is not valid MCP recent TOML: {error}",
                path.display()
            )],
        ),
    }
}

fn render_mcp_recent(recent: &BTreeMap<String, Vec<String>>) -> String {
    toml::to_string(&McpRecentFile {
        servers: recent.clone(),
    })
    .unwrap_or_default()
}

pub fn save_mcp_recent(recent: &BTreeMap<String, Vec<String>>) -> std::io::Result<()> {
    let Some(path) = mcp_recent_path() else {
        return Err(std::io::Error::other("no config directory"));
    };
    let text = render_mcp_recent(recent);
    if let Some(parent) = path.parent()
        && !parent.as_os_str().is_empty()
    {
        std::fs::create_dir_all(parent)?;
    }
    let temporary = path.with_extension("toml.tmp");
    std::fs::write(&temporary, text)?;
    std::fs::rename(&temporary, path)
}

pub fn default_path() -> Option<PathBuf> {
    config_dir().map(|dir| dir.join("config.toml"))
}

pub fn history_path() -> Option<PathBuf> {
    config_dir().map(|dir| dir.join("history.toml"))
}

pub fn load() -> (AppConfig, Vec<String>) {
    match default_path() {
        Some(path) => {
            let (mut config, mut warnings) = load_from(&path);
            warnings.extend(merge_managed_lsp(&mut config));
            warnings.extend(merge_managed_mcp(&mut config));
            (config, warnings)
        }
        None => (AppConfig::default(), Vec::new()),
    }
}

pub fn load_from(path: &Path) -> (AppConfig, Vec<String>) {
    let mut warnings = Vec::new();
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return (AppConfig::default(), warnings);
        }
        Err(error) => {
            warnings.push(format!("cannot read {}: {error}", path.display()));
            return (AppConfig::default(), warnings);
        }
    };

    let value: toml::Value = match toml::from_str(&text) {
        Ok(value) => value,
        Err(error) => {
            warnings.push(format!("{} is not valid TOML: {error}", path.display()));
            return (AppConfig::default(), warnings);
        }
    };

    let source = ConfigSource {
        path: path.to_path_buf(),
        text: text.clone(),
    };
    const SECTIONS: [&str; 10] = [
        "editor",
        "explorer",
        "appearance",
        "terminal",
        "keys",
        "lsp",
        "mcp",
        "ai",
        "git",
        "lua",
    ];
    if let Some(table) = value.as_table() {
        for key in table.keys() {
            if !SECTIONS.contains(&key.as_str()) {
                warnings.push(format!(
                    "{}unknown section [{key}] ignored",
                    source.prefix(key, None)
                ));
            }
        }
    }

    for (name, keys) in [
        ("editor", EDITOR_KEYS),
        ("explorer", EXPLORER_KEYS),
        ("appearance", APPEARANCE_KEYS),
        ("terminal", TERMINAL_KEYS),
        ("keys", KEYS_KEYS),
        ("git", GIT_KEYS),
        ("ai", AI_KEYS),
        ("lua", LUA_KEYS),
    ] {
        warn_unknown_keys(&value, name, keys, &source, &mut warnings);
    }

    let mut config = AppConfig::default();
    take_section(&value, "editor", &mut config.editor, &source, &mut warnings);
    take_section(
        &value,
        "explorer",
        &mut config.explorer,
        &source,
        &mut warnings,
    );
    take_section(
        &value,
        "appearance",
        &mut config.appearance,
        &source,
        &mut warnings,
    );
    take_section(
        &value,
        "terminal",
        &mut config.terminal,
        &source,
        &mut warnings,
    );
    take_section(&value, "keys", &mut config.keys, &source, &mut warnings);
    take_section(&value, "lsp", &mut config.lsp, &source, &mut warnings);
    take_section(&value, "mcp", &mut config.mcp, &source, &mut warnings);
    take_section(&value, "ai", &mut config.ai, &source, &mut warnings);
    take_section(&value, "git", &mut config.git, &source, &mut warnings);
    take_section(&value, "lua", &mut config.lua, &source, &mut warnings);
    config.keys.source = Some(source);
    if config.git.binary.trim().is_empty() {
        config.git.binary = "git".to_string();
    }
    config.lua.time_budget_ms = config.lua.time_budget_ms.clamp(10, 5_000);
    config.explorer.width = config.explorer.width.clamp(5, 50);
    config.terminal.height = config.terminal.height.clamp(3, 60);
    config.terminal.scrollback = config.terminal.scrollback.clamp(100, 100_000);
    config.ai.timeout_secs = config.ai.timeout_secs.clamp(1, 600);
    config.ai.max_context_tokens = config.ai.max_context_tokens.clamp(512, 1_000_000);
    config.ai.max_tool_output_chars = config.ai.max_tool_output_chars.clamp(500, 1_000_000);
    config.ai.max_history_turns = config.ai.max_history_turns.clamp(1, 500);
    config.ai.auto_context_files = config.ai.auto_context_files.clamp(1, 20);
    config.ai.auto_context_chars = config.ai.auto_context_chars.clamp(500, 200_000);
    config.ai.max_session_tokens = config.ai.max_session_tokens.min(1_000_000_000);
    config.ai.max_session_cost_usd = config.ai.max_session_cost_usd.max(0.0);
    config.ai.input_cost_per_million = config.ai.input_cost_per_million.max(0.0);
    config.ai.output_cost_per_million = config.ai.output_cost_per_million.max(0.0);
    config.ai.retry_attempts = config.ai.retry_attempts.clamp(0, 5);
    config.ai.retry_base_ms = config.ai.retry_base_ms.clamp(50, 60_000);
    config.ai.retry_max_ms = config.ai.retry_max_ms.clamp(100, 120_000);
    config.ai.panel_width = config.ai.panel_width.clamp(15, 40);
    (config, warnings)
}

const EDITOR_KEYS: &[&str] = &[
    "tab_size",
    "insert_spaces",
    "line_numbers",
    "auto_indent",
    "word_wrap",
];
const EXPLORER_KEYS: &[&str] = &["visible", "width", "show_hidden"];
const APPEARANCE_KEYS: &[&str] = &[
    "theme",
    "show_status_bar",
    "show_document_tabs",
    "show_workspace_tabs",
];
const TERMINAL_KEYS: &[&str] = &["height", "shell", "scrollback"];
const GIT_KEYS: &[&str] = &["binary"];
const AI_KEYS: &[&str] = &[
    "base_url",
    "model",
    "api_key_env",
    "api_key",
    "timeout_secs",
    "max_context_tokens",
    "max_tool_output_chars",
    "max_history_turns",
    "auto_context",
    "auto_context_files",
    "auto_context_chars",
    "max_session_tokens",
    "max_session_cost_usd",
    "allowed_tools",
    "denied_tools",
    "retry_attempts",
    "retry_base_ms",
    "retry_max_ms",
    "panel_width",
    "stream",
    "persist_history",
    "background_agents",
    "agent_auto_queue",
    "input_cost_per_million",
    "output_cost_per_million",
    "keyring_service",
    "keyring_account",
    "background_auto_tools",
];
const LUA_KEYS: &[&str] = &["enabled", "allow_file", "allow_process", "time_budget_ms"];
const KEYS_KEYS: &[&str] = &["preset", "global", "editing", "explorer"];

/// Report keys the loader does not understand without discarding the rest of
/// the section, matching the per-entry fallback contract.
fn warn_unknown_keys(
    value: &toml::Value,
    name: &str,
    known: &[&str],
    source: &ConfigSource,
    warnings: &mut Vec<String>,
) {
    let Some(section) = value.get(name).and_then(toml::Value::as_table) else {
        return;
    };
    for key in section.keys() {
        if !known.contains(&key.as_str()) {
            warnings.push(format!(
                "{}[{name}] unknown key {key} ignored",
                source.prefix(name, Some(key))
            ));
        }
    }
}

fn take_section<T: for<'de> Deserialize<'de> + std::fmt::Debug>(
    value: &toml::Value,
    name: &str,
    target: &mut T,
    source: &ConfigSource,
    warnings: &mut Vec<String>,
) {
    let Some(section) = value.get(name) else {
        return;
    };
    match section.clone().try_into::<T>() {
        Ok(parsed) => *target = parsed,
        Err(error) => {
            let prefix = match error_key(&error) {
                Some(key) => source.prefix(name, Some(&key)),
                None => source.prefix(name, None),
            };
            warnings.push(format!("{prefix}[{name}] ignored: {error}"));
        }
    }
}

/// `toml` reports the failing field only inside its display text
/// (`... in `tab_size``), so recover it to point at the entry's line.
fn error_key(error: &toml::de::Error) -> Option<String> {
    let text = error.to_string();
    let path = text.rsplit_once("in `")?.1.split_once('`')?.0;
    // KeysConfig errors report `global.save`; the line holds `save`.
    Some(path.rsplit('.').next().unwrap_or(path).to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_file_yields_defaults_without_warnings() {
        let (config, warnings) = load_from(Path::new("/nonexistent/i3-editor/config.toml"));
        assert!(warnings.is_empty());
        assert_eq!(config.editor.tab_size, 4);
        assert_eq!(config.explorer.width, 20);
    }

    #[test]
    fn defaults_fill_missing_sections() {
        let dir = std::env::temp_dir().join(format!("i3-config-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("dir");
        let path = dir.join("config.toml");
        std::fs::write(&path, "[editor]\ntab_size = 2\n").expect("write");
        let (config, warnings) = load_from(&path);
        assert!(warnings.is_empty());
        assert_eq!(config.editor.tab_size, 2);
        assert!(config.editor.auto_indent);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_broken_section_is_reported_and_replaced() {
        let dir = std::env::temp_dir().join(format!("i3-config-bad-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("dir");
        let path = dir.join("config.toml");
        std::fs::write(&path, "[editor]\ntab_size = \"four\"\n").expect("write");
        let (config, warnings) = load_from(&path);
        assert_eq!(warnings.len(), 1, "{warnings:?}");
        assert_eq!(config.editor.tab_size, 4);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn ai_config_is_read_and_timeout_is_clamped() {
        let dir = std::env::temp_dir().join(format!("i3-config-ai-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("dir");
        let path = dir.join("config.toml");
        std::fs::write(
            &path,
            "[ai]\nbase_url = \"http://localhost:11434/v1\"\nmodel = \"local\"\napi_key_env = \"LOCAL_KEY\"\ntimeout_secs = 9999\nmax_context_tokens = 4096\nmax_tool_output_chars = 1000\nmax_history_turns = 6\nauto_context = true\nauto_context_files = 3\nauto_context_chars = 4000\nmax_session_tokens = 9000\nmax_session_cost_usd = 0.25\nallowed_tools = [\"mock/echo\"]\ndenied_tools = [\"mock/add\"]\nretry_attempts = 4\nretry_base_ms = 100\nretry_max_ms = 2000\npanel_width = 35\ninput_cost_per_million = 1.5\noutput_cost_per_million = 6.0\n",
        )
        .expect("write");
        let (config, warnings) = load_from(&path);
        assert!(warnings.is_empty(), "{warnings:?}");
        assert_eq!(config.ai.base_url, "http://localhost:11434/v1");
        assert_eq!(config.ai.model, "local");
        assert_eq!(config.ai.api_key_env, "LOCAL_KEY");
        assert_eq!(config.ai.timeout_secs, 600);
        assert_eq!(config.ai.max_context_tokens, 4096);
        assert_eq!(config.ai.max_tool_output_chars, 1000);
        assert_eq!(config.ai.max_history_turns, 6);
        assert!(config.ai.auto_context);
        assert_eq!(config.ai.auto_context_files, 3);
        assert_eq!(config.ai.auto_context_chars, 4000);
        assert_eq!(config.ai.max_session_tokens, 9000);
        assert_eq!(config.ai.max_session_cost_usd, 0.25);
        assert_eq!(config.ai.allowed_tools, ["mock/echo"]);
        assert_eq!(config.ai.denied_tools, ["mock/add"]);
        assert_eq!(config.ai.retry_attempts, 4);
        assert_eq!(config.ai.retry_base_ms, 100);
        assert_eq!(config.ai.retry_max_ms, 2000);
        assert_eq!(config.ai.panel_width, 35);
        assert_eq!(config.ai.input_cost_per_million, 1.5);
        assert_eq!(config.ai.output_cost_per_million, 6.0);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn mcp_servers_are_read_from_nested_tables() {
        let dir = std::env::temp_dir().join(format!("i3-config-mcp-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("dir");
        let path = dir.join("config.toml");
        std::fs::write(
            &path,
            "[mcp.filesystem]\ncommand = \"npx\"\nargs = [\"-y\", \"server\"]\ncwd = \"/tmp\"\nenv = { TOKEN = \"x\" }\n",
        )
        .expect("write");
        let (config, warnings) = load_from(&path);
        assert!(warnings.is_empty(), "{warnings:?}");
        let server = config.mcp.servers.get("filesystem").expect("mcp server");
        assert_eq!(server.command, "npx");
        assert_eq!(server.args, vec!["-y".to_string(), "server".to_string()]);
        assert_eq!(server.cwd.as_deref(), Some(std::path::Path::new("/tmp")));
        assert_eq!(server.env.get("TOKEN").map(String::as_str), Some("x"));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn invalid_toml_is_reported_and_ignored() {
        let dir = std::env::temp_dir().join(format!("i3-config-syntax-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("dir");
        let path = dir.join("config.toml");
        std::fs::write(&path, "[editor\n").expect("write");
        let (config, warnings) = load_from(&path);
        assert_eq!(warnings.len(), 1);
        assert_eq!(config.editor.tab_size, 4);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn lsp_servers_are_read_from_nested_tables() {
        let dir = std::env::temp_dir().join(format!("i3-config-lsp-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("dir");
        let path = dir.join("config.toml");
        std::fs::write(
            &path,
            "[lsp.rust]\ncommand = \"rust-analyzer\"\nargs = [\"--stdio\"]\nextensions = [\"rs\"]\n",
        )
        .expect("write");
        let (config, warnings) = load_from(&path);
        assert!(warnings.is_empty(), "{warnings:?}");
        let rust = config.lsp.servers.get("rust").expect("rust server");
        assert_eq!(rust.command, "rust-analyzer");
        assert_eq!(rust.args, vec!["--stdio".to_string()]);
        assert_eq!(rust.extensions, vec!["rs".to_string()]);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn git_binary_defaults_and_overrides() {
        let dir = std::env::temp_dir().join(format!("i3-config-git-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("dir");
        let path = dir.join("config.toml");
        std::fs::write(&path, "[git]\nbinary = \"/usr/bin/git\"\n").expect("write");
        let (config, warnings) = load_from(&path);
        assert!(warnings.is_empty(), "{warnings:?}");
        assert_eq!(config.git.binary, "/usr/bin/git");

        std::fs::write(&path, "[git]\nbinary = \"\"\n").expect("write");
        let (config, _) = load_from(&path);
        assert_eq!(config.git.binary, "git");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn explorer_width_is_clamped() {
        let dir = std::env::temp_dir().join(format!("i3-config-width-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("dir");
        let path = dir.join("config.toml");
        std::fs::write(&path, "[explorer]\nwidth = 90\n").expect("write");
        let (config, _) = load_from(&path);
        assert_eq!(config.explorer.width, 50);
        std::fs::remove_dir_all(&dir).ok();
    }
}

pub fn persist_theme_at(path: &Path, name: &str) -> std::io::Result<()> {
    let existing = std::fs::read_to_string(path).unwrap_or_default();
    let updated = set_theme_value(&existing, name);
    if let Some(parent) = path.parent()
        && !parent.as_os_str().is_empty()
    {
        std::fs::create_dir_all(parent)?;
    }
    let temporary = path.with_extension("toml.tmp");
    std::fs::write(&temporary, updated)?;
    std::fs::rename(&temporary, path)?;
    Ok(())
}

fn set_theme_value(text: &str, name: &str) -> String {
    let escaped = name.replace('\\', "\\\\").replace('"', "\\\"");
    let mut lines: Vec<String> = text.lines().map(str::to_string).collect();

    let mut section_start = None;
    let mut section_end = lines.len();
    for (index, line) in lines.iter().enumerate() {
        let trimmed = line.trim_start();
        if trimmed.starts_with('[') {
            let header = trimmed.split('#').next().unwrap_or("").trim();
            match section_start {
                Some(_) => {
                    section_end = index;
                    break;
                }
                None if header == "[appearance]" => section_start = Some(index),
                None => {}
            }
        }
    }

    match section_start {
        Some(start) => {
            let key = (start + 1..section_end.min(lines.len())).find(|index| {
                let trimmed = lines[*index].trim_start();
                trimmed
                    .split('=')
                    .next()
                    .map(|name| name.trim() == "theme")
                    .unwrap_or(false)
            });
            match key {
                Some(index) => lines[index] = format!("theme = \"{escaped}\""),
                None => lines.insert(start + 1, format!("theme = \"{escaped}\"")),
            }
        }
        None => {
            if lines.last().is_some_and(|line| !line.trim().is_empty()) {
                lines.push(String::new());
            }
            lines.push("[appearance]".to_string());
            lines.push(format!("theme = \"{escaped}\""));
        }
    }

    let mut out = lines.join("\n");
    out.push('\n');
    out
}

#[cfg(test)]
mod theme_persist_tests {
    use super::*;

    fn temp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("i3-persist-{}-{name}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("dir");
        dir.join("config.toml")
    }

    #[test]
    fn replaces_theme_and_keeps_other_sections() {
        let path = temp("replace");
        std::fs::write(
            &path,
            "[editor]\ntab_size = 2\n\n[appearance]\n# look\nshow_status_bar = false\ntheme = \"dark\"\n\n[terminal]\nheight = 8\n",
        )
        .expect("write");
        persist_theme_at(&path, "mine").expect("persist");
        let text = std::fs::read_to_string(&path).expect("read");
        assert!(text.contains("theme = \"mine\""), "{text}");
        assert!(text.contains("tab_size = 2"), "{text}");
        assert!(text.contains("height = 8"), "{text}");
        assert!(text.contains("# look"), "{text}");
        assert_eq!(text.matches("theme =").count(), 1, "{text}");
        assert!(!text.contains("theme = \"dark\""), "{text}");
        std::fs::remove_dir_all(path.parent().expect("parent")).ok();
    }

    #[test]
    fn appends_the_section_when_missing() {
        let path = temp("append");
        std::fs::write(&path, "[editor]\ntab_size = 2\n").expect("write");
        persist_theme_at(&path, "light").expect("persist");
        let text = std::fs::read_to_string(&path).expect("read");
        assert!(text.contains("[appearance]"), "{text}");
        assert!(text.contains("theme = \"light\""), "{text}");
        assert!(text.contains("tab_size = 2"), "{text}");
        std::fs::remove_dir_all(path.parent().expect("parent")).ok();
    }

    #[test]
    fn inserts_the_key_into_an_existing_section() {
        let path = temp("insert");
        std::fs::write(
            &path,
            "[appearance]\nshow_status_bar = true\n[terminal]\nheight = 8\n",
        )
        .expect("write");
        persist_theme_at(&path, "high-contrast").expect("persist");
        let text = std::fs::read_to_string(&path).expect("read");
        let appearance = text.find("[appearance]").expect("section");
        let terminal = text.find("[terminal]").expect("terminal");
        let theme = text.find("theme = \"high-contrast\"").expect("theme");
        assert!(appearance < theme && theme < terminal, "{text}");
        std::fs::remove_dir_all(path.parent().expect("parent")).ok();
    }

    #[test]
    fn creates_a_missing_file() {
        let path = temp("create");
        persist_theme_at(&path, "dark").expect("persist");
        let text = std::fs::read_to_string(&path).expect("read");
        assert_eq!(text, "[appearance]\ntheme = \"dark\"\n");
        std::fs::remove_dir_all(path.parent().expect("parent")).ok();
    }

    #[test]
    fn escapes_quotes_in_the_name() {
        let path = temp("escape");
        persist_theme_at(&path, "a\"b").expect("persist");
        let text = std::fs::read_to_string(&path).expect("read");
        assert!(text.contains("theme = \"a\\\"b\""), "{text}");
        std::fs::remove_dir_all(path.parent().expect("parent")).ok();
    }
}

#[cfg(test)]
mod strict_tests {
    use super::*;

    fn config_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("i3-strict-{}-{name}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("dir");
        dir
    }

    #[test]
    fn unknown_key_is_reported_and_valid_keys_survive() {
        let dir = config_dir("key");
        let path = dir.join("config.toml");
        std::fs::write(&path, "[editor]\ntab_size = 2\ntab_sise = 4\n").expect("write");
        let (config, warnings) = load_from(&path);
        assert_eq!(warnings.len(), 1, "{warnings:?}");
        assert!(warnings[0].contains("tab_sise"), "{warnings:?}");
        assert!(
            warnings[0].contains(&format!("{}:3:", path.display())),
            "warning must name file and line: {warnings:?}"
        );
        assert_eq!(
            config.editor.tab_size, 2,
            "valid keys in the section survive"
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn invalid_value_names_the_entry_and_line() {
        let dir = config_dir("value");
        let path = dir.join("config.toml");
        std::fs::write(&path, "[editor]\ntab_size = \"wide\"\n").expect("write");
        let (config, warnings) = load_from(&path);
        assert_eq!(warnings.len(), 1, "{warnings:?}");
        assert!(
            warnings[0].contains(&format!("{}:2:", path.display())),
            "warning must name the entry line: {warnings:?}"
        );
        assert!(warnings[0].contains("tab_size"), "{warnings:?}");
        assert_eq!(config.editor.tab_size, 4, "section falls back to defaults");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn invalid_binding_points_at_the_nested_entry() {
        let dir = config_dir("binding");
        let path = dir.join("config.toml");
        std::fs::write(&path, "[keys.global]\nsave = 3\n").expect("write");
        let (_, warnings) = load_from(&path);
        assert_eq!(warnings.len(), 1, "{warnings:?}");
        assert!(
            warnings[0].contains(&format!("{}:2:", path.display())),
            "warning must name the nested entry: {warnings:?}"
        );
        assert!(warnings[0].contains("save"), "{warnings:?}");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn unknown_key_sections_do_not_reset_bindings() {
        let dir = config_dir("keys");
        let path = dir.join("config.toml");
        std::fs::write(
            &path,
            "[keys]\npreset = \"safe\"\n\n[keys.bogus]\nsave = \"ctrl+s\"\n",
        )
        .expect("write");
        let (config, warnings) = load_from(&path);
        assert_eq!(warnings.len(), 1, "{warnings:?}");
        assert!(warnings[0].contains("bogus"), "{warnings:?}");
        assert_eq!(config.keys.preset.as_deref(), Some("safe"));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn known_key_lists_cover_every_config_field() {
        let value = toml::Value::try_from(AppConfig::default()).expect("serialize");
        let sections = [
            ("editor", EDITOR_KEYS),
            ("explorer", EXPLORER_KEYS),
            ("appearance", APPEARANCE_KEYS),
            ("terminal", TERMINAL_KEYS),
            ("keys", KEYS_KEYS),
        ];
        for (name, known) in sections {
            let table = value
                .get(name)
                .and_then(toml::Value::as_table)
                .unwrap_or_else(|| panic!("missing [{name}]"));
            for key in table.keys() {
                assert!(
                    known.contains(&key.as_str()),
                    "[{name}] {key} is not listed"
                );
            }
        }
    }

    #[test]
    fn managed_lsp_entries_merge_without_overriding_explicit_config() {
        let mut config = AppConfig::default();
        config.lsp.servers.insert(
            "rust".to_string(),
            LspServerConfig {
                command: "explicit-rust".to_string(),
                ..LspServerConfig::default()
            },
        );
        let mut managed = LspConfig::default();
        managed.servers.insert(
            "rust".to_string(),
            LspServerConfig {
                command: "managed-rust".to_string(),
                ..LspServerConfig::default()
            },
        );
        managed.servers.insert(
            "python".to_string(),
            LspServerConfig {
                command: "managed-python".to_string(),
                ..LspServerConfig::default()
            },
        );
        merge_lsp_config(&mut config, managed);
        assert_eq!(config.lsp.servers["rust"].command, "explicit-rust");
        assert_eq!(config.lsp.servers["python"].command, "managed-python");
    }

    #[test]
    fn filenames_support_prefix_and_suffix_patterns() {
        assert!(filename_matches("Dockerfile", "dockerfile"));
        assert!(filename_matches("Dockerfile.*", "Dockerfile.dev"));
        assert!(filename_matches("*.Dockerfile", "app.Dockerfile"));
        assert!(filename_matches("Containerfile", "containerfile"));
        assert!(!filename_matches("Dockerfile.*", "DockerfileX"));
    }

    #[test]
    fn mcp_recent_config_round_trips() {
        let recent = BTreeMap::from([
            (
                "filesystem".to_string(),
                vec!["read_text_file".to_string(), "list_directory".to_string()],
            ),
            ("git".to_string(), vec!["git_status".to_string()]),
        ]);
        let text = render_mcp_recent(&recent);
        let file: McpRecentFile = toml::from_str(&text).expect("parse");
        assert_eq!(file.servers, recent);
    }

    #[test]
    fn mcp_trust_config_round_trips() {
        let trusted: HashSet<String> = ["hash-b".to_string(), "hash-a".to_string()].into();
        let text = render_mcp_trust(&trusted);
        let file: McpTrustFile = toml::from_str(&text).expect("parse");
        assert_eq!(file.trusted.into_iter().collect::<HashSet<_>>(), trusted);
    }

    #[test]
    fn managed_lsp_config_round_trips() {
        let mut config = LspConfig::default();
        config.servers.insert(
            "dockerfile".to_string(),
            LspServerConfig {
                command: "/tmp/docker-langserver".to_string(),
                args: vec!["--stdio".to_string()],
                extensions: vec!["dockerfile".to_string()],
                filenames: vec!["Dockerfile".to_string()],
            },
        );
        let text = toml::to_string(&config).expect("serialize");
        let restored: LspConfig = toml::from_str(&text).expect("parse");
        assert_eq!(restored, config);
    }

    #[test]
    fn unknown_section_is_reported() {
        let dir = config_dir("section");
        let path = dir.join("config.toml");
        std::fs::write(&path, "[bogus]\nvalue = 1\n\n[editor]\ntab_size = 2\n").expect("write");
        let (config, warnings) = load_from(&path);
        assert_eq!(warnings.len(), 1, "{warnings:?}");
        assert!(warnings[0].contains("bogus"), "{warnings:?}");
        assert_eq!(config.editor.tab_size, 2, "known sections still load");
        std::fs::remove_dir_all(&dir).ok();
    }
}

#[cfg(test)]
mod terminal_tests {
    use super::*;

    #[test]
    fn scrollback_is_clamped_and_shell_is_kept() {
        let dir = std::env::temp_dir().join(format!("i3-terminal-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("dir");
        let path = dir.join("config.toml");

        std::fs::write(&path, "[terminal]\nscrollback = 1\nshell = \"/bin/sh\"\n").expect("write");
        let (config, warnings) = load_from(&path);
        assert!(warnings.is_empty(), "{warnings:?}");
        assert_eq!(config.terminal.scrollback, 100);
        assert_eq!(config.terminal.shell.as_deref(), Some("/bin/sh"));

        std::fs::write(&path, "[terminal]\nscrollback = 9999999\n").expect("write");
        let (config, _) = load_from(&path);
        assert_eq!(config.terminal.scrollback, 100_000);
        assert_eq!(config.terminal.shell, None);
        std::fs::remove_dir_all(&dir).ok();
    }
}
