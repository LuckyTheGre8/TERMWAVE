//! Responsive terminal dashboard layout and region rendering.

use std::path::Path;

use ratatui::Frame;
use ratatui::layout::{Alignment, Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, List, ListItem, ListState, Paragraph};

use super::TuiState;
use super::keymap::DOCUMENTED_SHORTCUTS;

/// Primary navigation sections displayed in the sidebar.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum NavSection {
    /// Overview and quick summary.
    Home,
    /// All indexed tracks in the local library.
    #[default]
    Library,
    /// Browse by artist.
    Artists,
    /// Browse by album.
    Albums,
    /// User-curated playlists.
    Playlists,
    /// Active playback queue.
    Queue,
    /// Favorited tracks.
    Favorites,
    /// Recently played tracks.
    RecentlyPlayed,
    /// Interactive library search.
    Search,
    /// Application settings and themes.
    Settings,
}

impl NavSection {
    /// Ordered list of all navigation sections for the sidebar.
    pub const ALL: [Self; 10] = [
        Self::Home,
        Self::Library,
        Self::Artists,
        Self::Albums,
        Self::Playlists,
        Self::Queue,
        Self::Favorites,
        Self::RecentlyPlayed,
        Self::Search,
        Self::Settings,
    ];

    /// Human-readable label shown in the navigation sidebar.
    pub const fn label(self) -> &'static str {
        match self {
            Self::Home => "Home",
            Self::Library => "Library",
            Self::Artists => "Artists",
            Self::Albums => "Albums",
            Self::Playlists => "Playlists",
            Self::Queue => "Queue",
            Self::Favorites => "Favorites",
            Self::RecentlyPlayed => "Recently Played",
            Self::Search => "Search",
            Self::Settings => "Settings",
        }
    }

    /// Brief description shown inside the main panel placeholder.
    pub const fn description(self) -> &'static str {
        match self {
            Self::Home => "Welcome to Termwave — lossless-first terminal Hi-Fi music player.",
            Self::Library => "Indexed local music library tracks.",
            Self::Artists => "Browse library artists.",
            Self::Albums => "Browse library albums and release years.",
            Self::Playlists => "Manage and play saved playlists.",
            Self::Queue => "Inspect and reorder the active playback queue.",
            Self::Favorites => "Quick access to your favorited tracks.",
            Self::RecentlyPlayed => "Tracks played in recent listening sessions.",
            Self::Search => "Search tracks, artists, and albums.",
            Self::Settings => "Audio output, themes, and keyboard preferences.",
        }
    }

    /// Returns the next section in sidebar order (wrapping around).
    pub fn next(self) -> Self {
        let idx = Self::ALL.iter().position(|&s| s == self).unwrap_or(0);
        Self::ALL[(idx + 1) % Self::ALL.len()]
    }

    /// Returns the previous section in sidebar order (wrapping around).
    pub fn previous(self) -> Self {
        let idx = Self::ALL.iter().position(|&s| s == self).unwrap_or(0);
        Self::ALL[(idx + Self::ALL.len() - 1) % Self::ALL.len()]
    }
}

/// Logical focus regions within the dashboard.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum FocusRegion {
    /// Left-hand navigation sidebar.
    #[default]
    Sidebar,
    /// Center/right primary content panel.
    MainContent,
    /// Bottom playback status and controls panel.
    PlaybackBar,
}

impl FocusRegion {
    /// Cycles focus forward across dashboard regions.
    pub const fn next(self) -> Self {
        match self {
            Self::Sidebar => Self::MainContent,
            Self::MainContent => Self::PlaybackBar,
            Self::PlaybackBar => Self::Sidebar,
        }
    }
}

/// Computed rectangular regions for the Termwave dashboard.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DashboardLayout {
    /// Top application header bar.
    pub header: Rect,
    /// Left navigation sidebar (`None` if viewport is too narrow).
    pub sidebar: Option<Rect>,
    /// Central music library / view content area.
    pub main_content: Rect,
    /// Bottom playback status and progress bar.
    pub playback_bar: Rect,
}

