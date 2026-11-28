//! Terminal User Interface (TUI) subsystem powered by Ratatui and Crossterm.
//!
//! Owns terminal initialization, raw-mode/alternate-screen restoration, and
//! the event-driven rendering loop while keeping audio decoding and playback
//! decoupled.

pub mod browser;
pub mod keymap;
pub mod layout;
pub mod now_playing;
pub mod terminal;

use std::io::{self, IsTerminal};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use crossterm::event::{self, Event, KeyEvent};
use ratatui::Frame;
use ratatui::backend::Backend;
use thiserror::Error;

use crate::audio::{PlaybackController, PlaybackSnapshot};
use crate::library::{LibraryDatabase, LibraryError};

pub use browser::{LibraryBrowser, ListCursor};
pub use keymap::{
    DOCUMENTED_SHORTCUTS, RepeatMode, ShortcutDoc, TuiAction, dispatch_playback_action,
    resolve_key_action,
};
pub use layout::{DashboardLayout, FocusRegion, NavSection, render_dashboard};
pub use now_playing::{NowPlayingInfo, compute_progress_ratio, seek_target_from_column};
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
#[derive(Debug, Clone, PartialEq)]
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
    /// Latest centralized playback snapshot synchronized from `PlaybackController`.
    pub playback: PlaybackSnapshot,
    /// Whether the `?` keyboard shortcuts help overlay is visible.
    pub show_help: bool,
    /// Whether queue shuffle is enabled.
    pub shuffle_enabled: bool,
    /// Active repeat mode (`Off`, `All`, `One`).
    pub repeat_mode: RepeatMode,
    /// Whether a text input widget (e.g. search box) currently captures character keys.
    pub text_input_active: bool,
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
            playback: PlaybackController::new().snapshot(),
            show_help: false,
            shuffle_enabled: false,
            repeat_mode: RepeatMode::default(),
            text_input_active: false,
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

    /// Synchronizes centralized playback state and marks `needs_redraw` only
    /// when visible playback fields changed.
    pub fn sync_playback(&mut self, snapshot: PlaybackSnapshot) {
        let changed = self.playback.revision != snapshot.revision
            || self.playback.state != snapshot.state
            || self.playback.position.as_secs() != snapshot.position.as_secs()
            || self.playback.volume_percent != snapshot.volume_percent
            || self.playback.muted != snapshot.muted
            || self.playback.last_error != snapshot.last_error;
        self.playback = snapshot;
        if changed {
            self.needs_redraw = true;
        }
    }

    /// Builds a formatted [`NowPlayingInfo`] view model from the current snapshot.
    pub fn now_playing(&self) -> NowPlayingInfo {
        NowPlayingInfo::from_snapshot(&self.playback)
    }

    /// Processes a single [`TuiInputEvent`], updates UI state, and returns the
    /// resolved [`TuiAction`] for optional controller dispatch.
    pub fn handle_event(&mut self, event: TuiInputEvent) -> TuiAction {
        match event {
            TuiInputEvent::Key(key) => {
                let action =
                    resolve_key_action(key, self.focus, self.show_help, self.text_input_active);
                self.apply_action(action);
                action
            }
            TuiInputEvent::Resize(cols, rows) => {
                self.terminal_size = (cols, rows);
                self.needs_redraw = true;
                TuiAction::Noop
            }
            TuiInputEvent::Tick => TuiAction::Noop,
        }
    }

    fn apply_action(&mut self, action: TuiAction) {
        match action {
            TuiAction::Quit => self.should_quit = true,
            TuiAction::ToggleHelp => {
                self.show_help = !self.show_help;
                self.needs_redraw = true;
            }
            TuiAction::CloseHelp => {
                self.show_help = false;
                self.needs_redraw = true;
            }
            TuiAction::CycleFocus => {
                self.focus = self.focus.next();
                self.needs_redraw = true;
            }
            TuiAction::FocusSidebar => {
                self.focus = FocusRegion::Sidebar;
                self.needs_redraw = true;
            }
            TuiAction::FocusMainContent => {
                self.focus = FocusRegion::MainContent;
                self.needs_redraw = true;
            }
            TuiAction::SidebarUp => {
                self.active_section = self.active_section.previous();
                self.needs_redraw = true;
            }
            TuiAction::SidebarDown => {
                self.active_section = self.active_section.next();
                self.needs_redraw = true;
            }
            TuiAction::ListUp => {
                self.library.select_previous(self.active_section);
                self.needs_redraw = true;
            }
            TuiAction::ListDown => {
                self.library.select_next(self.active_section);
                self.needs_redraw = true;
            }
            TuiAction::ListPageUp => {
                self.library.page_up(self.active_section, 10);
                self.needs_redraw = true;
            }
            TuiAction::ListPageDown => {
                self.library.page_down(self.active_section, 10);
                self.needs_redraw = true;
            }
            TuiAction::ListFirst => {
                self.library.select_first(self.active_section);
                self.needs_redraw = true;
            }
            TuiAction::ListLast => {
                self.library.select_last(self.active_section);
                self.needs_redraw = true;
            }
            TuiAction::ToggleShuffle => {
                self.shuffle_enabled = !self.shuffle_enabled;
                self.needs_redraw = true;
            }
            TuiAction::CycleRepeat => {
                self.repeat_mode = self.repeat_mode.next();
                self.needs_redraw = true;
            }
            TuiAction::OpenSearch => {
                self.active_section = NavSection::Search;
                self.focus = FocusRegion::MainContent;
                self.needs_redraw = true;
            }
            TuiAction::OpenLibrary => {
                self.active_section = NavSection::Library;
                self.focus = FocusRegion::MainContent;
                self.needs_redraw = true;
            }
            TuiAction::SelectOrPlay
            | TuiAction::TogglePlayPause
            | TuiAction::NextTrack
            | TuiAction::PreviousTrack
            | TuiAction::SeekBackward
            | TuiAction::SeekForward
            | TuiAction::SeekPercent(_)
            | TuiAction::VolumeUp
            | TuiAction::VolumeDown
            | TuiAction::ToggleMute
            | TuiAction::ToggleFavorite
            | TuiAction::Noop => {}
        }
    }
}

