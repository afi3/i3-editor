//! Embedded terminal: a real shell in a PTY, shown in a bottom panel.

use std::io::{Read, Write};
use std::num::NonZeroU16;
use std::path::Path;
use std::sync::mpsc::{self, Receiver};
use std::thread;

use portable_pty::{Child, CommandBuilder, MasterPty, PtySize, native_pty_system};
use ratatui::crossterm::event::{
    KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
};
use vt100::{MouseProtocolEncoding, MouseProtocolMode, Parser};

use crate::error::{Error, Result};

/// Bytes of PTY output parsed per frame; keeps a flooding program responsive.
const PUMP_BUDGET: usize = 256 * 1024;

/// Bounded queue between the PTY reader and the parser. Without the bound a
/// flooding child (`yes`, a noisy build) grows the queue faster than a frame
/// drains it and the editor runs out of memory; with it the PTY blocks, which
/// is the backpressure a terminal is supposed to apply.
const OUTPUT_QUEUE: usize = 128;

fn drain(output: &Receiver<Vec<u8>>, parser: &mut Parser, mut budget: usize) {
    while budget > 0 {
        let Ok(chunk) = output.try_recv() else { break };
        budget = budget.saturating_sub(chunk.len());
        parser.process(&chunk);
    }
}

fn wrap_paste(text: &str) -> Vec<u8> {
    let text = text.replace("\u{1b}[201~", "");
    let mut bytes = Vec::with_capacity(text.len() + 12);
    bytes.extend_from_slice(b"\x1b[200~");
    bytes.extend_from_slice(text.as_bytes());
    bytes.extend_from_slice(b"\x1b[201~");
    bytes
}

pub struct ShellPanel {
    master: Box<dyn MasterPty + Send>,
    writer: Box<dyn Write + Send>,
    child: Box<dyn Child + Send + Sync>,
    parser: Parser,
    output: Receiver<Vec<u8>>,
    rows: u16,
    cols: u16,
    scrollback: usize,
    max_scrollback: usize,
    title: String,
    cwd: std::path::PathBuf,
    exit_reported: bool,
}

impl ShellPanel {
    /// Spawn the configured shell (fallback `$SHELL`) in `cwd`.
    pub fn spawn(
        cwd: &Path,
        rows: u16,
        cols: u16,
        shell: Option<&str>,
        scrollback: usize,
    ) -> Result<Self> {
        let shell = resolve_shell(shell);
        Self::spawn_shell(cwd, rows, cols, &shell, scrollback)
    }

    pub fn spawn_shell(
        cwd: &Path,
        rows: u16,
        cols: u16,
        shell: &str,
        scrollback: usize,
    ) -> Result<Self> {
        let rows = rows.max(1);
        let cols = cols.max(2);
        let pair = native_pty_system()
            .openpty(PtySize {
                rows,
                cols,
                pixel_width: 0,
                pixel_height: 0,
            })
            .map_err(|error| Error::Shell(format!("cannot open pty: {error}")))?;

        let mut command = CommandBuilder::new(shell);
        command.cwd(cwd);
        command.env("TERM", "xterm-256color");
        let child = pair
            .slave
            .spawn_command(command)
            .map_err(|error| Error::Shell(format!("cannot start {shell}: {error}")))?;
        drop(pair.slave);

        let mut reader = pair
            .master
            .try_clone_reader()
            .map_err(|error| Error::Shell(format!("cannot read pty: {error}")))?;
        let writer = pair
            .master
            .take_writer()
            .map_err(|error| Error::Shell(format!("cannot write pty: {error}")))?;

        let (sender, output) = mpsc::sync_channel(OUTPUT_QUEUE);
        thread::spawn(move || {
            let mut buffer = [0u8; 8192];
            loop {
                match reader.read(&mut buffer) {
                    Ok(0) | Err(_) => break,
                    Ok(read) => {
                        if sender.send(buffer[..read].to_vec()).is_err() {
                            break;
                        }
                    }
                }
            }
        });

        Ok(Self {
            master: pair.master,
            writer,
            child,
            parser: Parser::new(nonzero(rows), nonzero(cols), scrollback),
            output,
            rows,
            cols,
            scrollback: 0,
            max_scrollback: scrollback,
            title: shell.to_string(),
            cwd: cwd.to_path_buf(),
            exit_reported: false,
        })
    }

    /// Drain pending PTY output into the screen model. A flooding program such
    /// as `yes` must not starve input: each frame parses at most `PUMP_BUDGET`
    /// bytes and the rest waits for the next one.
    pub fn pump(&mut self) {
        drain(&self.output, &mut self.parser, PUMP_BUDGET);
    }

