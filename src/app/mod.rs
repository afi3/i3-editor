//! Application coordinator: event loop, key routing, pane lifecycle.

pub mod cursor;
pub mod dispatch;
pub mod editing;
pub mod mouse;
pub mod prompt;
pub mod search;
pub mod shell_ops;
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
            last_explorer_click: None,
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

pub struct App {
    pub state: AppState,
    pub terminal: Terminal<CrosstermBackend<Stdout>>,
    pub(crate) drag: Option<mouse::Drag>,
    /// Last explorer row click, for double-click detection.
    pub(crate) last_explorer_click: Option<(usize, std::time::Instant)>,
}

impl App {
    pub fn new(
        setup: Setup,
        config: AppConfig,
        warnings: Vec<String>,
        target: Option<PathBuf>,
    ) -> Result<Self> {
        let (root, file) = split_target(target)?;
        let explorer = ExplorerState::new(root.clone(), config.explorer.show_hidden)?;
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
            workspaces: vec![Workspace::new(workspace_id, name, pane_id)],
            active: 0,
        };
        let config_path = crate::config::default_path();
        let (keymap, key_warnings) = KeyMap::from_config(&config.keys);
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
            fullscreen: None,
            moving: false,
            shell: None,
            themes_dir,
            config_path,
        };
        state.warnings.extend(key_warnings);
        state.warnings.extend(theme_warnings);
        state.ui.message = state.warnings.first().cloned().unwrap_or_default();

        let mut app = Self {
            state,
            terminal: setup.terminal,
            drag: None,
            last_explorer_click: None,
        };
        if let Some(path) = file {
            app.open_document(path)?;
        }
        Ok(app)
    }

    pub fn run(mut self) -> Result<()> {
        while !self.state.should_quit {
            self.pump_shell();
            self.draw()?;
            let timeout = if self.state.shell.is_some() && self.state.focus == Focus::Shell {
                Duration::from_millis(30)
            } else if self.state.shell.is_some() {
                Duration::from_millis(60)
            } else {
                Duration::from_millis(250)
            };
            if !event::poll(timeout).map_err(Error::from)? {
                continue;
            }
            match event::read().map_err(Error::from)? {
                Event::Key(key) => self.handle_key(key),
                Event::Mouse(mouse) => self.handle_mouse(mouse),
                Event::Paste(text) => {
                    let result = self.dispatch(Command::InsertText(text));
                    self.finish(result);
                }
                _ => {}
            }
        }
        Ok(())
    }

    fn draw(&mut self) -> Result<()> {
        let App {
            terminal, state, ..
        } = self;
        let size = terminal.size()?;
        let area = ratatui::layout::Rect::new(0, 0, size.width, size.height);
        ui::compute_layout(state, area);
        terminal.draw(|frame| ui::render(frame, state))?;
        self.resize_shell();
        Ok(())
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
            self.handle_prompt_key(key, combo);
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
        if matches!(self.state.focus, Focus::Pane(_))
            && !key
                .modifiers
                .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT)
            && let KeyCode::Char(ch) = key.code
        {
            let result = self.dispatch(Command::InsertText(ch.to_string()));
            self.finish(result);
        }
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

    pub(crate) fn open_document(&mut self, path: PathBuf) -> Result<()> {
        let path = absolutize(&path)?;
        if let Some(existing) = self.find_document_by_path(&path) {
            self.activate_document(existing)?;
            return Ok(());
        }
        let id = self.state.ids.document();
        let document = Document::open(id, path.clone())?;
        self.state.documents.insert(id, document);
        self.activate_document(id)?;
        self.notify(format!("opened {}", path.display()));
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
        self.state.focus = Focus::Pane(pane.id);
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

fn split_target(target: Option<PathBuf>) -> Result<(PathBuf, Option<PathBuf>)> {
    let current = std::env::current_dir().map_err(|error| Error::io("cwd", ".", error))?;
    match target {
        None => Ok((current, None)),
        Some(path) => {
            let absolute = if path.is_absolute() {
                path
            } else {
                current.join(path)
            };
            if absolute.is_dir() {
                Ok((absolute, None))
            } else {
                let parent = absolute.parent().map(Path::to_path_buf).unwrap_or(current);
                Ok((parent, Some(absolute)))
            }
        }
    }
}

pub(crate) fn absolutize(path: &Path) -> Result<PathBuf> {
    if path.is_absolute() {
        return Ok(path.to_path_buf());
    }
    let current = std::env::current_dir().map_err(|error| Error::io("cwd", ".", error))?;
    Ok(current.join(path))
}
