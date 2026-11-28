//! Playback history persistence, recently played queries, and batched write buffering.

use std::time::Duration;

use rusqlite::params;

use super::LibraryError;
use super::db::{LibraryDatabase, TrackRecord, row_to_track_record, unix_timestamp_secs};

/// Default minimum playback duration before a track play is recorded in history.
pub const DEFAULT_MIN_HISTORY_DURATION: Duration = Duration::from_secs(5);

/// Default number of buffered history entries before automatically flushing in one transaction.
pub const DEFAULT_HISTORY_BATCH_SIZE: usize = 16;

/// A recorded playback history entry joined with its track metadata.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlaybackHistoryEntry {
    /// History row primary key.
    pub id: i64,
    /// Track that was played.
    pub track: TrackRecord,
    /// Unix timestamp (seconds) when the playback was recorded.
    pub played_at: i64,
    /// Playback duration in milliseconds.
    pub playback_duration_ms: u64,
}

/// Pending history entry queued for batched insertion.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PendingHistoryEntry {
    /// Target track ID in `tracks`.
    pub track_id: i64,
    /// Unix timestamp (seconds) when the track was played.
    pub played_at: i64,
    /// Playback duration in milliseconds.
    pub playback_duration_ms: u64,
}

/// Policy controlling when and how playback history is persisted to avoid excessive writes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HistoryPolicy {
    /// Minimum playback duration required to record an entry (ignores brief skips).
    pub min_duration: Duration,
    /// Maximum number of entries buffered in memory before triggering a batched flush.
    pub max_buffered: usize,
}

impl Default for HistoryPolicy {
    fn default() -> Self {
        Self {
            min_duration: DEFAULT_MIN_HISTORY_DURATION,
            max_buffered: DEFAULT_HISTORY_BATCH_SIZE,
        }
    }
}

/// In-memory write-coalescing buffer that avoids per-tick SQLite writes by
/// filtering short skips and flushing entries in a single transaction.
#[derive(Debug, Clone)]
pub struct HistoryRecorder {
    policy: HistoryPolicy,
    pending: Vec<PendingHistoryEntry>,
}

impl HistoryRecorder {
    /// Creates a new recorder with the given write-throttling [`HistoryPolicy`].
    pub fn new(policy: HistoryPolicy) -> Self {
        Self {
            policy,
            pending: Vec::new(),
        }
    }

    /// Returns the number of entries currently buffered in memory.
    pub fn pending_count(&self) -> usize {
        self.pending.len()
    }

    /// Queues a track play if `playback_duration` meets `policy.min_duration`.
    ///
    /// Returns `true` if the entry was buffered (or `false` if skipped as too short).
    pub fn queue_play(&mut self, track_id: i64, playback_duration: Duration) -> bool {
        self.queue_play_at(track_id, unix_timestamp_secs(), playback_duration)
    }

    /// Queues a track play with an explicit timestamp if `playback_duration` meets `policy.min_duration`.
    pub fn queue_play_at(
        &mut self,
        track_id: i64,
        played_at: i64,
        playback_duration: Duration,
    ) -> bool {
        if playback_duration < self.policy.min_duration {
            return false;
        }
        let duration_ms = playback_duration.as_millis() as u64;
        self.pending.push(PendingHistoryEntry {
            track_id,
            played_at,
            playback_duration_ms: duration_ms,
        });
        true
    }

    /// Flushes buffered entries to `db` only if `pending.len() >= policy.max_buffered`.
    pub fn flush_if_needed(&mut self, db: &mut LibraryDatabase) -> Result<usize, LibraryError> {
        if self.pending.len() >= self.policy.max_buffered.max(1) {
            self.flush(db)
        } else {
            Ok(0)
        }
    }

    /// Flushes all buffered history entries to `db` inside a single SQLite transaction.
    pub fn flush(&mut self, db: &mut LibraryDatabase) -> Result<usize, LibraryError> {
        if self.pending.is_empty() {
            return Ok(0);
        }
        let written = db.record_playback_batch(&self.pending)?;
        self.pending.clear();
        Ok(written)
    }
}

impl Default for HistoryRecorder {
    fn default() -> Self {
        Self::new(HistoryPolicy::default())
    }
}

impl LibraryDatabase {
    /// Records a single track play at the current timestamp.
    pub fn record_playback(
        &self,
        track_id: i64,
        playback_duration: Duration,
    ) -> Result<i64, LibraryError> {
        self.record_playback_at(track_id, unix_timestamp_secs(), playback_duration)
    }

