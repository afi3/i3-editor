//! i3-editor: a keyboard-first terminal editor.

mod app;
mod clipboard;
mod command;
mod config;
mod editor;
mod error;
mod explorer;
mod input;
mod shell;
mod state;
mod syntax;
mod terminal;
mod theme;
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
        use std::io::Write;

        let (keymap, key_warnings) = input::KeyMap::from_config(&config.keys);
        let themes_dir = config::themes_dir().unwrap_or_default();
        let (theme, theme_warnings) = theme::Theme::from_config(&config.appearance, &themes_dir);
        let mut out = std::io::stdout();
        let _ = writeln!(out, "theme: {}", theme.name());
        for line in keymap.labels() {
            let _ = writeln!(out, "{line}");
        }
        for warning in warnings
            .iter()
            .chain(key_warnings.iter())
            .chain(theme_warnings.iter())
        {
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
