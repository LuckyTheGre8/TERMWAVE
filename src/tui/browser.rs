//! Interactive music library browser state and database query integration.

use crate::library::{
    AlbumRecord, ArtistRecord, FavoriteTrack, LibraryDatabase, LibraryError, PlaybackHistoryEntry,
    TrackRecord,
};

use super::layout::NavSection;

/// Bounded cursor for navigable list views.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ListCursor {
    selected: usize,
}

impl ListCursor {
    /// Returns the currently selected index (clamped to `0` when `len == 0`).
    pub const fn index(self, len: usize) -> usize {
        if len == 0 {
            0
        } else if self.selected >= len {
            len - 1
        } else {
            self.selected
        }
    }

    /// Clamps the stored cursor index after list contents change.
    pub fn clamp(&mut self, len: usize) {
        self.selected = self.index(len);
    }

    /// Moves selection down by one item (clamped at the end).
    pub fn next(&mut self, len: usize) {
        if len > 0 {
            let current = self.index(len);
            if current + 1 < len {
                self.selected = current + 1;
            }
        }
    }

    /// Moves selection up by one item (clamped at `0`).
    pub fn previous(&mut self, len: usize) {
        if len > 0 {
            let current = self.index(len);
            self.selected = current.saturating_sub(1);
        }
    }

    /// Moves selection down by `step` items.
    pub fn page_down(&mut self, len: usize, step: usize) {
        if len > 0 {
            let current = self.index(len);
            self.selected = current.saturating_add(step).min(len - 1);
        }
    }

    /// Moves selection up by `step` items.
    pub fn page_up(&mut self, len: usize, step: usize) {
        if len > 0 {
            let current = self.index(len);
            self.selected = current.saturating_sub(step);
        }
    }

    /// Jumps to the first item.
    pub fn first(&mut self) {
        self.selected = 0;
    }

    /// Jumps to the last item.
    pub fn last(&mut self, len: usize) {
        self.selected = len.saturating_sub(1);
    }
}

/// Holds library records and per-section selection state for the TUI browser.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct LibraryBrowser {
    /// Indexed tracks loaded from SQLite.
    pub tracks: Vec<TrackRecord>,
    /// Indexed albums loaded from SQLite.
    pub albums: Vec<AlbumRecord>,
    /// Indexed artists loaded from SQLite.
    pub artists: Vec<ArtistRecord>,
    /// Favorited tracks loaded from SQLite.
    pub favorites: Vec<FavoriteTrack>,
    /// Recently played history entries loaded from SQLite.
    pub recently_played: Vec<PlaybackHistoryEntry>,
    tracks_cursor: ListCursor,
    albums_cursor: ListCursor,
    artists_cursor: ListCursor,
    favorites_cursor: ListCursor,
    recent_cursor: ListCursor,
}

impl LibraryBrowser {
    /// Queries tracks, albums, artists, favorites, and recent history from `db`.
    pub fn load_from_database(&mut self, db: &LibraryDatabase) -> Result<(), LibraryError> {
        self.tracks = db.list_tracks()?;
        self.albums = db.list_albums()?;
        self.artists = db.list_artists()?;
        self.favorites = db.list_favorites()?;
        self.recently_played = db.recently_played_unique_tracks(50)?;

        self.tracks_cursor.clamp(self.tracks.len());
        self.albums_cursor.clamp(self.albums.len());
        self.artists_cursor.clamp(self.artists.len());
        self.favorites_cursor.clamp(self.favorites.len());
        self.recent_cursor.clamp(self.recently_played.len());

        Ok(())
    }

    /// Returns the number of items in `section`.
    pub fn section_len(&self, section: NavSection) -> usize {
        match section {
            NavSection::Library => self.tracks.len(),
            NavSection::Albums => self.albums.len(),
            NavSection::Artists => self.artists.len(),
            NavSection::Favorites => self.favorites.len(),
            NavSection::RecentlyPlayed => self.recently_played.len(),
            _ => 0,
        }
    }

    /// Returns the selected index for `section` (or `None` if the section is empty or non-list).
    pub fn selected_index(&self, section: NavSection) -> Option<usize> {
        let len = self.section_len(section);
        if len == 0 {
            return None;
        }
        let idx = match section {
            NavSection::Library => self.tracks_cursor.index(len),
            NavSection::Albums => self.albums_cursor.index(len),
            NavSection::Artists => self.artists_cursor.index(len),
            NavSection::Favorites => self.favorites_cursor.index(len),
            NavSection::RecentlyPlayed => self.recent_cursor.index(len),
            _ => return None,
        };
        Some(idx)
    }

    /// Moves selection down in `section`.
    pub fn select_next(&mut self, section: NavSection) {
        let len = self.section_len(section);
        if let Some(cursor) = self.cursor_mut(section) {
            cursor.next(len);
        }
    }

    /// Moves selection up in `section`.
    pub fn select_previous(&mut self, section: NavSection) {
        let len = self.section_len(section);
        if let Some(cursor) = self.cursor_mut(section) {
            cursor.previous(len);
        }
    }

    /// Moves selection down by `step` items in `section`.
    pub fn page_down(&mut self, section: NavSection, step: usize) {
        let len = self.section_len(section);
        if let Some(cursor) = self.cursor_mut(section) {
            cursor.page_down(len, step);
        }
    }

