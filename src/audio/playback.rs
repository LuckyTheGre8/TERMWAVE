//! Local FLAC playback pipeline connecting decoding and system audio output.
//!
//! Decoding runs on a dedicated background worker thread feeding a bounded
//! channel so real-time audio callbacks never perform disk I/O or block.

use std::num::{NonZeroU16, NonZeroU32};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, SyncSender, TryRecvError, TrySendError};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use rodio::{ChannelCount, Player, SampleRate, Source};

use super::{
    AudioError, AudioOutput, AudioStreamInfo, FlacDecoder, OutputDeviceInfo, PlaybackState,
    ValidatedAudioFile, validate_audio_file,
};

/// Maximum number of decoded PCM packet buffers kept in flight.
const PACKET_CHANNEL_CAPACITY: usize = 16;

/// Polling interval while waiting for playback completion or shutdown.
const POLL_INTERVAL: Duration = Duration::from_millis(25);

/// Metadata describing an active track and initialized audio output.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlaybackTrackInfo {
    /// Validated local audio file information.
    pub file: ValidatedAudioFile,
    /// Decoded source stream properties.
    pub stream: AudioStreamInfo,
    /// Active system audio output properties.
    pub output: OutputDeviceInfo,
}

/// Lifecycle events emitted during local playback.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PlaybackEvent {
    /// Playback engine transitioned to a new state.
    StateChanged(PlaybackState),
    /// Track metadata, source stream properties, and output device are ready.
    TrackLoaded(PlaybackTrackInfo),
}

/// Summary returned when local playback finishes or stops cleanly.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlaybackSummary {
    /// Terminal state (`Ended` or `Stopped`).
    pub final_state: PlaybackState,
    /// Wall-clock duration spent playing before completion or stop.
    pub elapsed: Duration,
    /// Decoded source stream properties.
    pub stream: AudioStreamInfo,
    /// Output device properties used for playback.
    pub output: OutputDeviceInfo,
}

/// Plays a validated local FLAC file through the default system audio device.
///
/// Waits until playback completes or `shutdown` is set to `true` (e.g. via Ctrl+C),
/// then releases audio resources and joins the decoder thread cleanly.
pub fn play_local_flac(
    path: impl AsRef<Path>,
    shutdown: Arc<AtomicBool>,
    mut on_event: impl FnMut(PlaybackEvent),
) -> Result<PlaybackSummary, AudioError> {
    on_event(PlaybackEvent::StateChanged(PlaybackState::Loading));

    let validated = match validate_audio_file(path) {
        Ok(file) => file,
        Err(err) => {
            on_event(PlaybackEvent::StateChanged(PlaybackState::Error));
            return Err(err);
        }
    };

    let decoder = match FlacDecoder::open(&validated.path) {
        Ok(dec) => dec,
        Err(err) => {
            on_event(PlaybackEvent::StateChanged(PlaybackState::Error));
            return Err(err);
        }
    };

    let stream_info = decoder.stream_info().clone();
    let channels = NonZeroU16::new(stream_info.channels).ok_or_else(|| {
        AudioError::UnsupportedConfiguration("channel count cannot be zero".to_string())
    })?;
    let sample_rate = NonZeroU32::new(stream_info.sample_rate).ok_or_else(|| {
        AudioError::UnsupportedConfiguration("sample rate cannot be zero".to_string())
    })?;

    let shared_error = Arc::new(Mutex::new(None));

    let output = match AudioOutput::open_default_with_error_sink(Some(Arc::clone(&shared_error))) {
        Ok(out) => out,
        Err(err) => {
            on_event(PlaybackEvent::StateChanged(PlaybackState::Error));
            return Err(err);
        }
    };
    let output_info = output.device_info().clone();

    on_event(PlaybackEvent::TrackLoaded(PlaybackTrackInfo {
        file: validated,
        stream: stream_info.clone(),
        output: output_info.clone(),
    }));

    let (tx, rx) = mpsc::sync_channel::<Vec<f32>>(PACKET_CHANNEL_CAPACITY);
    let stop_decoder = Arc::new(AtomicBool::new(false));
    let decoder_done = Arc::new(AtomicBool::new(false));

    let decoder_handle = spawn_decoder_worker(
        decoder,
        tx,
        Arc::clone(&stop_decoder),
        Arc::clone(&decoder_done),
        Arc::clone(&shared_error),
    );

    let first_chunk =
        match wait_for_initial_chunk(&rx, &shutdown, &stop_decoder, &decoder_done, &shared_error) {
            Ok(Some(chunk)) => chunk,
            Ok(None) => {
                join_decoder_worker(decoder_handle, &stop_decoder);
                on_event(PlaybackEvent::StateChanged(PlaybackState::Stopped));
                return Ok(PlaybackSummary {
                    final_state: PlaybackState::Stopped,
                    elapsed: Duration::ZERO,
                    stream: stream_info,
                    output: output_info,
                });
            }
            Err(err) => {
                join_decoder_worker(decoder_handle, &stop_decoder);
                on_event(PlaybackEvent::StateChanged(PlaybackState::Error));
                return Err(err);
            }
        };

    let source = ChannelAudioSource::new(
        first_chunk,
        rx,
        channels,
        sample_rate,
        stream_info.duration,
        Arc::clone(&decoder_done),
    );

    let player = Player::connect_new(output.mixer());
    player.append(source);
    let started_at = Instant::now();
    on_event(PlaybackEvent::StateChanged(PlaybackState::Playing));

    wait_for_playback(
        player,
        output,
        decoder_handle,
        started_at,
        stream_info,
        output_info,
        shutdown,
        stop_decoder,
        decoder_done,
        shared_error,
        on_event,
    )
}

