//! Now-playing view model formatting track, artist, album, playback state,
//! and source-vs-output audio properties with missing-metadata fallbacks.

use std::path::Path;
use std::time::Duration;

use ratatui::style::Color;

use crate::audio::{PlaybackSnapshot, PlaybackState};

/// Formatted now-playing display fields derived from a [`PlaybackSnapshot`].
#[derive(Debug, Clone, PartialEq)]
pub struct NowPlayingInfo {
    /// Current engine playback state.
    pub state: PlaybackState,
    /// Human-readable state label (e.g. `"Playing"`, `"Paused"`, `"Stopped"`).
    pub state_label: &'static str,
    /// Semantic color for the playback state badge.
    pub state_color: Color,
    /// Whether a track is currently loaded in the snapshot.
    pub has_track: bool,
    /// Track title (falls back to filename if title tag is missing).
    pub title: String,
    /// Track artist (falls back to `"Unknown Artist"` when a track is loaded).
    pub artist: String,
    /// Album title with optional year (falls back to `"Unknown Album"` when a track is loaded).
    pub album: String,
    /// Source codec/sample-rate/bit-depth/channels vs output sample rate summary.
    pub audio_info: String,
    /// Formatted `elapsed / total` duration string.
    pub position_label: String,
    /// Formatted volume percentage and mute indicator.
    pub volume_label: String,
    /// Optional queue position badge (e.g. `"[2/10]"`).
    pub queue_label: Option<String>,
    /// Last playback error message, if any.
    pub last_error: Option<String>,
    /// Elapsed playback position.
    pub elapsed: Duration,
    /// Total track duration, if known.
    pub duration: Option<Duration>,
    /// Formatted elapsed timestamp (`MM:SS` or `HH:MM:SS`).
    pub elapsed_label: String,
    /// Formatted total duration (`MM:SS`, `HH:MM:SS`, or `--:--`).
    pub total_label: String,
    /// Normalized progress ratio clamped to `0.0..=1.0`.
    pub progress_ratio: f64,
    /// Whether the active track supports seeking.
    pub seekable: bool,
}

impl NowPlayingInfo {
    /// Builds a [`NowPlayingInfo`] view model from a centralized [`PlaybackSnapshot`].
    pub fn from_snapshot(snapshot: &PlaybackSnapshot) -> Self {
        let (state_label, state_color) = format_state_badge(snapshot.state);
        let elapsed_label = format_duration(snapshot.position);
        let total_label = snapshot
            .duration
            .map_or_else(|| "--:--".to_string(), format_duration);
        let position_label = format!("{elapsed_label} / {total_label}");
        let progress_ratio = compute_progress_ratio(snapshot.position, snapshot.duration);
        let volume_label = if snapshot.muted {
            format!("Vol {}% (muted)", snapshot.volume_percent)
        } else {
            format!("Vol {}%", snapshot.volume_percent)
        };
        let queue_label = match (snapshot.track_index, snapshot.track_count) {
            (Some(idx), count) if count > 1 => Some(format!("[{}/{}]", idx + 1, count)),
            _ => None,
        };

        if let Some(track) = &snapshot.track {
            let title = clean_or_fallback(track.metadata.title.as_deref(), || {
                fallback_filename(&track.file.path)
            });
            let artist =
                clean_or_fallback(track.metadata.artist.as_deref(), || "Unknown Artist".into());
            let base_album =
                clean_or_fallback(track.metadata.album.as_deref(), || "Unknown Album".into());
            let album = match track.metadata.year {
                Some(year) => format!("{base_album} ({year})"),
                None => base_album,
            };

            let bit_depth = track
                .stream
                .bits_per_sample
                .map_or_else(|| "?-bit".to_string(), |b| format!("{b}-bit"));
            let src_rate = format_khz(track.stream.sample_rate);
            let out_rate = format_khz(track.output.sample_rate);
            let audio_info = format!(
                "{} {}/{} {}ch -> Out {}",
                track.stream.codec, bit_depth, src_rate, track.stream.channels, out_rate
            );

            Self {
                state: snapshot.state,
                state_label,
                state_color,
                has_track: true,
                title,
                artist,
                album,
                audio_info,
                position_label,
                volume_label,
                queue_label,
                last_error: snapshot.last_error.clone(),
                elapsed: snapshot.position,
                duration: snapshot.duration,
                elapsed_label,
                total_label,
                progress_ratio,
                seekable: track.stream.seekable,
            }
        } else {
            Self {
                state: snapshot.state,
                state_label,
                state_color,
                has_track: false,
                title: "No track loaded".to_string(),
                artist: "—".to_string(),
                album: "—".to_string(),
                audio_info: "FLAC Lossless Engine Ready".to_string(),
                position_label,
                volume_label,
                queue_label,
                last_error: snapshot.last_error.clone(),
                elapsed: snapshot.position,
                duration: snapshot.duration,
                elapsed_label,
                total_label,
                progress_ratio,
                seekable: false,
            }
        }
    }
}

