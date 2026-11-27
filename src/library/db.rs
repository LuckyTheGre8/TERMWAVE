//! SQLite persistence layer, schema migrations, and database access API.

use std::fs;
use std::path::Path;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use rusqlite::{Connection, OptionalExtension, Transaction, params};

use super::LibraryError;

/// A single versioned SQLite schema migration.
struct Migration {
    version: u32,
    name: &'static str,
    sql: &'static str,
}

/// Ordered list of schema migrations applied to the library database.
const MIGRATIONS: &[Migration] = &[Migration {
    version: 1,
    name: "create_core_library_tables",
    sql: "
        CREATE TABLE IF NOT EXISTS artists (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            name TEXT NOT NULL UNIQUE
        );

        CREATE TABLE IF NOT EXISTS albums (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            name TEXT NOT NULL,
            artist_id INTEGER REFERENCES artists(id) ON DELETE SET NULL,
            release_year INTEGER,
            artwork_ref TEXT,
            UNIQUE(name, artist_id)
        );

        CREATE TABLE IF NOT EXISTS tracks (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            source_path TEXT NOT NULL UNIQUE,
            title TEXT,
            artist TEXT,
            album TEXT,
            album_artist TEXT,
            artist_id INTEGER REFERENCES artists(id) ON DELETE SET NULL,
            album_id INTEGER REFERENCES albums(id) ON DELETE SET NULL,
            genre TEXT,
            duration_ms INTEGER,
            codec TEXT NOT NULL,
            sample_rate INTEGER,
            bit_depth INTEGER,
            channels INTEGER,
            file_size INTEGER NOT NULL,
            track_number INTEGER,
            disc_number INTEGER,
            date_added INTEGER NOT NULL,
            last_modified INTEGER
        );

        CREATE INDEX IF NOT EXISTS idx_tracks_artist_id ON tracks(artist_id);
        CREATE INDEX IF NOT EXISTS idx_tracks_album_id ON tracks(album_id);
        CREATE INDEX IF NOT EXISTS idx_tracks_title ON tracks(title);
        CREATE INDEX IF NOT EXISTS idx_tracks_artist ON tracks(artist);
        CREATE INDEX IF NOT EXISTS idx_tracks_album ON tracks(album);
        CREATE INDEX IF NOT EXISTS idx_albums_artist_id ON albums(artist_id);
    ",
}];

/// Stored artist row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArtistRecord {
    /// Database primary key.
    pub id: i64,
    /// Artist display name.
    pub name: String,
}

/// Stored album row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AlbumRecord {
    /// Database primary key.
    pub id: i64,
    /// Album title.
    pub name: String,
    /// Optional foreign key referencing [`ArtistRecord`].
    pub artist_id: Option<i64>,
    /// Optional release year.
    pub release_year: Option<u32>,
    /// Optional artwork reference or content hash.
    pub artwork_ref: Option<String>,
}

/// Input parameters for inserting or updating a track row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewTrackRecord {
    /// Unique local file path or source identifier.
    pub source_path: String,
    /// Track title.
    pub title: Option<String>,
    /// Track artist name.
    pub artist: Option<String>,
    /// Album title.
    pub album: Option<String>,
    /// Album artist name.
    pub album_artist: Option<String>,
    /// Optional foreign key to `artists`.
    pub artist_id: Option<i64>,
    /// Optional foreign key to `albums`.
    pub album_id: Option<i64>,
    /// Genre tag.
    pub genre: Option<String>,
    /// Track duration in milliseconds.
    pub duration_ms: Option<u64>,
    /// Audio codec name (e.g. `"FLAC"`).
    pub codec: String,
    /// Source sample rate in Hz.
    pub sample_rate: Option<u32>,
    /// Source bit depth.
    pub bit_depth: Option<u32>,
    /// Audio channel count.
    pub channels: Option<u16>,
    /// File size in bytes.
    pub file_size: u64,
    /// Track number within album/disc.
    pub track_number: Option<u32>,
    /// Disc number.
    pub disc_number: Option<u32>,
    /// Last modified Unix timestamp in seconds.
    pub last_modified: Option<i64>,
}

