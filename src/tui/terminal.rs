//! Terminal initialization, raw-mode management, and restoration guards.

use std::io::{self, Stdout};
use std::sync::Once;

use crossterm::execute;
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use ratatui::Terminal;
use ratatui::backend::{Backend, CrosstermBackend};

static PANIC_HOOK_ONCE: Once = Once::new();

/// Restores terminal raw mode and alternate screen state.
///
/// Safe to call multiple times or from a panic hook.
pub fn restore_terminal_state() -> io::Result<()> {
    let mut first_err: Option<io::Error> = None;

    if let Err(err) = disable_raw_mode() {
        first_err = Some(err);
    }

    let mut stdout = io::stdout();
    if let Err(err) = execute!(stdout, LeaveAlternateScreen)
        && first_err.is_none()
    {
        first_err = Some(err);
    }

    match first_err {
        Some(err) => Err(err),
        None => Ok(()),
    }
}

/// Installs a chained panic hook once so terminal state is restored even if a panic occurs.
pub fn install_panic_restore_hook() {
    PANIC_HOOK_ONCE.call_once(|| {
        let previous_hook = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |panic_info| {
            let _ = restore_terminal_state();
            previous_hook(panic_info);
        }));
    });
}

/// RAII wrapper around a `ratatui::Terminal` that guarantees raw mode and the
/// alternate screen are restored on exit, error, or drop.
pub struct TuiTerminal<B: Backend = CrosstermBackend<Stdout>> {
    terminal: Terminal<B>,
    owns_raw_terminal: bool,
    restored: bool,
}

impl TuiTerminal<CrosstermBackend<Stdout>> {
    /// Enters raw mode and the alternate screen, returning an initialized Ratatui terminal.
    pub fn enter() -> io::Result<Self> {
        install_panic_restore_hook();
        enable_raw_mode()?;

        let mut stdout = io::stdout();
        if let Err(err) = execute!(stdout, EnterAlternateScreen) {
            let _ = disable_raw_mode();
            return Err(err);
        }

        let backend = CrosstermBackend::new(stdout);
        let mut terminal = match Terminal::new(backend) {
            Ok(t) => t,
            Err(err) => {
                let _ = restore_terminal_state();
                return Err(err);
            }
        };

        if let Err(err) = terminal.hide_cursor() {
            let _ = restore_terminal_state();
            return Err(err);
        }

        Ok(Self {
            terminal,
            owns_raw_terminal: true,
            restored: false,
        })
    }
}

impl<B: Backend> TuiTerminal<B> {
    /// Wraps an arbitrary Ratatui backend (such as `TestBackend`) without
    /// modifying process-wide raw mode or alternate screen state.
    pub fn with_backend(backend: B) -> io::Result<Self> {
        let terminal = Terminal::new(backend)?;
        Ok(Self {
            terminal,
            owns_raw_terminal: false,
            restored: false,
        })
    }

    /// Returns a mutable reference to the underlying Ratatui [`Terminal`].
    pub fn inner_mut(&mut self) -> &mut Terminal<B> {
        &mut self.terminal
    }

    /// Returns an immutable reference to the underlying Ratatui [`Terminal`].
    pub fn inner(&self) -> &Terminal<B> {
        &self.terminal
    }

    /// Explicitly restores the terminal state (idempotent).
    pub fn restore(&mut self) -> io::Result<()> {
        if self.restored {
            return Ok(());
        }
        self.restored = true;

        let mut first_err: Option<io::Error> = None;
        if let Err(err) = self.terminal.show_cursor() {
            first_err = Some(err);
        }
        if self.owns_raw_terminal
            && let Err(err) = restore_terminal_state()
            && first_err.is_none()
        {
            first_err = Some(err);
        }

        match first_err {
            Some(err) => Err(err),
            None => Ok(()),
        }
    }
}

impl<B: Backend> Drop for TuiTerminal<B> {
    fn drop(&mut self) {
        let _ = self.restore();
    }
}