    /// Write pasted text. When the program enabled bracketed paste, wrap the
    /// payload so a multi-line paste arrives as one paste instead of a run of
    /// executed lines.
    pub fn paste(&mut self, text: &str) {
        if self.parser.screen().bracketed_paste() {
            self.write(&wrap_paste(text));
        } else {
            self.write(text.as_bytes());
        }
    }

    pub fn resize(&mut self, rows: u16, cols: u16) {
        let rows = rows.max(1);
        let cols = cols.max(2);
        if rows == self.rows && cols == self.cols {
            return;
        }
        self.rows = rows;
        self.cols = cols;
        let _ = self.master.resize(PtySize {
            rows,
            cols,
            pixel_width: 0,
            pixel_height: 0,
        });
        self.parser.set_size(nonzero(rows), nonzero(cols));
    }

    pub fn write(&mut self, bytes: &[u8]) {
        let _ = self.writer.write_all(bytes);
        let _ = self.writer.flush();
    }

    pub fn send_key(&mut self, key: KeyEvent) {
        self.write(&encode_key(key));
    }

    /// True when the program in the PTY enabled mouse reporting
    /// (`DECSET 1000`/`1002`/`1003`), as tracked by the vt100 screen.
    pub fn reports_mouse(&self) -> bool {
        self.parser.screen().mouse_protocol_mode() != MouseProtocolMode::None
    }

    /// Forward a mouse event to the program behind the PTY. `column` and `row`
    /// are zero-based inside the panel body. Returns false when the program did
    /// not ask for this event, in which case the caller keeps its own handling.
    pub fn send_mouse(&mut self, mouse: MouseEvent, column: u16, row: u16) -> bool {
        let (mode, encoding) = {
            let screen = self.parser.screen();
            (
                screen.mouse_protocol_mode(),
                screen.mouse_protocol_encoding(),
            )
        };
        let Some(bytes) = encode_mouse(mouse, mode, encoding, column, row) else {
            return false;
        };
        self.write(&bytes);
        true
    }

    /// Scroll the visible window into scrollback. Returns true on change.
    pub fn scroll(&mut self, delta: isize) -> bool {
        let next =
            (self.scrollback as isize + delta).clamp(0, self.max_scrollback as isize) as usize;
        if next == self.scrollback {
            return false;
        }
        self.scrollback = next;
        self.parser.screen_mut().set_scrollback(next);
        true
    }

    pub fn screen(&self) -> &vt100::Screen {
        self.parser.screen()
    }

    pub fn cursor(&self) -> (u16, u16) {
        self.parser.screen().cursor_position()
    }

    pub fn title(&self) -> &str {
        &self.title
    }

    pub fn set_title(&mut self, title: impl Into<String>) {
        self.title = title.into();
    }

    pub fn cwd(&self) -> &std::path::Path {
        &self.cwd
    }

    /// True once, if the child exited since the last call.
    pub fn poll_exit(&mut self) -> Option<String> {
        if self.exit_reported {
            return None;
        }
        match self.child.try_wait() {
            Ok(Some(status)) => {
                self.exit_reported = true;
                Some(format!("terminal exited ({status:?})"))
            }
            _ => None,
        }
    }
}

impl Drop for ShellPanel {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// Shells shown in the bottom panel, with one active tab. Dropping the
/// collection kills every shell it owns.
#[derive(Default)]
pub struct Terminals {
    shells: Vec<ShellPanel>,
    active: usize,
}

impl Terminals {
    pub fn is_empty(&self) -> bool {
        self.shells.is_empty()
    }

    pub fn len(&self) -> usize {
        self.shells.len()
    }

    pub fn active(&self) -> Option<&ShellPanel> {
        self.shells.get(self.active)
    }

    pub fn active_mut(&mut self) -> Option<&mut ShellPanel> {
        self.shells.get_mut(self.active)
    }

    pub fn active_index(&self) -> usize {
        self.active
    }

    pub fn shells(&self) -> &[ShellPanel] {
        &self.shells
    }

    pub fn iter_mut(&mut self) -> std::slice::IterMut<'_, ShellPanel> {
        self.shells.iter_mut()
    }

    pub fn push(&mut self, shell: ShellPanel) {
        self.shells.push(shell);
        self.active = self.shells.len() - 1;
    }

