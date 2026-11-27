//! Music library indexer: metadata indexing, deduplication, change detection,
//! rescanning, move/rename reconciliation, and orphan cleanup.

use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::metadata::{TrackMetadata, extract_metadata_or_default};

use super::LibraryError;
use super::db::{
    LibraryDatabase, NewTrackRecord, TrackRecord, delete_track_by_id_conn, list_tracks_conn,
    prune_orphaned_albums_and_artists_conn, update_track_source_path_conn, upsert_album_conn,
    upsert_artist_conn, upsert_track_conn,
};
use super::scanner::{ScanOptions, ScanSummary, ScannedTrack, scan_directories};

/// Options controlling how scanned files are indexed into the SQLite library.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IndexOptions {
    /// Re-extract metadata even when file size and modification time are unchanged.
    pub force_reextract: bool,
    /// Remove tracks under the scanned root directories that no longer exist on disk.
    pub prune_missing: bool,
    /// Detect renamed/moved files by matching size and modification timestamp
    /// among missing tracks before pruning, preserving track IDs and `date_added`.
    pub detect_moves: bool,
}

impl Default for IndexOptions {
    fn default() -> Self {
        Self {
            force_reextract: false,
            prune_missing: true,
            detect_moves: true,
        }
    }
}

/// Summary statistics produced by an indexing or rescanning operation.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct IndexSummary {
    /// Newly inserted tracks.
    pub added_tracks: usize,
    /// Existing tracks whose metadata or file properties were updated.
    pub updated_tracks: usize,
    /// Tracks whose `source_path` was updated in-place after a move or rename.
    pub moved_tracks: usize,
    /// Existing tracks skipped because size and modification time were unchanged.
    pub unchanged_tracks: usize,
    /// Duplicate paths skipped within the same scan batch.
    pub duplicate_tracks_skipped: usize,
    /// Tracks removed from the database because they no longer exist under scanned roots.
    pub removed_tracks: usize,
    /// Orphaned albums pruned after track updates/removals.
    pub pruned_albums: usize,
    /// Orphaned artists pruned after track updates/removals.
    pub pruned_artists: usize,
}

/// Scans a single directory and indexes discovered tracks into `db`.
pub fn index_directory(
    db: &mut LibraryDatabase,
    root: impl AsRef<Path>,
    scan_options: &ScanOptions,
    index_options: &IndexOptions,
) -> Result<IndexSummary, LibraryError> {
    let root_path = root.as_ref().to_path_buf();
    rescan_library(db, &[root_path], scan_options, index_options)
}

/// Recursively scans `roots` and reconciles the SQLite library atomically.
pub fn rescan_library(
    db: &mut LibraryDatabase,
    roots: &[PathBuf],
    scan_options: &ScanOptions,
    index_options: &IndexOptions,
) -> Result<IndexSummary, LibraryError> {
    let scan_summary = scan_directories(roots, scan_options)?;
    index_scan_summary(db, roots, &scan_summary, index_options)
}

