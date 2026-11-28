//! Terminal User Interface (TUI) subsystem powered by Ratatui and Crossterm.
//!
//! Owns terminal initialization, raw-mode/alternate-screen restoration, and
//! the event-driven rendering loop while keeping audio decoding and playback
//! decoupled.

pub mod layout;
pub mod terminal;

use std::io::{self, IsTerminal};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use ratatui::Frame;
use ratatui::backend::Backend;
use thiserror::Error;

pub use layout::{DashboardLayout, FocusRegion, NavSection, render_dashboard};
pub use terminal::{TuiTerminal, install_panic_restore_hook, restore_terminal_state};

/// Default event polling interval for the TUI rendering loop.
pub const DEFAULT_TICK_RATE: Duration = Duration::from_millis(250);

/// Errors produced by the terminal user interface subsystem.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum TuiError {
    /// Interactive TUI requires an attached terminal (TTY).
    #[error("interactive terminal UI requires a TTY")]
    NotATerminal,

    /// Underlying terminal I/O or rendering failure.
    #[error("terminal UI error: {0}")]
    Io(#[from] io::Error),
}

/// High-level input event processed by the TUI loop.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TuiInputEvent {
    /// Keyboard press event.
    Key(KeyEvent),
    /// Terminal resized to `(columns, rows)`.
    Resize(u16, u16),
    /// Periodic tick for clock/status updates.
    Tick,
}

/// Core state tracked by the TUI rendering loop.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TuiState {
    /// Whether the loop should terminate cleanly.
    pub should_quit: bool,
    /// Whether the UI has pending visual changes requiring a redraw.
    pub needs_redraw: bool,
    /// Current terminal dimensions `(width, height)` observed by the UI.
    pub terminal_size: (u16, u16),
    /// Total frames rendered so far.
    pub rendered_frames: u64,
    /// Currently selected navigation section in the sidebar.
    pub active_section: NavSection,
    /// Currently focused dashboard region.
    pub focus: FocusRegion,
}

impl Default for TuiState {
    fn default() -> Self {
        Self {
            should_quit: false,
            needs_redraw: true,
            terminal_size: (0, 0),
            rendered_frames: 0,
            active_section: NavSection::default(),
            focus: FocusRegion::default(),
        }
    }
}

impl TuiState {
    /// Processes a single [`TuiInputEvent`] and updates dirty/quit state.
    pub fn handle_event(&mut self, event: TuiInputEvent) {
        match event {
            TuiInputEvent::Key(key) => {
                if key.kind != KeyEventKind::Press {
                    return;
                }
                let is_quit_key = matches!(
                    key.code,
                    KeyCode::Char('q') | KeyCode::Char('Q') | KeyCode::Esc
                ) || (key.code == KeyCode::Char('c')
                    && key.modifiers.contains(KeyModifiers::CONTROL));
                if is_quit_key {
                    self.should_quit = true;
                    return;
                }

                match key.code {
                    KeyCode::Tab => {
                        self.focus = self.focus.next();
                        self.needs_redraw = true;
                    }
                    KeyCode::Up | KeyCode::Char('k') if self.focus == FocusRegion::Sidebar => {
                        self.active_section = self.active_section.previous();
                        self.needs_redraw = true;
                    }
                    KeyCode::Down | KeyCode::Char('j') if self.focus == FocusRegion::Sidebar => {
                        self.active_section = self.active_section.next();
                        self.needs_redraw = true;
                    }
                    _ => {}
                }
            }
            TuiInputEvent::Resize(cols, rows) => {
                self.terminal_size = (cols, rows);
                self.needs_redraw = true;
            }
            TuiInputEvent::Tick => {
                // Only redraw on tick when active playback/progress updates require it.
            }
        }
    }
}

/// Launches the interactive terminal user interface on the current TTY and
/// guarantees terminal restoration upon exit or error.
pub fn run_tui(shutdown: Arc<AtomicBool>) -> Result<(), TuiError> {
    if !io::stdin().is_terminal() || !io::stdout().is_terminal() {
        return Err(TuiError::NotATerminal);
    }

    let mut terminal = TuiTerminal::enter()?;
    let result = run_tui_loop(&mut terminal, shutdown, DEFAULT_TICK_RATE);
    let restore_res = terminal.restore();

    result?;
    restore_res?;
    Ok(())
}

/// Executes the event-driven rendering loop against an initialized [`TuiTerminal`].
pub fn run_tui_loop<B: Backend>(
    terminal: &mut TuiTerminal<B>,
    shutdown: Arc<AtomicBool>,
    tick_rate: Duration,
) -> Result<TuiState, TuiError> {
    let mut state = TuiState::default();
    if let Ok(area) = terminal.inner().size() {
        state.terminal_size = (area.width, area.height);
    }

    let mut last_tick = Instant::now();

    while !state.should_quit && !shutdown.load(Ordering::Acquire) {
        if state.needs_redraw {
            terminal
                .inner_mut()
                .draw(|frame| render_frame(frame, &state))?;
            state.rendered_frames = state.rendered_frames.saturating_add(1);
            state.needs_redraw = false;
        }

        let timeout = tick_rate.saturating_sub(last_tick.elapsed());
        if event::poll(timeout)? {
            match event::read()? {
                Event::Key(key) => state.handle_event(TuiInputEvent::Key(key)),
                Event::Resize(cols, rows) => state.handle_event(TuiInputEvent::Resize(cols, rows)),
                _ => {}
            }
        }

        if last_tick.elapsed() >= tick_rate {
            state.handle_event(TuiInputEvent::Tick);
            last_tick = Instant::now();
        }
    }

    Ok(state)
}

