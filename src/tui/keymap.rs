//! Global and contextual keyboard shortcut resolution, focus management,
//! and documented keybindings for the Termwave TUI.

use std::fmt;

use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};

use crate::audio::{DEFAULT_SEEK_STEP, PlaybackController};

use super::layout::FocusRegion;

/// Repeat mode state cycled by the `r` keyboard shortcut.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum RepeatMode {
    /// Play queue once without repeating.
    #[default]
    Off,
    /// Repeat the entire queue/playlist.
    All,
    /// Repeat the current track.
    One,
}

impl RepeatMode {
    /// Cycles `Off -> All -> One -> Off`.
    pub const fn next(self) -> Self {
        match self {
            Self::Off => Self::All,
            Self::All => Self::One,
            Self::One => Self::Off,
        }
    }
}

impl fmt::Display for RepeatMode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let label = match self {
            Self::Off => "Off",
            Self::All => "All",
            Self::One => "One",
        };
        f.write_str(label)
    }
}

/// Logical action produced by resolving a keyboard shortcut in context.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TuiAction {
    /// Exit the TUI cleanly.
    Quit,
    /// Toggle the keyboard shortcuts help overlay (`?`).
    ToggleHelp,
    /// Close the help overlay (`Esc` / `q` when help is open).
    CloseHelp,
    /// Cycle focus across dashboard regions (`Tab`).
    CycleFocus,
    /// Move focus back to the navigation sidebar.
    FocusSidebar,
    /// Move focus into the main content panel.
    FocusMainContent,
    /// Move sidebar selection up.
    SidebarUp,
    /// Move sidebar selection down.
    SidebarDown,
    /// Move main list selection up.
    ListUp,
    /// Move main list selection down.
    ListDown,
    /// Jump main list selection up by one page.
    ListPageUp,
    /// Jump main list selection down by one page.
    ListPageDown,
    /// Jump to first item in main list.
    ListFirst,
    /// Jump to last item in main list.
    ListLast,
    /// Select the focused item or trigger playback (`Enter`).
    SelectOrPlay,
    /// Toggle Play / Pause (`Space`).
    TogglePlayPause,
    /// Advance to next track (`n`).
    NextTrack,
    /// Return to previous track (`p`).
    PreviousTrack,
    /// Seek backward (`Left` in PlaybackBar or `[`).
    SeekBackward,
    /// Seek forward (`Right` in PlaybackBar or `]`).
    SeekForward,
    /// Increase playback volume (`Up` in PlaybackBar or `+`/`=`).
    VolumeUp,
    /// Decrease playback volume (`Down` in PlaybackBar or `-`).
    VolumeDown,
    /// Toggle mute (`m`).
    ToggleMute,
    /// Toggle shuffle mode (`s`).
    ToggleShuffle,
    /// Cycle repeat mode (`r`).
    CycleRepeat,
    /// Switch to Search section (`/`).
    OpenSearch,
    /// Switch to Library section (`l`).
    OpenLibrary,
    /// Toggle favorite status for the selected or active track (`f`).
    ToggleFavorite,
    /// Key had no bound action in the current context.
    Noop,
}

/// A documented keyboard shortcut entry displayed in the `?` Help overlay.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ShortcutDoc {
    /// Key label (e.g. `"Space"`, `"Up / Down"`).
    pub key: &'static str,
    /// Scope where the shortcut applies (`"Global"`, `"Sidebar"`, `"List"`, `"Playback"`).
    pub scope: &'static str,
    /// Human-readable description of the action.
    pub description: &'static str,
}

/// Authoritative list of documented keyboard shortcuts shown in the Help overlay.
pub const DOCUMENTED_SHORTCUTS: &[ShortcutDoc] = &[
    ShortcutDoc {
        key: "Space",
        scope: "Global",
        description: "Play / Pause",
    },
    ShortcutDoc {
        key: "n / p",
        scope: "Global",
        description: "Next / Previous track",
    },
    ShortcutDoc {
        key: "+ / - / m",
        scope: "Global",
        description: "Volume up / down / mute",
    },
    ShortcutDoc {
        key: "[ / ]",
        scope: "Global",
        description: "Seek backward / forward",
    },
    ShortcutDoc {
        key: "s / r",
        scope: "Global",
        description: "Toggle Shuffle / Cycle Repeat",
    },
    ShortcutDoc {
        key: "l / /",
        scope: "Global",
        description: "Jump to Library / Search",
    },
    ShortcutDoc {
        key: "f",
        scope: "Global",
        description: "Toggle Favorite",
    },
    ShortcutDoc {
        key: "Tab",
        scope: "Global",
        description: "Cycle focus region",
    },
    ShortcutDoc {
        key: "?",
        scope: "Global",
        description: "Toggle Help overlay",
    },
    ShortcutDoc {
        key: "q / Ctrl+C",
        scope: "Global",
        description: "Quit Termwave",
    },
    ShortcutDoc {
        key: "Up / Down (k / j)",
        scope: "Sidebar / List",
        description: "Navigate items",
    },
    ShortcutDoc {
        key: "Enter / Right (l)",
        scope: "Sidebar",
        description: "Focus main panel",
    },
    ShortcutDoc {
        key: "Esc / Left (h)",
        scope: "Main",
        description: "Back to sidebar",
    },
    ShortcutDoc {
        key: "Left / Right",
        scope: "Playback Bar",
        description: "Seek backward / forward",
    },
    ShortcutDoc {
        key: "Up / Down",
        scope: "Playback Bar",
        description: "Increase / decrease volume",
    },
];