/// Computes a clamped `0.0..=1.0` progress ratio from `elapsed` and `duration`.
pub fn compute_progress_ratio(elapsed: Duration, duration: Option<Duration>) -> f64 {
    let Some(total) = duration else {
        return 0.0;
    };
    let total_secs = total.as_secs_f64();
    if total_secs <= 0.0 || !total_secs.is_finite() {
        return 0.0;
    }
    (elapsed.as_secs_f64() / total_secs).clamp(0.0, 1.0)
}

/// Computes a seek target [`Duration`] for a horizontal position `col` inside `bar_width`.
pub fn seek_target_from_column(
    col: u16,
    bar_x: u16,
    bar_width: u16,
    duration: Duration,
) -> Option<Duration> {
    if bar_width == 0 || duration.is_zero() {
        return None;
    }
    let rel = col.saturating_sub(bar_x).min(bar_width);
    let ratio = (rel as f64 / bar_width as f64).clamp(0.0, 1.0);
    Some(Duration::from_secs_f64(duration.as_secs_f64() * ratio))
}

fn format_state_badge(state: PlaybackState) -> (&'static str, Color) {
    match state {
        PlaybackState::Stopped => ("Stopped", Color::Yellow),
        PlaybackState::Loading => ("Loading", Color::Cyan),
        PlaybackState::Playing => ("Playing", Color::Green),
        PlaybackState::Paused => ("Paused", Color::Yellow),
        PlaybackState::Buffering => ("Buffering", Color::Cyan),
        PlaybackState::Seeking => ("Seeking", Color::Cyan),
        PlaybackState::Ended => ("Ended", Color::Gray),
        PlaybackState::Error => ("Error", Color::Red),
    }
}

fn clean_or_fallback(value: Option<&str>, fallback: impl FnOnce() -> String) -> String {
    value
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map_or_else(fallback, str::to_string)
}

fn fallback_filename(path: &Path) -> String {
    path.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.display().to_string())
}

fn format_duration(duration: Duration) -> String {
    let total_secs = duration.as_secs();
    let hours = total_secs / 3600;
    let mins = (total_secs % 3600) / 60;
    let secs = total_secs % 60;
    if hours > 0 {
        format!("{hours}:{mins:02}:{secs:02}")
    } else {
        format!("{mins:02}:{secs:02}")
    }
}

