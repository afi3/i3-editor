//! Real-binary PTY smoke test: open a file, type, save, confirm quit.
#![cfg(unix)]

use std::io::{Read, Write};
use std::num::NonZeroU16;
use std::sync::mpsc::{self, Receiver};
use std::time::{Duration, Instant};

use portable_pty::{CommandBuilder, PtySize, native_pty_system};
use vt100::Parser;

const ROWS: u16 = 24;
const COLS: u16 = 80;

fn pump(parser: &mut Parser, rx: &Receiver<Vec<u8>>) {
    while let Ok(chunk) = rx.try_recv() {
        parser.process(&chunk);
    }
}

fn wait_for(parser: &mut Parser, rx: &Receiver<Vec<u8>>, needle: &str, timeout: Duration) -> bool {
    let deadline = Instant::now() + timeout;
    loop {
        pump(parser, rx);
        if parser.screen().contents().contains(needle) {
            return true;
        }
        if Instant::now() >= deadline {
            return false;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}

#[test]
fn type_save_and_confirm_quit() {
    let dir = tempfile::tempdir().expect("tempdir");
    let file = dir.path().join("note.txt");
    std::fs::write(&file, "").expect("empty target");

    let pair = native_pty_system()
        .openpty(PtySize {
            rows: ROWS,
            cols: COLS,
            pixel_width: 0,
            pixel_height: 0,
        })
        .expect("pty");

    let mut command = CommandBuilder::new(env!("CARGO_BIN_EXE_i3-editor"));
    command.cwd(dir.path());
    command.arg(&file);
    command.env("TERM", "xterm-256color");
    command.env("NO_COLOR", "1");
    command.env("XDG_CONFIG_HOME", dir.path().join("config"));
    let mut child = pair.slave.spawn_command(command).expect("spawn editor");
    drop(pair.slave);

    let mut reader = pair.master.try_clone_reader().expect("reader");
    let mut writer = pair.master.take_writer().expect("writer");
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let mut buffer = [0u8; 8192];
        loop {
            match reader.read(&mut buffer) {
                Ok(0) | Err(_) => break,
                Ok(count) => {
                    if tx.send(buffer[..count].to_vec()).is_err() {
                        break;
                    }
                }
            }
        }
    });

    let mut parser = Parser::new(
        NonZeroU16::new(ROWS).unwrap(),
        NonZeroU16::new(COLS).unwrap(),
        1000,
    );
    assert!(
        wait_for(&mut parser, &rx, "note.txt", Duration::from_secs(10)),
        "startup screen:\n{}",
        parser.screen().contents()
    );

    writer.write_all(b"hello").expect("type");
    assert!(
        wait_for(&mut parser, &rx, "hello", Duration::from_secs(5)),
        "typed screen:\n{}",
        parser.screen().contents()
    );

    writer.write_all(&[0x13]).expect("save"); // Ctrl+S
    writer.write_all(&[0x11]).expect("quit"); // Ctrl+Q
    assert!(
        wait_for(&mut parser, &rx, "Quit", Duration::from_secs(5)),
        "quit prompt screen:\n{}",
        parser.screen().contents()
    );
    writer.write_all(b"\r").expect("confirm quit");

    let deadline = Instant::now() + Duration::from_secs(10);
    let status = loop {
        pump(&mut parser, &rx);
        match child.try_wait().expect("wait") {
            Some(status) => break status,
            None if Instant::now() >= deadline => {
                let _ = child.kill();
                panic!("editor did not exit:\n{}", parser.screen().contents());
            }
            None => std::thread::sleep(Duration::from_millis(10)),
        }
    };

    assert!(status.success(), "editor exit status: {status:?}");
    assert_eq!(std::fs::read_to_string(&file).expect("saved file"), "hello");
}
