//! Termwave core library.
//!
//! Responsibilities are isolated into dedicated top-level modules so that
//! audio playback, metadata extraction, library persistence, configuration,
//! and terminal/CLI presentation remain decoupled.

pub mod audio;
pub mod cli;
pub mod config;
pub mod error;
pub mod library;
pub mod metadata;

pub use library::{
    AlbumRecord, ArtistRecord, BackgroundScanHandle, DEFAULT_SEARCH_LIMIT, IndexOptions,
    IndexSummary, LibraryDatabase, LibraryError, NewTrackRecord, ScanOptions, ScanProgress,
    ScanSummary, ScanWarning, ScannedAudioFormat, ScannedTrack, SearchOptions, SearchResults,
    TrackRecord, index_directory, index_scan_summary, rescan_library, scan_directories,
    scan_directory, spawn_directory_scan,
};
pub use metadata::{
    ArtworkDimensions, ArtworkExtractOptions, ArtworkFormat, ArtworkRole, ArtworkSummary,
    EmbeddedArtwork, MetadataError, TerminalArtwork, TerminalArtworkLayout, TrackMetadata,
    extract_artwork, extract_artwork_or_none, extract_metadata, extract_metadata_or_default,
};
