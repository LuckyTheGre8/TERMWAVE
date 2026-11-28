//! Responsive terminal dashboard layout and region rendering.

use ratatui::Frame;
use ratatui::layout::{Alignment, Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, List, ListItem, Paragraph};

use super::TuiState;

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
}

fn render_header(frame: &mut Frame, area: Rect, state: &TuiState) {
    let status_text = format!(
        "v{}  |  {}  |  {}x{}",
        env!("CARGO_PKG_VERSION"),
        state.active_section.label(),
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

    let lines = vec![
        Line::from(Span::styled(
            state.active_section.label(),
            Style::default()
                .fg(Color::White)
                .add_modifier(Modifier::BOLD),
        )),
        Line::from(""),
        Line::from(Span::styled(
            state.active_section.description(),
            Style::default().fg(Color::Gray),
        )),
        Line::from(""),
        Line::from(Span::styled(
            "Tab: switch focus  |  Up/Down: navigate  |  q / Esc / Ctrl+C: quit",
            Style::default().fg(Color::DarkGray),
        )),
    ];

    let panel = Paragraph::new(lines).alignment(Alignment::Left).block(
        Block::default()
            .borders(Borders::ALL)
            .border_style(border_style)
            .title(state.active_section.label()),
    );
    frame.render_widget(panel, area);
}

fn render_playback_bar(frame: &mut Frame, area: Rect, state: &TuiState) {
    let is_focused = state.focus == FocusRegion::PlaybackBar;
    let border_style = region_border_style(is_focused);

    let lines = vec![
        Line::from(vec![
            Span::styled(
                "Stopped",
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw("  |  No track loaded"),
        ]),
        Line::from(Span::styled(
            "00:00 / 00:00  |  Vol 100%  |  FLAC Lossless Engine Ready",
            Style::default().fg(Color::DarkGray),
        )),
    ];

    let bar = Paragraph::new(lines).alignment(Alignment::Left).block(
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