impl DashboardLayout {
    /// Computes responsive dashboard regions for the given terminal viewport `area`.
    pub fn compute(area: Rect) -> Self {
        let header_height = if area.height < 12 { 1 } else { 3 };
        let playback_height = if area.height < 16 { 3 } else { 4 };

        let vertical = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(header_height),
                Constraint::Min(3),
                Constraint::Length(playback_height),
            ])
            .split(area);

        let body = vertical[1];
        let (sidebar, main_content) = if body.width >= 44 {
            let sidebar_width = if body.width >= 80 { 22 } else { 17 };
            let horizontal = Layout::default()
                .direction(Direction::Horizontal)
                .constraints([Constraint::Length(sidebar_width), Constraint::Min(20)])
                .split(body);
            (Some(horizontal[0]), horizontal[1])
        } else {
            (None, body)
        };

        Self {
            header: vertical[0],
            sidebar,
            main_content,
            playback_bar: vertical[2],
        }
    }
}

/// Renders the complete Termwave dashboard layout into `frame`.
pub fn render_dashboard(frame: &mut Frame, state: &TuiState) {
    let area = frame.area();
    let layout = DashboardLayout::compute(area);

    render_header(frame, layout.header, state);
    if let Some(sidebar_rect) = layout.sidebar {
        render_sidebar(frame, sidebar_rect, state);
    }
    render_main_content(frame, layout.main_content, state);
    render_playback_bar(frame, layout.playback_bar, state);

    if state.show_help {
        render_help_overlay(frame, area);
    }
}