#[allow(clippy::too_many_arguments)]
fn wait_for_playback(
    player: Player,
    output: AudioOutput,
    decoder_handle: JoinHandle<()>,
    started_at: Instant,
    stream_info: AudioStreamInfo,
    output_info: OutputDeviceInfo,
    shutdown: Arc<AtomicBool>,
    stop_decoder: Arc<AtomicBool>,
    decoder_done: Arc<AtomicBool>,
    shared_error: Arc<Mutex<Option<AudioError>>>,
    mut on_event: impl FnMut(PlaybackEvent),
) -> Result<PlaybackSummary, AudioError> {
    loop {
        if shutdown.load(Ordering::Acquire) {
            let elapsed = started_at.elapsed();
            player.stop();
            drop(player);
            drop(output);
            join_decoder_worker(decoder_handle, &stop_decoder);
            on_event(PlaybackEvent::StateChanged(PlaybackState::Stopped));
            return Ok(PlaybackSummary {
                final_state: PlaybackState::Stopped,
                elapsed,
                stream: stream_info,
                output: output_info,
            });
        }

        if let Some(err) = take_shared_error(&shared_error) {
            player.stop();
            drop(player);
            drop(output);
            join_decoder_worker(decoder_handle, &stop_decoder);
            on_event(PlaybackEvent::StateChanged(PlaybackState::Error));
            return Err(err);
        }

        if player.empty() && decoder_done.load(Ordering::Acquire) {
            // Allow the hardware ring buffer a brief drain window unless interrupted.
            if !shutdown.load(Ordering::Acquire) {
                thread::sleep(Duration::from_millis(50));
            }
            let elapsed = started_at.elapsed();
            drop(player);
            drop(output);
            join_decoder_worker(decoder_handle, &stop_decoder);

            if let Some(err) = take_shared_error(&shared_error) {
                on_event(PlaybackEvent::StateChanged(PlaybackState::Error));
                return Err(err);
            }

            let final_state = if shutdown.load(Ordering::Acquire) {
                PlaybackState::Stopped
            } else {
                PlaybackState::Ended
            };
            on_event(PlaybackEvent::StateChanged(final_state));
            return Ok(PlaybackSummary {
                final_state,
                elapsed,
                stream: stream_info,
                output: output_info,
            });
        }

        thread::sleep(POLL_INTERVAL);
    }
}

