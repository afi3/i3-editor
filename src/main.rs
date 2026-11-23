//! i3-editor: a keyboard-first terminal editor.

mod app;
mod cli;
mod clipboard;
mod command;
mod config;
mod conflict;
mod editor;
mod error;
mod explorer;
mod git;
mod input;
mod lsp;
mod plugins;
mod project_search;
mod recovery;
mod search_history;
mod search_options;
mod session;
mod shell;
mod state;
mod structure;
mod syntax;
mod terminal;
mod theme;
mod ui;
mod update;
mod watch;
mod workspace;

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
    let options = cli::parse()?;

    if options.version {
        use std::io::Write;
        let mut out = std::io::stdout();
        let _ = writeln!(out, "i3-editor {}", env!("CARGO_PKG_VERSION"));
        return Ok(());
    }
    if options.help {
        print_usage();
        return Ok(());
    }
    if options.update {
        return update::run();
    }

    let (config, warnings) = config::load();
    if options.dump_keymap {
        use std::io::Write;

        let enhanced = terminal::keyboard_enhancement_supported();
        let (keymap, key_warnings) = input::KeyMap::from_config(&config.keys, enhanced);
        let themes_dir = config::themes_dir().unwrap_or_default();
        let (theme, theme_warnings) = theme::Theme::from_config(&config.appearance, &themes_dir);
        let mut out = std::io::stdout();
        let _ = writeln!(out, "keyboard_enhancement: {enhanced}");
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
    let result = app::App::new(setup, config, warnings, options.targets).and_then(app::App::run);
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
         usage: i3-editor [OPTIONS] [FILE|DIR]...\n\
         \n\
         FILE            open one or more files as document tabs\n\
         FILE:LINE       open one file at a 1-based line\n\
         DIR             open a directory as the workspace root (only target)\n\
         --update        download and install the latest release, then exit\n\
         --dump-keymap   print resolved key bindings and theme, then exit\n\
         --version, -V   print the version and exit\n\
         --help, -h      print this help and exit\n\
         \n\
         Unknown options are errors. Multiple targets must be files.",
        env!("CARGO_PKG_VERSION")
    );
}