fn render_header(frame: &mut Frame, area: Rect, state: &TuiState) {
    let np = state.now_playing();
    let status_text = format!(
        "v{}  |  {}  |  {}  |  {}x{}",
        env!("CARGO_PKG_VERSION"),
        state.active_section.label(),
        np.state_label,
        state.terminal_size.0,
        state.terminal_size.1
    );

    let line = Line::from(vec![
        Span::styled(
            " TERMWAVE ",
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw("— "),
        Span::styled(status_text, Style::default().fg(Color::Gray)),
    ]);

    let widget = if area.height >= 3 {
        Paragraph::new(line)
            .alignment(Alignment::Left)
            .block(Block::default().borders(Borders::ALL).title("Termwave"))
    } else {
        Paragraph::new(line).alignment(Alignment::Left)
    };

    frame.render_widget(widget, area);
}

fn render_sidebar(frame: &mut Frame, area: Rect, state: &TuiState) {
    let is_focused = state.focus == FocusRegion::Sidebar;
    let border_style = region_border_style(is_focused);

    let items: Vec<ListItem> = NavSection::ALL
        .iter()
        .map(|section| {
            let active = *section == state.active_section;
            let prefix = if active { "> " } else { "  " };
            let style = if active {
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(Color::White)
            };
            ListItem::new(Line::from(Span::styled(
                format!("{prefix}{}", section.label()),
                style,
            )))
        })
        .collect();

    let list = List::new(items).block(
        Block::default()
            .borders(Borders::ALL)
            .border_style(border_style)
            .title("Navigation"),
    );
    frame.render_widget(list, area);
}

fn render_main_content(frame: &mut Frame, area: Rect, state: &TuiState) {
    let is_focused = state.focus == FocusRegion::MainContent;
    let border_style = region_border_style(is_focused);

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(border_style)
        .title(main_panel_title(state));

    match state.active_section {
        NavSection::Library => {
            if state.library.tracks.is_empty() {
                render_empty_section(
                    frame,
                    area,
                    block,
                    "No tracks indexed in library yet.",
                    "Scan a music directory into the library database to browse tracks.",
                );
                return;
            }
            let items: Vec<ListItem> = state
                .library
                .tracks
                .iter()
                .map(|track| {
                    let title = track_display_title(track.title.as_deref(), &track.source_path);
                    let artist = track.artist.as_deref().unwrap_or("Unknown Artist");
                    let album = track.album.as_deref().unwrap_or("Unknown Album");
                    let dur = format_duration_ms(track.duration_ms);
                    ListItem::new(Line::from(vec![
                        Span::styled(title, Style::default().fg(Color::White)),
                        Span::styled(
                            format!("  —  {artist}  [{album}]  ({dur})"),
                            Style::default().fg(Color::Gray),
                        ),
                    ]))
                })
                .collect();
            render_selectable_list(
                frame,
                area,
                block,
                items,
                state.library.selected_index(NavSection::Library),
                is_focused,
            );
        }
        NavSection::Albums => {
            if state.library.albums.is_empty() {
                render_empty_section(
                    frame,
                    area,
                    block,
                    "No albums indexed in library yet.",
                    "Albums appear automatically when tagged tracks are scanned.",
                );
                return;
            }
            let items: Vec<ListItem> = state
                .library
                .albums
                .iter()
                .map(|album| {
                    let year = album
                        .release_year
                        .map_or_else(String::new, |y| format!(" ({y})"));
                    let count = state.library.track_count_for_album(album.id);
                    ListItem::new(Line::from(vec![
                        Span::styled(album.name.clone(), Style::default().fg(Color::White)),
                        Span::styled(
                            format!("{year}  —  {count} track(s)"),
                            Style::default().fg(Color::Gray),
                        ),
                    ]))
                })
                .collect();
            render_selectable_list(
                frame,
                area,
                block,
                items,
                state.library.selected_index(NavSection::Albums),
                is_focused,
            );
        }
        NavSection::Artists => {
            if state.library.artists.is_empty() {
                render_empty_section(
                    frame,
                    area,
                    block,
                    "No artists indexed in library yet.",
                    "Artists appear automatically when tagged tracks are scanned.",
                );
                return;
            }
            let items: Vec<ListItem> = state
                .library
                .artists
                .iter()
                .map(|artist| {
                    let count = state.library.track_count_for_artist(artist.id);
                    ListItem::new(Line::from(vec![
                        Span::styled(artist.name.clone(), Style::default().fg(Color::White)),
                        Span::styled(
                            format!("  —  {count} track(s)"),
                            Style::default().fg(Color::Gray),
                        ),
                    ]))
                })
                .collect();
            render_selectable_list(
                frame,
                area,
                block,
                items,
                state.library.selected_index(NavSection::Artists),
                is_focused,
            );
        }
        NavSection::Favorites => {
            if state.library.favorites.is_empty() {
                render_empty_section(
                    frame,
                    area,
                    block,
                    "No favorite tracks yet.",
                    "Mark tracks as favorites to access them quickly here.",
                );
                return;
            }
            let items: Vec<ListItem> = state
                .library
                .favorites
                .iter()
                .map(|fav| {
                    let title =
                        track_display_title(fav.track.title.as_deref(), &fav.track.source_path);
                    let artist = fav.track.artist.as_deref().unwrap_or("Unknown Artist");
                    let missing = if fav.file_exists { "" } else { " [missing]" };
                    ListItem::new(Line::from(vec![
                        Span::styled("* ", Style::default().fg(Color::Yellow)),
                        Span::styled(title, Style::default().fg(Color::White)),
                        Span::styled(
                            format!("  —  {artist}{missing}"),
                            Style::default().fg(Color::Gray),
                        ),
                    ]))
                })
                .collect();
            render_selectable_list(
                frame,
                area,
                block,
                items,
                state.library.selected_index(NavSection::Favorites),
                is_focused,
            );
        }
        NavSection::RecentlyPlayed => {
            if state.library.recently_played.is_empty() {
                render_empty_section(
                    frame,
                    area,
                    block,
                    "No recently played tracks.",
                    "Playback history entries will appear here after listening.",
                );
                return;
            }
            let items: Vec<ListItem> = state
                .library
                .recently_played
                .iter()
                .map(|entry| {
                    let title =
                        track_display_title(entry.track.title.as_deref(), &entry.track.source_path);
                    let artist = entry.track.artist.as_deref().unwrap_or("Unknown Artist");
                    let dur = format_duration_ms(Some(entry.playback_duration_ms));
                    ListItem::new(Line::from(vec![
                        Span::styled(title, Style::default().fg(Color::White)),
                        Span::styled(
                            format!("  —  {artist}  (played {dur})"),
                            Style::default().fg(Color::Gray),
                        ),
                    ]))
                })
                .collect();
            render_selectable_list(
                frame,
                area,
                block,
                items,
                state.library.selected_index(NavSection::RecentlyPlayed),
                is_focused,
            );
        }
        NavSection::Home => {
            let np = state.now_playing();
            let mut lines = vec![
                Line::from(Span::styled(
                    "Library Overview",
                    Style::default()
                        .fg(Color::White)
                        .add_modifier(Modifier::BOLD),
                )),
                Line::from(format!(
                    "Tracks: {}  |  Albums: {}  |  Artists: {}  |  Favorites: {}",
                    state.library.tracks.len(),
                    state.library.albums.len(),
                    state.library.artists.len(),
                    state.library.favorites.len(),
                )),
                Line::from(""),
                Line::from(Span::styled(
                    "Now Playing",
                    Style::default()
                        .fg(Color::White)
                        .add_modifier(Modifier::BOLD),
                )),
                Line::from(vec![
                    Span::styled(
                        format!("[{}] ", np.state_label),
                        Style::default()
                            .fg(np.state_color)
                            .add_modifier(Modifier::BOLD),
                    ),
                    Span::styled(np.title, Style::default().fg(Color::White)),
                ]),
            ];
            if np.has_track {
                lines.push(Line::from(Span::styled(
                    format!("Artist: {}  |  Album: {}", np.artist, np.album),
                    Style::default().fg(Color::Gray),
                )));
                lines.push(Line::from(Span::styled(
                    format!("Audio:  {}", np.audio_info),
                    Style::default().fg(Color::DarkGray),
                )));
            }
            lines.push(Line::from(""));
            lines.push(Line::from(Span::styled(
                "Tab / Enter: focus list  |  Up/Down (j/k): navigate  |  q: quit",
                Style::default().fg(Color::DarkGray),
            )));
            let panel = Paragraph::new(lines)
                .alignment(Alignment::Left)
                .block(block);
            frame.render_widget(panel, area);
        }
        other => {
            let lines = vec![
                Line::from(Span::styled(
                    other.label(),
                    Style::default()
                        .fg(Color::White)
                        .add_modifier(Modifier::BOLD),
                )),
                Line::from(""),
                Line::from(Span::styled(
                    other.description(),
                    Style::default().fg(Color::Gray),
                )),
                Line::from(""),
                Line::from(Span::styled(
                    "Tab: switch focus  |  Up/Down: navigate  |  q / Esc / Ctrl+C: quit",
                    Style::default().fg(Color::DarkGray),
                )),
            ];
            let panel = Paragraph::new(lines)
                .alignment(Alignment::Left)
                .block(block);
            frame.render_widget(panel, area);
        }
    }
}

fn main_panel_title(state: &TuiState) -> String {
    let count = state.library.section_len(state.active_section);
    if count > 0 {
        let pos = state
            .library
            .selected_index(state.active_section)
            .map_or(0, |i| i + 1);
        format!("{} ({pos}/{count})", state.active_section.label())
    } else {
        state.active_section.label().to_string()
    }
}

fn render_empty_section(
    frame: &mut Frame,
    area: Rect,
    block: Block<'_>,
    headline: &str,
    hint: &str,
) {
    let lines = vec![
        Line::from(Span::styled(headline, Style::default().fg(Color::White))),
        Line::from(""),
        Line::from(Span::styled(hint, Style::default().fg(Color::DarkGray))),
    ];
    let paragraph = Paragraph::new(lines)
        .alignment(Alignment::Left)
        .block(block);
    frame.render_widget(paragraph, area);
}

fn render_selectable_list(
    frame: &mut Frame,
    area: Rect,
    block: Block<'_>,
    items: Vec<ListItem<'_>>,
    selected: Option<usize>,
    is_focused: bool,
) {
    let highlight_style = if is_focused {
        Style::default()
            .fg(Color::Black)
            .bg(Color::Cyan)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default()
            .fg(Color::Cyan)
            .add_modifier(Modifier::BOLD)
    };

    let list = List::new(items)
        .block(block)
        .highlight_symbol("> ")
        .highlight_style(highlight_style);

    let mut list_state = ListState::default();
    list_state.select(selected);
    frame.render_stateful_widget(list, area, &mut list_state);
}

fn track_display_title(title: Option<&str>, source_path: &str) -> String {
    if let Some(t) = title.map(str::trim).filter(|s| !s.is_empty()) {
        return t.to_string();
    }
    Path::new(source_path)
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| source_path.to_string())
}

