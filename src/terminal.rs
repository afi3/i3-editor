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

/// Probe once; any terminal that never answers counts as unsupported.
pub fn keyboard_enhancement_supported() -> bool {
    supports_keyboard_enhancement().unwrap_or(false)
}

pub struct Setup {
    pub terminal: Terminal<CrosstermBackend<Stdout>>,
    pub keyboard_enhancement: bool,
}

pub fn init() -> io::Result<Setup> {
    let terminal = match ratatui::try_init() {
        Ok(terminal) => terminal,
        Err(error) => {
            // `try_init` may have entered raw mode before failing.
            ratatui::restore();
            return Err(error);
        }
    };
    let mut stdout = io::stdout();
    if let Err(error) = execute!(stdout, EnableBracketedPaste, EnableMouseCapture) {
        // The first sequence may have reached the terminal before the failure.
        let _ = execute!(stdout, DisableBracketedPaste, DisableMouseCapture);
        ratatui::restore();
        return Err(error);
    }
    let keyboard_enhancement = keyboard_enhancement_supported();
    if keyboard_enhancement
        && let Err(error) = execute!(
            stdout,
            PushKeyboardEnhancementFlags(KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES)
        )
    {
        let _ = execute!(stdout, DisableBracketedPaste, DisableMouseCapture);
        ratatui::restore();
        return Err(error);
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
#[cfg(unix)]
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

#[cfg(not(unix))]
pub fn install_signal_restore(_keyboard_enhancement: bool) -> io::Result<()> {
    Ok(())
}
