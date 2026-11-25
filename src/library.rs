//! Persistent music library boundary.
//!
//! Responsible for directory scanning, SQLite indexing, library search,
//! favorites, and playback history. Single-file playback must never depend
//! on this subsystem.

use thiserror::Error;

/// Errors produced by music library indexing or queries.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum LibraryError {
    /// Library storage is unavailable or uninitialized.
    #[error("library unavailable: {0}")]
    Unavailable(String),

    /// Underlying I/O error during library operations.
    #[error("library I/O error: {0}")]
    Io(#[from] std::io::Error),
}