    pub fn cycle(&mut self, delta: isize) {
        let len = self.shells.len();
        if len < 2 {
            return;
        }
        self.active = (self.active as isize + delta).rem_euclid(len as isize) as usize;
    }

    pub fn close_active(&mut self) {
        if self.active < self.shells.len() {
            self.shells.remove(self.active);
            self.active = self.active.min(self.shells.len().saturating_sub(1));
        }
    }

    pub fn rename_active(&mut self, title: &str) -> bool {
        let Some(shell) = self.shells.get_mut(self.active) else {
            return false;
        };
        shell.set_title(title);
        true
    }
}

/// Encode a key event as the byte sequence a terminal would send.
pub fn encode_key(key: KeyEvent) -> Vec<u8> {
    let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
    let alt = key.modifiers.contains(KeyModifiers::ALT);
    let shift = key.modifiers.contains(KeyModifiers::SHIFT);
    let modifier = 1 + u8::from(shift) + if alt { 2 } else { 0 } + if ctrl { 4 } else { 0 };
    if let KeyCode::Char(ch) = key.code {
        return encode_char(ch, ctrl, alt);
    }
    encode_special(key.code, modifier, modifier > 1)
}

fn encode_char(ch: char, ctrl: bool, alt: bool) -> Vec<u8> {
    let mut out = Vec::new();
    if ctrl {
        let lower = ch.to_ascii_lowercase();
        let byte = match lower {
            'a'..='z' => Some((lower as u8) & 0x1f),
            '[' => Some(0x1b),
            '\\' => Some(0x1c),
            ']' => Some(0x1d),
            '^' => Some(0x1e),
            '_' => Some(0x1f),
            ' ' => Some(0),
            _ => None,
        };
        if let Some(byte) = byte {
            if alt {
                out.push(0x1b);
            }
            out.push(byte);
        }
    } else {
        if alt {
            out.push(0x1b);
        }
        out.extend(ch.to_string().bytes());
    }
    out
}

fn encode_special(code: KeyCode, modifier: u8, modified: bool) -> Vec<u8> {
    match code {
        KeyCode::Enter => vec![b'\r'],
        KeyCode::Tab => vec![b'\t'],
        KeyCode::Backspace => vec![0x7f],
        KeyCode::Esc => vec![0x1b],
        KeyCode::Up
        | KeyCode::Down
        | KeyCode::Right
        | KeyCode::Left
        | KeyCode::Home
        | KeyCode::End => encode_navigation(code, modifier, modified),
        KeyCode::Delete | KeyCode::Insert | KeyCode::PageUp | KeyCode::PageDown => {
            encode_tilde_key(code, modifier, modified)
        }
        KeyCode::BackTab => b"\x1b[Z".to_vec(),
        KeyCode::F(number) => {
            let mut out = Vec::new();
            push_function(&mut out, number, modifier, modified);
            out
        }
        _ => Vec::new(),
    }
}

fn encode_navigation(code: KeyCode, modifier: u8, modified: bool) -> Vec<u8> {
    let final_byte = match code {
        KeyCode::Up => b'A',
        KeyCode::Down => b'B',
        KeyCode::Right => b'C',
        KeyCode::Left => b'D',
        KeyCode::Home => b'H',
        KeyCode::End => b'F',
        _ => return Vec::new(),
    };
    let mut out = Vec::new();
    push_csi(&mut out, final_byte, modifier, modified);
    out
}

fn encode_tilde_key(code: KeyCode, modifier: u8, modified: bool) -> Vec<u8> {
    let number = match code {
        KeyCode::Delete => 3,
        KeyCode::Insert => 2,
        KeyCode::PageUp => 5,
        KeyCode::PageDown => 6,
        _ => return Vec::new(),
    };
    let mut out = Vec::new();
    push_tilde(&mut out, number, modifier, modified);
    out
}

fn push_csi(out: &mut Vec<u8>, final_byte: u8, modifier: u8, modified: bool) {
    if modified {
        out.extend(format!("\x1b[1;{modifier}{}", final_byte as char).bytes());
    } else {
        out.extend([0x1b, b'[', final_byte]);
    }
}

fn push_tilde(out: &mut Vec<u8>, code: u8, modifier: u8, modified: bool) {
    if modified {
        out.extend(format!("\x1b[{code};{modifier}~").bytes());
    } else {
        out.extend(format!("\x1b[{code}~").bytes());
    }
}