    /// Moves selection up by `step` items in `section`.
    pub fn page_up(&mut self, section: NavSection, step: usize) {
        let len = self.section_len(section);
        if let Some(cursor) = self.cursor_mut(section) {
            cursor.page_up(len, step);
        }
    }

    /// Jumps to the first item in `section`.
    pub fn select_first(&mut self, section: NavSection) {
        if let Some(cursor) = self.cursor_mut(section) {
            cursor.first();
        }
    }

    /// Jumps to the last item in `section`.
    pub fn select_last(&mut self, section: NavSection) {
        let len = self.section_len(section);
        if let Some(cursor) = self.cursor_mut(section) {
            cursor.last(len);
        }
    }

    /// Returns the currently selected track when viewing `Library`.
    pub fn selected_track(&self) -> Option<&TrackRecord> {
        self.selected_index(NavSection::Library)
            .and_then(|i| self.tracks.get(i))
    }

    /// Returns the currently selected album when viewing `Albums`.
    pub fn selected_album(&self) -> Option<&AlbumRecord> {
        self.selected_index(NavSection::Albums)
            .and_then(|i| self.albums.get(i))
    }

    /// Returns the currently selected artist when viewing `Artists`.
    pub fn selected_artist(&self) -> Option<&ArtistRecord> {
        self.selected_index(NavSection::Artists)
            .and_then(|i| self.artists.get(i))
    }

    /// Counts how many indexed tracks belong to `album_id`.
    pub fn track_count_for_album(&self, album_id: i64) -> usize {
        self.tracks
            .iter()
            .filter(|t| t.album_id == Some(album_id))
            .count()
    }

    /// Counts how many indexed tracks belong to `artist_id`.
    pub fn track_count_for_artist(&self, artist_id: i64) -> usize {
        self.tracks
            .iter()
            .filter(|t| t.artist_id == Some(artist_id))
            .count()
    }

    fn cursor_mut(&mut self, section: NavSection) -> Option<&mut ListCursor> {
        match section {
            NavSection::Library => Some(&mut self.tracks_cursor),
            NavSection::Albums => Some(&mut self.albums_cursor),
            NavSection::Artists => Some(&mut self.artists_cursor),
            NavSection::Favorites => Some(&mut self.favorites_cursor),
            NavSection::RecentlyPlayed => Some(&mut self.recent_cursor),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use crate::library::NewTrackRecord;

    use super::*;

    fn seed_db() -> LibraryDatabase {
        let db = LibraryDatabase::open_in_memory().unwrap();
        let a1 = db.upsert_artist("Miles Davis").unwrap();
        let a2 = db.upsert_artist("Daft Punk").unwrap();
        let alb1 = db
            .upsert_album("Kind of Blue", Some(a1.id), Some(1959), None)
            .unwrap();
        let alb2 = db
            .upsert_album("Discovery", Some(a2.id), Some(2001), None)
            .unwrap();

        let t1 = db
            .upsert_track(&NewTrackRecord {
                source_path: "/music/so_what.flac".to_string(),
                title: Some("So What".to_string()),
                artist: Some("Miles Davis".to_string()),
                album: Some("Kind of Blue".to_string()),
                album_artist: None,
                artist_id: Some(a1.id),
                album_id: Some(alb1.id),
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

        let _t2 = db
            .upsert_track(&NewTrackRecord {
                source_path: "/music/one_more_time.flac".to_string(),
                title: Some("One More Time".to_string()),
                artist: Some("Daft Punk".to_string()),
                album: Some("Discovery".to_string()),
                album_artist: None,
                artist_id: Some(a2.id),
                album_id: Some(alb2.id),
                genre: Some("Electronic".to_string()),
                duration_ms: Some(320_000),
                codec: "FLAC".to_string(),
                sample_rate: Some(44_100),
                bit_depth: Some(16),
                channels: Some(2),
                file_size: 40_000,
                track_number: Some(1),
                disc_number: Some(1),
                last_modified: Some(101),
            })
            .unwrap();

        db.add_favorite(t1.id).unwrap();
        db.record_playback_at(t1.id, 1_000, Duration::from_secs(120))
            .unwrap();
        db
    }

    #[test]
    fn loads_library_queries_and_navigates_cursors() {
        let db = seed_db();
        let mut browser = LibraryBrowser::default();
        browser.load_from_database(&db).unwrap();

        assert_eq!(browser.section_len(NavSection::Library), 2);
        assert_eq!(browser.section_len(NavSection::Albums), 2);
        assert_eq!(browser.section_len(NavSection::Artists), 2);
        assert_eq!(browser.section_len(NavSection::Favorites), 1);
        assert_eq!(browser.section_len(NavSection::RecentlyPlayed), 1);

        assert_eq!(
            browser.selected_track().and_then(|t| t.title.as_deref()),
            Some("One More Time")
        );
        browser.select_next(NavSection::Library);
        assert_eq!(
            browser.selected_track().and_then(|t| t.title.as_deref()),
            Some("So What")
        );
        browser.select_previous(NavSection::Library);
        assert_eq!(
            browser.selected_track().and_then(|t| t.title.as_deref()),
            Some("One More Time")
        );

        browser.select_last(NavSection::Albums);
        assert_eq!(browser.selected_index(NavSection::Albums), Some(1));
        browser.select_first(NavSection::Albums);
        assert_eq!(browser.selected_index(NavSection::Albums), Some(0));
    }
}