    /// Records a single track play with an explicit Unix timestamp (seconds).
    pub fn record_playback_at(
        &self,
        track_id: i64,
        played_at: i64,
        playback_duration: Duration,
    ) -> Result<i64, LibraryError> {
        if self.get_track_by_id(track_id)?.is_none() {
            return Err(LibraryError::TrackNotFound { track_id });
        }

        let duration_ms = playback_duration.as_millis() as i64;
        self.conn().execute(
            "INSERT INTO playback_history (track_id, played_at, playback_duration_ms)
             VALUES (?1, ?2, ?3)",
            params![track_id, played_at, duration_ms],
        )?;
        Ok(self.conn().last_insert_rowid())
    }

    /// Records a single track play by `source_path`.
    pub fn record_playback_by_path(
        &self,
        source_path: &str,
        playback_duration: Duration,
    ) -> Result<i64, LibraryError> {
        let track =
            self.get_track_by_path(source_path)?
                .ok_or_else(|| LibraryError::PathNotFound {
                    path: source_path.into(),
                })?;
        self.record_playback(track.id, playback_duration)
    }

    /// Inserts multiple playback history entries inside a single SQLite transaction.
    pub fn record_playback_batch(
        &mut self,
        entries: &[PendingHistoryEntry],
    ) -> Result<usize, LibraryError> {
        if entries.is_empty() {
            return Ok(0);
        }

        self.with_transaction(|tx| {
            let mut stmt = tx.prepare(
                "INSERT INTO playback_history (track_id, played_at, playback_duration_ms)
                 VALUES (?1, ?2, ?3)",
            )?;
            let mut count = 0usize;
            for entry in entries {
                let exists: i64 = tx.query_row(
                    "SELECT COUNT(*) FROM tracks WHERE id = ?1",
                    params![entry.track_id],
                    |row| row.get(0),
                )?;
                if exists == 0 {
                    return Err(LibraryError::TrackNotFound {
                        track_id: entry.track_id,
                    });
                }
                stmt.execute(params![
                    entry.track_id,
                    entry.played_at,
                    entry.playback_duration_ms as i64
                ])?;
                count += 1;
            }
            Ok(count)
        })
    }

    /// Queries the most recently played history entries (newest first).
    pub fn recently_played(&self, limit: usize) -> Result<Vec<PlaybackHistoryEntry>, LibraryError> {
        if limit == 0 {
            return Ok(Vec::new());
        }

        let mut stmt = self.conn().prepare(
            "SELECT t.id, t.source_path, t.title, t.artist, t.album, t.album_artist,
                    t.artist_id, t.album_id, t.genre, t.duration_ms, t.codec,
                    t.sample_rate, t.bit_depth, t.channels, t.file_size,
                    t.track_number, t.disc_number, t.date_added, t.last_modified,
                    h.id, h.played_at, h.playback_duration_ms
             FROM playback_history h
             INNER JOIN tracks t ON t.id = h.track_id
             ORDER BY h.played_at DESC, h.id DESC
             LIMIT ?1",
        )?;

        let rows = stmt.query_map(params![limit as i64], |row| {
            let track = row_to_track_record(row)?;
            let history_id: i64 = row.get(19)?;
            let played_at: i64 = row.get(20)?;
            let duration_ms: i64 = row.get(21)?;
            Ok(PlaybackHistoryEntry {
                id: history_id,
                track,
                played_at,
                playback_duration_ms: duration_ms.max(0) as u64,
            })
        })?;

        rows.collect::<Result<Vec<_>, _>>()
            .map_err(LibraryError::from)
    }

    /// Queries the most recently played distinct tracks (newest play per track first).
    pub fn recently_played_unique_tracks(
        &self,
        limit: usize,
    ) -> Result<Vec<PlaybackHistoryEntry>, LibraryError> {
        if limit == 0 {
            return Ok(Vec::new());
        }

        let mut stmt = self.conn().prepare(
            "SELECT t.id, t.source_path, t.title, t.artist, t.album, t.album_artist,
                    t.artist_id, t.album_id, t.genre, t.duration_ms, t.codec,
                    t.sample_rate, t.bit_depth, t.channels, t.file_size,
                    t.track_number, t.disc_number, t.date_added, t.last_modified,
                    h.id, h.played_at, h.playback_duration_ms
             FROM playback_history h
             INNER JOIN tracks t ON t.id = h.track_id
             WHERE h.id = (
                 SELECT h2.id
                 FROM playback_history h2
                 WHERE h2.track_id = h.track_id
                 ORDER BY h2.played_at DESC, h2.id DESC
                 LIMIT 1
             )
             ORDER BY h.played_at DESC, h.id DESC
             LIMIT ?1",
        )?;

        let rows = stmt.query_map(params![limit as i64], |row| {
            let track = row_to_track_record(row)?;
            let history_id: i64 = row.get(19)?;
            let played_at: i64 = row.get(20)?;
            let duration_ms: i64 = row.get(21)?;
            Ok(PlaybackHistoryEntry {
                id: history_id,
                track,
                played_at,
                playback_duration_ms: duration_ms.max(0) as u64,
            })
        })?;

        rows.collect::<Result<Vec<_>, _>>()
            .map_err(LibraryError::from)
    }