/// Indexes a pre-scanned [`ScanSummary`] into `db` within a single transaction.
///
/// Handles deduplication, unchanged-file skipping, changed-file metadata updates,
/// move/rename reconciliation, missing-file pruning, and orphan cleanup.
pub fn index_scan_summary(
    db: &mut LibraryDatabase,
    scanned_roots: &[PathBuf],
    scan_summary: &ScanSummary,
    index_options: &IndexOptions,
) -> Result<IndexSummary, LibraryError> {
    let normalized_roots: Vec<PathBuf> = scanned_roots
        .iter()
        .map(|r| normalize_path_buf(r))
        .collect();

    // Deduplicate scanned tracks by normalized path before hitting the database.
    let mut unique_tracks: Vec<(String, &ScannedTrack)> =
        Vec::with_capacity(scan_summary.tracks.len());
    let mut seen_paths: HashSet<String> = HashSet::with_capacity(scan_summary.tracks.len());
    let mut duplicate_tracks_skipped = 0usize;

    for track in &scan_summary.tracks {
        let key = normalize_path_string(&track.path);
        if seen_paths.insert(key.clone()) {
            unique_tracks.push((key, track));
        } else {
            duplicate_tracks_skipped += 1;
        }
    }

    // Pre-read existing tracks so we can detect unchanged files, moves, and removals.
    db.with_transaction(|tx| {
        let existing_tracks = list_tracks_conn(tx)?;
        let mut by_path: HashMap<String, TrackRecord> = existing_tracks
            .iter()
            .map(|t| (t.source_path.clone(), t.clone()))
            .collect();

        // Identify missing tracks that live under the scanned roots.
        let mut missing_under_roots: Vec<TrackRecord> = if index_options.prune_missing {
            existing_tracks
                .into_iter()
                .filter(|t| {
                    !seen_paths.contains(&t.source_path)
                        && is_under_any_root(Path::new(&t.source_path), &normalized_roots)
                })
                .collect()
        } else {
            Vec::new()
        };

        let mut summary = IndexSummary {
            duplicate_tracks_skipped,
            ..IndexSummary::default()
        };

        for (source_path, scanned) in unique_tracks {
            let modified_secs = system_time_to_unix_secs(scanned.modified);

            if let Some(existing) = by_path.get(&source_path) {
                let unchanged = !index_options.force_reextract
                    && existing.file_size == scanned.size_bytes
                    && existing.last_modified == modified_secs;
                if unchanged {
                    summary.unchanged_tracks += 1;
                    continue;
                }

                // File changed on disk (or forced re-extraction): re-extract and update.
                let meta = extract_metadata_or_default(&scanned.path);
                let new_record =
                    build_new_track_record(tx, source_path, scanned, modified_secs, &meta)?;
                upsert_track_conn(tx, &new_record)?;
                summary.updated_tracks += 1;
                continue;
            }

            // Not found at this path: check if it matches a missing track (moved/renamed).
            if index_options.detect_moves
                && let Some(move_idx) =
                    find_moved_track_index(&missing_under_roots, scanned.size_bytes, modified_secs)
            {
                let old_record = missing_under_roots.swap_remove(move_idx);
                by_path.remove(&old_record.source_path);
                update_track_source_path_conn(tx, old_record.id, &source_path)?;

                if index_options.force_reextract {
                    let meta = extract_metadata_or_default(&scanned.path);
                    let new_record =
                        build_new_track_record(tx, source_path, scanned, modified_secs, &meta)?;
                    upsert_track_conn(tx, &new_record)?;
                }

                summary.moved_tracks += 1;
                continue;
            }

            // Brand new track: extract metadata and insert.
            let meta = extract_metadata_or_default(&scanned.path);
            let new_record =
                build_new_track_record(tx, source_path, scanned, modified_secs, &meta)?;
            upsert_track_conn(tx, &new_record)?;
            summary.added_tracks += 1;
        }

        // Remove any remaining tracks under scanned roots that disappeared from disk.
        if index_options.prune_missing {
            for missing in missing_under_roots {
                if delete_track_by_id_conn(tx, missing.id)? {
                    summary.removed_tracks += 1;
                }
            }
        }

        // Keep artists and albums consistent when tracks are updated or removed.
        let (pruned_albums, pruned_artists) = prune_orphaned_albums_and_artists_conn(tx)?;
        summary.pruned_albums = pruned_albums;
        summary.pruned_artists = pruned_artists;

        Ok(summary)
    })
}

fn build_new_track_record(
    conn: &rusqlite::Connection,
    source_path: String,
    scanned: &ScannedTrack,
    modified_secs: Option<i64>,
    meta: &TrackMetadata,
) -> Result<NewTrackRecord, LibraryError> {
    let track_artist_name = meta
        .artist
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty());
    let album_artist_name = meta
        .album_artist
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .or(track_artist_name);

    let artist_id = match track_artist_name {
        Some(name) => Some(upsert_artist_conn(conn, name)?.id),
        None => None,
    };

    let album_artist_id = match album_artist_name {
        Some(name) if Some(name) == track_artist_name => artist_id,
        Some(name) => Some(upsert_artist_conn(conn, name)?.id),
        None => None,
    };

    let artwork_ref = meta.artwork.as_ref().map(|art| {
        let dims = art
            .dimensions
            .map_or_else(|| "unknown".to_string(), |d| d.to_string());
        format!("{}:{dims}:{}", art.format, art.byte_len)
    });

    let album_id = match meta
        .album
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        Some(album_name) => Some(
            upsert_album_conn(
                conn,
                album_name,
                album_artist_id,
                meta.year,
                artwork_ref.as_deref(),
            )?
            .id,
        ),
        None => None,
    };

    Ok(NewTrackRecord {
        source_path,
        title: meta.title.clone(),
        artist: meta.artist.clone(),
        album: meta.album.clone(),
        album_artist: meta.album_artist.clone(),
        artist_id,
        album_id,
        genre: meta.genre.clone(),
        duration_ms: meta.duration.map(|d| d.as_millis() as u64),
        codec: scanned.format.to_string(),
        sample_rate: meta.sample_rate,
        bit_depth: meta.bits_per_sample,
        channels: meta.channels,
        file_size: scanned.size_bytes,
        track_number: meta.track_number,
        disc_number: meta.disc_number,
        last_modified: modified_secs,
    })
}

