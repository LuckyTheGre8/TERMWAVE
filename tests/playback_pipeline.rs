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

#[test]
fn supports_volume_adjustment_and_mute_across_playlist_playback() {
    let tracks = vec![
        fixture_path("sine_16bit_44100_stereo.flac"),
        fixture_path("sine_24bit_48000_stereo.flac"),
    ];
    let controller = PlaybackController::new();
    let shutdown = Arc::new(AtomicBool::new(false));
    let (playing_tx, playing_rx) = std::sync::mpsc::sync_channel::<()>(4);

    // Preconfigure muted volume at 40% before playback starts.
    controller.set_volume_percent(40).unwrap();
    controller.mute();
    assert_eq!(controller.effective_volume(), 0.0);

    let worker_controller = controller.clone();
    let worker_shutdown = Arc::clone(&shutdown);
    let handle = thread::spawn(move || {
        play_local_playlist_with_controller(&tracks, worker_controller, worker_shutdown, |event| {
            if matches!(event, PlaybackEvent::StateChanged(PlaybackState::Playing)) {
                let _ = playing_tx.try_send(());
            }
        })
    });

    if playing_rx.recv_timeout(Duration::from_secs(3)).is_ok() {
        // Verify invalid range rejected during live playback
        assert!(matches!(
            controller.set_volume(1.5),
            Err(AudioError::InvalidVolume(_))
        ));
        assert!(controller.is_muted());
        assert_eq!(controller.volume_percent(), 40);

        // Unmute and adjust volume while Track 1 is playing
        controller.unmute();
        assert!(!controller.is_muted());
        controller.increase_volume();
        assert_eq!(controller.volume_percent(), 45);

        // Skip to Track 2 and confirm volume persists across track changes
        let _ = controller.next_track();
        let _ = playing_rx.recv_timeout(Duration::from_secs(3));
        assert_eq!(controller.volume_percent(), 45);
        assert!(!controller.is_muted());
    }

    match handle.join().expect("playback thread should join cleanly") {
        Ok(summary) => {
            assert_eq!(summary.final_state, PlaybackState::Ended);
            assert_eq!(controller.volume_percent(), 45);
        }
        Err(AudioError::NoOutputDevice) => {}
        Err(other) => panic!("unexpected playback error: {other}"),
    }
}

#[test]
fn supports_seeking_during_playback_and_pause() {
    let controller = PlaybackController::new();
    let shutdown = Arc::new(AtomicBool::new(false));
    let events = Arc::new(Mutex::new(Vec::new()));
    let (playing_tx, playing_rx) = std::sync::mpsc::sync_channel::<PlaybackState>(16);

    let worker_controller = controller.clone();
    let worker_shutdown = Arc::clone(&shutdown);
    let worker_events = Arc::clone(&events);
    let path = fixture_path("sine_24bit_192000_stereo.flac");

    let handle = thread::spawn(move || {
        play_local_flac_with_controller(path, worker_controller, worker_shutdown, |event| {
            if let PlaybackEvent::StateChanged(state) = event {
                worker_events.lock().unwrap().push(state);
                if matches!(state, PlaybackState::Playing | PlaybackState::Paused) {
                    let _ = playing_tx.try_send(state);
                }
            }
        })
    });

    if let Ok(PlaybackState::Playing) = playing_rx.recv_timeout(Duration::from_secs(3)) {
        assert!(controller.is_seekable());
        assert_eq!(controller.duration(), Some(Duration::from_millis(100)));

        // Seek forward during active playback
        let landed_fwd = controller.seek_forward(Duration::from_millis(40)).unwrap();
        assert!(landed_fwd >= Duration::from_millis(35));
        assert_eq!(
            playing_rx.recv_timeout(Duration::from_secs(3)).unwrap(),
            PlaybackState::Playing
        );

        // Pause and perform absolute + backward seeking while Paused
        controller.pause().unwrap();
        assert_eq!(
            playing_rx.recv_timeout(Duration::from_secs(3)).unwrap(),
            PlaybackState::Paused
        );

        let landed_abs = controller.seek(Duration::from_millis(70)).unwrap();
        assert_eq!(landed_abs, Duration::from_millis(70));
        // Seeking while Paused must return cleanly to Paused (not auto-resume!)
        assert_eq!(
            playing_rx.recv_timeout(Duration::from_secs(3)).unwrap(),
            PlaybackState::Paused
        );
        assert!(controller.is_paused());

        let landed_back = controller.seek_backward(Duration::from_millis(50)).unwrap();
        assert_eq!(landed_back, Duration::from_millis(20));
        assert_eq!(
            playing_rx.recv_timeout(Duration::from_secs(3)).unwrap(),
            PlaybackState::Paused
        );
        assert!(controller.is_paused());

        // Resume from seeked position and finish cleanly
        controller.resume().unwrap();
    }

    match handle.join().expect("playback thread should join cleanly") {
        Ok(summary) => {
            assert_eq!(summary.final_state, PlaybackState::Ended);
            let recorded = events.lock().unwrap().clone();
            assert!(
                recorded.contains(&PlaybackState::Seeking),
                "expected Seeking state event in {recorded:?}"
            );
            assert_eq!(recorded.first(), Some(&PlaybackState::Loading));
            assert_eq!(recorded.last(), Some(&PlaybackState::Ended));
        }
        Err(AudioError::NoOutputDevice) => {}
        Err(other) => panic!("unexpected playback error: {other}"),
    }
}

