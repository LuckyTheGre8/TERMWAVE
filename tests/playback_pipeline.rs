//! End-to-end tests for local FLAC playback pipeline.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use termwave::audio::{AudioError, PlaybackEvent, PlaybackState, play_local_flac};

fn fixture_path(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join(name)
}

#[test]
fn plays_16bit_44100hz_stereo_flac_to_completion() {
    let shutdown = Arc::new(AtomicBool::new(false));
    let mut states = Vec::new();
    let mut loaded = None;

    let result = play_local_flac(
        fixture_path("sine_16bit_44100_stereo.flac"),
        shutdown,
        |event| match event {
            PlaybackEvent::StateChanged(state) => states.push(state),
            PlaybackEvent::TrackLoaded(info) => loaded = Some(info),
        },
    );

    match result {
        Ok(summary) => {
            assert_eq!(summary.final_state, PlaybackState::Ended);
            assert_eq!(summary.stream.sample_rate, 44_100);
            assert_eq!(summary.stream.bits_per_sample, Some(16));
            assert_eq!(summary.stream.channels, 2);
            assert!(loaded.is_some());
            assert_eq!(
                states,
                vec![
                    PlaybackState::Loading,
                    PlaybackState::Playing,
                    PlaybackState::Ended,
                ]
            );
        }
        Err(AudioError::NoOutputDevice) => {
            // Acceptable in headless CI environments without audio hardware.
        }
        Err(other) => panic!("unexpected playback error: {other}"),
    }
}

#[test]
fn plays_24bit_96000hz_mono_flac_to_completion() {
    let shutdown = Arc::new(AtomicBool::new(false));
    let mut states = Vec::new();

    let result = play_local_flac(
        fixture_path("sine_24bit_96000_mono.flac"),
        shutdown,
        |event| {
            if let PlaybackEvent::StateChanged(state) = event {
                states.push(state);
            }
        },
    );

    match result {
        Ok(summary) => {
            assert_eq!(summary.final_state, PlaybackState::Ended);
            assert_eq!(summary.stream.sample_rate, 96_000);
            assert_eq!(summary.stream.bits_per_sample, Some(24));
            assert_eq!(summary.stream.channels, 1);
            assert_eq!(
                states,
                vec![
                    PlaybackState::Loading,
                    PlaybackState::Playing,
                    PlaybackState::Ended,
                ]
            );
        }
        Err(AudioError::NoOutputDevice) => {}
        Err(other) => panic!("unexpected playback error: {other}"),
    }
}

#[test]
fn supports_clean_early_shutdown_during_playback() {
    let shutdown = Arc::new(AtomicBool::new(false));
    let shutdown_signal = Arc::clone(&shutdown);
    let mut states = Vec::new();
    let start = Instant::now();

    let result = play_local_flac(
        fixture_path("sine_24bit_192000_stereo.flac"),
        shutdown,
        |event| {
            if let PlaybackEvent::StateChanged(state) = event {
                states.push(state);
                if state == PlaybackState::Playing {
                    shutdown_signal.store(true, Ordering::Release);
                }
            }
        },
    );

    match result {
        Ok(summary) => {
            assert_eq!(summary.final_state, PlaybackState::Stopped);
            assert!(
                start.elapsed() < Duration::from_secs(2),
                "shutdown should return promptly without hanging"
            );
            assert_eq!(
                states,
                vec![
                    PlaybackState::Loading,
                    PlaybackState::Playing,
                    PlaybackState::Stopped,
                ]
            );
        }
        Err(AudioError::NoOutputDevice) => {}
        Err(other) => panic!("unexpected shutdown error: {other}"),
    }
}

#[test]
fn reports_error_for_corrupted_flac_file() {
    let mut bytes =
        fs::read(fixture_path("sine_16bit_44100_stereo.flac")).expect("read fixture bytes");
    let start = bytes.len().saturating_sub(256);
    for byte in &mut bytes[start..start + 64] {
        *byte ^= 0xFF;
    }

    let temp_path = std::env::temp_dir().join(format!(
        "termwave_corrupt_playback_{}.flac",
        std::process::id()
    ));
    fs::write(&temp_path, &bytes).expect("write corrupted temp flac");

    let shutdown = Arc::new(AtomicBool::new(false));
    let mut states = Vec::new();
    let result = play_local_flac(&temp_path, shutdown, |event| {
        if let PlaybackEvent::StateChanged(state) = event {
            states.push(state);
        }
    });
    let _ = fs::remove_file(&temp_path);

    match result {
        Err(AudioError::DecodeError(_)) => {
            assert_eq!(states.first(), Some(&PlaybackState::Loading));
            assert_eq!(states.last(), Some(&PlaybackState::Error));
        }
        Err(AudioError::NoOutputDevice) => {}
        other => panic!("expected DecodeError, got {other:?}"),
    }
}
