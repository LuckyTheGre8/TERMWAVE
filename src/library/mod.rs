//! Persistent music library boundary.
//!
//! Responsible for directory scanning, SQLite indexing, library search,
//! favorites, and playback history. Single-file playback must never depend
//! on this subsystem.

pub mod db;
pub mod indexer;
pub mod scanner;

use std::path::PathBuf;

use thiserror::Error;

pub use db::{AlbumRecord, ArtistRecord, LibraryDatabase, NewTrackRecord, TrackRecord};
pub use indexer::{
    IndexOptions, IndexSummary, index_directory, index_scan_summary, rescan_library,
};
pub use scanner::{
    BackgroundScanHandle, ScanOptions, ScanProgress, ScanSummary, ScanWarning, ScannedAudioFormat,
    ScannedTrack, scan_directories, scan_directory, spawn_directory_scan,
};

/// Errors produced by music library scanning, indexing, or queries.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum LibraryError {
    /// Library storage is unavailable or uninitialized.
    #[error("library unavailable: {0}")]
    Unavailable(String),

    /// Target scan root does not exist.
    #[error("scan path not found: {path}")]
    PathNotFound { path: PathBuf },

    /// Target scan root is not a directory.
    #[error("scan path is not a directory: {path}")]
    NotADirectory { path: PathBuf },

    /// Permission denied accessing a scan root directory.
    #[error("permission denied accessing directory: {path}")]
    PermissionDenied { path: PathBuf },

    /// Database migration failed.
    #[error("library migration failed at version {version}: {message}")]
    MigrationFailed { version: u32, message: String },

    /// SQLite database operation failed.
    #[error("library database error: {0}")]
    Database(#[from] rusqlite::Error),

    /// Underlying I/O error during library operations.
    #[error("library I/O error: {0}")]
    Io(#[from] std::io::Error),
}