fn push_function(out: &mut Vec<u8>, number: u8, modifier: u8, modified: bool) {
    if !modified {
        out.extend(function_key(number));
        return;
    }
    match number {
        1..=4 => {
            let final_byte = b'P' + number - 1;
            out.extend(format!("\x1b[1;{modifier}{}", final_byte as char).bytes());
        }
        5..=12 => {
            let code = match number {
                5 => 15,
                6 => 17,
                7 => 18,
                8 => 19,
                9 => 20,
                10 => 21,
                11 => 23,
                _ => 24,
            };
            out.extend(format!("\x1b[{code};{modifier}~").bytes());
        }
        _ => {}
    }
}

fn button_number(button: MouseButton) -> u8 {
    match button {
        MouseButton::Left => 0,
        MouseButton::Middle => 1,
        MouseButton::Right => 2,
    }
}

/// Encode a mouse event the way a terminal reports it to a program. `column`
/// and `row` are zero-based inside the panel body. Returns `None` when the
/// program's protocol mode does not ask for this event.
pub fn encode_mouse(
    mouse: MouseEvent,
    mode: MouseProtocolMode,
    encoding: MouseProtocolEncoding,
    column: u16,
    row: u16,
) -> Option<Vec<u8>> {
    let (code, release) = mouse_report(mouse, mode)?;
    Some(encode_report(code, release, encoding, column, row))
}

/// Button number plus modifier and motion bits for an event the protocol mode
/// reports, along with whether it is a release.
fn mouse_report(mouse: MouseEvent, mode: MouseProtocolMode) -> Option<(u8, bool)> {
    let release = matches!(mouse.kind, MouseEventKind::Up(_));
    let motion = matches!(mouse.kind, MouseEventKind::Drag(_) | MouseEventKind::Moved);
    let wanted = match mode {
        MouseProtocolMode::None => false,
        // X10 reports presses only; the wheel counts as a press.
        MouseProtocolMode::Press => !release && !motion,
        MouseProtocolMode::PressRelease => !motion,
        MouseProtocolMode::ButtonMotion => !matches!(mouse.kind, MouseEventKind::Moved),
        MouseProtocolMode::AnyMotion => true,
    };
    if !wanted {
        return None;
    }

    let button = match mouse.kind {
        MouseEventKind::Down(button)
        | MouseEventKind::Up(button)
        | MouseEventKind::Drag(button) => button_number(button),
        MouseEventKind::Moved => 3,
        MouseEventKind::ScrollUp => 64,
        MouseEventKind::ScrollDown => 65,
        MouseEventKind::ScrollLeft => 66,
        MouseEventKind::ScrollRight => 67,
    };
    let mut code = button;
    if motion {
        code += 32;
    }
    if mouse.modifiers.contains(KeyModifiers::SHIFT) {
        code += 4;
    }
    if mouse.modifiers.contains(KeyModifiers::ALT) {
        code += 8;
    }
    if mouse.modifiers.contains(KeyModifiers::CONTROL) {
        code += 16;
    }
    Some((code, release))
}

fn encode_report(
    code: u8,
    release: bool,
    encoding: MouseProtocolEncoding,
    column: u16,
    row: u16,
) -> Vec<u8> {
    let x = column.saturating_add(1);
    let y = row.saturating_add(1);
    match encoding {
        MouseProtocolEncoding::Sgr => {
            let terminator = if release { 'm' } else { 'M' };
            format!("\x1b[<{code};{x};{y}{terminator}").into_bytes()
        }
        MouseProtocolEncoding::Utf8 => {
            let mut out = b"\x1b[M".to_vec();
            // X10 has one release code (3); SGR keeps the button number.
            let button = if release { 3 } else { code };
            for value in [button as u32 + 32, x as u32 + 32, y as u32 + 32] {
                let ch = char::from_u32(value).unwrap_or('?');
                let mut buffer = [0u8; 4];
                out.extend_from_slice(ch.encode_utf8(&mut buffer).as_bytes());
            }
            out
        }
        MouseProtocolEncoding::Default => {
            let button = if release { 3 } else { code };
            vec![
                0x1b,
                b'[',
                b'M',
                button + 32,
                x.saturating_add(32).min(255) as u8,
                y.saturating_add(32).min(255) as u8,
            ]
        }
    }
}

fn nonzero(value: u16) -> NonZeroU16 {
    NonZeroU16::new(value.max(1)).expect("terminal dimensions are at least 1")
}

