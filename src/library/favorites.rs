//! Favorite tracks persistence and querying.

use std::path::Path;

use rusqlite::params;

use super::LibraryError;
use super::db::{LibraryDatabase, TrackRecord, row_to_track_record, unix_timestamp_secs};

/// A favorited library track along with its favorite timestamp and file availability.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FavoriteTrack {
    /// Stored track record.
    pub track: TrackRecord,
    /// Unix timestamp (seconds) when the track was marked as favorite.
    pub favorited_at: i64,
    /// Whether the underlying file currently exists as a regular file on disk.
    pub file_exists: bool,
}

impl LibraryDatabase {
    /// Marks a track as favorite by `track_id` (idempotent).
    ///
    /// Returns [`LibraryError::TrackNotFound`] if no track with `track_id` exists.
    pub fn add_favorite(&self, track_id: i64) -> Result<(), LibraryError> {
        if self.get_track_by_id(track_id)?.is_none() {
            return Err(LibraryError::TrackNotFound { track_id });
        }

        let now = unix_timestamp_secs();
        self.conn().execute(
            "INSERT OR IGNORE INTO favorites (track_id, date_added) VALUES (?1, ?2)",
            params![track_id, now],
        )?;
        Ok(())
    }

    /// Marks a track as favorite by `source_path`.
    pub fn add_favorite_by_path(&self, source_path: &str) -> Result<(), LibraryError> {
        let track =
            self.get_track_by_path(source_path)?
                .ok_or_else(|| LibraryError::PathNotFound {
                    path: source_path.into(),
                })?;
        self.add_favorite(track.id)
    }

    /// Removes a track from favorites by `track_id`, returning `true` if it was present.
    pub fn remove_favorite(&self, track_id: i64) -> Result<bool, LibraryError> {
        let affected = self.conn().execute(
            "DELETE FROM favorites WHERE track_id = ?1",
            params![track_id],
        )?;
        Ok(affected > 0)
    }

    /// Removes a track from favorites by `source_path`, returning `true` if it was present.
    pub fn remove_favorite_by_path(&self, source_path: &str) -> Result<bool, LibraryError> {
        let Some(track) = self.get_track_by_path(source_path)? else {
            return Ok(false);
        };
        self.remove_favorite(track.id)
    }

    /// Toggles favorite status for `track_id` and returns the new status (`true` = favorited).
    pub fn toggle_favorite(&self, track_id: i64) -> Result<bool, LibraryError> {
        if self.is_favorite(track_id)? {
            self.remove_favorite(track_id)?;
            Ok(false)
        } else {
            self.add_favorite(track_id)?;
            Ok(true)
        }
    }

    /// Returns `true` if `track_id` is currently marked as favorite.
    pub fn is_favorite(&self, track_id: i64) -> Result<bool, LibraryError> {
        let count: i64 = self.conn().query_row(
            "SELECT COUNT(*) FROM favorites WHERE track_id = ?1",
            params![track_id],
            |row| row.get(0),
        )?;
        Ok(count > 0)
    }

    /// Returns the total number of favorited tracks.
    pub fn count_favorites(&self) -> Result<usize, LibraryError> {
        let count: i64 = self
            .conn()
            .query_row("SELECT COUNT(*) FROM favorites", [], |row| row.get(0))?;
        Ok(count.max(0) as usize)
    }

    /// Lists all favorited tracks ordered by most recently favorited first,
    /// annotating whether each backing file currently exists on disk.
    pub fn list_favorites(&self) -> Result<Vec<FavoriteTrack>, LibraryError> {
        let mut stmt = self.conn().prepare(
            "SELECT t.id, t.source_path, t.title, t.artist, t.album, t.album_artist,
                    t.artist_id, t.album_id, t.genre, t.duration_ms, t.codec,
                    t.sample_rate, t.bit_depth, t.channels, t.file_size,
                    t.track_number, t.disc_number, t.date_added, t.last_modified,
                    f.date_added
             FROM favorites f
             INNER JOIN tracks t ON t.id = f.track_id
             ORDER BY f.date_added DESC, t.id DESC",
        )?;

        let rows = stmt.query_map([], |row| {
            let track = row_to_track_record(row)?;
            let favorited_at: i64 = row.get(19)?;
            let file_exists = Path::new(&track.source_path).is_file();
            Ok(FavoriteTrack {
                track,
                favorited_at,
                file_exists,
            })
        })?;

        rows.collect::<Result<Vec<_>, _>>()
            .map_err(LibraryError::from)
    }

    /// Lists only favorited tracks whose underlying audio file currently exists on disk.
    pub fn list_available_favorites(&self) -> Result<Vec<FavoriteTrack>, LibraryError> {
        Ok(self
            .list_favorites()?
            .into_iter()
            .filter(|fav| fav.file_exists)
            .collect())
    }

