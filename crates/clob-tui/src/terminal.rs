//! Owning the terminal, and giving it back.
//!
//! A TUI takes over the screen: alternate buffer, raw mode, no cursor. If the process ends
//! without undoing that, the shell is left with no echo, no line editing and no cursor —
//! the user's next move is `reset` in a terminal that is not showing what they type.
//!
//! So restoration happens in three places, and the redundancy is the point:
//!
//! 1. `Drop`, for the ordinary path and for `?` returning early.
//! 2. A panic hook, because a panic unwinds past everything else and is exactly when this
//!    is most likely to be forgotten.
//! 3. Ctrl-C, handled as an event rather than as a signal, so quitting is the same code
//!    path as pressing `q`.

use std::io::{Stdout, stdout};

use anyhow::{Context, Result};
use crossterm::execute;
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;

/// A terminal that puts itself back.
pub struct Screen {
    inner: Terminal<CrosstermBackend<Stdout>>,
}

impl Screen {
    /// Takes over the terminal, and arranges for it to be returned however this ends.
    pub fn open() -> Result<Self> {
        // Installed before raw mode, so a panic between here and the first frame still
        // restores. The default hook runs after, so the message is printed to a terminal
        // that can display it.
        let previous = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            let _ = restore();
            previous(info);
        }));

        enable_raw_mode().context("cannot put the terminal in raw mode")?;
        let mut out = stdout();
        execute!(out, EnterAlternateScreen).context("cannot open the alternate screen")?;

        let inner = Terminal::new(CrosstermBackend::new(out)).context("cannot drive the terminal")?;
        Ok(Self { inner })
    }

    pub fn terminal(&mut self) -> &mut Terminal<CrosstermBackend<Stdout>> {
        &mut self.inner
    }
}

impl Drop for Screen {
    fn drop(&mut self) {
        // Nothing to do about a failure here — the process is ending either way, and
        // panicking in a `Drop` during an unwind aborts.
        let _ = restore();
        let _ = self.inner.show_cursor();
    }
}

/// Undoes everything `open` did. Safe to call more than once.
fn restore() -> Result<()> {
    disable_raw_mode()?;
    execute!(stdout(), LeaveAlternateScreen)?;
    Ok(())
}