/// Stored track row from the `tracks` table.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrackRecord {
    /// Database primary key.
    pub id: i64,
    /// Unique local file path or source identifier.
    pub source_path: String,
    /// Track title.
    pub title: Option<String>,
    /// Track artist name.
    pub artist: Option<String>,
    /// Album title.
    pub album: Option<String>,
    /// Album artist name.
    pub album_artist: Option<String>,
    /// Foreign key to `artists`.
    pub artist_id: Option<i64>,
    /// Foreign key to `albums`.
    pub album_id: Option<i64>,
    /// Genre tag.
    pub genre: Option<String>,
    /// Track duration in milliseconds.
    pub duration_ms: Option<u64>,
    /// Audio codec name.
    pub codec: String,
    /// Source sample rate in Hz.
    pub sample_rate: Option<u32>,
    /// Source bit depth.
    pub bit_depth: Option<u32>,
    /// Audio channel count.
    pub channels: Option<u16>,
    /// File size in bytes.
    pub file_size: u64,
    /// Track number.
    pub track_number: Option<u32>,
    /// Disc number.
    pub disc_number: Option<u32>,
    /// Unix timestamp (seconds) when first added to the library.
    pub date_added: i64,
    /// Last modified Unix timestamp (seconds).
    pub last_modified: Option<i64>,
}

/// SQLite database access layer for the persistent music library.
pub struct LibraryDatabase {
    conn: Connection,
}

impl LibraryDatabase {
    /// Opens or creates a persistent SQLite library database at `path`,
    /// configures connection pragmas, and runs any pending migrations.
    pub fn open(path: impl AsRef<Path>) -> Result<Self, LibraryError> {
        let path = path.as_ref();
        if let Some(parent) = path.parent()
            && !parent.as_os_str().is_empty()
        {
            fs::create_dir_all(parent)?;
        }

        let conn = Connection::open(path)?;
        let mut db = Self { conn };
        db.configure_connection(true)?;
        db.run_migrations()?;
        Ok(db)
    }

    /// Opens an ephemeral in-memory SQLite library database and runs migrations.
    pub fn open_in_memory() -> Result<Self, LibraryError> {
        let conn = Connection::open_in_memory()?;
        let mut db = Self { conn };
        db.configure_connection(false)?;
        db.run_migrations()?;
        Ok(db)
    }

    fn configure_connection(&self, enable_wal: bool) -> Result<(), LibraryError> {
        self.conn.busy_timeout(Duration::from_secs(5))?;
        self.conn.execute_batch("PRAGMA foreign_keys = ON;")?;
        if enable_wal {
            self.conn.execute_batch("PRAGMA journal_mode = WAL;")?;
        }
        Ok(())
    }

    /// Applies any unapplied schema migrations inside transactions.
    fn run_migrations(&mut self) -> Result<(), LibraryError> {
        self.conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS schema_migrations (
                version INTEGER PRIMARY KEY,
                name TEXT NOT NULL,
                applied_at INTEGER NOT NULL
            );",
        )?;

        let current = self.schema_version()?;
        let now = unix_timestamp_secs();

        for migration in MIGRATIONS {
            if migration.version <= current {
                continue;
            }
            let tx = self.conn.transaction()?;
            if let Err(err) = tx.execute_batch(migration.sql) {
                return Err(LibraryError::MigrationFailed {
                    version: migration.version,
                    message: err.to_string(),
                });
            }
            tx.execute(
                "INSERT INTO schema_migrations (version, name, applied_at) VALUES (?1, ?2, ?3)",
                params![migration.version, migration.name, now],
            )?;
            tx.commit()?;
        }