/// Renders the Termwave dashboard layout into `frame`.
pub fn render_frame(frame: &mut Frame, state: &TuiState) {
    render_dashboard(frame, state);
}

#[cfg(test)]
mod tests {
    use ratatui::backend::TestBackend;
    use ratatui::layout::Rect;

    use super::*;

    fn render_to_string(width: u16, height: u16, state: &TuiState) -> String {
        let backend = TestBackend::new(width, height);
        let mut tui_terminal = TuiTerminal::with_backend(backend).unwrap();
        tui_terminal
            .inner_mut()
            .draw(|frame| render_frame(frame, state))
            .unwrap();
        tui_terminal
            .inner()
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|cell| cell.symbol())
            .collect()
    }

    #[test]
    fn renders_dashboard_header_sidebar_main_and_playback_bar() {
        let state = TuiState {
            terminal_size: (100, 30),
            ..TuiState::default()
        };

        let content = render_to_string(100, 30, &state);
        assert!(content.contains("TERMWAVE"));
        assert!(content.contains("Navigation"));
        assert!(content.contains("Library"));
        assert!(content.contains("Artists"));
        assert!(content.contains("Albums"));
        assert!(content.contains("Favorites"));
        assert!(content.contains("Now Playing"));
        assert!(content.contains("Stopped"));
    }

    #[test]
    fn supports_responsive_resizing_including_compact_viewports() {
        let wide = DashboardLayout::compute(Rect::new(0, 0, 120, 40));
        assert!(wide.sidebar.is_some());
        assert_eq!(wide.sidebar.unwrap().width, 22);
        assert_eq!(wide.header.height, 3);
        assert_eq!(wide.playback_bar.height, 4);

        let medium = DashboardLayout::compute(Rect::new(0, 0, 60, 14));
        assert!(medium.sidebar.is_some());
        assert_eq!(medium.sidebar.unwrap().width, 17);
        assert_eq!(medium.playback_bar.height, 3);

        // Ultra-compact viewport collapses sidebar without panicking
        let narrow = DashboardLayout::compute(Rect::new(0, 0, 36, 10));
        assert!(narrow.sidebar.is_none());
        assert_eq!(narrow.header.height, 1);

        let state = TuiState {
            terminal_size: (36, 10),
            ..TuiState::default()
        };
        let compact_render = render_to_string(36, 10, &state);
        assert!(compact_render.contains("TERMWAVE"));
    }

    #[test]
    fn navigates_sidebar_sections_and_cycles_focus() {
        let mut state = TuiState::default();
        assert_eq!(state.focus, FocusRegion::Sidebar);
        assert_eq!(state.active_section, NavSection::Library);

        state.handle_event(TuiInputEvent::Key(KeyEvent::new(
            KeyCode::Down,
            KeyModifiers::NONE,
        )));
        assert_eq!(state.active_section, NavSection::Artists);

        state.handle_event(TuiInputEvent::Key(KeyEvent::new(
            KeyCode::Up,
            KeyModifiers::NONE,
        )));
        assert_eq!(state.active_section, NavSection::Library);

        state.handle_event(TuiInputEvent::Key(KeyEvent::new(
            KeyCode::Tab,
            KeyModifiers::NONE,
        )));
        assert_eq!(state.focus, FocusRegion::MainContent);
    }

    #[test]
    fn handles_resize_quit_keys_and_avoids_redundant_redraws() {
        let mut state = TuiState::default();
        assert!(state.needs_redraw);
        state.needs_redraw = false;

        // Tick alone should not force redundant redraws when idle
        state.handle_event(TuiInputEvent::Tick);
        assert!(!state.needs_redraw);

        // Resize triggers redraw and updates dimensions
        state.handle_event(TuiInputEvent::Resize(120, 40));
        assert!(state.needs_redraw);
        assert_eq!(state.terminal_size, (120, 40));

        // Non-press key events (Release/Repeat) should not trigger quit
        let release_q = KeyEvent::new_with_kind(
            KeyCode::Char('q'),
            KeyModifiers::NONE,
            KeyEventKind::Release,
        );
        state.handle_event(TuiInputEvent::Key(release_q));
        assert!(!state.should_quit);

        // Pressing 'q' sets should_quit
        state.handle_event(TuiInputEvent::Key(KeyEvent::new(
            KeyCode::Char('q'),
            KeyModifiers::NONE,
        )));
        assert!(state.should_quit);

        // Ctrl+C also sets should_quit
        let mut state2 = TuiState::default();
        state2.handle_event(TuiInputEvent::Key(KeyEvent::new(
            KeyCode::Char('c'),
            KeyModifiers::CONTROL,
        )));
        assert!(state2.should_quit);
    }

    #[test]
    fn terminal_guard_restore_is_idempotent() {
        let backend = TestBackend::new(40, 12);
        let mut tui_terminal = TuiTerminal::with_backend(backend).unwrap();
        tui_terminal.restore().unwrap();
        // Calling restore multiple times or dropping afterward must succeed cleanly
        tui_terminal.restore().unwrap();
    }
}