/// Resolves a [`KeyEvent`] into a conflict-free [`TuiAction`] based on current
/// focus region, help overlay state, and text-input capture mode.
pub fn resolve_key_action(
    key: KeyEvent,
    focus: FocusRegion,
    help_open: bool,
    text_input_active: bool,
) -> TuiAction {
    if key.kind != KeyEventKind::Press {
        return TuiAction::Noop;
    }

    // Ctrl+C always quits regardless of modal or text-input state.
    if key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL) {
        return TuiAction::Quit;
    }

    // Help overlay captures navigation/quit keys so 'q' or Esc closes Help first.
    if help_open {
        return match key.code {
            KeyCode::Esc | KeyCode::Char('?') | KeyCode::Char('q') | KeyCode::Char('Q') => {
                TuiAction::CloseHelp
            }
            _ => TuiAction::Noop,
        };
    }

    // When a text input (such as Search) is active, avoid firing global letter shortcuts.
    if text_input_active {
        return match key.code {
            KeyCode::Esc => TuiAction::FocusSidebar,
            KeyCode::Tab => TuiAction::CycleFocus,
            KeyCode::Enter => TuiAction::SelectOrPlay,
            _ => TuiAction::Noop,
        };
    }

    // Global shortcuts (unambiguous across all non-text-input focus regions).
    match key.code {
        KeyCode::Char('q') | KeyCode::Char('Q') => return TuiAction::Quit,
        KeyCode::Char('?') => return TuiAction::ToggleHelp,
        KeyCode::Tab => return TuiAction::CycleFocus,
        KeyCode::Char(' ') => return TuiAction::TogglePlayPause,
        KeyCode::Char('n') | KeyCode::Char('N') => return TuiAction::NextTrack,
        KeyCode::Char('p') | KeyCode::Char('P') => return TuiAction::PreviousTrack,
        KeyCode::Char('m') | KeyCode::Char('M') => return TuiAction::ToggleMute,
        KeyCode::Char('+') | KeyCode::Char('=') => return TuiAction::VolumeUp,
        KeyCode::Char('-') => return TuiAction::VolumeDown,
        KeyCode::Char('[') => return TuiAction::SeekBackward,
        KeyCode::Char(']') => return TuiAction::SeekForward,
        KeyCode::Char('s') | KeyCode::Char('S') => return TuiAction::ToggleShuffle,
        KeyCode::Char('r') | KeyCode::Char('R') => return TuiAction::CycleRepeat,
        KeyCode::Char('/') => return TuiAction::OpenSearch,
        KeyCode::Char('f') | KeyCode::Char('F') => return TuiAction::ToggleFavorite,
        KeyCode::Char('l') | KeyCode::Char('L') if focus != FocusRegion::Sidebar => {
            return TuiAction::OpenLibrary;
        }
        _ => {}
    }

    // Contextual shortcuts scoped by the active FocusRegion.
    match focus {
        FocusRegion::Sidebar => match key.code {
            KeyCode::Esc => TuiAction::Quit,
            KeyCode::Enter | KeyCode::Right | KeyCode::Char('l') => TuiAction::FocusMainContent,
            KeyCode::Up | KeyCode::Char('k') => TuiAction::SidebarUp,
            KeyCode::Down | KeyCode::Char('j') => TuiAction::SidebarDown,
            _ => TuiAction::Noop,
        },
        FocusRegion::MainContent => match key.code {
            KeyCode::Esc | KeyCode::Left | KeyCode::Char('h') => TuiAction::FocusSidebar,
            KeyCode::Enter => TuiAction::SelectOrPlay,
            KeyCode::Up | KeyCode::Char('k') => TuiAction::ListUp,
            KeyCode::Down | KeyCode::Char('j') => TuiAction::ListDown,
            KeyCode::PageUp => TuiAction::ListPageUp,
            KeyCode::PageDown => TuiAction::ListPageDown,
            KeyCode::Home | KeyCode::Char('g') => TuiAction::ListFirst,
            KeyCode::End | KeyCode::Char('G') => TuiAction::ListLast,
            _ => TuiAction::Noop,
        },
        FocusRegion::PlaybackBar => match key.code {
            KeyCode::Esc => TuiAction::FocusSidebar,
            KeyCode::Enter => TuiAction::TogglePlayPause,
            KeyCode::Left => TuiAction::SeekBackward,
            KeyCode::Right => TuiAction::SeekForward,
            KeyCode::Up => TuiAction::VolumeUp,
            KeyCode::Down => TuiAction::VolumeDown,
            _ => TuiAction::Noop,
        },
    }
}

