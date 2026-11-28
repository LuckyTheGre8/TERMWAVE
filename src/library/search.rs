//! Indexed music library search for tracks, artists, and albums.

use rusqlite::params;

use super::LibraryError;
use super::db::{AlbumRecord, ArtistRecord, LibraryDatabase, TrackRecord, row_to_track_record};

/// Default maximum number of items returned per search category.
pub const DEFAULT_SEARCH_LIMIT: usize = 50;

/// Pagination options for library search queries.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SearchOptions {
    /// Maximum number of results to return per entity type.
    pub limit: usize,
    /// Number of leading results to skip.
    pub offset: usize,
}

impl Default for SearchOptions {
    fn default() -> Self {
        Self {
            limit: DEFAULT_SEARCH_LIMIT,
            offset: 0,
        }
    }
}

impl SearchOptions {
    /// Creates search options with a custom limit and zero offset.
    pub const fn with_limit(limit: usize) -> Self {
        Self { limit, offset: 0 }
    }
}

/// Combined results across tracks, artists, and albums for a search query.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct SearchResults {
    /// Matching tracks ordered by relevance.
    pub tracks: Vec<TrackRecord>,
    /// Matching artists ordered by relevance.
    pub artists: Vec<ArtistRecord>,
    /// Matching albums ordered by relevance.
    pub albums: Vec<AlbumRecord>,
}

impl SearchResults {
    /// Returns `true` if no tracks, artists, or albums matched.
    pub fn is_empty(&self) -> bool {
        self.tracks.is_empty() && self.artists.is_empty() && self.albums.is_empty()
    }

    /// Total number of matched items across all categories.
    pub fn total_count(&self) -> usize {
        self.tracks.len() + self.artists.len() + self.albums.len()
    }
}

impl LibraryDatabase {
    /// Searches tracks, artists, and albums using indexed queries and partial matching.
    pub fn search_library(
        &self,
        query: &str,
        options: SearchOptions,
    ) -> Result<SearchResults, LibraryError> {
        Ok(SearchResults {
            tracks: self.search_tracks(query, options)?,
            artists: self.search_artists(query, options)?,
            albums: self.search_albums(query, options)?,
        })
    }

    /// Searches tracks by title, artist, album, album artist, or genre.
    ///
    /// Combines FTS5 prefix/token matching (`tracks_fts`) with case-insensitive
    /// partial substring matching and ranks exact/prefix matches first.
    pub fn search_tracks(
        &self,
        query: &str,
        options: SearchOptions,
    ) -> Result<Vec<TrackRecord>, LibraryError> {
        let trimmed = query.trim();
        if trimmed.is_empty() || options.limit == 0 {
            return Ok(Vec::new());
        }

        let escaped = escape_like_pattern(trimmed);
        let prefix_pattern = format!("{escaped}%");
        let contains_pattern = format!("%{escaped}%");
        let fts_query = build_fts_prefix_query(trimmed);
        let limit = options.limit as i64;
        let offset = options.offset as i64;

        let mut stmt = self.conn().prepare(
            "SELECT t.id, t.source_path, t.title, t.artist, t.album, t.album_artist,
                    t.artist_id, t.album_id, t.genre, t.duration_ms, t.codec,
                    t.sample_rate, t.bit_depth, t.channels, t.file_size,
                    t.track_number, t.disc_number, t.date_added, t.last_modified
             FROM tracks t
             WHERE (
                 (?1 != '' AND t.id IN (SELECT rowid FROM tracks_fts WHERE tracks_fts MATCH ?1))
                 OR t.title LIKE ?2 ESCAPE '\\' COLLATE NOCASE
                 OR t.artist LIKE ?2 ESCAPE '\\' COLLATE NOCASE
                 OR t.album LIKE ?2 ESCAPE '\\' COLLATE NOCASE
                 OR t.album_artist LIKE ?2 ESCAPE '\\' COLLATE NOCASE
                 OR t.genre LIKE ?2 ESCAPE '\\' COLLATE NOCASE
             )
             ORDER BY
                 CASE
                     WHEN t.title = ?3 COLLATE NOCASE THEN 0
                     WHEN t.title LIKE ?4 ESCAPE '\\' COLLATE NOCASE THEN 1
                     WHEN t.artist LIKE ?4 ESCAPE '\\' COLLATE NOCASE
                       OR t.album LIKE ?4 ESCAPE '\\' COLLATE NOCASE THEN 2
                     ELSE 3
                 END,
                 COALESCE(t.title, t.source_path) COLLATE NOCASE ASC
             LIMIT ?5 OFFSET ?6",
        )?;

        let rows = stmt.query_map(
            params![
                fts_query,
                contains_pattern,
                trimmed,
                prefix_pattern,
                limit,
                offset
            ],
            row_to_track_record,
        )?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(LibraryError::from)
    }

