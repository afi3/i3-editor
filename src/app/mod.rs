//! Application coordinator: event loop, key routing, pane lifecycle.

pub mod ai_ops;
mod audit;
pub mod comment;
pub mod config_ops;
pub mod conflict_ops;
pub mod cursor;
pub mod dispatch;
pub mod editing;
pub mod file_ops;
pub mod folds;
pub mod git_ops;
pub mod help;
pub mod lines;
pub mod lsp_ops;
pub mod mcp_ops;
pub mod mouse;
pub mod outline;
pub mod plugin_ops;
pub mod project_search;
pub mod prompt;
pub mod recovery_ops;
pub mod search;
pub mod shell_ops;
pub mod watch_ops;
pub mod workspace_ops;

#[cfg(test)]
mod tests;

#[cfg(test)]
impl App {
    /// Build an app without touching the real terminal, for dispatch tests.
    pub(crate) fn headless(state: AppState) -> App {
        let backend = CrosstermBackend::new(std::io::stdout());
        let mut terminal = Terminal::with_options(
            backend,
            ratatui::TerminalOptions {
                viewport: ratatui::Viewport::Fixed(ratatui::layout::Rect::new(0, 0, 80, 24)),
            },
        )
        .expect("terminal");
        terminal.hide_cursor().ok();
        App {
            state,
            terminal,
            drag: None,
            scrollbar_drag: None,
            lsp_install: None,
            last_explorer_click: None,
            watchers: Vec::new(),
            external_watcher: None,
            external_watch_dirs: std::collections::HashSet::new(),
            quick_open_files: None,
            project_task: None,
            selecting: None,
            enhanced: false,
            recoveries: Vec::new(),
            swap_dir: None,
            last_swap: std::time::Instant::now(),
            lsp: None,
            mcp: None,
            ai_task: None,
            mcp_trusted: std::collections::HashSet::new(),
            mcp_recent: std::collections::BTreeMap::new(),
            persist_mcp_state: false,
            plugins: None,
            block_anchor: None,
            git_status_entries: Vec::new(),
            last_git: std::time::Instant::now(),
            search_history: Default::default(),
            history_path: None,
            session_path: None,
            last_session_save: std::time::Instant::now(),
            dirty: true,
            watch_signature: None,
        }
    }
}

use std::collections::HashMap;
use std::io::Stdout;
use std::path::{Path, PathBuf};
use std::time::Duration;

use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use ratatui::crossterm::event::{self, Event, KeyCode, KeyEvent, KeyModifiers};

use crate::command::{Command, Direction};
use crate::config::AppConfig;
use crate::editor::document::Document;
use crate::error::{Error, Result};
use crate::explorer::ExplorerState;
use crate::input::{KeyCombo, KeyMap};
use crate::state::{AppState, DocumentId, Focus, Ids, InputMode, Pane, PaneId};
use crate::terminal::Setup;
use crate::ui;
use crate::workspace::{Workspace, WorkspaceManager};

const SESSION_AUTOSAVE: Duration = Duration::from_secs(5);

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OpenTarget {
    pub path: PathBuf,
    pub line: Option<usize>,
}