        Ok(())
    }

    /// Returns the highest applied schema migration version (`0` if none).
    pub fn schema_version(&self) -> Result<u32, LibraryError> {
        let version: Option<u32> =
            self.conn
                .query_row("SELECT MAX(version) FROM schema_migrations", [], |row| {
                    row.get(0)
                })?;
        Ok(version.unwrap_or(0))
    }

    /// Executes a closure within an explicit SQLite transaction.
    pub fn with_transaction<F, T>(&mut self, f: F) -> Result<T, LibraryError>
    where
        F: FnOnce(&Transaction<'_>) -> Result<T, LibraryError>,
    {
        let tx = self.conn.transaction()?;
        let result = f(&tx)?;
        tx.commit()?;
        Ok(result)
    }

    /// Inserts or retrieves an artist by exact name.
    pub fn upsert_artist(&self, name: &str) -> Result<ArtistRecord, LibraryError> {
        self.conn.execute(
            "INSERT OR IGNORE INTO artists (name) VALUES (?1)",
            params![name],
        )?;
        self.conn
            .query_row(
                "SELECT id, name FROM artists WHERE name = ?1",
                params![name],
                |row| {
                    Ok(ArtistRecord {
                        id: row.get(0)?,
                        name: row.get(1)?,
                    })
                },
            )
            .map_err(LibraryError::from)
    }

    /// Retrieves an artist by primary key.
    pub fn get_artist_by_id(&self, id: i64) -> Result<Option<ArtistRecord>, LibraryError> {
        self.conn
            .query_row(
                "SELECT id, name FROM artists WHERE id = ?1",
                params![id],
                |row| {
                    Ok(ArtistRecord {
                        id: row.get(0)?,
                        name: row.get(1)?,
                    })
                },
            )
            .optional()
            .map_err(LibraryError::from)
    }

    /// Lists all artists ordered alphabetically by name.
    pub fn list_artists(&self) -> Result<Vec<ArtistRecord>, LibraryError> {
        let mut stmt = self
            .conn
            .prepare("SELECT id, name FROM artists ORDER BY name ASC")?;
        let rows = stmt.query_map([], |row| {
            Ok(ArtistRecord {
                id: row.get(0)?,
                name: row.get(1)?,
            })
        })?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(LibraryError::from)
    }

    /// Inserts or updates an album keyed by `(name, artist_id)`.
    pub fn upsert_album(
        &self,
        name: &str,
        artist_id: Option<i64>,
        release_year: Option<u32>,
        artwork_ref: Option<&str>,
    ) -> Result<AlbumRecord, LibraryError> {
        let existing_id: Option<i64> = match artist_id {
            Some(aid) => self
                .conn
                .query_row(
                    "SELECT id FROM albums WHERE name = ?1 AND artist_id = ?2",
                    params![name, aid],
                    |row| row.get(0),
                )
                .optional()?,
            None => self
                .conn
                .query_row(
                    "SELECT id FROM albums WHERE name = ?1 AND artist_id IS NULL",
                    params![name],
                    |row| row.get(0),
                )
                .optional()?,
        };

        let album_id = if let Some(id) = existing_id {
            self.conn.execute(
                "UPDATE albums
                 SET release_year = COALESCE(?1, release_year),
                     artwork_ref = COALESCE(?2, artwork_ref)
                 WHERE id = ?3",
                params![release_year, artwork_ref, id],
            )?;
            id
        } else {
            self.conn.execute(
                "INSERT INTO albums (name, artist_id, release_year, artwork_ref)
                 VALUES (?1, ?2, ?3, ?4)",
                params![name, artist_id, release_year, artwork_ref],
            )?;
            self.conn.last_insert_rowid()
        };

        self.get_album_by_id(album_id)?
            .ok_or_else(|| LibraryError::Unavailable("inserted album missing".to_string()))
    }

    /// Retrieves an album by primary key.
    pub fn get_album_by_id(&self, id: i64) -> Result<Option<AlbumRecord>, LibraryError> {
        self.conn
            .query_row(
                "SELECT id, name, artist_id, release_year, artwork_ref FROM albums WHERE id = ?1",
                params![id],
                |row| {
                    Ok(AlbumRecord {
                        id: row.get(0)?,
                        name: row.get(1)?,
                        artist_id: row.get(2)?,
                        release_year: row.get(3)?,
                        artwork_ref: row.get(4)?,
                    })
                },
            )
            .optional()
            .map_err(LibraryError::from)
    }

    /// Lists all albums ordered by name.
    pub fn list_albums(&self) -> Result<Vec<AlbumRecord>, LibraryError> {
        let mut stmt = self.conn.prepare(
            "SELECT id, name, artist_id, release_year, artwork_ref FROM albums ORDER BY name ASC",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok(AlbumRecord {
                id: row.get(0)?,
                name: row.get(1)?,
                artist_id: row.get(2)?,
                release_year: row.get(3)?,
                artwork_ref: row.get(4)?,
            })
        })?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(LibraryError::from)
    }

    /// Inserts a new track or updates an existing track matching `source_path`.
    pub fn upsert_track(&self, track: &NewTrackRecord) -> Result<TrackRecord, LibraryError> {
        let now = unix_timestamp_secs();
        let duration_ms = track.duration_ms.map(|d| d as i64);
        let file_size = track.file_size as i64;

        self.conn.execute(
            "INSERT INTO tracks (
                source_path, title, artist, album, album_artist,
                artist_id, album_id, genre, duration_ms, codec,
                sample_rate, bit_depth, channels, file_size,
                track_number, disc_number, date_added, last_modified
            ) VALUES (
                ?1, ?2, ?3, ?4, ?5,
                ?6, ?7, ?8, ?9, ?10,
                ?11, ?12, ?13, ?14,
                ?15, ?16, ?17, ?18
            )
            ON CONFLICT(source_path) DO UPDATE SET
                title = excluded.title,
                artist = excluded.artist,
                album = excluded.album,
                album_artist = excluded.album_artist,
                artist_id = excluded.artist_id,
                album_id = excluded.album_id,
                genre = excluded.genre,
                duration_ms = excluded.duration_ms,
                codec = excluded.codec,
                sample_rate = excluded.sample_rate,
                bit_depth = excluded.bit_depth,
                channels = excluded.channels,
                file_size = excluded.file_size,
                track_number = excluded.track_number,
                disc_number = excluded.disc_number,
                last_modified = excluded.last_modified",
            params![
                track.source_path,
                track.title,
                track.artist,
                track.album,
                track.album_artist,
                track.artist_id,
                track.album_id,
                track.genre,
                duration_ms,
                track.codec,
                track.sample_rate,
                track.bit_depth,
                track.channels,
                file_size,
                track.track_number,
                track.disc_number,
                now,
                track.last_modified,
            ],
        )?;

        self.get_track_by_path(&track.source_path)?
            .ok_or_else(|| LibraryError::Unavailable("upserted track missing".to_string()))
    }

    /// Retrieves a track by primary key.
    pub fn get_track_by_id(&self, id: i64) -> Result<Option<TrackRecord>, LibraryError> {
        self.conn
            .query_row(
                "SELECT id, source_path, title, artist, album, album_artist,
                        artist_id, album_id, genre, duration_ms, codec,
                        sample_rate, bit_depth, channels, file_size,
                        track_number, disc_number, date_added, last_modified
                 FROM tracks WHERE id = ?1",
                params![id],
                row_to_track_record,
            )
            .optional()
            .map_err(LibraryError::from)
    }

    /// Retrieves a track by unique `source_path`.
    pub fn get_track_by_path(
        &self,
        source_path: &str,
    ) -> Result<Option<TrackRecord>, LibraryError> {
        self.conn
            .query_row(
                "SELECT id, source_path, title, artist, album, album_artist,
                        artist_id, album_id, genre, duration_ms, codec,
                        sample_rate, bit_depth, channels, file_size,
                        track_number, disc_number, date_added, last_modified
                 FROM tracks WHERE source_path = ?1",
                params![source_path],
                row_to_track_record,
            )
            .optional()
            .map_err(LibraryError::from)
    }

    /// Lists all tracks ordered by album, disc, track number, and title/path.
    pub fn list_tracks(&self) -> Result<Vec<TrackRecord>, LibraryError> {
        let mut stmt = self.conn.prepare(
            "SELECT id, source_path, title, artist, album, album_artist,
                    artist_id, album_id, genre, duration_ms, codec,
                    sample_rate, bit_depth, channels, file_size,
                    track_number, disc_number, date_added, last_modified
             FROM tracks
             ORDER BY COALESCE(album, ''), COALESCE(disc_number, 0),
                      COALESCE(track_number, 0), COALESCE(title, source_path)",
        )?;
        let rows = stmt.query_map([], row_to_track_record)?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(LibraryError::from)
    }

    /// Deletes a track matching `source_path`, returning `true` if a row was removed.
    pub fn delete_track_by_path(&self, source_path: &str) -> Result<bool, LibraryError> {
        let affected = self.conn.execute(
            "DELETE FROM tracks WHERE source_path = ?1",
            params![source_path],
        )?;
        Ok(affected > 0)
    }

    /// Returns the total number of tracks stored in the database.
    pub fn count_tracks(&self) -> Result<usize, LibraryError> {
        let count: i64 = self
            .conn
            .query_row("SELECT COUNT(*) FROM tracks", [], |row| row.get(0))?;
        Ok(count.max(0) as usize)
    }

    /// Returns the total number of albums stored in the database.
    pub fn count_albums(&self) -> Result<usize, LibraryError> {
        let count: i64 = self
            .conn
            .query_row("SELECT COUNT(*) FROM albums", [], |row| row.get(0))?;
        Ok(count.max(0) as usize)
    }

    /// Returns the total number of artists stored in the database.
    pub fn count_artists(&self) -> Result<usize, LibraryError> {
        let count: i64 = self
            .conn
            .query_row("SELECT COUNT(*) FROM artists", [], |row| row.get(0))?;
        Ok(count.max(0) as usize)
    }
}