/// Applies playback-related [`TuiAction`]s to `controller` safely without
/// propagating non-fatal state errors (e.g. seeking while stopped).
pub fn dispatch_playback_action(action: TuiAction, controller: &PlaybackController) {
    match action {
        TuiAction::TogglePlayPause => {
            let _ = controller.toggle_pause();
        }
        TuiAction::NextTrack => {
            let _ = controller.next_track();
        }
        TuiAction::PreviousTrack => {
            let _ = controller.previous_track();
        }
        TuiAction::SeekBackward => {
            let _ = controller.seek_backward(DEFAULT_SEEK_STEP);
        }
        TuiAction::SeekForward => {
            let _ = controller.seek_forward(DEFAULT_SEEK_STEP);
        }
        TuiAction::VolumeUp => {
            controller.increase_volume();
        }
        TuiAction::VolumeDown => {
            controller.decrease_volume();
        }
        TuiAction::ToggleMute => {
            controller.toggle_mute();
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn press(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    #[test]
    fn resolves_global_and_contextual_shortcuts_without_conflicts() {
        // Space toggles play/pause globally
        assert_eq!(
            resolve_key_action(
                press(KeyCode::Char(' ')),
                FocusRegion::Sidebar,
                false,
                false
            ),
            TuiAction::TogglePlayPause
        );
        assert_eq!(
            resolve_key_action(
                press(KeyCode::Char(' ')),
                FocusRegion::MainContent,
                false,
                false
            ),
            TuiAction::TogglePlayPause
        );

        // Up/Down navigate in Sidebar/MainContent, but adjust volume in PlaybackBar
        assert_eq!(
            resolve_key_action(press(KeyCode::Up), FocusRegion::Sidebar, false, false),
            TuiAction::SidebarUp
        );
        assert_eq!(
            resolve_key_action(press(KeyCode::Up), FocusRegion::MainContent, false, false),
            TuiAction::ListUp
        );
        assert_eq!(
            resolve_key_action(press(KeyCode::Up), FocusRegion::PlaybackBar, false, false),
            TuiAction::VolumeUp
        );

        // Left/Right seek in PlaybackBar, but navigate focus in Sidebar/MainContent
        assert_eq!(
            resolve_key_action(press(KeyCode::Left), FocusRegion::PlaybackBar, false, false),
            TuiAction::SeekBackward
        );
        assert_eq!(
            resolve_key_action(press(KeyCode::Left), FocusRegion::MainContent, false, false),
            TuiAction::FocusSidebar
        );
    }

    #[test]
    fn help_overlay_and_text_input_prevent_shortcut_conflicts() {
        // When help is open, 'q' closes help instead of quitting
        assert_eq!(
            resolve_key_action(press(KeyCode::Char('q')), FocusRegion::Sidebar, true, false),
            TuiAction::CloseHelp
        );

        // When text input is active, letter shortcuts are suppressed
        assert_eq!(
            resolve_key_action(
                press(KeyCode::Char('q')),
                FocusRegion::MainContent,
                false,
                true
            ),
            TuiAction::Noop
        );
        assert_eq!(
            resolve_key_action(
                press(KeyCode::Char(' ')),
                FocusRegion::MainContent,
                false,
                true
            ),
            TuiAction::Noop
        );
        // Ctrl+C still quits even during text input
        assert_eq!(
            resolve_key_action(
                KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL),
                FocusRegion::MainContent,
                false,
                true
            ),
            TuiAction::Quit
        );
    }

    #[test]
    fn cycles_repeat_mode_and_dispatches_volume_to_controller() {
        assert_eq!(RepeatMode::Off.next(), RepeatMode::All);
        assert_eq!(RepeatMode::All.next(), RepeatMode::One);
        assert_eq!(RepeatMode::One.next(), RepeatMode::Off);

        let controller = PlaybackController::new();
        assert_eq!(controller.volume_percent(), 100);
        dispatch_playback_action(TuiAction::VolumeDown, &controller);
        assert!(controller.volume_percent() < 100);
        dispatch_playback_action(TuiAction::ToggleMute, &controller);
        assert!(controller.is_muted());
    }
}