fn spawn_decoder_worker(
    mut decoder: FlacDecoder,
    tx: SyncSender<Vec<f32>>,
    stop_decoder: Arc<AtomicBool>,
    decoder_done: Arc<AtomicBool>,
    shared_error: Arc<Mutex<Option<AudioError>>>,
) -> JoinHandle<()> {
    thread::spawn(move || {
        while !stop_decoder.load(Ordering::Acquire) {
            match decoder.decode_next() {
                Ok(Some(samples)) => {
                    if !send_chunk_or_stop(&tx, samples.to_vec(), &stop_decoder) {
                        break;
                    }
                }
                Ok(None) => break,
                Err(err) => {
                    if let Ok(mut slot) = shared_error.lock() {
                        *slot = Some(err);
                    }
                    break;
                }
            }
        }
        decoder_done.store(true, Ordering::Release);
    })
}

fn send_chunk_or_stop(
    tx: &SyncSender<Vec<f32>>,
    mut chunk: Vec<f32>,
    stop_decoder: &AtomicBool,
) -> bool {
    loop {
        if stop_decoder.load(Ordering::Acquire) {
            return false;
        }
        match tx.try_send(chunk) {
            Ok(()) => return true,
            Err(TrySendError::Full(returned)) => {
                chunk = returned;
                thread::sleep(Duration::from_millis(5));
            }
            Err(TrySendError::Disconnected(_)) => return false,
        }
    }
}

fn wait_for_initial_chunk(
    rx: &Receiver<Vec<f32>>,
    shutdown: &AtomicBool,
    stop_decoder: &AtomicBool,
    decoder_done: &AtomicBool,
    shared_error: &Mutex<Option<AudioError>>,
) -> Result<Option<Vec<f32>>, AudioError> {
    loop {
        if shutdown.load(Ordering::Acquire) {
            stop_decoder.store(true, Ordering::Release);
            return Ok(None);
        }
        match rx.try_recv() {
            Ok(chunk) => return Ok(Some(chunk)),
            Err(TryRecvError::Disconnected) => {
                if let Some(err) = take_shared_error(shared_error) {
                    return Err(err);
                }
                return Err(AudioError::DecodeError(
                    "FLAC stream produced no audio samples".to_string(),
                ));
            }
            Err(TryRecvError::Empty) => {
                if let Some(err) = take_shared_error(shared_error) {
                    return Err(err);
                }
                if decoder_done.load(Ordering::Acquire) {
                    return Err(AudioError::DecodeError(
                        "FLAC stream produced no audio samples".to_string(),
                    ));
                }
                thread::sleep(Duration::from_millis(5));
            }
        }
    }
}

fn take_shared_error(shared_error: &Mutex<Option<AudioError>>) -> Option<AudioError> {
    shared_error.lock().ok().and_then(|mut guard| guard.take())
}

fn join_decoder_worker(handle: JoinHandle<()>, stop_decoder: &AtomicBool) {
    stop_decoder.store(true, Ordering::Release);
    let _ = handle.join();
}

/// Non-blocking `rodio::Source` fed by pre-decoded PCM buffers over a bounded channel.
struct ChannelAudioSource {
    current_chunk: Vec<f32>,
    offset: usize,
    underrun_silence_remaining: usize,
    rx: Receiver<Vec<f32>>,
    channels: ChannelCount,
    sample_rate: SampleRate,
    total_duration: Option<Duration>,
    decoder_done: Arc<AtomicBool>,
}

impl ChannelAudioSource {
    fn new(
        initial_chunk: Vec<f32>,
        rx: Receiver<Vec<f32>>,
        channels: ChannelCount,
        sample_rate: SampleRate,
        total_duration: Option<Duration>,
        decoder_done: Arc<AtomicBool>,
    ) -> Self {
        Self {
            current_chunk: initial_chunk,
            offset: 0,
            underrun_silence_remaining: 0,
            rx,
            channels,
            sample_rate,
            total_duration,
            decoder_done,
        }
    }
}

impl Iterator for ChannelAudioSource {
    type Item = f32;