fn find_moved_track_index(
    missing: &[TrackRecord],
    size_bytes: u64,
    modified_secs: Option<i64>,
) -> Option<usize> {
    let mod_ts = modified_secs?;
    let mut matches = missing
        .iter()
        .enumerate()
        .filter(|(_, t)| t.file_size == size_bytes && t.last_modified == Some(mod_ts));
    let first = matches.next()?.0;
    if matches.next().is_some() {
        // Ambiguous if multiple missing files share identical size and timestamp.
        return None;
    }
    Some(first)
}

fn normalize_path_buf(path: &Path) -> PathBuf {
    fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}

fn normalize_path_string(path: &Path) -> String {
    normalize_path_buf(path).to_string_lossy().into_owned()
}

fn is_under_any_root(path: &Path, normalized_roots: &[PathBuf]) -> bool {
    normalized_roots.iter().any(|root| path.starts_with(root))
}

fn system_time_to_unix_secs(time: Option<SystemTime>) -> Option<i64> {
    time.and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map(|d| d.as_secs() as i64)
}

#[cfg(test)]
mod tests {
    use std::fs;

    use lofty::config::WriteOptions;
    use lofty::tag::{Accessor, ItemKey, Tag, TagExt, TagType};

    use super::*;
    use crate::library::scanner::ScannedAudioFormat;