    /// Removes favorite entries whose underlying audio file no longer exists on disk,
    /// returning the number of pruned favorites.
    pub fn prune_missing_favorites(&self) -> Result<usize, LibraryError> {
        let favorites = self.list_favorites()?;
        let mut pruned = 0usize;
        for fav in favorites {
            if !fav.file_exists && self.remove_favorite(fav.track.id)? {
                pruned += 1;
            }
        }
        Ok(pruned)
    }
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::PathBuf;

    use super::*;
    use crate::library::db::NewTrackRecord;

    fn fixture_path(name: &str) -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("tests")
            .join("fixtures")
            .join(name)
    }

    fn make_track(path: &str, title: &str) -> NewTrackRecord {
        NewTrackRecord {
            source_path: path.to_string(),
            title: Some(title.to_string()),
            artist: Some("Artist".to_string()),
            album: Some("Album".to_string()),
            album_artist: None,
            artist_id: None,
            album_id: None,
            genre: None,
            duration_ms: Some(120_000),
            codec: "FLAC".to_string(),
            sample_rate: Some(44_100),
            bit_depth: Some(16),
            channels: Some(2),
            file_size: 10_000,
            track_number: Some(1),
            disc_number: Some(1),
            last_modified: Some(100),
        }
    }

    #[test]
    fn adds_removes_toggles_and_persists_favorites() {
        let temp_dir = std::env::temp_dir().join("termwave_phase22_fav_persist");
        let _ = fs::remove_dir_all(&temp_dir);
        let db_path = temp_dir.join("library.sqlite");

        let track_id = {
            let db = LibraryDatabase::open(&db_path).unwrap();
            let t = db
                .upsert_track(&make_track("/music/fav_song.flac", "Fav Song"))
                .unwrap();

            assert!(!db.is_favorite(t.id).unwrap());
            db.add_favorite(t.id).unwrap();
            // Idempotent re-add
            db.add_favorite(t.id).unwrap();
            assert!(db.is_favorite(t.id).unwrap());
            assert_eq!(db.count_favorites().unwrap(), 1);
            t.id
        };

        // Reopen database and verify persistence
        {
            let db = LibraryDatabase::open(&db_path).unwrap();
            assert!(db.is_favorite(track_id).unwrap());
            let favs = db.list_favorites().unwrap();
            assert_eq!(favs.len(), 1);
            assert_eq!(favs[0].track.title.as_deref(), Some("Fav Song"));

            // Toggle off and back on
            assert!(!db.toggle_favorite(track_id).unwrap());
            assert_eq!(db.count_favorites().unwrap(), 0);
            assert!(db.toggle_favorite(track_id).unwrap());
            assert_eq!(db.count_favorites().unwrap(), 1);

            assert!(db.remove_favorite_by_path("/music/fav_song.flac").unwrap());
            assert!(!db.remove_favorite(track_id).unwrap());
            assert_eq!(db.count_favorites().unwrap(), 0);
        }

        let _ = fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn handles_missing_tracks_and_missing_files_cleanly() {
        let db = LibraryDatabase::open_in_memory().unwrap();

        // Adding favorite for non-existent track ID returns TrackNotFound
        let err = db.add_favorite(999_999).unwrap_err();
        assert!(matches!(
            err,
            LibraryError::TrackNotFound { track_id: 999_999 }
        ));

        // Create one real file on disk and one missing path
        let real_path = fixture_path("sine_16bit_44100_stereo.flac");
        let real_str = real_path.to_string_lossy().into_owned();
        let missing_str = "C:/nonexistent_termwave_missing_999.flac";

        let real_track = db
            .upsert_track(&make_track(&real_str, "Existing Track"))
            .unwrap();
        let missing_track = db
            .upsert_track(&make_track(missing_str, "Missing Track"))
            .unwrap();

        db.add_favorite(real_track.id).unwrap();
        db.add_favorite(missing_track.id).unwrap();

        let all_favs = db.list_favorites().unwrap();
        assert_eq!(all_favs.len(), 2);
        assert!(
            all_favs
                .iter()
                .any(|f| f.track.id == real_track.id && f.file_exists)
        );
        assert!(
            all_favs
                .iter()
                .any(|f| f.track.id == missing_track.id && !f.file_exists)
        );

        let available = db.list_available_favorites().unwrap();
        assert_eq!(available.len(), 1);
        assert_eq!(available[0].track.id, real_track.id);

        let pruned = db.prune_missing_favorites().unwrap();
        assert_eq!(pruned, 1);
        assert_eq!(db.count_favorites().unwrap(), 1);
    }
}