fn row_to_track_record(row: &rusqlite::Row<'_>) -> rusqlite::Result<TrackRecord> {
    let duration_ms: Option<i64> = row.get(9)?;
    let file_size: i64 = row.get(14)?;
    Ok(TrackRecord {
        id: row.get(0)?,
        source_path: row.get(1)?,
        title: row.get(2)?,
        artist: row.get(3)?,
        album: row.get(4)?,
        album_artist: row.get(5)?,
        artist_id: row.get(6)?,
        album_id: row.get(7)?,
        genre: row.get(8)?,
        duration_ms: duration_ms.map(|d| d.max(0) as u64),
        codec: row.get(10)?,
        sample_rate: row.get(11)?,
        bit_depth: row.get(12)?,
        channels: row.get(13)?,
        file_size: file_size.max(0) as u64,
        track_number: row.get(15)?,
        disc_number: row.get(16)?,
        date_added: row.get(17)?,
        last_modified: row.get(18)?,
    })
}

fn unix_timestamp_secs() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn opens_in_memory_and_runs_idempotent_migrations() {
        let mut db = LibraryDatabase::open_in_memory().expect("open in-memory db");
        assert_eq!(db.schema_version().unwrap(), 1);

        // Re-running migrations should be a safe no-op
        db.run_migrations().expect("idempotent migrations");
        assert_eq!(db.schema_version().unwrap(), 1);
    }

    #[test]
    fn persists_to_disk_and_reopens_cleanly() {
        let temp_dir = std::env::temp_dir().join("termwave_phase19_db_test");
        let _ = fs::remove_dir_all(&temp_dir);
        let db_path = temp_dir.join("nested").join("library.sqlite");

        {
            let db = LibraryDatabase::open(&db_path).expect("create persistent db");
            assert_eq!(db.schema_version().unwrap(), 1);
            let artist = db.upsert_artist("Miles Davis").unwrap();
            let album = db
                .upsert_album("Kind of Blue", Some(artist.id), Some(1959), Some("cover_1"))
                .unwrap();
            let track = db
                .upsert_track(&NewTrackRecord {
                    source_path: "/music/so_what.flac".to_string(),
                    title: Some("So What".to_string()),
                    artist: Some("Miles Davis".to_string()),
                    album: Some("Kind of Blue".to_string()),
                    album_artist: Some("Miles Davis".to_string()),
                    artist_id: Some(artist.id),
                    album_id: Some(album.id),
                    genre: Some("Jazz".to_string()),
                    duration_ms: Some(562_000),
                    codec: "FLAC".to_string(),
                    sample_rate: Some(96_000),
                    bit_depth: Some(24),
                    channels: Some(2),
                    file_size: 120_000_000,
                    track_number: Some(1),
                    disc_number: Some(1),
                    last_modified: Some(1_700_000_000),
                })
                .unwrap();
            assert_eq!(track.title.as_deref(), Some("So What"));
        }

        // Re-open existing database file and verify persisted state
        let reopened = LibraryDatabase::open(&db_path).expect("reopen persistent db");
        assert_eq!(reopened.count_artists().unwrap(), 1);
        assert_eq!(reopened.count_albums().unwrap(), 1);
        assert_eq!(reopened.count_tracks().unwrap(), 1);

        let stored = reopened
            .get_track_by_path("/music/so_what.flac")
            .unwrap()
            .expect("track should persist");
        assert_eq!(stored.sample_rate, Some(96_000));
        assert_eq!(stored.bit_depth, Some(24));

        drop(reopened);
        let _ = fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn upserts_avoid_duplicates_and_support_queries_and_deletes() {
        let db = LibraryDatabase::open_in_memory().unwrap();
        let a1 = db.upsert_artist("Daft Punk").unwrap();
        let a2 = db.upsert_artist("Daft Punk").unwrap();
        assert_eq!(a1.id, a2.id);
        assert_eq!(db.count_artists().unwrap(), 1);

        let alb1 = db
            .upsert_album("Discovery", Some(a1.id), Some(2001), None)
            .unwrap();
        let alb2 = db
            .upsert_album("Discovery", Some(a1.id), Some(2001), Some("art_hash"))
            .unwrap();
        assert_eq!(alb1.id, alb2.id);
        assert_eq!(alb2.artwork_ref.as_deref(), Some("art_hash"));
        assert_eq!(db.count_albums().unwrap(), 1);

        let mut track = NewTrackRecord {
            source_path: "/music/one_more_time.flac".to_string(),
            title: Some("One More Time".to_string()),
            artist: Some("Daft Punk".to_string()),
            album: Some("Discovery".to_string()),
            album_artist: Some("Daft Punk".to_string()),
            artist_id: Some(a1.id),
            album_id: Some(alb1.id),
            genre: Some("Electronic".to_string()),
            duration_ms: Some(320_000),
            codec: "FLAC".to_string(),
            sample_rate: Some(44_100),
            bit_depth: Some(16),
            channels: Some(2),
            file_size: 40_000_000,
            track_number: Some(1),
            disc_number: Some(1),
            last_modified: Some(100),
        };
        let inserted = db.upsert_track(&track).unwrap();
        track.title = Some("One More Time (Remastered)".to_string());
        track.last_modified = Some(200);
        let updated = db.upsert_track(&track).unwrap();

        assert_eq!(inserted.id, updated.id);
        assert_eq!(updated.title.as_deref(), Some("One More Time (Remastered)"));
        assert_eq!(db.count_tracks().unwrap(), 1);
        assert_eq!(db.list_tracks().unwrap().len(), 1);
        assert_eq!(db.list_albums().unwrap().len(), 1);
        assert_eq!(db.list_artists().unwrap().len(), 1);

        assert!(
            db.delete_track_by_path("/music/one_more_time.flac")
                .unwrap()
        );
        assert!(
            !db.delete_track_by_path("/music/one_more_time.flac")
                .unwrap()
        );
        assert_eq!(db.count_tracks().unwrap(), 0);
    }

    #[test]
    fn enforces_foreign_key_constraints() {
        let db = LibraryDatabase::open_in_memory().unwrap();
        let err = db
            .upsert_album("Orphan", Some(999_999), None, None)
            .unwrap_err();
        assert!(matches!(err, LibraryError::Database(_)));
    }
}