fn function_key(number: u8) -> &'static [u8] {
    match number {
        1 => b"\x1bOP",
        2 => b"\x1bOQ",
        3 => b"\x1bOR",
        4 => b"\x1bOS",
        5 => b"\x1b[15~",
        6 => b"\x1b[17~",
        7 => b"\x1b[18~",
        8 => b"\x1b[19~",
        9 => b"\x1b[20~",
        10 => b"\x1b[21~",
        11 => b"\x1b[23~",
        12 => b"\x1b[24~",
        _ => b"",
    }
}

/// Configured shell wins; an empty value falls back to `$SHELL`.
fn resolve_shell(configured: Option<&str>) -> String {
    if let Some(shell) = configured.map(str::trim).filter(|shell| !shell.is_empty()) {
        return shell.to_string();
    }
    std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encodes_control_characters() {
        assert_eq!(
            encode_key(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL)),
            vec![0x03]
        );
        assert_eq!(
            encode_key(KeyEvent::new(KeyCode::Char(' '), KeyModifiers::CONTROL)),
            vec![0]
        );
        assert_eq!(
            encode_key(KeyEvent::new(KeyCode::Char('é'), KeyModifiers::NONE)),
            "é".as_bytes().to_vec()
        );
    }

    #[test]
    fn encodes_special_keys() {
        assert_eq!(
            encode_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)),
            vec![b'\r']
        );
        assert_eq!(
            encode_key(KeyEvent::new(KeyCode::Left, KeyModifiers::NONE)),
            b"\x1b[D".to_vec()
        );
        assert_eq!(
            encode_key(KeyEvent::new(KeyCode::F(5), KeyModifiers::NONE)),
            b"\x1b[15~".to_vec()
        );
        assert_eq!(
            encode_key(KeyEvent::new(KeyCode::Char('a'), KeyModifiers::ALT)),
            vec![0x1b, b'a']
        );
    }

    #[test]
    fn encodes_modified_navigation_and_function_keys() {
        assert_eq!(
            encode_key(KeyEvent::new(KeyCode::Left, KeyModifiers::CONTROL)),
            b"\x1b[1;5D".to_vec()
        );
        assert_eq!(
            encode_key(KeyEvent::new(KeyCode::Up, KeyModifiers::SHIFT)),
            b"\x1b[1;2A".to_vec()
        );
        assert_eq!(
            encode_key(KeyEvent::new(KeyCode::Left, KeyModifiers::ALT)),
            b"\x1b[1;3D".to_vec()
        );
        assert_eq!(
            encode_key(KeyEvent::new(
                KeyCode::F(5),
                KeyModifiers::CONTROL | KeyModifiers::SHIFT
            )),
            b"\x1b[15;6~".to_vec()
        );
        assert_eq!(
            encode_key(KeyEvent::new(KeyCode::Char('['), KeyModifiers::CONTROL)),
            vec![0x1b]
        );
        assert_eq!(
            encode_key(KeyEvent::new(KeyCode::Char('_'), KeyModifiers::CONTROL)),
            vec![0x1f]
        );
    }

    #[test]
    fn scrollback_uses_the_configured_limit() {
        let dir = std::env::temp_dir().join(format!("i3-shell-scroll-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("dir");
        let mut panel = ShellPanel::spawn_shell(&dir, 4, 40, "/bin/sh", 5000).expect("shell");
        assert!(panel.scroll(10_000));
        assert_eq!(panel.scrollback, 5000);
        assert!(!panel.scroll(1));
        drop(panel);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn configured_shell_beats_the_environment() {
        assert_eq!(resolve_shell(Some(" /bin/sh ")), "/bin/sh");
        assert!(!resolve_shell(Some("  ")).is_empty());
    }

    #[test]
    fn encodes_sgr_mouse_reports() {
        let sgr = MouseProtocolEncoding::Sgr;
        let mode = MouseProtocolMode::PressRelease;
        assert_eq!(
            encode_mouse(
                mouse_event(MouseEventKind::Down(MouseButton::Left), KeyModifiers::NONE),
                mode,
                sgr,
                4,
                2
            ),
            Some(b"\x1b[<0;5;3M".to_vec())
        );
        assert_eq!(
            encode_mouse(
                mouse_event(MouseEventKind::Up(MouseButton::Left), KeyModifiers::NONE),
                mode,
                sgr,
                4,
                2
            ),
            Some(b"\x1b[<0;5;3m".to_vec())
        );
        assert_eq!(
            encode_mouse(
                mouse_event(
                    MouseEventKind::Drag(MouseButton::Left),
                    KeyModifiers::CONTROL
                ),
                MouseProtocolMode::ButtonMotion,
                sgr,
                4,
                2
            ),
            Some(b"\x1b[<48;5;3M".to_vec())
        );
        assert_eq!(
            encode_mouse(
                mouse_event(MouseEventKind::ScrollUp, KeyModifiers::NONE),
                MouseProtocolMode::Press,
                sgr,
                0,
                0
            ),
            Some(b"\x1b[<64;1;1M".to_vec())
        );
        assert_eq!(
            encode_mouse(
                mouse_event(
                    MouseEventKind::Down(MouseButton::Right),
                    KeyModifiers::ALT | KeyModifiers::SHIFT
                ),
                mode,
                sgr,
                9,
                0
            ),
            Some(b"\x1b[<14;10;1M".to_vec())
        );
    }

    #[test]
    fn protocol_mode_gates_events() {
        let sgr = MouseProtocolEncoding::Sgr;
        let up = mouse_event(MouseEventKind::Up(MouseButton::Left), KeyModifiers::NONE);
        assert_eq!(encode_mouse(up, MouseProtocolMode::Press, sgr, 0, 0), None);
        assert!(encode_mouse(up, MouseProtocolMode::PressRelease, sgr, 0, 0).is_some());

        let drag = mouse_event(MouseEventKind::Drag(MouseButton::Left), KeyModifiers::NONE);
        assert_eq!(
            encode_mouse(drag, MouseProtocolMode::PressRelease, sgr, 0, 0),
            None
        );
        assert!(encode_mouse(drag, MouseProtocolMode::ButtonMotion, sgr, 0, 0).is_some());

        let moved = mouse_event(MouseEventKind::Moved, KeyModifiers::NONE);
        assert_eq!(
            encode_mouse(moved, MouseProtocolMode::ButtonMotion, sgr, 0, 0),
            None
        );
        assert_eq!(
            encode_mouse(moved, MouseProtocolMode::None, sgr, 0, 0),
            None
        );
        assert_eq!(
            encode_mouse(moved, MouseProtocolMode::AnyMotion, sgr, 0, 0),
            Some(b"\x1b[<35;1;1M".to_vec())
        );
    }

    #[test]
    fn legacy_encodings_use_single_release_code() {
        let up = mouse_event(MouseEventKind::Up(MouseButton::Left), KeyModifiers::NONE);
        assert_eq!(
            encode_mouse(
                up,
                MouseProtocolMode::PressRelease,
                MouseProtocolEncoding::Default,
                4,
                2
            ),
            Some(vec![0x1b, b'[', b'M', 35, 37, 35])
        );
        let down = mouse_event(MouseEventKind::Down(MouseButton::Left), KeyModifiers::NONE);
        assert_eq!(
            encode_mouse(
                down,
                MouseProtocolMode::Press,
                MouseProtocolEncoding::Utf8,
                200,
                5
            ),
            Some(vec![0x1b, b'[', b'M', 32, 0xc3, 0xa9, 38])
        );
    }

    fn mouse_event(kind: MouseEventKind, modifiers: KeyModifiers) -> MouseEvent {
        MouseEvent {
            kind,
            column: 0,
            row: 0,
            modifiers,
        }
    }

    #[test]
    fn spawns_a_shell_and_reads_output() {
        let dir = std::env::temp_dir();
        let mut panel = ShellPanel::spawn_shell(&dir, 10, 60, "/bin/sh", 2000).expect("spawn");
        panel.write(b"printf 'panel-ok\\n'\n");
        let mut visible = String::new();
        for _ in 0..100 {
            panel.pump();
            visible = panel.screen().contents();
            if visible.contains("panel-ok") {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        assert!(visible.contains("panel-ok"), "screen: {visible:?}");
        drop(panel);
    }

    #[test]
    fn drain_stops_at_the_budget() {
        let (sender, output) = mpsc::channel();
        for _ in 0..5 {
            sender.send(vec![b'a'; 400]).expect("send");
        }
        drop(sender);
        let mut parser = Parser::new(
            NonZeroU16::new(24).expect("rows"),
            NonZeroU16::new(80).expect("cols"),
            10,
        );
        drain(&output, &mut parser, 1024);
        assert_eq!(output.try_iter().count(), 2);
    }

    #[test]
    fn paste_is_wrapped_and_drops_a_forged_terminator() {
        let wrapped = wrap_paste("one\ntwo\x1b[201~three");
        assert!(wrapped.starts_with(b"\x1b[200~"));
        assert!(wrapped.ends_with(b"\x1b[201~"));
        assert_eq!(&wrapped[6..wrapped.len() - 6], b"one\ntwothree");
    }
}
