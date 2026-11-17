//! Embedded terminal: a real shell in a PTY, shown in a bottom panel.

use std::io::{Read, Write};
use std::num::NonZeroU16;
use std::path::Path;
use std::sync::mpsc::{self, Receiver};
use std::thread;

use portable_pty::{Child, CommandBuilder, MasterPty, PtySize, native_pty_system};
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use vt100::Parser;

use crate::error::{Error, Result};

pub struct ShellPanel {
    master: Box<dyn MasterPty + Send>,
    writer: Box<dyn Write + Send>,
    child: Box<dyn Child + Send + Sync>,
    parser: Parser,
    output: Receiver<Vec<u8>>,
    rows: u16,
    cols: u16,
    scrollback: usize,
    shell: String,
    exit_reported: bool,
}

impl ShellPanel {
    /// Spawn `$SHELL` (fallback `/bin/sh`) in `cwd`.
    pub fn spawn(cwd: &Path, rows: u16, cols: u16) -> Result<Self> {
        let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".to_string());
        Self::spawn_shell(cwd, rows, cols, &shell)
    }

    pub fn spawn_shell(cwd: &Path, rows: u16, cols: u16, shell: &str) -> Result<Self> {
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

        let (sender, output) = mpsc::channel();
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
            parser: Parser::new(nonzero(rows), nonzero(cols), 2000),
            output,
            rows,
            cols,
            scrollback: 0,
            shell: shell.to_string(),
            exit_reported: false,
        })
    }

    /// Drain pending PTY output into the screen model.
    pub fn pump(&mut self) {
        while let Ok(chunk) = self.output.try_recv() {
            self.parser.process(&chunk);
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

    /// Scroll the visible window into scrollback. Returns true on change.
    pub fn scroll(&mut self, delta: isize) -> bool {
        let next = (self.scrollback as isize + delta).clamp(0, 2000) as usize;
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
        &self.shell
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
    }
}

/// Encode a key event as the byte sequence a terminal would send.
pub fn encode_key(key: KeyEvent) -> Vec<u8> {
    let mut out = Vec::new();
    let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
    let alt = key.modifiers.contains(KeyModifiers::ALT);
    if alt {
        out.push(0x1b);
    }
    match key.code {
        KeyCode::Char(ch) => {
            if ctrl {
                let lower = ch.to_ascii_lowercase();
                if lower.is_ascii_lowercase() {
                    out.push((lower as u8) & 0x1f);
                } else if ch == ' ' {
                    out.push(0);
                }
            } else {
                out.extend(ch.to_string().bytes());
            }
        }
        KeyCode::Enter => out.push(b'\r'),
        KeyCode::Tab => out.push(b'\t'),
        KeyCode::Backspace => out.push(0x7f),
        KeyCode::Esc => out.push(0x1b),
        KeyCode::Up => out.extend(b"\x1b[A"),
        KeyCode::Down => out.extend(b"\x1b[B"),
        KeyCode::Right => out.extend(b"\x1b[C"),
        KeyCode::Left => out.extend(b"\x1b[D"),
        KeyCode::Home => out.extend(b"\x1b[H"),
        KeyCode::End => out.extend(b"\x1b[F"),
        KeyCode::Delete => out.extend(b"\x1b[3~"),
        KeyCode::Insert => out.extend(b"\x1b[2~"),
        KeyCode::PageUp => out.extend(b"\x1b[5~"),
        KeyCode::PageDown => out.extend(b"\x1b[6~"),
        KeyCode::BackTab => out.extend(b"\x1b[Z"),
        KeyCode::F(number) => out.extend(function_key(number)),
        _ => {}
    }
    out
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
    fn spawns_a_shell_and_reads_output() {
        let dir = std::env::temp_dir();
        let mut panel = ShellPanel::spawn_shell(&dir, 10, 60, "/bin/sh").expect("spawn");
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
}
