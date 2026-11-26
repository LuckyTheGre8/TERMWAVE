//! End-to-end tests for local FLAC playback pipeline.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use termwave::audio::{
    AudioError, PlaybackController, PlaybackEvent, PlaybackState, play_local_flac,
    play_local_flac_with_controller,
};

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

#[test]
fn reports_error_for_missing_file() {
    let shutdown = Arc::new(AtomicBool::new(false));
    let mut states = Vec::new();
    let result = play_local_flac("non_existent_audio_file_12345.flac", shutdown, |event| {
        if let PlaybackEvent::StateChanged(state) = event {
            states.push(state);
        }
    });

    match result {
        Err(AudioError::FileNotFound { path }) => {
            assert_eq!(path, PathBuf::from("non_existent_audio_file_12345.flac"));
            assert_eq!(states, vec![PlaybackState::Loading, PlaybackState::Error]);
        }
        other => panic!("expected FileNotFound, got {other:?}"),
    }
}

#[test]
fn reports_error_for_directory_input() {
    let shutdown = Arc::new(AtomicBool::new(false));
    let mut states = Vec::new();
    let result = play_local_flac(fixture_path(""), shutdown, |event| {
        if let PlaybackEvent::StateChanged(state) = event {
            states.push(state);
        }
    });

    match result {
        Err(AudioError::NotAFile { .. }) => {
            assert_eq!(states, vec![PlaybackState::Loading, PlaybackState::Error]);
        }
        other => panic!("expected NotAFile, got {other:?}"),
    }
}

#[test]
fn reports_error_for_zero_byte_flac_file() {
    let temp_path =
        std::env::temp_dir().join(format!("termwave_empty_{}.flac", std::process::id()));
    fs::write(&temp_path, b"").expect("write empty temp file");

    let shutdown = Arc::new(AtomicBool::new(false));
    let mut states = Vec::new();
    let result = play_local_flac(&temp_path, shutdown, |event| {
        if let PlaybackEvent::StateChanged(state) = event {
            states.push(state);
        }
    });
    let _ = fs::remove_file(&temp_path);

    match result {
        Err(AudioError::EmptyFile { .. }) => {
            assert_eq!(states, vec![PlaybackState::Loading, PlaybackState::Error]);
        }
        other => panic!("expected EmptyFile, got {other:?}"),
    }
}

#[test]
fn reports_error_for_truncated_stream_mid_playback() {
    let full = fs::read(fixture_path("sine_24bit_192000_stereo.flac")).expect("read fixture");
    // Keep header and first packet, drop the rest
    let truncated = full[..full.len() - 1024].to_vec();

    let temp_path = std::env::temp_dir().join(format!(
        "termwave_mid_truncated_{}.flac",
        std::process::id()
    ));
    fs::write(&temp_path, &truncated).expect("write truncated temp file");

    let shutdown = Arc::new(AtomicBool::new(false));
    let mut states = Vec::new();
    let result = play_local_flac(&temp_path, shutdown, |event| {
        if let PlaybackEvent::StateChanged(state) = event {
            states.push(state);
        }
    });
    let _ = fs::remove_file(&temp_path);

    match result {
        Err(AudioError::DecodeError(msg)) => {
            assert!(
                msg.contains("truncated")
                    || msg.contains("unexpected end")
                    || msg.contains("corrupted")
            );
            assert_eq!(states.last(), Some(&PlaybackState::Error));
        }
        Err(AudioError::NoOutputDevice) => {}
        other => panic!("expected DecodeError, got {other:?}"),
    }
}