pub struct App {
    pub state: AppState,
    pub terminal: Terminal<CrosstermBackend<Stdout>>,
    pub(crate) drag: Option<mouse::Drag>,
    /// Active vertical scrollbar drag, if any.
    pub(crate) scrollbar_drag: Option<mouse::ScrollbarDrag>,
    /// Running on-demand language-server installer.
    pub(crate) lsp_install: Option<crate::lsp_install::Task>,
    /// Last explorer row click, for double-click detection.
    pub(crate) last_explorer_click: Option<(usize, std::time::Instant)>,
    /// Recursive watcher per workspace root; empty when unavailable.
    pub(crate) watchers: Vec<(crate::state::WorkspaceId, crate::watch::Watcher)>,
    /// Watches parent directories of open documents outside every workspace
    /// root. One non-recursive watcher covers all of them.
    pub(crate) external_watcher: Option<crate::watch::Watcher>,
    pub(crate) external_watch_dirs: std::collections::HashSet<PathBuf>,
    /// Quick-open paths with the explorer revision they were scanned at.
    pub(crate) quick_open_files: Option<(u64, Vec<String>)>,
    /// Running Find in Files scan, if any.
    pub(crate) project_task: Option<crate::project_search::Task>,
    /// Pane whose text is being selected by a mouse drag.
    pub(crate) selecting: Option<crate::state::PaneId>,
    /// Probe result, reused when the config is reloaded.
    pub(crate) enhanced: bool,
    /// Documents restored from swap files, resolved one prompt at a time.
    pub(crate) recoveries: Vec<crate::state::DocumentId>,
    /// Swap directory; `None` when no config directory can be resolved.
    pub(crate) swap_dir: Option<PathBuf>,
    /// Time of the last swap pass, to bound writes.
    pub(crate) last_swap: std::time::Instant,
    /// Language servers for open documents; `None` in headless fixtures.
    pub(crate) lsp: Option<crate::lsp::LspManager>,
    /// MCP servers configured for this workspace.
    pub(crate) mcp: Option<crate::mcp::McpManager>,
    /// Running AI request, if any.
    pub(crate) ai_task: Option<crate::ai::Task>,
    /// Trusted MCP server config hashes for this session.
    pub(crate) mcp_trusted: std::collections::HashSet<String>,
    /// Recent MCP tool names per server.
    pub(crate) mcp_recent: std::collections::BTreeMap<String, Vec<String>>,
    /// Persist MCP trust/recent state; disabled in headless tests.
    pub(crate) persist_mcp_state: bool,
    /// Alt-drag block selection: pane, anchor line, anchor char column.
    pub(crate) block_anchor: Option<(crate::state::PaneId, usize, usize)>,
    /// Lua plugins; `None` when disabled or unavailable.
    pub(crate) plugins: Option<crate::plugins::Plugins>,
    /// Entries backing the Git status prompt.
    pub(crate) git_status_entries: Vec<crate::git::StatusEntry>,
    /// Time of the last git marker refresh.
    pub(crate) last_git: std::time::Instant,
    /// Persistent search queries and options.
    pub(crate) search_history: crate::search_history::SearchHistory,
    pub(crate) history_path: Option<PathBuf>,
    /// Session file used by periodic autosave and graceful exit.
    pub(crate) session_path: Option<PathBuf>,
    pub(crate) last_session_save: std::time::Instant,
    /// Set by input or background work; cleared after a successful draw.
    dirty: bool,
    /// Hash of roots, explorer revision, and open paths used to resync watchers.
    watch_signature: Option<u64>,
}

impl App {
    pub fn new(
        setup: Setup,
        config: AppConfig,
        warnings: Vec<String>,
        targets: Vec<OpenTarget>,
    ) -> Result<Self> {
        let enhanced = setup.keyboard_enhancement;
        let restore_session = targets.is_empty();
        let (root, files) = split_targets(targets)?;
        let mut explorer = ExplorerState::new(root.clone(), config.explorer.show_hidden)?;
        explorer.visible = config.explorer.visible;
        let mut ids = Ids::default();
        let pane_id = ids.pane();
        let workspace_id = ids.workspace();
        let name = root
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| "workspace".to_string());

        let mut panes = HashMap::new();
        panes.insert(pane_id, Pane::new(pane_id));
        let workspaces = WorkspaceManager {
            workspaces: vec![Workspace::new(workspace_id, name, pane_id, root.clone())],
            active: 0,
        };
        let config_path = crate::config::default_path();
        let history_path = crate::config::history_path();
        let search_history = crate::search_history::SearchHistory::load(history_path.as_deref());
        let (keymap, key_warnings) = KeyMap::from_config(&config.keys, enhanced);
        if std::env::var_os("I3_EDITOR_DEBUG").is_some() {
            log_keymap(config_path.as_deref(), &keymap, enhanced);
        }
        let themes_dir = crate::config::themes_dir().unwrap_or_default();
        let (theme, theme_warnings) =
            crate::theme::Theme::from_config(&config.appearance, &themes_dir);

