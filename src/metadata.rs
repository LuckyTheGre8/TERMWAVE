//! Audio metadata and embedded artwork boundary.
//!
//! Responsible for extracting tags and technical stream properties while
//! distinguishing source audio properties from output device properties.

use thiserror::Error;

/// Errors produced during metadata or embedded artwork extraction.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum MetadataError {
    /// Failed to read metadata tags from an audio source.
    #[error("failed to read metadata: {0}")]
    ReadFailed(String),

    /// Underlying I/O error while inspecting metadata.
    #[error("metadata I/O error: {0}")]
    Io(#[from] std::io::Error),
}
