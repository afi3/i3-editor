//! Terminal lifecycle: setup, capability probe, and cleanup on every exit path.

use std::io::{self, Stdout};

use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use ratatui::crossterm::event::{
    DisableBracketedPaste, DisableMouseCapture, EnableBracketedPaste, EnableMouseCapture,
    KeyboardEnhancementFlags, PopKeyboardEnhancementFlags, PushKeyboardEnhancementFlags,
};
use ratatui::crossterm::execute;
use ratatui::crossterm::terminal::supports_keyboard_enhancement;

pub struct Setup {
    pub terminal: Terminal<CrosstermBackend<Stdout>>,
    pub keyboard_enhancement: bool,
}

pub fn init() -> io::Result<Setup> {
    let terminal = ratatui::try_init()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnableBracketedPaste, EnableMouseCapture)?;
    let keyboard_enhancement = supports_keyboard_enhancement().unwrap_or(false);
    if keyboard_enhancement {
        execute!(
            stdout,
            PushKeyboardEnhancementFlags(KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES)
        )?;
    }
    install_panic_hook(keyboard_enhancement);
    Ok(Setup {
        terminal,
        keyboard_enhancement,
    })
}

pub fn restore(keyboard_enhancement: bool) {
    let mut stdout = io::stdout();
    if keyboard_enhancement {
        let _ = execute!(stdout, PopKeyboardEnhancementFlags);
    }
    let _ = execute!(stdout, DisableBracketedPaste, DisableMouseCapture);
    ratatui::restore();
}

/// Cleanup before the default hook runs, so a panic never leaves the terminal in
/// bracketed-paste or mouse-capture mode.
fn install_panic_hook(keyboard_enhancement: bool) {
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let mut stdout = io::stdout();
        if keyboard_enhancement {
            let _ = execute!(stdout, PopKeyboardEnhancementFlags);
        }
        let _ = execute!(stdout, DisableBracketedPaste, DisableMouseCapture);
        previous(info);
    }));
}

/// Restore the terminal when the process is terminated by a signal.
pub fn install_signal_restore(keyboard_enhancement: bool) -> io::Result<()> {
    let mut signals = signal_hook::iterator::Signals::new([
        signal_hook::consts::SIGTERM,
        signal_hook::consts::SIGHUP,
        signal_hook::consts::SIGINT,
    ])?;
    std::thread::spawn(move || {
        if signals.forever().next().is_some() {
            restore(keyboard_enhancement);
            std::process::exit(130);
        }
    });
    Ok(())
}