/// Launches the interactive terminal user interface on the current TTY and
/// guarantees terminal restoration upon exit or error.
pub fn run_tui(shutdown: Arc<AtomicBool>) -> Result<(), TuiError> {
    run_tui_with_controller(PlaybackController::new(), shutdown)
}

/// Launches the interactive terminal user interface connected to `controller`.
pub fn run_tui_with_controller(
    controller: PlaybackController,
    shutdown: Arc<AtomicBool>,
) -> Result<(), TuiError> {
    if !io::stdin().is_terminal() || !io::stdout().is_terminal() {
        return Err(TuiError::NotATerminal);
    }

    let mut terminal = TuiTerminal::enter()?;
    let result =
        run_tui_loop_with_controller(&mut terminal, &controller, shutdown, DEFAULT_TICK_RATE);
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
    let controller = PlaybackController::new();
    run_tui_loop_with_controller(terminal, &controller, shutdown, tick_rate)
}

/// Executes the event-driven rendering loop synchronized with `controller`.
pub fn run_tui_loop_with_controller<B: Backend>(
    terminal: &mut TuiTerminal<B>,
    controller: &PlaybackController,
    shutdown: Arc<AtomicBool>,
    tick_rate: Duration,
) -> Result<TuiState, TuiError> {
    let mut state = TuiState::new();
    state.sync_playback(controller.snapshot());
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
                Event::Key(key) => {
                    let action = state.handle_event(TuiInputEvent::Key(key));
                    dispatch_playback_action(action, controller);
                    state.sync_playback(controller.snapshot());
                }
                Event::Resize(cols, rows) => {
                    state.handle_event(TuiInputEvent::Resize(cols, rows));
                }
                _ => {}
            }
        }

        if last_tick.elapsed() >= tick_rate {
            state.handle_event(TuiInputEvent::Tick);
            state.sync_playback(controller.snapshot());
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
    use crossterm::event::{KeyCode, KeyEventKind, KeyModifiers};
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
        assert_eq!(wide.playback_bar.height, 5);

        let medium = DashboardLayout::compute(Rect::new(0, 0, 60, 14));
        assert!(medium.sidebar.is_some());
        assert_eq!(medium.sidebar.unwrap().width, 17);
        assert_eq!(medium.playback_bar.height, 4);

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

    #[test]
    fn toggles_help_overlay_shuffle_repeat_and_quick_jumps() {
        let mut state = TuiState::new();
        state.terminal_size = (100, 30);

        // Toggle help overlay with '?'
        state.handle_event(TuiInputEvent::Key(KeyEvent::new(
            KeyCode::Char('?'),
            KeyModifiers::NONE,
        )));
        assert!(state.show_help);
        let help_render = render_to_string(100, 30, &state);
        assert!(help_render.contains("Keyboard Shortcuts"));
        assert!(help_render.contains("Play / Pause"));

        // Pressing 'q' while help is open closes help without quitting
        state.handle_event(TuiInputEvent::Key(KeyEvent::new(
            KeyCode::Char('q'),
            KeyModifiers::NONE,
        )));
        assert!(!state.show_help);
        assert!(!state.should_quit);

        // Toggle shuffle and repeat
        state.handle_event(TuiInputEvent::Key(KeyEvent::new(
            KeyCode::Char('s'),
            KeyModifiers::NONE,
        )));
        state.handle_event(TuiInputEvent::Key(KeyEvent::new(
            KeyCode::Char('r'),
            KeyModifiers::NONE,
        )));
        assert!(state.shuffle_enabled);
        assert_eq!(state.repeat_mode, RepeatMode::All);

        // Quick jump to Search ('/') and Library ('l')
        state.handle_event(TuiInputEvent::Key(KeyEvent::new(
            KeyCode::Char('/'),
            KeyModifiers::NONE,
        )));
        assert_eq!(state.active_section, NavSection::Search);
        assert_eq!(state.focus, FocusRegion::MainContent);

        state.handle_event(TuiInputEvent::Key(KeyEvent::new(
            KeyCode::Char('l'),
            KeyModifiers::NONE,
        )));
        assert_eq!(state.active_section, NavSection::Library);
        assert_eq!(state.focus, FocusRegion::MainContent);
    }

    #[test]
    fn sync_playback_throttles_subsecond_redraws_and_renders_progress_bar() {
        let mut state = TuiState::new();
        state.terminal_size = (100, 30);
        state.needs_redraw = false;

        let mut snap = PlaybackController::new().snapshot();
        snap.position = Duration::from_millis(10_200);
        snap.duration = Some(Duration::from_secs(100));
        state.sync_playback(snap.clone());
        assert!(state.needs_redraw);
        state.needs_redraw = false;

        // Sub-second advance within same displayed second (10s) must NOT force redraw
        snap.position = Duration::from_millis(10_850);
        state.sync_playback(snap.clone());
        assert!(!state.needs_redraw);

        // Crossing into second 11 triggers redraw
        snap.position = Duration::from_millis(11_050);
        state.sync_playback(snap);
        assert!(state.needs_redraw);

        let rendered = render_to_string(100, 30, &state);
        assert!(rendered.contains("00:11"));
        assert!(rendered.contains("01:40"));
    }
}