#[test]
fn synchronizes_centralized_state_across_subscribers_until_completion() {
    let tracks = vec![
        fixture_path("sine_16bit_44100_stereo.flac"),
        fixture_path("sine_24bit_48000_stereo.flac"),
    ];
    let controller = PlaybackController::new();
    let rx_ui = controller.subscribe();
    let rx_observer = controller.subscribe();
    let shutdown = Arc::new(AtomicBool::new(false));

    let watcher_ctrl = controller.clone();
    let waiter = thread::spawn(move || {
        let mut seen = 0;
        let mut observed_states = Vec::new();
        let deadline = Instant::now() + Duration::from_secs(5);
        while Instant::now() < deadline {
            let snap = watcher_ctrl.wait_for_update(seen, Duration::from_millis(250));
            if snap.revision > seen {
                seen = snap.revision;
                observed_states.push(snap.state);
                if matches!(snap.state, PlaybackState::Ended | PlaybackState::Error) {
                    break;
                }
            }
        }
        observed_states
    });

    let result = play_local_playlist_with_controller(&tracks, controller.clone(), shutdown, |_| {});
    let waited_states = waiter.join().expect("state waiter thread should join");

    let mut ui_snapshots = Vec::new();
    while let Ok(snap) = rx_ui.try_recv() {
        ui_snapshots.push(snap);
    }
    let mut observer_snapshots = Vec::new();
    while let Ok(snap) = rx_observer.try_recv() {
        observer_snapshots.push(snap);
    }

    // Both subscribers always receive strictly matching ordered snapshot streams.
    assert_eq!(ui_snapshots, observer_snapshots);
    for pair in ui_snapshots.windows(2) {
        assert!(pair[0].revision < pair[1].revision);
    }

    match result {
        Ok(summary) => {
            assert_eq!(summary.final_state, PlaybackState::Ended);
            let final_snapshot = controller.snapshot();
            assert_eq!(final_snapshot.state, PlaybackState::Ended);
            assert_eq!(final_snapshot.last_error, None);
            assert_eq!(final_snapshot.track_index, Some(1));
            assert_eq!(final_snapshot.track_count, 2);
            assert!(!final_snapshot.has_next);
            assert!(final_snapshot.has_previous);
            assert!(final_snapshot.track.is_some());
            assert_eq!(
                ui_snapshots.last().map(|s| s.state),
                Some(PlaybackState::Ended)
            );
            assert!(waited_states.contains(&PlaybackState::Playing));
            assert_eq!(waited_states.last(), Some(&PlaybackState::Ended));
        }
        Err(AudioError::NoOutputDevice) => {
            assert_eq!(controller.state(), PlaybackState::Error);
            assert!(controller.last_error().is_some());
        }
        Err(other) => panic!("unexpected playback error: {other}"),
    }
}