        let mut state = AppState {
            should_quit: false,
            explorer,
            documents: HashMap::new(),
            panes,
            workspaces,
            focus: Focus::Pane(pane_id),
            mode: InputMode::Normal,
            keymap,
            ui: crate::ui::UiState::default(),
            config,
            theme,
            ids,
            warnings,
            clipboard: String::new(),
            search: None,
            search_options: search_history.options,
            project_search: None,
            lsp_diagnostics: std::collections::HashMap::new(),
            completions: None,
            hover: None,
            git_marks: std::collections::HashMap::new(),
            git_diff: None,
            help: None,
            mcp_view: None,
            ai: crate::state::AiPanel::default(),
            hunks: None,
            plugin_status: Vec::new(),
            fullscreen: None,
            moving: false,
            terminals: crate::shell::Terminals::default(),
            themes_dir,
            config_path: config_path.clone(),
        };
        state.warnings.extend(key_warnings);
        state.warnings.extend(theme_warnings);
        let (mcp_trusted, trust_warnings) = crate::config::load_mcp_trust();
        state.warnings.extend(trust_warnings);
        let (mcp_recent, recent_warnings) = crate::config::load_mcp_recent();
        state.warnings.extend(recent_warnings);
        if let Some(message) = capability_warning(enhanced) {
            state.warnings.insert(0, message);
        }
        if restore_session {
            let session_warnings = crate::session::restore(&mut state);
            state.warnings.extend(session_warnings);
        }
        let mut app = Self {
            state,
            terminal: setup.terminal,
            drag: None,
            scrollbar_drag: None,
            lsp_install: None,
            last_explorer_click: None,
            watchers: Vec::new(),
            external_watcher: None,
            external_watch_dirs: std::collections::HashSet::new(),
            quick_open_files: None,
            project_task: None,
            selecting: None,
            enhanced,
            recoveries: Vec::new(),
            swap_dir: None,
            last_swap: std::time::Instant::now(),
            lsp: None,
            mcp: None,
            ai_task: None,
            mcp_trusted,
            mcp_recent,
            persist_mcp_state: true,
            plugins: None,
            block_anchor: None,
            git_status_entries: Vec::new(),
            last_git: std::time::Instant::now(),
            search_history,
            history_path,
            session_path: crate::session::default_path(),
            last_session_save: std::time::Instant::now(),
            dirty: true,
            watch_signature: None,
        };
        if let Err(error) = app.apply_active_root() {
            app.state
                .warnings
                .push(format!("workspace root unavailable: {error}"));
        }
        app.lsp = Some(crate::lsp::LspManager::new(
            &app.state.config.lsp,
            app.state.explorer.root.clone(),
        ));
        app.mcp = Some(crate::mcp::McpManager::new(
            app.state.config.mcp.servers.clone(),
            app.state.explorer.root.clone(),
        ));
        if app.state.config.lua.enabled
            && let Some(dir) = crate::plugins::Plugins::default_dir()
        {
            match crate::plugins::Plugins::new(dir, &app.state.config.lua) {
                Ok(mut plugins) => {
                    let warnings = plugins.take_warnings();
                    app.state.warnings.extend(warnings);
                    app.plugins = Some(plugins);
                }
                Err(error) => app
                    .state
                    .warnings
                    .push(format!("Lua plugins disabled: {error}")),
            }
        }
        app.init_watchers(config_path.as_deref());
        app.open_targets(files)?;
        app.load_recoveries();
        app.apply_document_indents();
        app.refresh_plugin_status();
        app.state.ui.message = startup_message(&app.state.warnings);
        Ok(app)
    }

    /// Make the explorer match the active workspace. Called after a session
    /// restore, a workspace switch, or a root change.
    pub(crate) fn apply_active_root(&mut self) -> Result<()> {
        let root = self.state.workspaces.active().root.clone();
        if self.state.explorer.root == root {
            return Ok(());
        }
        let visible = self.state.explorer.visible;
        let show_hidden = self.state.explorer.show_hidden;
        let mut explorer = ExplorerState::new(root, show_hidden)?;
        explorer.visible = visible;
        self.state.explorer = explorer;
        self.quick_open_files = None;
        self.state.project_search = None;
        self.restart_lsp();
        if let Some(mcp) = self.mcp.as_mut() {
            mcp.restart(
                self.state.config.mcp.servers.clone(),
                self.state.explorer.root.clone(),
            );
        }
        self.apply_document_indents();
        Ok(())
    }

    fn init_watchers(&mut self, config: Option<&Path>) {
        let roots: Vec<(crate::state::WorkspaceId, PathBuf)> = self
            .state
            .workspaces
            .workspaces
            .iter()
            .map(|workspace| (workspace.id, workspace.root.clone()))
            .collect();
        for (index, (id, root)) in roots.into_iter().enumerate() {
            let watched_config = if index == 0 { config } else { None };
            match crate::watch::Watcher::new(&root, watched_config) {
                Ok(watcher) => self.watchers.push((id, watcher)),
                Err(error) => self.state.warnings.push(format!(
                    "file watching disabled for {}: {error}",
                    root.display()
                )),
            }
        }
    }

    pub(crate) fn replace_watcher(&mut self, id: crate::state::WorkspaceId, root: &Path) {
        self.watchers.retain(|(watched, _)| *watched != id);
        match crate::watch::Watcher::new(root, None) {
            Ok(watcher) => self.watchers.push((id, watcher)),
            Err(error) => self.state.warnings.push(format!(
                "file watching disabled for {}: {error}",
                root.display()
            )),
        }
    }

    pub fn run(mut self) -> Result<()> {
        while !self.state.should_quit {
            self.pump_shell();
            self.pump_watcher();
            self.pump_project_search();
            self.pump_lsp();
            self.pump_mcp();
            self.pump_ai();
            self.pump_lsp_install();
            self.refresh_git_if_due();
            self.autosave_swaps();
            if self.dirty {
                self.draw()?;
                self.dirty = false;
            }
            self.autosave_session();
            let timeout = self.frame_timeout();
            if !event::poll(timeout).map_err(Error::from)? {
                continue;
            }
            self.dirty = true;
            match event::read().map_err(Error::from)? {
                Event::Key(key) => self.handle_key(key),
                Event::Mouse(mouse) => self.handle_mouse(mouse),
                Event::Paste(text) => {
                    let result = self.handle_paste(text);
                    self.finish(result);
                }
                _ => {}
            }
            // Never run Lua from `draw`: once per input event keeps the
            // statusline fresh without a per-frame callback cost.
            self.refresh_plugin_status();
        }
        if let Some(dir) = &self.swap_dir {
            crate::recovery::clear_all(dir);
        }
        self.save_session()?;
        Ok(())
    }

    /// A paste belongs to whichever surface owns input: prompt field, shell
    /// PTY, or the active document. The explorer has no paste target.
    fn handle_paste(&mut self, text: String) -> Result<()> {
        if text.is_empty() {
            return Ok(());
        }
        if self.state.mode.is_prompt() {
            if let InputMode::Prompt(prompt) = &mut self.state.mode {
                prompt.insert_text(&text);
            }
            self.refresh_prompt_preview();
            return Ok(());
        }
        match self.state.focus {
            Focus::Shell => {
                self.paste_to_shell(&text);
                Ok(())
            }
            Focus::Explorer => Ok(()),
            Focus::Pane(_) => self.dispatch(Command::InsertText(text)),
        }
    }

    fn reveal_active_tabs(&mut self) {
        let targets: Vec<(PaneId, DocumentId)> = self
            .state
            .panes
            .iter()
            .filter(|(_, pane)| pane.tab_follow)
            .filter_map(|(pane_id, pane)| pane.active.map(|id| (*pane_id, id)))
            .collect();
        for (pane_id, id) in targets {
            self.reveal_document_tab(pane_id, id);
        }
    }

    fn draw(&mut self) -> Result<()> {
        let size = self.terminal.size()?;
        let area = ratatui::layout::Rect::new(0, 0, size.width, size.height);
        ui::compute_layout(&mut self.state, area);
        self.reveal_active_tabs();
        let App {
            terminal, state, ..
        } = self;
        terminal.draw(|frame| ui::render(frame, state))?;
        self.resize_shell();
        Ok(())
    }

    fn frame_timeout(&self) -> Duration {
        if self.lsp_install.is_some() || self.ai_task.is_some() {
            return Duration::from_millis(100);
        }
        if !self.state.terminals.is_empty() {
            if self.state.focus == Focus::Shell {
                Duration::from_millis(30)
            } else {
                Duration::from_millis(60)
            }
        } else if self.project_task.is_some() {
            Duration::from_millis(50)
        } else if self
            .lsp
            .as_ref()
            .is_some_and(crate::lsp::LspManager::is_configured)
        {
            Duration::from_millis(250)
        } else {
            Duration::from_secs(1)
        }
    }

    fn handle_key(&mut self, key: KeyEvent) {
        let Some(combo) = KeyCombo::from_event(key) else {
            return;
        };
        self.handle_key_combo(key, combo);
    }

    fn handle_key_combo(&mut self, key: KeyEvent, combo: KeyCombo) {
        // Quit is the one binding that always wins, but modified documents still
        // have to be saved or discarded explicitly.
        if matches!(self.state.keymap.global.get(&combo), Some(Command::Quit)) {
            let result = self.request_quit();
            self.finish(result);
            return;
        }
        if self.state.mode.is_prompt() {
            self.handle_prompt_key(combo);
            return;
        }
        if self.handle_read_only_overlay_key(combo) || self.handle_popup_key(combo) {
            return;
        }
        if matches!(self.state.focus, Focus::Shell) {
            if let Some(command) = self.state.keymap.global.get(&combo).cloned()
                && matches!(
                    command,
                    Command::Quit
                        | Command::Palette
                        | Command::ToggleTerminal
                        | Command::SelectTheme
                        | Command::TerminalNew
                        | Command::TerminalClose
                        | Command::NextTerminal
                        | Command::PrevTerminal
                        | Command::NextPaneTab
                        | Command::PrevPaneTab
                )
            {
                let result = self.dispatch(command);
                self.finish(result);
                return;
            }
            self.forward_to_shell(key);
            return;
        }
        if self.state.moving {
            self.handle_move_key(combo);
            return;
        }
        if let Some(command) = self.state.keymap.resolve(self.state.focus, combo) {
            let result = self.dispatch(command);
            self.finish(result);
            return;
        }
        if let Some(command) = self.plugin_binding(combo) {
            self.run_plugin_command(&command);
            return;
        }
        if matches!(self.state.focus, Focus::Pane(_))
            && !key
                .modifiers
                .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT)
            && let KeyCode::Char(ch) = key.code
        {
            let result = self.insert_typed_char(ch);
            self.finish(result);
        }
    }

    fn handle_read_only_overlay_key(&mut self, combo: KeyCombo) -> bool {
        self.handle_mcp_key(combo)
            || self.handle_ai_key(combo)
            || self.handle_help_key(combo)
            || self.handle_hunk_key(combo)
            || self.handle_diff_key(combo)
    }

    fn handle_popup_key(&mut self, combo: KeyCombo) -> bool {
        self.handle_completion_key(combo) || self.handle_hover_key(combo)
    }

    pub(crate) fn finish(&mut self, result: Result<()>) {
        if let Err(error) = result {
            self.state.ui.message = error.to_string();
        }
    }

    /// In move mode the arrow keys move the pane instead of the cursor.
    fn handle_move_key(&mut self, combo: KeyCombo) {
        let direction = match combo.code {
            KeyCode::Left => Some(Direction::Left),
            KeyCode::Right => Some(Direction::Right),
            KeyCode::Up => Some(Direction::Up),
            KeyCode::Down => Some(Direction::Down),
            _ => None,
        };
        if let Some(direction) = direction {
            let result = self.dispatch(Command::MovePane(direction));
            self.finish(result);
            return;
        }
        if matches!(combo.code, KeyCode::Esc | KeyCode::F(10)) {
            self.state.moving = false;
            self.notify("move mode off");
        }
    }

    pub(crate) fn notify(&mut self, message: impl Into<String>) {
        self.state.ui.message = message.into();
    }

    fn save_session(&self) -> Result<()> {
        let Some(path) = &self.session_path else {
            return Ok(());
        };
        crate::session::save_to(path, &self.state)
            .map_err(|error| Error::io("write session", path, error))
    }

    fn autosave_session(&mut self) {
        if self.last_session_save.elapsed() < SESSION_AUTOSAVE {
            return;
        }
        self.last_session_save = std::time::Instant::now();
        if let Err(error) = self.save_session() {
            self.notify(error.to_string());
        }
    }

    fn base_indent(&self) -> crate::editorconfig::IndentSettings {
        crate::editorconfig::IndentSettings {
            tab_width: self.state.config.editor.tab_size.max(1),
            insert_spaces: self.state.config.editor.insert_spaces,
        }
    }

    fn document_indent(&self, path: Option<&Path>) -> crate::editorconfig::IndentSettings {
        let base = self.base_indent();
        path.map_or(base, |path| {
            crate::editorconfig::resolve(&self.state.explorer.root, path, base)
        })
    }

    pub(crate) fn apply_document_indents(&mut self) {
        let root = self.state.explorer.root.clone();
        let base = self.base_indent();
        for document in self.state.documents.values_mut() {
            document.indent = document
                .path
                .as_deref()
                .map_or(base, |path| crate::editorconfig::resolve(&root, path, base));
        }
    }

    pub(crate) fn open_targets(&mut self, targets: Vec<OpenTarget>) -> Result<()> {
        for target in targets {
            self.open_document(target.path)?;
            if let Some(line) = target.line {
                self.goto_line(line.saturating_sub(1))?;
            }
        }
        Ok(())
    }

    pub(crate) fn open_document(&mut self, path: PathBuf) -> Result<()> {
        let path = absolutize(&path)?;
        if let Some(existing) = self.find_document_by_path(&path) {
            self.activate_document(existing)?;
            return Ok(());
        }
        let id = self.state.ids.document();
        let indent = self.document_indent(Some(&path));
        let mut document = Document::open(id, path.clone())?;
        document.indent = indent;
        self.state.documents.insert(id, document);
        self.activate_document(id)?;
        self.notify(format!("opened {}", path.display()));
        self.refresh_git_marks();
        self.plugin_event("open");
        Ok(())
    }

    fn find_document_by_path(&self, path: &Path) -> Option<DocumentId> {
        self.state
            .documents
            .values()
            .find(|document| document.path.as_deref() == Some(path))
            .map(|document| document.id)
    }

    fn activate_document(&mut self, id: DocumentId) -> Result<()> {
        let pane = self.focused_or_first_pane()?;
        let pane = self
            .state
            .panes
            .get_mut(&pane)
            .ok_or(Error::NoFocusedPane)?;
        if !pane.tabs.contains(&id) {
            pane.tabs.push(id);
        }
        pane.active = Some(id);
        pane.anchor = None;
        pane.cursor = pane.cursor.min(self.state.documents[&id].len_chars());
        let pane_id = pane.id;
        self.state.focus = Focus::Pane(pane_id);
        self.reveal_document_tab(pane_id, id);
        self.plugin_event("focus");
        Ok(())
    }

    pub(crate) fn focused_or_first_pane(&mut self) -> Result<PaneId> {
        if let Some(id) = self.state.focused_pane_id() {
            return Ok(id);
        }
        let workspace = self.state.workspaces.active();
        let remembered = workspace.focus;
        let target = if workspace.layout.contains(remembered) {
            remembered
        } else {
            workspace
                .layout
                .leaves()
                .first()
                .copied()
                .ok_or(Error::NoFocusedPane)?
        };
        self.state.workspaces.active_mut().focus = target;
        self.state.focus = Focus::Pane(target);
        Ok(target)
    }
}

