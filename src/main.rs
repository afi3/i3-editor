//! i3-editor: a keyboard-first terminal editor.

mod app;
mod clipboard;
mod command;
mod config;
mod editor;
mod error;
mod explorer;
mod input;
mod project_search;
mod session;
mod shell;
mod state;
mod syntax;
mod terminal;
mod theme;
mod ui;
mod update;
mod watch;
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
    let mut show_version = false;
    let mut show_help = false;
    let mut run_update = false;
    for argument in std::env::args().skip(1) {
        match argument.as_str() {
            "--dump-keymap" => dump_keymap = true,
            "--version" | "-V" => show_version = true,
            "--help" | "-h" => show_help = true,
            "--update" => run_update = true,
            other => {
                if target.is_none() {
                    target = Some(PathBuf::from(other));
                }
            }
        }
    }

    if show_version {
        use std::io::Write;
        let mut out = std::io::stdout();
        let _ = writeln!(out, "i3-editor {}", env!("CARGO_PKG_VERSION"));
        return Ok(());
    }
    if show_help {
        print_usage();
        return Ok(());
    }
    if run_update {
        return update::run();
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

fn print_usage() {
    use std::io::Write;

    let mut out = std::io::stdout();
    let _ = writeln!(
        out,
        "i3-editor {}\n\
         \n\
         usage: i3-editor [PATH] [--update] [--dump-keymap] [--version]\n\
         \n\
         PATH            file or directory to open (default: current directory)\n\
         --update        download and install the latest release, then exit\n\
         --dump-keymap   print resolved key bindings and theme, then exit\n\
         --version, -V   print the version and exit\n\
         --help, -h      print this help and exit",
        env!("CARGO_PKG_VERSION")
    );
}