#[test]
fn supports_pause_and_resume_with_state_validation_during_playback() {
    let controller = PlaybackController::new();
    let shutdown = Arc::new(AtomicBool::new(false));
    let events = Arc::new(Mutex::new(Vec::new()));
    let (started_tx, started_rx) = std::sync::mpsc::sync_channel::<()>(1);
    let notified_start = Arc::new(AtomicBool::new(false));

    let worker_controller = controller.clone();
    let worker_shutdown = Arc::clone(&shutdown);
    let worker_events = Arc::clone(&events);
    let worker_notified = Arc::clone(&notified_start);
    let path = fixture_path("sine_24bit_192000_stereo.flac");

    let handle = thread::spawn(move || {
        play_local_flac_with_controller(path, worker_controller, worker_shutdown, |event| {
            if let PlaybackEvent::StateChanged(state) = event {
                worker_events.lock().unwrap().push(state);
                if state == PlaybackState::Playing && !worker_notified.swap(true, Ordering::AcqRel)
                {
                    let _ = started_tx.try_send(());
                }
            }
        })
    });

    if started_rx.recv_timeout(Duration::from_secs(3)).is_ok() {
        // Invalid: resuming while already playing.
        assert!(matches!(
            controller.resume(),
            Err(AudioError::InvalidState(_))
        ));

        // Valid: Playing -> Paused.
        controller.pause().unwrap();
        assert_eq!(controller.state(), PlaybackState::Paused);
        assert!(controller.is_paused());

        // Invalid: pausing while already paused.
        assert!(matches!(
            controller.pause(),
            Err(AudioError::InvalidState(_))
        ));

        // Remain paused longer than the track duration (100ms) and verify playback does not end.
        thread::sleep(Duration::from_millis(160));
        assert_eq!(controller.state(), PlaybackState::Paused);

        // Valid: Paused -> Playing.
        controller.resume().unwrap();
        assert_eq!(controller.state(), PlaybackState::Playing);
        assert!(controller.is_playing());
    }

    match handle.join().expect("playback thread should join cleanly") {
        Ok(summary) => {
            assert_eq!(summary.final_state, PlaybackState::Ended);
            assert_eq!(controller.state(), PlaybackState::Ended);
            // Post-completion pause/resume must be rejected.
            assert!(matches!(
                controller.pause(),
                Err(AudioError::InvalidState(_))
            ));
            assert!(matches!(
                controller.resume(),
                Err(AudioError::InvalidState(_))
            ));

            let recorded = events.lock().unwrap().clone();
            assert_eq!(
                recorded,
                vec![
                    PlaybackState::Loading,
                    PlaybackState::Playing,
                    PlaybackState::Paused,
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
fn supports_shutdown_while_playback_is_paused() {
    let controller = PlaybackController::new();
    let shutdown = Arc::new(AtomicBool::new(false));
    let events = Arc::new(Mutex::new(Vec::new()));
    let (started_tx, started_rx) = std::sync::mpsc::sync_channel::<()>(1);
    let notified_start = Arc::new(AtomicBool::new(false));

    let worker_controller = controller.clone();
    let worker_shutdown = Arc::clone(&shutdown);
    let worker_events = Arc::clone(&events);
    let worker_notified = Arc::clone(&notified_start);
    let path = fixture_path("sine_24bit_192000_stereo.flac");

    let handle = thread::spawn(move || {
        play_local_flac_with_controller(path, worker_controller, worker_shutdown, |event| {
            if let PlaybackEvent::StateChanged(state) = event {
                worker_events.lock().unwrap().push(state);
                if state == PlaybackState::Playing && !worker_notified.swap(true, Ordering::AcqRel)
                {
                    let _ = started_tx.try_send(());
                }
            }
        })
    });

    if started_rx.recv_timeout(Duration::from_secs(3)).is_ok() {
        controller.pause().unwrap();
        assert_eq!(controller.state(), PlaybackState::Paused);
        shutdown.store(true, Ordering::Release);
    }

    match handle.join().expect("playback thread should join cleanly") {
        Ok(summary) => {
            assert_eq!(summary.final_state, PlaybackState::Stopped);
            assert_eq!(controller.state(), PlaybackState::Stopped);
            let recorded = events.lock().unwrap().clone();
            assert_eq!(
                recorded,
                vec![
                    PlaybackState::Loading,
                    PlaybackState::Playing,
                    PlaybackState::Paused,
                    PlaybackState::Stopped,
                ]
            );
        }
        Err(AudioError::NoOutputDevice) => {}
        Err(other) => panic!("unexpected playback error: {other}"),
    }
}