fn capability_warning(enhanced: bool) -> Option<String> {
    if enhanced {
        return None;
    }
    let unavailable = KeyMap::unavailable_without_enhancement();
    let names = if unavailable.is_empty() {
        "none".to_string()
    } else {
        unavailable.join(", ")
    };
    Some(format!(
        "keyboard enhancement unavailable; safe bindings active; unavailable: {names}; palette: next_workspace, prev_workspace"
    ))
}

/// First warning plus a pointer to the rest; the `messages` palette command
/// lists them all.
fn startup_message(warnings: &[String]) -> String {
    match warnings.split_first() {
        Some((first, [])) => first.clone(),
        Some((first, rest)) => format!("{first} (+{} more; palette: messages)", rest.len()),
        None => String::new(),
    }
}

/// Write the resolved binding table next to the config when `I3_EDITOR_DEBUG`
/// is set. A file is used because stderr would corrupt the terminal UI.
fn log_keymap(path: Option<&Path>, keymap: &KeyMap, enhanced: bool) {
    let Some(path) = path.map(|path| path.with_file_name("keymap.log")) else {
        return;
    };
    let mut text = format!("keyboard_enhancement: {enhanced}\n");
    for label in keymap.labels() {
        text.push_str(&label);
        text.push('\n');
    }
    let _ = std::fs::write(path, text);
}