#[test]
fn records_centralized_error_state_and_recovers_on_subsequent_playback() {
    let controller = PlaybackController::new();
    let rx = controller.subscribe();
    let temp_dir = std::env::temp_dir().join("termwave_phase14_error_sync_test");
    fs::create_dir_all(&temp_dir).unwrap();
    let corrupt_flac = temp_dir.join("broken.flac");
    fs::write(&corrupt_flac, b"fLaCnot_valid_frames_at_all").unwrap();

    let err = play_local_flac_with_controller(
        &corrupt_flac,
        controller.clone(),
        Arc::new(AtomicBool::new(false)),
        |_| {},
    )
    .expect_err("corrupt FLAC should fail playback");
    let err_msg = err.to_string();

    let snap = controller.snapshot();
    assert_eq!(snap.state, PlaybackState::Error);
    assert_eq!(snap.last_error.as_deref(), Some(err_msg.as_str()));

    let mut received_states = Vec::new();
    while let Ok(s) = rx.try_recv() {
        received_states.push(s);
    }
    assert_eq!(
        received_states.last().map(|s| s.state),
        Some(PlaybackState::Error)
    );
    assert_eq!(
        received_states.last().and_then(|s| s.last_error.as_deref()),
        Some(err_msg.as_str())
    );

    // Subsequent valid load must clear last_error and transition cleanly from Error -> Loading.
    let valid_res = play_local_flac_with_controller(
        fixture_path("sine_16bit_44100_stereo.flac"),
        controller.clone(),
        Arc::new(AtomicBool::new(false)),
        |_| {},
    );
    match valid_res {
        Ok(summary) => {
            assert_eq!(summary.final_state, PlaybackState::Ended);
            assert_eq!(controller.last_error(), None);
            assert_eq!(controller.state(), PlaybackState::Ended);
        }
        Err(AudioError::NoOutputDevice) => {
            assert_eq!(controller.state(), PlaybackState::Error);
            assert!(controller.last_error().is_some());
        }
        Err(other) => panic!("unexpected playback error: {other}"),
    }

    let _ = fs::remove_dir_all(&temp_dir);
}

#[test]
fn concurrent_state_readers_and_controls_remain_race_free() {
    let controller = PlaybackController::new();
    let shutdown = Arc::new(AtomicBool::new(false));
    let done = Arc::new(AtomicBool::new(false));
    let (playing_tx, playing_rx) = std::sync::mpsc::sync_channel::<()>(4);

    let worker_ctrl = controller.clone();
    let worker_shutdown = Arc::clone(&shutdown);
    let playback_handle = thread::spawn(move || {
        play_local_flac_with_controller(
            fixture_path("sine_24bit_192000_stereo.flac"),
            worker_ctrl,
            worker_shutdown,
            |event| {
                if let PlaybackEvent::StateChanged(PlaybackState::Playing) = event {
                    let _ = playing_tx.try_send(());
                }
            },
        )
    });

    // Spawn concurrent snapshot/subscription observers while controls mutate state.
    let mut readers = Vec::new();
    for _ in 0..3 {
        let reader_ctrl = controller.clone();
        let reader_done = Arc::clone(&done);
        let reader_rx = controller.subscribe();
        readers.push(thread::spawn(move || {
            let mut last_rev = 0;
            while !reader_done.load(Ordering::Acquire) {
                let snap = reader_ctrl.snapshot();
                assert!(snap.revision >= last_rev);
                last_rev = snap.revision;
                while let Ok(sub_snap) = reader_rx.try_recv() {
                    assert!(sub_snap.revision <= reader_ctrl.revision());
                }
                thread::sleep(Duration::from_millis(1));
            }
        }));
    }

    if playing_rx.recv_timeout(Duration::from_secs(3)).is_ok() {
        controller.pause().unwrap();
        controller.set_volume_percent(75).unwrap();
        let seek_rev_before = controller.revision();
        let _ = controller.seek(Duration::from_millis(30)).unwrap();

        // Wait via centralized state tracking for seek to settle back to Paused before resuming.
        let mut seen = seek_rev_before;
        let deadline = Instant::now() + Duration::from_secs(3);
        while Instant::now() < deadline {
            let snap = controller.wait_for_update(seen, Duration::from_millis(100));
            seen = snap.revision;
            if snap.state == PlaybackState::Paused {
                break;
            }
        }
        controller.resume().unwrap();
    }

    let result = playback_handle.join().expect("playback thread should join");
    done.store(true, Ordering::Release);
    for reader in readers {
        reader.join().expect("reader thread should join cleanly");
    }

    match result {
        Ok(summary) => assert_eq!(summary.final_state, PlaybackState::Ended),
        Err(AudioError::NoOutputDevice) => {}
        Err(other) => panic!("unexpected playback error: {other}"),
    }
}