fn format_duration_ms(duration_ms: Option<u64>) -> String {
    let Some(ms) = duration_ms else {
        return "--:--".to_string();
    };
    let total_secs = ms / 1000;
    let mins = total_secs / 60;
    let secs = total_secs % 60;
    format!("{mins:02}:{secs:02}")
}

fn render_playback_bar(frame: &mut Frame, area: Rect, state: &TuiState) {
    let is_focused = state.focus == FocusRegion::PlaybackBar;
    let border_style = region_border_style(is_focused);
    let np = state.now_playing();

    let mut top_spans = vec![
        Span::styled(
            np.state_label,
            Style::default()
                .fg(np.state_color)
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw("  |  "),
        Span::styled(
            np.title,
            Style::default()
                .fg(Color::White)
                .add_modifier(Modifier::BOLD),
        ),
    ];

    if np.has_track {
        top_spans.push(Span::styled(
            format!("  —  {}  [{}]", np.artist, np.album),
            Style::default().fg(Color::Gray),
        ));
    }
    if let Some(queue) = np.queue_label {
        top_spans.push(Span::styled(
            format!("  {queue}"),
            Style::default().fg(Color::Cyan),
        ));
    }

    let bottom_line = if let Some(err) = np.last_error {
        Line::from(Span::styled(
            format!("Error: {err}"),
            Style::default().fg(Color::Red),
        ))
    } else {
        let shuffle_label = if state.shuffle_enabled { "On" } else { "Off" };
        Line::from(Span::styled(
            format!(
                "{}  |  {}  |  Shuffle: {shuffle_label}  |  Repeat: {}  |  {}",
                np.position_label, np.volume_label, state.repeat_mode, np.audio_info
            ),
            Style::default().fg(Color::DarkGray),
        ))
    };

    let bar = Paragraph::new(vec![Line::from(top_spans), bottom_line])
        .alignment(Alignment::Left)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(border_style)
                .title("Now Playing"),
        );
    frame.render_widget(bar, area);
}

