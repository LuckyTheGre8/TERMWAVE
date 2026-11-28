//! Terminal User Interface (TUI) subsystem powered by Ratatui and Crossterm.
//!
//! Owns terminal initialization, raw-mode/alternate-screen restoration, and
//! the event-driven rendering loop while keeping audio decoding and playback
//! decoupled.

pub mod browser;
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

use crate::library::{LibraryDatabase, LibraryError};

pub use browser::{LibraryBrowser, ListCursor};
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
    /// Interactive music library browser state.
    pub library: LibraryBrowser,
}

impl Default for TuiState {
    fn default() -> Self {
        Self::new()
    }
}

impl TuiState {
    /// Creates a new [`TuiState`] with `needs_redraw` enabled.
    pub fn new() -> Self {
        Self {
            should_quit: false,
            needs_redraw: true,
            terminal_size: (0, 0),
            rendered_frames: 0,
            active_section: NavSection::default(),
            focus: FocusRegion::default(),
            library: LibraryBrowser::default(),
        }
    }

    /// Creates a [`TuiState`] preloaded with tracks, albums, artists, favorites,
    /// and recent history from `db`.
    pub fn from_database(db: &LibraryDatabase) -> Result<Self, LibraryError> {
        let mut state = Self::new();
        state.refresh_library(db)?;
        Ok(state)
    }

    /// Reloads library browser data from `db` and marks the UI for redraw.
    pub fn refresh_library(&mut self, db: &LibraryDatabase) -> Result<(), LibraryError> {
        self.library.load_from_database(db)?;
        self.needs_redraw = true;
        Ok(())
    }

    /// Processes a single [`TuiInputEvent`] and updates dirty/quit state.
    pub fn handle_event(&mut self, event: TuiInputEvent) {
        match event {
            TuiInputEvent::Key(key) => {
                if key.kind != KeyEventKind::Press {
                    return;
                }
                if matches!(key.code, KeyCode::Char('q') | KeyCode::Char('Q'))
                    || (key.code == KeyCode::Char('c')
                        && key.modifiers.contains(KeyModifiers::CONTROL))
                {
                    self.should_quit = true;
                    return;
                }

                if key.code == KeyCode::Esc {
                    if self.focus != FocusRegion::Sidebar {
                        self.focus = FocusRegion::Sidebar;
                        self.needs_redraw = true;
                    } else {
                        self.should_quit = true;
                    }
                    return;
                }

                match key.code {
                    KeyCode::Tab => {
                        self.focus = self.focus.next();
                        self.needs_redraw = true;
                    }
                    KeyCode::Enter | KeyCode::Right | KeyCode::Char('l')
                        if self.focus == FocusRegion::Sidebar =>
                    {
                        self.focus = FocusRegion::MainContent;
                        self.needs_redraw = true;
                    }
                    KeyCode::Left | KeyCode::Char('h')
                        if self.focus == FocusRegion::MainContent =>
                    {
                        self.focus = FocusRegion::Sidebar;
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
                    KeyCode::Up | KeyCode::Char('k') if self.focus == FocusRegion::MainContent => {
                        self.library.select_previous(self.active_section);
                        self.needs_redraw = true;
                    }
                    KeyCode::Down | KeyCode::Char('j')
                        if self.focus == FocusRegion::MainContent =>
                    {
                        self.library.select_next(self.active_section);
                        self.needs_redraw = true;
                    }
                    KeyCode::PageUp if self.focus == FocusRegion::MainContent => {
                        self.library.page_up(self.active_section, 10);
                        self.needs_redraw = true;
                    }
                    KeyCode::PageDown if self.focus == FocusRegion::MainContent => {
                        self.library.page_down(self.active_section, 10);
                        self.needs_redraw = true;
                    }
                    KeyCode::Home | KeyCode::Char('g')
                        if self.focus == FocusRegion::MainContent =>
                    {
                        self.library.select_first(self.active_section);
                        self.needs_redraw = true;
                    }
                    KeyCode::End | KeyCode::Char('G') if self.focus == FocusRegion::MainContent => {
                        self.library.select_last(self.active_section);
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
    let mut state = TuiState::new();
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
    fn renders_database_library_views_and_supports_keyboard_navigation() {
        use crate::library::{LibraryDatabase, NewTrackRecord};

        let db = LibraryDatabase::open_in_memory().unwrap();
        let artist = db.upsert_artist("Miles Davis").unwrap();
        let album = db
            .upsert_album("Kind of Blue", Some(artist.id), Some(1959), None)
            .unwrap();
        db.upsert_track(&NewTrackRecord {
            source_path: "/music/so_what.flac".to_string(),
            title: Some("So What".to_string()),
            artist: Some("Miles Davis".to_string()),
            album: Some("Kind of Blue".to_string()),
            album_artist: None,
            artist_id: Some(artist.id),
            album_id: Some(album.id),
            genre: Some("Jazz".to_string()),
            duration_ms: Some(562_000),
            codec: "FLAC".to_string(),
            sample_rate: Some(96_000),
            bit_depth: Some(24),
            channels: Some(2),
            file_size: 100_000,
            track_number: Some(1),
            disc_number: Some(1),
            last_modified: Some(100),
        })
        .unwrap();

        let mut state = TuiState::from_database(&db).unwrap();
        state.terminal_size = (100, 30);

        // Library tracks render
        let library_render = render_to_string(100, 30, &state);
        assert!(library_render.contains("So What"));
        assert!(library_render.contains("Miles Davis"));
        assert!(library_render.contains("09:22"));

        // Enter focuses MainContent; Esc returns focus to Sidebar
        state.handle_event(TuiInputEvent::Key(KeyEvent::new(
            KeyCode::Enter,
            KeyModifiers::NONE,
        )));
        assert_eq!(state.focus, FocusRegion::MainContent);
        state.handle_event(TuiInputEvent::Key(KeyEvent::new(
            KeyCode::Esc,
            KeyModifiers::NONE,
        )));
        assert_eq!(state.focus, FocusRegion::Sidebar);
        assert!(!state.should_quit);

        // Switch to Albums and render
        state.active_section = NavSection::Albums;
        let albums_render = render_to_string(100, 30, &state);
        assert!(albums_render.contains("Kind of Blue"));
        assert!(albums_render.contains("1959"));

        // Switch to Artists and render
        state.active_section = NavSection::Artists;
        let artists_render = render_to_string(100, 30, &state);
        assert!(artists_render.contains("Miles Davis"));
        assert!(artists_render.contains("1 track(s)"));
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