#[test]
fn preloads_next_track_and_transitions_consecutively_without_delay() {
    let t1 = fixture_path("sine_16bit_44100_stereo.flac");
    let t2 = fixture_path("sine_24bit_48000_stereo.flac");
    let tracks = vec![t1.clone(), t2.clone()];

    let controller = PlaybackController::new();
    let rx = controller.subscribe();
    let shutdown = Arc::new(AtomicBool::new(false));
    let mut states = Vec::new();

    // Set custom volume to verify preservation across track change
    controller.set_volume_percent(72).unwrap();

    let result =
        play_local_playlist_with_controller(&tracks, controller.clone(), shutdown, |event| {
            if let PlaybackEvent::StateChanged(state) = event {
                states.push(state);
            }
        });

    let mut snapshots = Vec::new();
    while let Ok(snap) = rx.try_recv() {
        snapshots.push(snap);
    }

    match result {
        Ok(summary) => {
            assert_eq!(summary.final_state, PlaybackState::Ended);
            // Verify next track was preloaded during track 1 playback
            assert!(
                snapshots.iter().any(|s| s.is_next_preloaded),
                "expected at least one snapshot with is_next_preloaded == true"
            );
            // Verify volume preserved across consecutive track transitions
            assert_eq!(controller.volume_percent(), 72);
            assert_eq!(controller.state(), PlaybackState::Ended);
            assert_eq!(controller.played_history(), vec![t1, t2]);
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
        Err(AudioError::NoOutputDevice) => {
            assert_eq!(controller.state(), PlaybackState::Error);
        }
        Err(other) => panic!("unexpected playback error: {other}"),
    }
}

#[test]
fn extracts_and_propagates_metadata_during_flac_playback() {
    use lofty::tag::{Accessor, ItemKey, Tag, TagExt, TagType};

    let src = fixture_path("sine_16bit_44100_stereo.flac");
    let temp_dir = std::env::temp_dir().join("termwave_phase16_meta_test");
    fs::create_dir_all(&temp_dir).unwrap();
    let temp_flac = temp_dir.join("tagged_song.flac");
    fs::copy(&src, &temp_flac).expect("copy fixture to temp");

    let mut tag = Tag::new(TagType::VorbisComments);
    tag.set_title("Midnight Groove".to_string());
    tag.set_artist("Acoustic Wave".to_string());
    tag.set_album("Late Night Sessions".to_string());
    tag.set_track(7);
    tag.set_track_total(12);
    tag.set_disk(1);
    tag.set_disk_total(1);
    tag.set_genre("Jazz".to_string());
    tag.insert_text(ItemKey::AlbumArtist, "Wave Trio".to_string());
    tag.insert_text(ItemKey::RecordingDate, "2023-04-15".to_string());
    tag.save_to_path(&temp_flac, lofty::config::WriteOptions::default())
        .expect("save tags");

    let controller = PlaybackController::new();
    let shutdown = Arc::new(AtomicBool::new(false));
    let mut loaded_track = None;

    let result =
        play_local_flac_with_controller(&temp_flac, controller.clone(), shutdown, |event| {
            if let PlaybackEvent::TrackLoaded(info) = event {
                loaded_track = Some(info);
            }
        });

    match result {
        Ok(summary) => {
            assert_eq!(summary.final_state, PlaybackState::Ended);
            let info = loaded_track.expect("track info should be emitted");
            assert_eq!(info.metadata.title.as_deref(), Some("Midnight Groove"));
            assert_eq!(info.metadata.artist.as_deref(), Some("Acoustic Wave"));
            assert_eq!(info.metadata.album.as_deref(), Some("Late Night Sessions"));
            assert_eq!(info.metadata.album_artist.as_deref(), Some("Wave Trio"));
            assert_eq!(info.metadata.genre.as_deref(), Some("Jazz"));
            assert_eq!(info.metadata.track_number, Some(7));
            assert_eq!(info.metadata.track_total, Some(12));
            assert_eq!(info.metadata.disc_number, Some(1));
            assert_eq!(info.metadata.year, Some(2023));
            assert_eq!(
                info.metadata.formatted_track_number(),
                Some("07/12".to_string())
            );
            assert_eq!(
                info.metadata.display_title(&temp_flac),
                "Midnight Groove — Acoustic Wave"
            );

            // Verify controller snapshot reflects the same metadata
            let snap = controller.snapshot();
            let snap_meta = &snap.track.unwrap().metadata;
            assert_eq!(snap_meta.title.as_deref(), Some("Midnight Groove"));
            assert_eq!(snap_meta.artist.as_deref(), Some("Acoustic Wave"));
        }
        Err(AudioError::NoOutputDevice) => {}
        Err(other) => panic!("unexpected playback error: {other}"),
    }

    let _ = fs::remove_dir_all(&temp_dir);
}

#[test]
fn extracts_embedded_artwork_summary_during_flac_playback() {
    use lofty::config::{ParseOptions, WriteOptions};
    use lofty::file::AudioFile;
    use lofty::flac::FlacFile;
    use lofty::ogg::OggPictureStorage;
    use lofty::picture::{MimeType, Picture, PictureInformation, PictureType};
    use termwave::metadata::{ArtworkDimensions, ArtworkFormat, ArtworkRole};

    let src = fixture_path("sine_16bit_44100_stereo.flac");
    let temp_dir = std::env::temp_dir().join("termwave_phase17_playback_artwork");
    fs::create_dir_all(&temp_dir).unwrap();
    let temp_flac = temp_dir.join("cover_song.flac");
    fs::copy(&src, &temp_flac).expect("copy fixture to temp");

    let mut png_data = vec![
        0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A, 0x00, 0x00, 0x00, 0x0D, b'I', b'H', b'D',
        b'R',
    ];
    png_data.extend_from_slice(&300u32.to_be_bytes());
    png_data.extend_from_slice(&300u32.to_be_bytes());
    png_data.extend_from_slice(&[8, 2, 0, 0, 0]);

    let mut flac_file = FlacFile::read_from(
        &mut fs::File::open(&temp_flac).unwrap(),
        ParseOptions::new(),
    )
    .unwrap();
    let cover = Picture::unchecked(png_data)
        .pic_type(PictureType::CoverFront)
        .mime_type(MimeType::Png)
        .description("Album Cover")
        .build();
    flac_file
        .insert_picture(
            cover,
            Some(PictureInformation {
                width: 300,
                height: 300,
                color_depth: 24,
                num_colors: 0,
            }),
        )
        .unwrap();
    flac_file
        .save_to_path(&temp_flac, WriteOptions::default())
        .expect("save flac with cover");

    let controller = PlaybackController::new();
    let shutdown = Arc::new(AtomicBool::new(false));
    let mut loaded_track = None;

    let result = play_local_flac_with_controller(&temp_flac, controller, shutdown, |event| {
        if let PlaybackEvent::TrackLoaded(info) = event {
            loaded_track = Some(info);
        }
    });

    match result {
        Ok(summary) => {
            assert_eq!(summary.final_state, PlaybackState::Ended);
            let info = loaded_track.expect("track info should be emitted");
            assert!(info.metadata.has_artwork);
            let art = info
                .metadata
                .artwork
                .expect("artwork summary should be present");
            assert_eq!(art.format, ArtworkFormat::Png);
            assert_eq!(art.role, ArtworkRole::CoverFront);
            assert_eq!(
                art.dimensions,
                Some(ArtworkDimensions {
                    width: 300,
                    height: 300
                })
            );
        }
        Err(AudioError::NoOutputDevice) => {}
        Err(other) => panic!("unexpected playback error: {other}"),
    }

    let _ = fs::remove_dir_all(&temp_dir);
}