fn format_khz(sample_rate_hz: u32) -> String {
    if sample_rate_hz.is_multiple_of(1000) {
        format!("{} kHz", sample_rate_hz / 1000)
    } else {
        format!("{:.1} kHz", sample_rate_hz as f64 / 1000.0)
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use crate::audio::{
        AudioFormat, AudioStreamInfo, OutputDeviceInfo, PlaybackController, PlaybackTrackInfo,
        ValidatedAudioFile,
    };
    use crate::metadata::TrackMetadata;

    use super::*;

    fn sample_snapshot(metadata: TrackMetadata, state: PlaybackState) -> PlaybackSnapshot {
        let mut snap = PlaybackController::new().snapshot();
        snap.state = state;
        snap.position = Duration::from_secs(65);
        snap.duration = Some(Duration::from_secs(240));
        snap.volume_percent = 85;
        snap.track_index = Some(1);
        snap.track_count = 5;
        snap.track = Some(PlaybackTrackInfo {
            file: ValidatedAudioFile {
                path: PathBuf::from("/music/02_midnight_groove.flac"),
                format: AudioFormat::Flac,
                size_bytes: 24_000_000,
            },
            stream: AudioStreamInfo {
                codec: "FLAC",
                sample_rate: 96_000,
                channels: 2,
                bits_per_sample: Some(24),
                duration: Some(Duration::from_secs(240)),
                total_frames: Some(96_000 * 240),
                seekable: true,
            },
            output: OutputDeviceInfo {
                name: "Default DAC".to_string(),
                sample_rate: 48_000,
                channels: 2,
                sample_format: "f32".to_string(),
            },
            metadata,
        });
        snap
    }

    #[test]
    fn formats_now_playing_with_full_metadata() {
        let meta = TrackMetadata {
            title: Some("Midnight Groove".to_string()),
            artist: Some("Acoustic Wave".to_string()),
            album: Some("Late Night Sessions".to_string()),
            year: Some(2024),
            ..TrackMetadata::default()
        };
        let snap = sample_snapshot(meta, PlaybackState::Playing);
        let np = NowPlayingInfo::from_snapshot(&snap);

        assert!(np.has_track);
        assert_eq!(np.state_label, "Playing");
        assert_eq!(np.title, "Midnight Groove");
        assert_eq!(np.artist, "Acoustic Wave");
        assert_eq!(np.album, "Late Night Sessions (2024)");
        assert_eq!(np.audio_info, "FLAC 24-bit/96 kHz 2ch -> Out 48 kHz");
        assert_eq!(np.position_label, "01:05 / 04:00");
        assert_eq!(np.volume_label, "Vol 85%");
        assert_eq!(np.queue_label.as_deref(), Some("[2/5]"));
    }

    #[test]
    fn handles_missing_metadata_and_empty_snapshot_gracefully() {
        let snap = sample_snapshot(TrackMetadata::default(), PlaybackState::Paused);
        let np = NowPlayingInfo::from_snapshot(&snap);

        assert!(np.has_track);
        assert_eq!(np.state_label, "Paused");
        assert_eq!(np.title, "02_midnight_groove.flac");
        assert_eq!(np.artist, "Unknown Artist");
        assert_eq!(np.album, "Unknown Album");

        let empty_snap = PlaybackController::new().snapshot();
        let idle = NowPlayingInfo::from_snapshot(&empty_snap);
        assert!(!idle.has_track);
        assert_eq!(idle.state_label, "Stopped");
        assert_eq!(idle.title, "No track loaded");
    }

    #[test]
    fn computes_progress_ratios_and_interactive_seek_targets() {
        assert_eq!(compute_progress_ratio(Duration::from_secs(30), None), 0.0);
        assert_eq!(
            compute_progress_ratio(Duration::from_secs(30), Some(Duration::ZERO)),
            0.0
        );
        assert!(
            (compute_progress_ratio(Duration::from_secs(60), Some(Duration::from_secs(240)))
                - 0.25)
                .abs()
                < 1e-9
        );
        assert_eq!(
            compute_progress_ratio(Duration::from_secs(300), Some(Duration::from_secs(240))),
            1.0
        );

        let mid = seek_target_from_column(60, 10, 100, Duration::from_secs(200)).unwrap();
        assert_eq!(mid, Duration::from_secs(100));
        assert_eq!(
            seek_target_from_column(0, 0, 0, Duration::from_secs(200)),
            None
        );
    }
}
