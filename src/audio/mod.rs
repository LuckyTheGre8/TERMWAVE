//! Audio engine boundary.
//!
//! This module houses the audio source validation, playback state model,
//! and future decoder, processing pipeline, and platform output backend.
//! Audio callbacks and real-time processing remain strictly isolated from
//! terminal rendering and blocking library operations.

pub mod decoder;
pub mod output;
pub mod playback;
pub mod queue;
pub mod validation;
pub mod volume;

use std::fmt;
use std::path::PathBuf;
use thiserror::Error;

pub use decoder::{AudioStreamInfo, DEFAULT_SEEK_STEP, FlacDecoder, SeekCapabilities};
pub use output::{AudioOutput, OutputDeviceInfo, detect_default_output_device};
pub use playback::{
    PlaybackController, PlaybackEvent, PlaybackSummary, PlaybackTrackInfo, play_local_flac,
    play_local_flac_with_controller, play_local_playlist, play_local_playlist_with_controller,
};
pub use queue::TrackQueue;
pub use validation::{AudioFormat, ValidatedAudioFile, validate_audio_file};
pub use volume::{
    DEFAULT_VOLUME, DEFAULT_VOLUME_STEP, MAX_VOLUME, MIN_VOLUME, VolumeState, prevent_clipping,
    scale_sample_safely, validate_volume, volume_from_percent,
};

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

impl PlaybackState {
    /// Returns `true` if transitioning from `self` to `next` is valid.
    pub fn can_transition_to(self, next: Self) -> bool {
        matches!(
            (self, next),
            (Self::Stopped, Self::Loading)
                | (
                    Self::Loading,
                    Self::Playing | Self::Buffering | Self::Stopped | Self::Error
                )
                | (
                    Self::Playing,
                    Self::Paused
                        | Self::Loading
                        | Self::Seeking
                        | Self::Buffering
                        | Self::Stopped
                        | Self::Ended
                        | Self::Error
                )
                | (
                    Self::Paused,
                    Self::Playing | Self::Loading | Self::Seeking | Self::Stopped | Self::Error
                )
                | (
                    Self::Buffering,
                    Self::Playing | Self::Paused | Self::Stopped | Self::Error
                )
                | (
                    Self::Seeking,
                    Self::Playing | Self::Paused | Self::Buffering | Self::Stopped | Self::Error
                )
                | (Self::Ended | Self::Error, Self::Loading | Self::Stopped)
        )
    }

    /// Validates and applies a transition from `self` to `next`.
    ///
    /// Returns [`AudioError::InvalidState`] if the transition is not permitted.
    pub fn transition_to(&mut self, next: Self) -> Result<(), AudioError> {
        if self.can_transition_to(next) {
            *self = next;
            Ok(())
        } else {
            Err(AudioError::InvalidState(format!(
                "cannot transition playback state from {self} to {next}"
            )))
        }
    }
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

    /// Provided volume level or adjustment is invalid or out of range.
    #[error("invalid audio volume: {0}")]
    InvalidVolume(String),

    /// Audio source does not support seeking operations.
    #[error("seeking unsupported: {0}")]
    SeekUnsupported(String),

    /// Failed to seek within the audio stream.
    #[error("audio seek error: {0}")]
    SeekError(String),

    /// No audio output device is available on the system.
    #[error("no audio output device available")]
    NoOutputDevice,

    /// Audio output device was disconnected or became unavailable.
    #[error("audio output device disconnected: {0}")]
    DeviceLost(String),

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

    #[test]
    fn playback_state_allows_valid_transitions_and_rejects_invalid_ones() {
        let mut state = PlaybackState::Stopped;

        // Invalid from Stopped
        assert!(matches!(
            state.transition_to(PlaybackState::Paused),
            Err(AudioError::InvalidState(_))
        ));
        assert!(matches!(
            state.transition_to(PlaybackState::Playing),
            Err(AudioError::InvalidState(_))
        ));
        assert!(matches!(
            state.transition_to(PlaybackState::Ended),
            Err(AudioError::InvalidState(_))
        ));
        assert_eq!(state, PlaybackState::Stopped);

        // Stopped -> Loading -> Playing -> Paused -> Playing -> Ended -> Stopped
        state.transition_to(PlaybackState::Loading).unwrap();
        assert!(matches!(
            state.transition_to(PlaybackState::Paused),
            Err(AudioError::InvalidState(_))
        ));
        state.transition_to(PlaybackState::Playing).unwrap();
        assert!(matches!(
            state.transition_to(PlaybackState::Playing),
            Err(AudioError::InvalidState(_))
        ));
        state.transition_to(PlaybackState::Paused).unwrap();
        assert!(matches!(
            state.transition_to(PlaybackState::Paused),
            Err(AudioError::InvalidState(_))
        ));
        assert!(matches!(
            state.transition_to(PlaybackState::Ended),
            Err(AudioError::InvalidState(_))
        ));
        state.transition_to(PlaybackState::Playing).unwrap();
        state.transition_to(PlaybackState::Ended).unwrap();
        assert!(matches!(
            state.transition_to(PlaybackState::Paused),
            Err(AudioError::InvalidState(_))
        ));
        assert!(matches!(
            state.transition_to(PlaybackState::Playing),
            Err(AudioError::InvalidState(_))
        ));
        state.transition_to(PlaybackState::Stopped).unwrap();
    }
}
