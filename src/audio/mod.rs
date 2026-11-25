//! Audio engine boundary.
//!
//! This module houses the audio source validation, playback state model,
//! and future decoder, processing pipeline, and platform output backend.
//! Audio callbacks and real-time processing remain strictly isolated from
//! terminal rendering and blocking library operations.

pub mod decoder;
pub mod output;
pub mod playback;
pub mod validation;

use std::fmt;
use std::path::PathBuf;
use thiserror::Error;

pub use decoder::{AudioStreamInfo, FlacDecoder};
pub use output::{AudioOutput, OutputDeviceInfo, detect_default_output_device};
pub use playback::{PlaybackEvent, PlaybackSummary, PlaybackTrackInfo, play_local_flac};
pub use validation::{AudioFormat, ValidatedAudioFile, validate_audio_file};

/// Clearly defined lifecycle state of the playback engine.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PlaybackState {
    /// No active playback.
    #[default]
    Stopped,
    /// Preparing a source and initializing decoding.
    Loading,
    /// Actively outputting decoded audio samples.
    Playing,
    /// Playback suspended with position retained.
    Paused,
    /// Waiting for additional buffered audio data.
    Buffering,
    /// Repositioning the stream read cursor.
    Seeking,
    /// Current track reached end-of-stream cleanly.
    Ended,
    /// Playback halted due to a recoverable or fatal error.
    Error,
}

impl fmt::Display for PlaybackState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let label = match self {
            Self::Stopped => "Stopped",
            Self::Loading => "Loading",
            Self::Playing => "Playing",
            Self::Paused => "Paused",
            Self::Buffering => "Buffering",
            Self::Seeking => "Seeking",
            Self::Ended => "Ended",
            Self::Error => "Error",
        };
        f.write_str(label)
    }
}

/// Errors produced by the audio subsystem.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum AudioError {
    /// Provided filesystem path is syntactically invalid or unusable.
    #[error("invalid audio file path '{}': {reason}", path.display())]
    InvalidPath {
        /// Path that failed validation.
        path: PathBuf,
        /// Explanation of why the path is invalid.
        reason: String,
    },

    /// Audio file does not exist at the specified location.
    #[error("audio file not found: '{}'", path.display())]
    FileNotFound {
        /// Missing file path.
        path: PathBuf,
    },

    /// Path exists but does not refer to a regular file.
    #[error("path is not a regular file: '{}'", path.display())]
    NotAFile {
        /// Path pointing to a directory or special file.
        path: PathBuf,
    },

    /// Insufficient permissions to inspect or read the audio file.
    #[error("permission denied accessing audio file: '{}'", path.display())]
    PermissionDenied {
        /// Unreadable file path.
        path: PathBuf,
    },

    /// File is zero bytes long and contains no audio stream.
    #[error("audio file is empty (0 bytes): '{}'", path.display())]
    EmptyFile {
        /// Empty file path.
        path: PathBuf,
    },

    /// File extension or container signature is not supported.
    #[error("unsupported audio format for '{}': {reason}", path.display())]
    UnsupportedFormat {
        /// File path with an unsupported or unrecognized format.
        path: PathBuf,
        /// Details about the unsupported extension or header.
        reason: String,
    },

    /// Audio stream uses an unsupported sample rate, channel layout, bit depth, or codec.
    #[error("unsupported audio configuration: {0}")]
    UnsupportedConfiguration(String),

    /// Audio stream could not be decoded or is corrupted.
    #[error("audio decode error: {0}")]
    DecodeError(String),

    /// Attempted an operation that is invalid in the current playback state.
    #[error("invalid playback state: {0}")]
    InvalidState(String),

    /// No audio output device is available on the system.
    #[error("no audio output device available")]
    NoOutputDevice,

    /// System audio output backend failure.
    #[error("audio output backend failure: {0}")]
    Backend(String),

    /// Underlying I/O error while reading audio data.
    #[error("audio I/O error: {0}")]
    Io(#[from] std::io::Error),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn playback_state_defaults_to_stopped() {
        assert_eq!(PlaybackState::default(), PlaybackState::Stopped);
        assert_eq!(PlaybackState::Stopped.to_string(), "Stopped");
    }
}