fn region_border_style(focused: bool) -> Style {
    if focused {
        Style::default().fg(Color::Cyan)
    } else {
        Style::default().fg(Color::DarkGray)
    }
}

fn render_help_overlay(frame: &mut Frame, area: Rect) {
    let width = area.width.saturating_sub(6).min(68).max(20.min(area.width));
    let height = area
        .height
        .saturating_sub(4)
        .min((DOCUMENTED_SHORTCUTS.len() as u16) + 5)
        .max(5.min(area.height));
    let x = area.x + (area.width.saturating_sub(width)) / 2;
    let y = area.y + (area.height.saturating_sub(height)) / 2;
    let popup = Rect::new(x, y, width, height);

    frame.render_widget(Clear, popup);

    let mut lines: Vec<Line> = DOCUMENTED_SHORTCUTS
        .iter()
        .map(|s| {
            Line::from(vec![
                Span::styled(
                    format!("{:<18}", s.key),
                    Style::default()
                        .fg(Color::Cyan)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::styled(
                    format!("{:<15}", format!("[{}]", s.scope)),
                    Style::default().fg(Color::Yellow),
                ),
                Span::styled(s.description, Style::default().fg(Color::White)),
            ])
        })
        .collect();
    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        "Press ?, Esc, or q to close help",
        Style::default().fg(Color::DarkGray),
    )));

    let help = Paragraph::new(lines).alignment(Alignment::Left).block(
        Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::Cyan))
            .title("Keyboard Shortcuts"),
    );
    frame.render_widget(help, popup);
}