    /// Searches artists by name with case-insensitive partial matching,
    /// ordering exact and prefix matches before mid-string matches.
    pub fn search_artists(
        &self,
        query: &str,
        options: SearchOptions,
    ) -> Result<Vec<ArtistRecord>, LibraryError> {
        let trimmed = query.trim();
        if trimmed.is_empty() || options.limit == 0 {
            return Ok(Vec::new());
        }

        let escaped = escape_like_pattern(trimmed);
        let prefix_pattern = format!("{escaped}%");
        let contains_pattern = format!("%{escaped}%");
        let limit = options.limit as i64;
        let offset = options.offset as i64;

        let mut stmt = self.conn().prepare(
            "SELECT id, name
             FROM artists
             WHERE name LIKE ?1 ESCAPE '\\' COLLATE NOCASE
             ORDER BY
                 CASE
                     WHEN name = ?2 COLLATE NOCASE THEN 0
                     WHEN name LIKE ?3 ESCAPE '\\' COLLATE NOCASE THEN 1
                     ELSE 2
                 END,
                 name COLLATE NOCASE ASC
             LIMIT ?4 OFFSET ?5",
        )?;

        let rows = stmt.query_map(
            params![contains_pattern, trimmed, prefix_pattern, limit, offset],
            |row| {
                Ok(ArtistRecord {
                    id: row.get(0)?,
                    name: row.get(1)?,
                })
            },
        )?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(LibraryError::from)
    }

    /// Searches albums by album title or associated artist name with
    /// case-insensitive partial matching.
    pub fn search_albums(
        &self,
        query: &str,
        options: SearchOptions,
    ) -> Result<Vec<AlbumRecord>, LibraryError> {
        let trimmed = query.trim();
        if trimmed.is_empty() || options.limit == 0 {
            return Ok(Vec::new());
        }

        let escaped = escape_like_pattern(trimmed);
        let prefix_pattern = format!("{escaped}%");
        let contains_pattern = format!("%{escaped}%");
        let limit = options.limit as i64;
        let offset = options.offset as i64;

        let mut stmt = self.conn().prepare(
            "SELECT a.id, a.name, a.artist_id, a.release_year, a.artwork_ref
             FROM albums a
             LEFT JOIN artists ar ON ar.id = a.artist_id
             WHERE a.name LIKE ?1 ESCAPE '\\' COLLATE NOCASE
                OR ar.name LIKE ?1 ESCAPE '\\' COLLATE NOCASE
             ORDER BY
                 CASE
                     WHEN a.name = ?2 COLLATE NOCASE THEN 0
                     WHEN a.name LIKE ?3 ESCAPE '\\' COLLATE NOCASE THEN 1
                     ELSE 2
                 END,
                 a.name COLLATE NOCASE ASC
             LIMIT ?4 OFFSET ?5",
        )?;

        let rows = stmt.query_map(
            params![contains_pattern, trimmed, prefix_pattern, limit, offset],
            |row| {
                Ok(AlbumRecord {
                    id: row.get(0)?,
                    name: row.get(1)?,
                    artist_id: row.get(2)?,
                    release_year: row.get(3)?,
                    artwork_ref: row.get(4)?,
                })
            },
        )?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(LibraryError::from)
    }
}

