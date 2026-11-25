//! Audio engine boundary.
//!
//! This module will house the audio source abstraction, decoder integration,
//! processing pipeline, playback manager, and platform output backend.
//! Audio callbacks and real-time processing remain strictly isolated from
//! terminal rendering and blocking library operations.

use std::fmt;
use thiserror::Error;

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
    /// Attempted an operation that is invalid in the current playback state.
    #[error("invalid playback state: {0}")]
    InvalidState(String),

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
