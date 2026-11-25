//! Centralized error types and result conventions for Termwave.
//!
//! Subsystem modules define focused domain errors with `thiserror`,
//! `TermwaveError` unifies them for library callers, and the binary/CLI
//! boundary propagates errors via `anyhow::Result`.

use thiserror::Error;

use crate::audio::AudioError;
use crate::config::ConfigError;
use crate::library::LibraryError;
use crate::metadata::MetadataError;

/// Crate-wide `Result` alias using [`TermwaveError`].
pub type Result<T> = std::result::Result<T, TermwaveError>;

/// Top-level error unifying subsystem failures across Termwave.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum TermwaveError {
    /// Failure originating in the audio engine subsystem.
    #[error("audio error: {0}")]
    Audio(#[from] AudioError),

    /// Failure originating in metadata or artwork extraction.
    #[error("metadata error: {0}")]
    Metadata(#[from] MetadataError),

    /// Failure originating in library scanning or persistence.
    #[error("library error: {0}")]
    Library(#[from] LibraryError),

    /// Failure originating in configuration loading or validation.
    #[error("configuration error: {0}")]
    Config(#[from] ConfigError),

    /// Uncategorized I/O failure.
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn subsystem_errors_convert_into_termwave_error() {
        let audio_err: TermwaveError = AudioError::InvalidState("not playing".into()).into();
        assert!(matches!(audio_err, TermwaveError::Audio(_)));
        assert_eq!(
            audio_err.to_string(),
            "audio error: invalid playback state: not playing"
        );

        let meta_err: TermwaveError = MetadataError::ReadFailed("corrupt header".into()).into();
        assert!(matches!(meta_err, TermwaveError::Metadata(_)));

        let lib_err: TermwaveError = LibraryError::Unavailable("not initialized".into()).into();
        assert!(matches!(lib_err, TermwaveError::Library(_)));

        let cfg_err: TermwaveError = ConfigError::Invalid("bad value".into()).into();
        assert!(matches!(cfg_err, TermwaveError::Config(_)));
    }
}