    fn temp_test_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("termwave_phase20_{name}"));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).expect("create temp dir");
        dir
    }

    fn fixture_path(name: &str) -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("tests")
            .join("fixtures")
            .join(name)
    }

    fn write_tagged_flac(dest: &Path, title: &str, artist: &str, album: &str, track_num: u32) {
        fs::copy(fixture_path("sine_16bit_44100_stereo.flac"), dest).unwrap();
        let mut tag = Tag::new(TagType::VorbisComments);
        tag.set_title(title.to_string());
        tag.set_artist(artist.to_string());
        tag.set_album(album.to_string());
        tag.set_track(track_num);
        tag.insert_text(ItemKey::Year, "2024".to_string());
        tag.save_to_path(dest, WriteOptions::default()).unwrap();
    }

    #[test]
    fn indexes_discovered_tracks_and_skips_unchanged_on_rescan() {
        let root = temp_test_dir("index_and_rescan");
        let track1 = root.join("01_dawn.flac");
        let track2 = root.join("02_dusk.flac");
        write_tagged_flac(&track1, "Dawn", "Solstice", "Equinox", 1);
        write_tagged_flac(&track2, "Dusk", "Solstice", "Equinox", 2);

        let mut db = LibraryDatabase::open_in_memory().unwrap();
        let first = index_directory(
            &mut db,
            &root,
            &ScanOptions::default(),
            &IndexOptions::default(),
        )
        .unwrap();
        assert_eq!(first.added_tracks, 2);
        assert_eq!(first.updated_tracks, 0);
        assert_eq!(db.count_tracks().unwrap(), 2);
        assert_eq!(db.count_artists().unwrap(), 1);
        assert_eq!(db.count_albums().unwrap(), 1);

        // Second scan without changes should skip re-extracting metadata
        let second = index_directory(
            &mut db,
            &root,
            &ScanOptions::default(),
            &IndexOptions::default(),
        )
        .unwrap();
        assert_eq!(second.added_tracks, 0);
        assert_eq!(second.updated_tracks, 0);
        assert_eq!(second.unchanged_tracks, 2);
        assert_eq!(db.count_tracks().unwrap(), 2);

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn detects_changed_files_and_updates_metadata() {
        let root = temp_test_dir("changed_file");
        let track_path = root.join("song.flac");
        write_tagged_flac(&track_path, "Original Title", "Artist A", "Album A", 1);

        let mut db = LibraryDatabase::open_in_memory().unwrap();
        let first = index_directory(
            &mut db,
            &root,
            &ScanOptions::default(),
            &IndexOptions::default(),
        )
        .unwrap();
        assert_eq!(first.added_tracks, 1);

        // Modify tags on disk (changes file size / contents)
        write_tagged_flac(
            &track_path,
            "Updated Title Extended",
            "Artist B",
            "Album B",
            3,
        );

        let second = index_directory(
            &mut db,
            &root,
            &ScanOptions::default(),
            &IndexOptions::default(),
        )
        .unwrap();
        assert_eq!(second.updated_tracks, 1);
        assert_eq!(second.added_tracks, 0);
        assert_eq!(db.count_tracks().unwrap(), 1);

        let stored = db.list_tracks().unwrap().pop().unwrap();
        assert_eq!(stored.title.as_deref(), Some("Updated Title Extended"));
        assert_eq!(stored.artist.as_deref(), Some("Artist B"));
        assert_eq!(stored.album.as_deref(), Some("Album B"));
        // Orphaned Artist A & Album A should be pruned
        assert_eq!(db.count_artists().unwrap(), 1);
        assert_eq!(db.count_albums().unwrap(), 1);
        assert_eq!(second.pruned_artists, 1);
        assert_eq!(second.pruned_albums, 1);

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn detects_renamed_files_and_prunes_deleted_files() {
        let root = temp_test_dir("rename_and_prune");
        let old_path = root.join("old_name.flac");
        let doomed_path = root.join("doomed.flac");
        write_tagged_flac(&old_path, "Track One", "Artist X", "Album X", 1);
        write_tagged_flac(
            &doomed_path,
            "Track Two (Different Length)",
            "Artist Y",
            "Album Y",
            2,
        );

        let mut db = LibraryDatabase::open_in_memory().unwrap();
        index_directory(
            &mut db,
            &root,
            &ScanOptions::default(),
            &IndexOptions::default(),
        )
        .unwrap();
        let original_id = db
            .get_track_by_path(&normalize_path_string(&old_path))
            .unwrap()
            .unwrap()
            .id;

        // Rename old_name.flac -> new_name.flac and delete doomed.flac
        let new_path = root.join("new_name.flac");
        fs::rename(&old_path, &new_path).unwrap();
        fs::remove_file(&doomed_path).unwrap();

        let rescan = index_directory(
            &mut db,
            &root,
            &ScanOptions::default(),
            &IndexOptions::default(),
        )
        .unwrap();
        assert_eq!(rescan.moved_tracks, 1);
        assert_eq!(rescan.removed_tracks, 1);
        assert_eq!(rescan.pruned_artists, 1);
        assert_eq!(rescan.pruned_albums, 1);
        assert_eq!(db.count_tracks().unwrap(), 1);

        let moved_record = db
            .get_track_by_path(&normalize_path_string(&new_path))
            .unwrap()
            .expect("renamed track should exist at new path");
        assert_eq!(moved_record.id, original_id);

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn deduplicates_duplicate_entries_in_scan_batch() {
        let root = temp_test_dir("dedup_batch");
        let track_path = root.join("dup.flac");
        write_tagged_flac(&track_path, "Dup", "Artist", "Album", 1);

        let meta = fs::metadata(&track_path).unwrap();
        let scanned = ScannedTrack {
            path: track_path.clone(),
            format: ScannedAudioFormat::Flac,
            size_bytes: meta.len(),
            modified: meta.modified().ok(),
        };
        let summary = ScanSummary {
            tracks: vec![scanned.clone(), scanned],
            visited_files: 2,
            ..ScanSummary::default()
        };

        let mut db = LibraryDatabase::open_in_memory().unwrap();
        let idx = index_scan_summary(
            &mut db,
            std::slice::from_ref(&root),
            &summary,
            &IndexOptions::default(),
        )
        .unwrap();
        assert_eq!(idx.added_tracks, 1);
        assert_eq!(idx.duplicate_tracks_skipped, 1);
        assert_eq!(db.count_tracks().unwrap(), 1);

        let _ = fs::remove_dir_all(&root);
    }
}
