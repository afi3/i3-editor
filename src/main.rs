//! i3-editor: a keyboard-first terminal editor.

mod app;
mod clipboard;
mod command;
mod config;
mod editor;
mod error;
mod explorer;
mod input;
mod state;
mod terminal;
mod ui;
mod workspace;

use std::path::PathBuf;

fn main() -> std::process::ExitCode {
    match run() {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("i3-editor: {error}");
            std::process::ExitCode::FAILURE
        }
    }
}

fn run() -> error::Result<()> {
    let mut target: Option<PathBuf> = None;
    let mut dump_keymap = false;
    for argument in std::env::args().skip(1) {
        if argument == "--dump-keymap" {
            dump_keymap = true;
        } else if target.is_none() {
            target = Some(PathBuf::from(argument));
        }
    }

    let (config, warnings) = config::load();
    if dump_keymap {
        let (keymap, key_warnings) = input::KeyMap::from_config(&config.keys);
        for line in keymap.labels() {
            println!("{line}");
        }
        for warning in warnings.iter().chain(key_warnings.iter()) {
            eprintln!("warning: {warning}");
        }
        return Ok(());
    }

    let setup = terminal::init()?;
    let keyboard_enhancement = setup.keyboard_enhancement;
    terminal::install_signal_restore(keyboard_enhancement).ok();
    let result = app::App::new(setup, config, warnings, target).and_then(app::App::run);
    terminal::restore(keyboard_enhancement);
    result
}