    fn next(&mut self) -> Option<Self::Item> {
        if self.underrun_silence_remaining > 0 {
            self.underrun_silence_remaining -= 1;
            return Some(0.0);
        }

        loop {
            if self.offset < self.current_chunk.len() {
                let sample = self.current_chunk[self.offset];
                self.offset += 1;
                return Some(sample);
            }

            match self.rx.try_recv() {
                Ok(chunk) => {
                    self.current_chunk = chunk;
                    self.offset = 0;
                }
                Err(TryRecvError::Disconnected) => return None,
                Err(TryRecvError::Empty) => {
                    if self.decoder_done.load(Ordering::Acquire) {
                        return None;
                    }
                    // Emit a full silent frame to preserve channel alignment.
                    self.underrun_silence_remaining = usize::from(self.channels.get()) - 1;
                    return Some(0.0);
                }
            }
        }
    }
}

impl Source for ChannelAudioSource {
    fn current_span_len(&self) -> Option<usize> {
        None
    }

    fn channels(&self) -> ChannelCount {
        self.channels
    }

    fn sample_rate(&self) -> SampleRate {
        self.sample_rate
    }

    fn total_duration(&self) -> Option<Duration> {
        self.total_duration
    }
}

#[cfg(test)]
mod tests {
    use std::num::NonZeroU16;

    use super::*;

    #[test]
    fn channel_audio_source_streams_chunks_and_terminates_cleanly() {
        let (tx, rx) = mpsc::sync_channel(4);
        let done = Arc::new(AtomicBool::new(false));
        let channels = NonZeroU16::new(2).unwrap();
        let rate = NonZeroU32::new(44_100).unwrap();

        tx.send(vec![0.3, 0.4]).unwrap();
        drop(tx);
        done.store(true, Ordering::Release);

        let src = ChannelAudioSource::new(vec![0.1, 0.2], rx, channels, rate, None, done);
        assert_eq!(src.channels(), channels);
        assert_eq!(src.sample_rate(), rate);

        let samples: Vec<f32> = src.collect();
        assert_eq!(samples, vec![0.1, 0.2, 0.3, 0.4]);
    }

    #[test]
    fn channel_audio_source_pads_full_frame_on_transient_underrun() {
        let (_tx, rx) = mpsc::sync_channel(4);
        let done = Arc::new(AtomicBool::new(false));
        let channels = NonZeroU16::new(2).unwrap();
        let rate = NonZeroU32::new(48_000).unwrap();

        let mut src =
            ChannelAudioSource::new(vec![0.5, -0.5], rx, channels, rate, None, Arc::clone(&done));

        // Initial stereo frame
        assert_eq!(src.next(), Some(0.5));
        assert_eq!(src.next(), Some(-0.5));

        // Channel is temporarily empty while decoder is still running -> full silent stereo frame
        assert_eq!(src.next(), Some(0.0));
        assert_eq!(src.next(), Some(0.0));

        // Once decoder finishes, source terminates cleanly
        done.store(true, Ordering::Release);
        assert_eq!(src.next(), None);
    }

    #[test]
    fn wait_for_initial_chunk_propagates_decoder_error() {
        let (_tx, rx) = mpsc::sync_channel(4);
        let shutdown = AtomicBool::new(false);
        let stop_decoder = AtomicBool::new(false);
        let decoder_done = AtomicBool::new(true);
        let shared_error = Mutex::new(Some(AudioError::DecodeError("bad packet".to_string())));

        let err =
            wait_for_initial_chunk(&rx, &shutdown, &stop_decoder, &decoder_done, &shared_error)
                .unwrap_err();

        assert!(matches!(err, AudioError::DecodeError(_)));
        assert_eq!(err.to_string(), "audio decode error: bad packet");
    }

    #[test]
    fn wait_for_initial_chunk_handles_shutdown_gracefully() {
        let (_tx, rx) = mpsc::sync_channel(4);
        let shutdown = AtomicBool::new(true);
        let stop_decoder = AtomicBool::new(false);
        let decoder_done = AtomicBool::new(false);
        let shared_error = Mutex::new(None);

        let res =
            wait_for_initial_chunk(&rx, &shutdown, &stop_decoder, &decoder_done, &shared_error)
                .unwrap();

        assert!(res.is_none());
        assert!(stop_decoder.load(Ordering::Acquire));
    }
}