    /// Returns the total number of recorded playback history entries.
    pub fn count_playback_history(&self) -> Result<usize, LibraryError> {
        let count: i64 =
            self.conn()
                .query_row("SELECT COUNT(*) FROM playback_history", [], |row| {
                    row.get(0)
                })?;
        Ok(count.max(0) as usize)
    }

    /// Clears all playback history entries and returns the number of removed rows.
    pub fn clear_playback_history(&self) -> Result<usize, LibraryError> {
        let affected = self.conn().execute("DELETE FROM playback_history", [])?;
        Ok(affected)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::library::db::NewTrackRecord;

    fn seed_track(db: &LibraryDatabase, path: &str, title: &str) -> TrackRecord {
        db.upsert_track(&NewTrackRecord {
            source_path: path.to_string(),
            title: Some(title.to_string()),
            artist: Some("Artist".to_string()),
            album: Some("Album".to_string()),
            album_artist: None,
            artist_id: None,
            album_id: None,
            genre: None,
            duration_ms: Some(200_000),
            codec: "FLAC".to_string(),
            sample_rate: Some(44_100),
            bit_depth: Some(16),
            channels: Some(2),
            file_size: 20_000,
            track_number: Some(1),
            disc_number: Some(1),
            last_modified: Some(100),
        })
        .unwrap()
    }

    #[test]
    fn records_and_queries_recently_played_tracks_and_unique_tracks() {
        let db = LibraryDatabase::open_in_memory().unwrap();
        let t1 = seed_track(&db, "/music/one.flac", "Track One");
        let t2 = seed_track(&db, "/music/two.flac", "Track Two");

        db.record_playback_at(t1.id, 1_000, Duration::from_secs(180))
            .unwrap();
        db.record_playback_at(t2.id, 2_000, Duration::from_secs(90))
            .unwrap();
        db.record_playback_at(t1.id, 3_000, Duration::from_secs(200))
            .unwrap();

        assert_eq!(db.count_playback_history().unwrap(), 3);

        let recent = db.recently_played(10).unwrap();
        assert_eq!(recent.len(), 3);
        assert_eq!(recent[0].track.id, t1.id);
        assert_eq!(recent[0].played_at, 3_000);
        assert_eq!(recent[0].playback_duration_ms, 200_000);
        assert_eq!(recent[1].track.id, t2.id);
        assert_eq!(recent[2].track.id, t1.id);

        let unique = db.recently_played_unique_tracks(10).unwrap();
        assert_eq!(unique.len(), 2);
        assert_eq!(unique[0].track.id, t1.id);
        assert_eq!(unique[0].played_at, 3_000);
        assert_eq!(unique[1].track.id, t2.id);

        assert_eq!(db.clear_playback_history().unwrap(), 3);
        assert_eq!(db.count_playback_history().unwrap(), 0);
    }

    #[test]
    fn history_recorder_filters_short_skips_and_batches_writes() {
        let mut db = LibraryDatabase::open_in_memory().unwrap();
        let t1 = seed_track(&db, "/music/track.flac", "Track");

        let mut recorder = HistoryRecorder::new(HistoryPolicy {
            min_duration: Duration::from_secs(5),
            max_buffered: 2,
        });

        // Short skip (< 5s) is ignored without buffering or writing
        assert!(!recorder.queue_play_at(t1.id, 100, Duration::from_secs(2)));
        assert_eq!(recorder.pending_count(), 0);

        // First valid play is buffered, not flushed yet (below max_buffered = 2)
        assert!(recorder.queue_play_at(t1.id, 200, Duration::from_secs(45)));
        assert_eq!(recorder.pending_count(), 1);
        assert_eq!(recorder.flush_if_needed(&mut db).unwrap(), 0);
        assert_eq!(db.count_playback_history().unwrap(), 0);

        // Second valid play reaches batch threshold and flushes in one transaction
        assert!(recorder.queue_play_at(t1.id, 300, Duration::from_secs(60)));
        assert_eq!(recorder.flush_if_needed(&mut db).unwrap(), 2);
        assert_eq!(recorder.pending_count(), 0);
        assert_eq!(db.count_playback_history().unwrap(), 2);
    }

    #[test]
    fn rejects_unknown_track_ids_cleanly() {
        let db = LibraryDatabase::open_in_memory().unwrap();
        let err = db
            .record_playback(999_999, Duration::from_secs(10))
            .unwrap_err();
        assert!(matches!(
            err,
            LibraryError::TrackNotFound { track_id: 999_999 }
        ));
    }
}