fn split_targets(targets: Vec<OpenTarget>) -> Result<(PathBuf, Vec<OpenTarget>)> {
    let current = std::env::current_dir().map_err(|error| Error::io("cwd", ".", error))?;
    let Some(first) = targets.first() else {
        return Ok((current, Vec::new()));
    };
    let absolute = absolutize(&first.path)?;
    if absolute.is_dir() {
        if targets.len() > 1 {
            return Err(Error::Cli(
                "only the first target may be a directory".to_string(),
            ));
        }
        if first.line.is_some() {
            return Err(Error::Cli("line numbers require a file".to_string()));
        }
        return Ok((absolute, Vec::new()));
    }
    let root = absolute.parent().map(Path::to_path_buf).unwrap_or(current);
    let mut files = Vec::with_capacity(targets.len());
    for target in targets {
        let path = absolutize(&target.path)?;
        if path.is_dir() {
            return Err(Error::Cli(
                "only the first target may be a directory".to_string(),
            ));
        }
        files.push(OpenTarget {
            path,
            line: target.line,
        });
    }
    Ok((root, files))
}

pub(crate) fn absolutize(path: &Path) -> Result<PathBuf> {
    if path.is_absolute() {
        return Ok(path.to_path_buf());
    }
    let current = std::env::current_dir().map_err(|error| Error::io("cwd", ".", error))?;
    Ok(current.join(path))
}

/// Compare paths after resolving symlinks, so `/var/...` and `/private/var/...`
/// refer to the same file on macOS.
pub(crate) fn same_path(open: &Path, target: &Path) -> bool {
    if open == target {
        return true;
    }
    matches!(
        (std::fs::canonicalize(open), std::fs::canonicalize(target)),
        (Ok(open), Ok(target)) if open == target
    )
}
