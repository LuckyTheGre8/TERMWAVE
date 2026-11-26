//! End-to-end tests for local FLAC playback pipeline.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use termwave::audio::{
    AudioError, PlaybackController, PlaybackEvent, PlaybackState, play_local_flac,
    play_local_flac_with_controller, play_local_playlist, play_local_playlist_with_controller,
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

#[test]
fn supports_controller_stop_during_active_playback_and_resets_position() {
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
        controller.stop().unwrap();
        assert_eq!(controller.state(), PlaybackState::Stopped);
        assert_eq!(controller.position(), Duration::ZERO);
        assert!(matches!(
            controller.stop(),
            Err(AudioError::InvalidState(_))
        ));
    }

    match handle.join().expect("playback thread should join cleanly") {
        Ok(summary) => {
            assert_eq!(summary.final_state, PlaybackState::Stopped);
            assert_eq!(controller.state(), PlaybackState::Stopped);
            assert_eq!(controller.position(), Duration::ZERO);

            let recorded = events.lock().unwrap().clone();
            assert_eq!(
                recorded,
                vec![
                    PlaybackState::Loading,
                    PlaybackState::Playing,
                    PlaybackState::Stopped,
                ]
            );

            // Verify controller can cleanly start a subsequent track after stopping.
            let mut next_states = Vec::new();
            let replay = play_local_flac_with_controller(
                fixture_path("sine_16bit_44100_stereo.flac"),
                controller.clone(),
                Arc::new(AtomicBool::new(false)),
                |event| {
                    if let PlaybackEvent::StateChanged(state) = event {
                        next_states.push(state);
                    }
                },
            )
            .expect("controller reuse after stop should succeed");
            assert_eq!(replay.final_state, PlaybackState::Ended);
            assert_eq!(
                next_states,
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
fn supports_controller_stop_during_loading() {
    let controller = PlaybackController::new();
    let shutdown = Arc::new(AtomicBool::new(false));
    let mut states = Vec::new();

    let result = play_local_flac_with_controller(
        fixture_path("sine_16bit_44100_stereo.flac"),
        controller.clone(),
        shutdown,
        |event| {
            if let PlaybackEvent::StateChanged(state) = event {
                states.push(state);
                if state == PlaybackState::Loading {
                    controller.stop().unwrap();
                }
            }
        },
    )
    .expect("stopping during loading should succeed cleanly");

    assert_eq!(result.final_state, PlaybackState::Stopped);
    assert_eq!(result.elapsed, Duration::ZERO);
    assert_eq!(controller.state(), PlaybackState::Stopped);
    assert_eq!(controller.position(), Duration::ZERO);
    assert_eq!(states, vec![PlaybackState::Loading, PlaybackState::Stopped]);
}

#[test]
fn rejects_empty_playlist() {
    let empty: Vec<PathBuf> = Vec::new();
    let shutdown = Arc::new(AtomicBool::new(false));
    let err = play_local_playlist(&empty, shutdown, |_| {}).unwrap_err();
    assert!(matches!(err, AudioError::InvalidState(_)));
}

#[test]
fn plays_playlist_tracks_sequentially_to_completion() {
    let tracks = vec![
        fixture_path("sine_16bit_44100_stereo.flac"),
        fixture_path("sine_24bit_48000_stereo.flac"),
    ];
    let controller = PlaybackController::new();
    let shutdown = Arc::new(AtomicBool::new(false));
    let mut loaded_tracks = Vec::new();
    let mut states = Vec::new();

    let result =
        play_local_playlist_with_controller(&tracks, controller.clone(), shutdown, |event| {
            match event {
                PlaybackEvent::TrackLoaded(info) => loaded_tracks.push(info.file.path),
                PlaybackEvent::StateChanged(state) => states.push(state),
            }
        });

    match result {
        Ok(summary) => {
            assert_eq!(summary.final_state, PlaybackState::Ended);
            assert_eq!(loaded_tracks.len(), 2);
            assert_eq!(controller.played_history().len(), 2);
            assert_eq!(controller.back_history().len(), 1);
            assert_eq!(
                states,
                vec![
                    PlaybackState::Loading,
                    PlaybackState::Playing,
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
fn supports_next_previous_navigation_and_boundaries_during_playback() {
    let t1 = fixture_path("sine_16bit_44100_stereo.flac");
    let t2 = fixture_path("sine_24bit_96000_mono.flac");
    let t3 = fixture_path("sine_24bit_192000_stereo.flac");
    let tracks = vec![t1.clone(), t2.clone(), t3.clone()];

    let controller = PlaybackController::new();
    let shutdown = Arc::new(AtomicBool::new(false));
    let loaded_tracks = Arc::new(Mutex::new(Vec::new()));
    let (playing_tx, playing_rx) = std::sync::mpsc::sync_channel::<usize>(8);
    let playing_count = Arc::new(std::sync::atomic::AtomicUsize::new(0));

    let worker_controller = controller.clone();
    let worker_shutdown = Arc::clone(&shutdown);
    let worker_loaded = Arc::clone(&loaded_tracks);
    let worker_count = Arc::clone(&playing_count);

    let handle = thread::spawn(move || {
        play_local_playlist_with_controller(&tracks, worker_controller, worker_shutdown, |event| {
            match event {
                PlaybackEvent::TrackLoaded(info) => {
                    worker_loaded.lock().unwrap().push(info.file.path);
                }
                PlaybackEvent::StateChanged(PlaybackState::Playing) => {
                    let count = worker_count.fetch_add(1, Ordering::AcqRel) + 1;
                    let _ = playing_tx.try_send(count);
                }
                PlaybackEvent::StateChanged(_) => {}
            }
        })
    });

    // Wait for Track 1 to start playing
    if playing_rx.recv_timeout(Duration::from_secs(3)).is_ok() {
        // Cannot go before track 1 (start boundary)
        assert!(matches!(
            controller.previous_track(),
            Err(AudioError::InvalidState(_))
        ));

        // Skip from track 1 -> track 2
        assert_eq!(controller.next_track().unwrap(), t2);
        // Wait for Track 2 to enter Playing
        let _ = playing_rx.recv_timeout(Duration::from_secs(3));

        // Return from track 2 -> track 1 using playback history
        assert_eq!(controller.previous_track().unwrap(), t1);
        // Wait for Track 1 to re-enter Playing
        let _ = playing_rx.recv_timeout(Duration::from_secs(3));

        // Now start boundary should reject previous again
        assert!(matches!(
            controller.previous_track(),
            Err(AudioError::InvalidState(_))
        ));

        // Skip forward 1 -> 2 -> 3
        assert_eq!(controller.next_track().unwrap(), t2);
        let _ = playing_rx.recv_timeout(Duration::from_secs(3));
        assert_eq!(controller.next_track().unwrap(), t3);
        let _ = playing_rx.recv_timeout(Duration::from_secs(3));

        // End boundary should reject next
        assert!(matches!(
            controller.next_track(),
            Err(AudioError::InvalidState(_))
        ));
    }

    match handle.join().expect("playback thread should join cleanly") {
        Ok(summary) => {
            assert_eq!(summary.final_state, PlaybackState::Ended);
            assert_eq!(
                controller.played_history(),
                vec![t1.clone(), t2.clone(), t1, t2, t3]
            );
        }
        Err(AudioError::NoOutputDevice) => {}
        Err(other) => panic!("unexpected playback error: {other}"),
    }
}