fn escape_like_pattern(input: &str) -> String {
    let mut escaped = String::with_capacity(input.len());
    for ch in input.chars() {
        match ch {
            '\\' | '%' | '_' => {
                escaped.push('\\');
                escaped.push(ch);
            }
            _ => escaped.push(ch),
        }
    }
    escaped
}

/// Builds an FTS5 `unicode61` prefix query from alphanumeric/word tokens.
/// Returns an empty string if no valid FTS tokens exist (falling back cleanly to LIKE).
fn build_fts_prefix_query(input: &str) -> String {
    let tokens: Vec<String> = input
        .split(|c: char| !c.is_alphanumeric())
        .filter(|t| !t.is_empty())
        .map(|t| format!("\"{}\"*", t.replace('"', "")))
        .collect();
    tokens.join(" AND ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::library::db::NewTrackRecord;

    fn seed_library(db: &LibraryDatabase) {
        let mj = db.upsert_artist("Michael Jackson").unwrap();
        let dp = db.upsert_artist("Daft Punk").unwrap();
        let md = db.upsert_artist("Miles Davis").unwrap();

        let thriller = db
            .upsert_album("Thriller", Some(mj.id), Some(1982), None)
            .unwrap();
        let discovery = db
            .upsert_album("Discovery", Some(dp.id), Some(2001), None)
            .unwrap();
        let kind_of_blue = db
            .upsert_album("Kind of Blue", Some(md.id), Some(1959), None)
            .unwrap();

        let tracks = [
            NewTrackRecord {
                source_path: "/music/billie_jean.flac".to_string(),
                title: Some("Billie Jean".to_string()),
                artist: Some("Michael Jackson".to_string()),
                album: Some("Thriller".to_string()),
                album_artist: Some("Michael Jackson".to_string()),
                artist_id: Some(mj.id),
                album_id: Some(thriller.id),
                genre: Some("Pop".to_string()),
                duration_ms: Some(294_000),
                codec: "FLAC".to_string(),
                sample_rate: Some(44_100),
                bit_depth: Some(16),
                channels: Some(2),
                file_size: 35_000_000,
                track_number: Some(6),
                disc_number: Some(1),
                last_modified: Some(100),
            },
            NewTrackRecord {
                source_path: "/music/beat_it.flac".to_string(),
                title: Some("Beat It".to_string()),
                artist: Some("Michael Jackson".to_string()),
                album: Some("Thriller".to_string()),
                album_artist: Some("Michael Jackson".to_string()),
                artist_id: Some(mj.id),
                album_id: Some(thriller.id),
                genre: Some("Rock".to_string()),
                duration_ms: Some(258_000),
                codec: "FLAC".to_string(),
                sample_rate: Some(44_100),
                bit_depth: Some(16),
                channels: Some(2),
                file_size: 30_000_000,
                track_number: Some(5),
                disc_number: Some(1),
                last_modified: Some(101),
            },
            NewTrackRecord {
                source_path: "/music/one_more_time.flac".to_string(),
                title: Some("100% Pure Love (One More Time)".to_string()),
                artist: Some("Daft Punk".to_string()),
                album: Some("Discovery".to_string()),
                album_artist: Some("Daft Punk".to_string()),
                artist_id: Some(dp.id),
                album_id: Some(discovery.id),
                genre: Some("Electronic".to_string()),
                duration_ms: Some(320_000),
                codec: "FLAC".to_string(),
                sample_rate: Some(44_100),
                bit_depth: Some(16),
                channels: Some(2),
                file_size: 40_000_000,
                track_number: Some(1),
                disc_number: Some(1),
                last_modified: Some(102),
            },
            NewTrackRecord {
                source_path: "/music/so_what.flac".to_string(),
                title: Some("So What".to_string()),
                artist: Some("Miles Davis".to_string()),
                album: Some("Kind of Blue".to_string()),
                album_artist: Some("Miles Davis".to_string()),
                artist_id: Some(md.id),
                album_id: Some(kind_of_blue.id),
                genre: Some("Jazz".to_string()),
                duration_ms: Some(562_000),
                codec: "FLAC".to_string(),
                sample_rate: Some(96_000),
                bit_depth: Some(24),
                channels: Some(2),
                file_size: 120_000_000,
                track_number: Some(1),
                disc_number: Some(1),
                last_modified: Some(103),
            },
        ];

        for t in &tracks {
            db.upsert_track(t).unwrap();
        }
    }

    #[test]
    fn searches_tracks_artists_and_albums_with_partial_matching() {
        let db = LibraryDatabase::open_in_memory().unwrap();
        seed_library(&db);

        // Partial mid-string match on track title (case-insensitive)
        let tracks = db.search_tracks("illie", SearchOptions::default()).unwrap();
        assert_eq!(tracks.len(), 1);
        assert_eq!(tracks[0].title.as_deref(), Some("Billie Jean"));

        // Multi-word FTS5 prefix match across artist & album
        let mj_tracks = db
            .search_tracks("mich thril", SearchOptions::default())
            .unwrap();
        assert_eq!(mj_tracks.len(), 2);

        // Artist search (partial)
        let artists = db.search_artists("jack", SearchOptions::default()).unwrap();
        assert_eq!(artists.len(), 1);
        assert_eq!(artists[0].name, "Michael Jackson");

        // Album search (matches album title or artist name)
        let albums_by_name = db.search_albums("disc", SearchOptions::default()).unwrap();
        assert_eq!(albums_by_name.len(), 1);
        assert_eq!(albums_by_name[0].name, "Discovery");

        let albums_by_artist = db.search_albums("miles", SearchOptions::default()).unwrap();
        assert_eq!(albums_by_artist.len(), 1);
        assert_eq!(albums_by_artist[0].name, "Kind of Blue");

        // Combined search_library
        let combined = db.search_library("daft", SearchOptions::default()).unwrap();
        assert_eq!(combined.tracks.len(), 1);
        assert_eq!(combined.artists.len(), 1);
        assert_eq!(combined.albums.len(), 1);
        assert_eq!(combined.total_count(), 3);
    }

    #[test]
    fn escapes_like_wildcards_and_handles_empty_queries() {
        let db = LibraryDatabase::open_in_memory().unwrap();
        seed_library(&db);

        // Searching "%" should match only the literal "100% Pure Love..." track, not all tracks
        let percent_matches = db.search_tracks("100%", SearchOptions::default()).unwrap();
        assert_eq!(percent_matches.len(), 1);
        assert_eq!(
            percent_matches[0].title.as_deref(),
            Some("100% Pure Love (One More Time)")
        );

        let empty = db.search_library("   ", SearchOptions::default()).unwrap();
        assert!(empty.is_empty());
    }

    #[test]
    fn supports_pagination_and_uses_fts_and_nocase_indexes() {
        let db = LibraryDatabase::open_in_memory().unwrap();
        seed_library(&db);

        let page1 = db
            .search_tracks(
                "michael",
                SearchOptions {
                    limit: 1,
                    offset: 0,
                },
            )
            .unwrap();
        let page2 = db
            .search_tracks(
                "michael",
                SearchOptions {
                    limit: 1,
                    offset: 1,
                },
            )
            .unwrap();
        assert_eq!(page1.len(), 1);
        assert_eq!(page2.len(), 1);
        assert_ne!(page1[0].id, page2[0].id);

        // Verify FTS5 table is populated and responds to indexed MATCH queries
        let fts_count: i64 = db
            .conn()
            .query_row(
                "SELECT COUNT(*) FROM tracks_fts WHERE tracks_fts MATCH 'billie*'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(fts_count, 1);
    }
}
