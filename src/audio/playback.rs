//! Local FLAC playback pipeline connecting decoding and system audio output.
//!
//! Decoding runs on a dedicated background worker thread feeding a bounded
//! channel so real-time audio callbacks never perform disk I/O or block.

use std::collections::VecDeque;
use std::fmt;
use std::num::{NonZeroU16, NonZeroU32};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, SyncSender, TryRecvError, TrySendError};
use std::sync::{Arc, Condvar, Mutex};
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

/// Thread-safe playback control and state handle.
///
/// Allows external callers (CLI, UI, or tests) to inspect [`PlaybackState`]
/// and issue validated pause/resume commands without blocking the audio
/// processing thread.
#[derive(Debug, Clone, Default)]
pub struct PlaybackController {
    shared: Arc<PlaybackShared>,
}

#[derive(Debug, Default)]
struct PlaybackShared {
    inner: Mutex<ControllerInner>,
    state_changed: Condvar,
}

#[derive(Default)]
struct ControllerInner {
    state: PlaybackState,
    player: Option<Player>,
    stop_signal: Option<Arc<AtomicBool>>,
    stop_requested: bool,
    accumulated_elapsed: Duration,
    active_since: Option<Instant>,
    last_stopped_elapsed: Duration,
    pending_events: VecDeque<PlaybackState>,
}

impl fmt::Debug for ControllerInner {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ControllerInner")
            .field("state", &self.state)
            .field("has_player", &self.player.is_some())
            .field("stop_requested", &self.stop_requested)
            .field("accumulated_elapsed", &self.accumulated_elapsed)
            .field("pending_events", &self.pending_events)
            .finish_non_exhaustive()
    }
}

impl ControllerInner {
    fn apply_pause(&mut self) -> Result<(), AudioError> {
        if self.state != PlaybackState::Playing {
            return Err(AudioError::InvalidState(format!(
                "cannot pause playback while in state {}",
                self.state
            )));
        }
        self.state.transition_to(PlaybackState::Paused)?;
        if let Some(player) = &self.player {
            player.pause();
        }
        if let Some(started) = self.active_since.take() {
            self.accumulated_elapsed += started.elapsed();
        }
        self.pending_events.push_back(PlaybackState::Paused);
        Ok(())
    }

    fn apply_resume(&mut self) -> Result<(), AudioError> {
        if self.state != PlaybackState::Paused {
            return Err(AudioError::InvalidState(format!(
                "cannot resume playback while in state {}",
                self.state
            )));
        }
        self.state.transition_to(PlaybackState::Playing)?;
        if let Some(player) = &self.player {
            player.play();
        }
        self.active_since = Some(Instant::now());
        self.pending_events.push_back(PlaybackState::Playing);
        Ok(())
    }

    fn apply_stop(&mut self) -> Result<Duration, AudioError> {
        if self.state == PlaybackState::Stopped {
            return Err(AudioError::InvalidState(
                "cannot stop playback while already in state Stopped".to_string(),
            ));
        }

        let session_elapsed = self.settle_active_elapsed();
        self.last_stopped_elapsed = session_elapsed;
        // Reset active playback position on stop.
        self.accumulated_elapsed = Duration::ZERO;
        self.active_since = None;

        if let Some(player) = self.player.take() {
            player.stop();
            drop(player);
        }

        self.stop_requested = true;
        if let Some(signal) = &self.stop_signal {
            signal.store(true, Ordering::Release);
        }

        self.state.transition_to(PlaybackState::Stopped)?;
        self.pending_events.push_back(PlaybackState::Stopped);
        Ok(session_elapsed)
    }

    fn settle_active_elapsed(&mut self) -> Duration {
        if let Some(started) = self.active_since.take() {
            self.accumulated_elapsed += started.elapsed();
        }
        self.accumulated_elapsed
    }
}

impl PlaybackController {
    /// Creates a new playback controller starting in [`PlaybackState::Stopped`].
    pub fn new() -> Self {
        Self::default()
    }

    /// Returns the current [`PlaybackState`].
    pub fn state(&self) -> PlaybackState {
        self.lock_inner().state
    }

    /// Returns `true` if playback is currently [`PlaybackState::Playing`].
    pub fn is_playing(&self) -> bool {
        self.state() == PlaybackState::Playing
    }

    /// Returns `true` if playback is currently [`PlaybackState::Paused`].
    pub fn is_paused(&self) -> bool {
        self.state() == PlaybackState::Paused
    }

    /// Returns the current playback position within the active track.
    ///
    /// Preserves the paused position while [`PlaybackState::Paused`] and resets
    /// to [`Duration::ZERO`] when stopped or loading.
    pub fn position(&self) -> Duration {
        let inner = self.lock_inner();
        let current_span = inner.active_since.map_or(Duration::ZERO, |t| t.elapsed());
        inner.accumulated_elapsed + current_span
    }

    /// Returns the active playback time elapsed so far (excluding paused durations),
    /// reset to [`Duration::ZERO`] when stopped.
    pub fn elapsed(&self) -> Duration {
        self.position()
    }

    /// Pauses active playback while retaining the current stream position.
    ///
    /// Returns [`AudioError::InvalidState`] if playback is not currently [`PlaybackState::Playing`].
    pub fn pause(&self) -> Result<(), AudioError> {
        let mut inner = self.lock_inner();
        inner.apply_pause()?;
        self.shared.state_changed.notify_all();
        Ok(())
    }

    /// Resumes paused playback from the retained stream position.
    ///
    /// Returns [`AudioError::InvalidState`] if playback is not currently [`PlaybackState::Paused`].
    pub fn resume(&self) -> Result<(), AudioError> {
        let mut inner = self.lock_inner();
        inner.apply_resume()?;
        self.shared.state_changed.notify_all();
        Ok(())
    }

    /// Toggles between [`PlaybackState::Playing`] and [`PlaybackState::Paused`].
    ///
    /// Returns the new [`PlaybackState`], or [`AudioError::InvalidState`] if playback
    /// is neither playing nor paused.
    pub fn toggle_pause(&self) -> Result<PlaybackState, AudioError> {
        let mut inner = self.lock_inner();
        let next = match inner.state {
            PlaybackState::Playing => {
                inner.apply_pause()?;
                PlaybackState::Paused
            }
            PlaybackState::Paused => {
                inner.apply_resume()?;
                PlaybackState::Playing
            }
            other => {
                return Err(AudioError::InvalidState(format!(
                    "cannot toggle pause while in state {other}"
                )));
            }
        };
        self.shared.state_changed.notify_all();
        Ok(next)
    }

    /// Stops playback (including during loading), releases audio resources, and resets position to zero.
    ///
    /// Returns [`AudioError::InvalidState`] if playback is already [`PlaybackState::Stopped`].
    pub fn stop(&self) -> Result<(), AudioError> {
        let mut inner = self.lock_inner();
        inner.apply_stop()?;
        self.shared.state_changed.notify_all();
        Ok(())
    }

    fn lock_inner(&self) -> std::sync::MutexGuard<'_, ControllerInner> {
        self.shared
            .inner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    #[cfg(test)]
    pub(crate) fn set_state_for_test(&self, state: PlaybackState) {
        let mut inner = self.lock_inner();
        inner.state = state;
        inner.stop_requested = false;
        inner.active_since = if state == PlaybackState::Playing {
            Some(Instant::now())
        } else {
            None
        };
    }

    #[cfg(test)]
    pub(crate) fn has_active_player(&self) -> bool {
        self.lock_inner().player.is_some()
    }

    fn begin_loading(&self) -> Result<(), AudioError> {
        let mut inner = self.lock_inner();
        inner.state.transition_to(PlaybackState::Loading)?;
        inner.player = None;
        inner.stop_signal = None;
        inner.stop_requested = false;
        inner.accumulated_elapsed = Duration::ZERO;
        inner.active_since = None;
        inner.last_stopped_elapsed = Duration::ZERO;
        inner.pending_events.clear();
        Ok(())
    }

    fn attach_stop_signal(&self, signal: Arc<AtomicBool>) -> bool {
        let mut inner = self.lock_inner();
        if inner.stop_requested {
            signal.store(true, Ordering::Release);
            false
        } else {
            inner.stop_signal = Some(signal);
            true
        }
    }

    fn is_stop_requested(&self) -> bool {
        self.lock_inner().stop_requested
    }

    fn start_playing(&self, player: Player, shutdown: &AtomicBool) -> Result<bool, AudioError> {
        let mut inner = self.lock_inner();
        if inner.stop_requested || shutdown.load(Ordering::Acquire) {
            player.stop();
            drop(player);
            return Ok(false);
        }
        inner.state.transition_to(PlaybackState::Playing)?;
        inner.player = Some(player);
        inner.active_since = Some(Instant::now());
        self.shared.state_changed.notify_all();
        Ok(true)
    }

    fn drain_pending_events(&self, on_event: &mut impl FnMut(PlaybackEvent)) {
        loop {
            let next = {
                let mut inner = self.lock_inner();
                inner.pending_events.pop_front()
            };
            match next {
                Some(state) => on_event(PlaybackEvent::StateChanged(state)),
                None => break,
            }
        }
    }

    fn is_ready_to_finish_playing(&self) -> bool {
        let inner = self.lock_inner();
        inner.state == PlaybackState::Playing && inner.player.as_ref().is_some_and(Player::empty)
    }

    fn finish_ended_if_playing(&self) -> Option<Duration> {
        let mut inner = self.lock_inner();
        if inner.state == PlaybackState::Playing && inner.player.as_ref().is_some_and(Player::empty)
        {
            let elapsed = inner.settle_active_elapsed();
            drop(inner.player.take());
            inner.stop_signal = None;
            let _ = inner.state.transition_to(PlaybackState::Ended);
            self.shared.state_changed.notify_all();
            Some(elapsed)
        } else {
            None
        }
    }

    fn finalize_stopped(&self) -> Duration {
        let mut inner = self.lock_inner();
        if inner.state != PlaybackState::Stopped {
            let _ = inner.apply_stop();
        } else {
            if let Some(player) = inner.player.take() {
                player.stop();
                drop(player);
            }
            inner.accumulated_elapsed = Duration::ZERO;
            inner.active_since = None;
        }
        inner.stop_signal = None;
        self.shared.state_changed.notify_all();
        inner.last_stopped_elapsed
    }

    fn fail_active(&self) {
        let mut inner = self.lock_inner();
        inner.settle_active_elapsed();
        if let Some(player) = inner.player.take() {
            player.stop();
            drop(player);
        }
        inner.stop_signal = None;
        let _ = inner.state.transition_to(PlaybackState::Error);
        self.shared.state_changed.notify_all();
    }

    fn wait_poll_or_event(&self, timeout: Duration) {
        let inner = self.lock_inner();
        if inner.pending_events.is_empty() && !inner.stop_requested {
            let _ = self.shared.state_changed.wait_timeout(inner, timeout);
        }
    }
}

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
    on_event: impl FnMut(PlaybackEvent),
) -> Result<PlaybackSummary, AudioError> {
    play_local_flac_with_controller(path, PlaybackController::new(), shutdown, on_event)
}

/// Plays a validated local FLAC file using a caller-supplied [`PlaybackController`].
///
/// Allows external callers to pause, resume, toggle, stop playback (including during loading),
/// and observe state changes concurrently while playback runs to completion or shutdown.
pub fn play_local_flac_with_controller(
    path: impl AsRef<Path>,
    controller: PlaybackController,
    shutdown: Arc<AtomicBool>,
    mut on_event: impl FnMut(PlaybackEvent),
) -> Result<PlaybackSummary, AudioError> {
    controller.begin_loading()?;
    on_event(PlaybackEvent::StateChanged(PlaybackState::Loading));

    if should_stop_now(&controller, &shutdown) {
        return Ok(finish_with_stop(
            &controller,
            None,
            None,
            None,
            AudioStreamInfo::uninitialized(),
            OutputDeviceInfo::uninitialized(),
            &mut on_event,
        ));
    }

    let validated = match validate_audio_file(path) {
        Ok(file) => file,
        Err(err) => {
            if should_stop_now(&controller, &shutdown) {
                return Ok(finish_with_stop(
                    &controller,
                    None,
                    None,
                    None,
                    AudioStreamInfo::uninitialized(),
                    OutputDeviceInfo::uninitialized(),
                    &mut on_event,
                ));
            }
            return Err(fail_and_emit(&controller, err, &mut on_event));
        }
    };

    if should_stop_now(&controller, &shutdown) {
        return Ok(finish_with_stop(
            &controller,
            None,
            None,
            None,
            AudioStreamInfo::uninitialized(),
            OutputDeviceInfo::uninitialized(),
            &mut on_event,
        ));
    }

    let decoder = match FlacDecoder::open(&validated.path) {
        Ok(dec) => dec,
        Err(err) => {
            if should_stop_now(&controller, &shutdown) {
                return Ok(finish_with_stop(
                    &controller,
                    None,
                    None,
                    None,
                    AudioStreamInfo::uninitialized(),
                    OutputDeviceInfo::uninitialized(),
                    &mut on_event,
                ));
            }
            return Err(fail_and_emit(&controller, err, &mut on_event));
        }
    };

    let stream_info = decoder.stream_info().clone();
    if should_stop_now(&controller, &shutdown) {
        drop(decoder);
        return Ok(finish_with_stop(
            &controller,
            None,
            None,
            None,
            stream_info,
            OutputDeviceInfo::uninitialized(),
            &mut on_event,
        ));
    }

    let channels = match NonZeroU16::new(stream_info.channels) {
        Some(ch) => ch,
        None => {
            return Err(fail_and_emit(
                &controller,
                AudioError::UnsupportedConfiguration("channel count cannot be zero".to_string()),
                &mut on_event,
            ));
        }
    };
    let sample_rate = match NonZeroU32::new(stream_info.sample_rate) {
        Some(rate) => rate,
        None => {
            return Err(fail_and_emit(
                &controller,
                AudioError::UnsupportedConfiguration("sample rate cannot be zero".to_string()),
                &mut on_event,
            ));
        }
    };

    let shared_error = Arc::new(Mutex::new(None));

    let output = match AudioOutput::open_default_with_error_sink(Some(Arc::clone(&shared_error))) {
        Ok(out) => out,
        Err(err) => {
            if should_stop_now(&controller, &shutdown) {
                drop(decoder);
                return Ok(finish_with_stop(
                    &controller,
                    None,
                    None,
                    None,
                    stream_info,
                    OutputDeviceInfo::uninitialized(),
                    &mut on_event,
                ));
            }
            return Err(fail_and_emit(&controller, err, &mut on_event));
        }
    };
    let output_info = output.device_info().clone();

    if should_stop_now(&controller, &shutdown) {
        drop(decoder);
        return Ok(finish_with_stop(
            &controller,
            Some(output),
            None,
            None,
            stream_info,
            output_info,
            &mut on_event,
        ));
    }

    on_event(PlaybackEvent::TrackLoaded(PlaybackTrackInfo {
        file: validated,
        stream: stream_info.clone(),
        output: output_info.clone(),
    }));

    let (tx, rx) = mpsc::sync_channel::<Vec<f32>>(PACKET_CHANNEL_CAPACITY);
    let stop_decoder = Arc::new(AtomicBool::new(false));
    let decoder_done = Arc::new(AtomicBool::new(false));

    if !controller.attach_stop_signal(Arc::clone(&stop_decoder)) || shutdown.load(Ordering::Acquire)
    {
        drop(decoder);
        return Ok(finish_with_stop(
            &controller,
            Some(output),
            None,
            None,
            stream_info,
            output_info,
            &mut on_event,
        ));
    }

    let decoder_handle = spawn_decoder_worker(
        decoder,
        tx,
        Arc::clone(&stop_decoder),
        Arc::clone(&decoder_done),
        Arc::clone(&shared_error),
    );

    let first_chunk = match wait_for_initial_chunk(
        &rx,
        &controller,
        &shutdown,
        &stop_decoder,
        &decoder_done,
        &shared_error,
    ) {
        Ok(Some(chunk)) => chunk,
        Ok(None) => {
            return Ok(finish_with_stop(
                &controller,
                Some(output),
                Some(decoder_handle),
                Some(&stop_decoder),
                stream_info,
                output_info,
                &mut on_event,
            ));
        }
        Err(err) => {
            controller.fail_active();
            drop(output);
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
    match controller.start_playing(player, &shutdown) {
        Ok(true) => {}
        Ok(false) => {
            return Ok(finish_with_stop(
                &controller,
                Some(output),
                Some(decoder_handle),
                Some(&stop_decoder),
                stream_info,
                output_info,
                &mut on_event,
            ));
        }
        Err(err) => {
            controller.fail_active();
            drop(output);
            join_decoder_worker(decoder_handle, &stop_decoder);
            on_event(PlaybackEvent::StateChanged(PlaybackState::Error));
            return Err(err);
        }
    }
    on_event(PlaybackEvent::StateChanged(PlaybackState::Playing));

    wait_for_playback(
        controller,
        output,
        decoder_handle,
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
    controller: PlaybackController,
    output: AudioOutput,
    decoder_handle: JoinHandle<()>,
    stream_info: AudioStreamInfo,
    output_info: OutputDeviceInfo,
    shutdown: Arc<AtomicBool>,
    stop_decoder: Arc<AtomicBool>,
    decoder_done: Arc<AtomicBool>,
    shared_error: Arc<Mutex<Option<AudioError>>>,
    mut on_event: impl FnMut(PlaybackEvent),
) -> Result<PlaybackSummary, AudioError> {
    loop {
        controller.drain_pending_events(&mut on_event);

        if should_stop_now(&controller, &shutdown) {
            return Ok(finish_with_stop(
                &controller,
                Some(output),
                Some(decoder_handle),
                Some(&stop_decoder),
                stream_info,
                output_info,
                &mut on_event,
            ));
        }

        if let Some(err) = take_shared_error(&shared_error) {
            controller.fail_active();
            drop(output);
            join_decoder_worker(decoder_handle, &stop_decoder);
            controller.drain_pending_events(&mut on_event);
            on_event(PlaybackEvent::StateChanged(PlaybackState::Error));
            return Err(err);
        }

        if controller.is_ready_to_finish_playing() && decoder_done.load(Ordering::Acquire) {
            // Allow the hardware ring buffer a brief drain window unless interrupted.
            if !should_stop_now(&controller, &shutdown) {
                controller.wait_poll_or_event(Duration::from_millis(50));
            }
            controller.drain_pending_events(&mut on_event);

            if should_stop_now(&controller, &shutdown) {
                return Ok(finish_with_stop(
                    &controller,
                    Some(output),
                    Some(decoder_handle),
                    Some(&stop_decoder),
                    stream_info,
                    output_info,
                    &mut on_event,
                ));
            }

            if let Some(err) = take_shared_error(&shared_error) {
                controller.fail_active();
                drop(output);
                join_decoder_worker(decoder_handle, &stop_decoder);
                on_event(PlaybackEvent::StateChanged(PlaybackState::Error));
                return Err(err);
            }

            if let Some(elapsed) = controller.finish_ended_if_playing() {
                drop(output);
                join_decoder_worker(decoder_handle, &stop_decoder);
                controller.drain_pending_events(&mut on_event);
                on_event(PlaybackEvent::StateChanged(PlaybackState::Ended));
                return Ok(PlaybackSummary {
                    final_state: PlaybackState::Ended,
                    elapsed,
                    stream: stream_info,
                    output: output_info,
                });
            }
        }

        controller.wait_poll_or_event(POLL_INTERVAL);
    }
}

fn should_stop_now(controller: &PlaybackController, shutdown: &AtomicBool) -> bool {
    shutdown.load(Ordering::Acquire) || controller.is_stop_requested()
}

fn fail_and_emit(
    controller: &PlaybackController,
    err: AudioError,
    on_event: &mut impl FnMut(PlaybackEvent),
) -> AudioError {
    controller.fail_active();
    on_event(PlaybackEvent::StateChanged(PlaybackState::Error));
    err
}

fn finish_with_stop(
    controller: &PlaybackController,
    output: Option<AudioOutput>,
    decoder_handle: Option<JoinHandle<()>>,
    stop_decoder: Option<&AtomicBool>,
    stream: AudioStreamInfo,
    output_info: OutputDeviceInfo,
    on_event: &mut impl FnMut(PlaybackEvent),
) -> PlaybackSummary {
    let elapsed = controller.finalize_stopped();
    drop(output);
    if let (Some(handle), Some(flag)) = (decoder_handle, stop_decoder) {
        join_decoder_worker(handle, flag);
    }
    controller.drain_pending_events(on_event);
    PlaybackSummary {
        final_state: PlaybackState::Stopped,
        elapsed,
        stream,
        output: output_info,
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
    controller: &PlaybackController,
    shutdown: &AtomicBool,
    stop_decoder: &AtomicBool,
    decoder_done: &AtomicBool,
    shared_error: &Mutex<Option<AudioError>>,
) -> Result<Option<Vec<f32>>, AudioError> {
    loop {
        if should_stop_now(controller, shutdown) {
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
                controller.wait_poll_or_event(Duration::from_millis(5));
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
        let controller = PlaybackController::new();
        let shutdown = AtomicBool::new(false);
        let stop_decoder = AtomicBool::new(false);
        let decoder_done = AtomicBool::new(true);
        let shared_error = Mutex::new(Some(AudioError::DecodeError("bad packet".to_string())));

        let err = wait_for_initial_chunk(
            &rx,
            &controller,
            &shutdown,
            &stop_decoder,
            &decoder_done,
            &shared_error,
        )
        .unwrap_err();

        assert!(matches!(err, AudioError::DecodeError(_)));
        assert_eq!(err.to_string(), "audio decode error: bad packet");
    }

    #[test]
    fn wait_for_initial_chunk_handles_shutdown_gracefully() {
        let (_tx, rx) = mpsc::sync_channel(4);
        let controller = PlaybackController::new();
        let shutdown = AtomicBool::new(true);
        let stop_decoder = AtomicBool::new(false);
        let decoder_done = AtomicBool::new(false);
        let shared_error = Mutex::new(None);

        let res = wait_for_initial_chunk(
            &rx,
            &controller,
            &shutdown,
            &stop_decoder,
            &decoder_done,
            &shared_error,
        )
        .unwrap();

        assert!(res.is_none());
        assert!(stop_decoder.load(Ordering::Acquire));
    }

    #[test]
    fn wait_for_initial_chunk_handles_controller_stop_during_loading() {
        let (_tx, rx) = mpsc::sync_channel(4);
        let controller = PlaybackController::new();
        let shutdown = AtomicBool::new(false);
        let stop_decoder = Arc::new(AtomicBool::new(false));
        let decoder_done = AtomicBool::new(false);
        let shared_error = Mutex::new(None);

        controller.begin_loading().unwrap();
        assert!(controller.attach_stop_signal(Arc::clone(&stop_decoder)));
        controller.stop().unwrap();

        let res = wait_for_initial_chunk(
            &rx,
            &controller,
            &shutdown,
            &stop_decoder,
            &decoder_done,
            &shared_error,
        )
        .unwrap();

        assert!(res.is_none());
        assert!(stop_decoder.load(Ordering::Acquire));
        assert_eq!(controller.state(), PlaybackState::Stopped);
        assert_eq!(controller.position(), Duration::ZERO);
    }

    #[test]
    fn controller_enforces_valid_pause_resume_and_toggle_transitions() {
        let controller = PlaybackController::new();
        assert_eq!(controller.state(), PlaybackState::Stopped);
        assert!(!controller.is_playing());
        assert!(!controller.is_paused());
        assert_eq!(controller.elapsed(), Duration::ZERO);

        // Cannot pause/resume/toggle while Stopped
        assert!(matches!(
            controller.pause(),
            Err(AudioError::InvalidState(_))
        ));
        assert!(matches!(
            controller.resume(),
            Err(AudioError::InvalidState(_))
        ));
        assert!(matches!(
            controller.toggle_pause(),
            Err(AudioError::InvalidState(_))
        ));

        // Cannot pause/resume while Loading
        controller.begin_loading().unwrap();
        assert_eq!(controller.state(), PlaybackState::Loading);
        assert!(matches!(
            controller.pause(),
            Err(AudioError::InvalidState(_))
        ));
        assert!(matches!(
            controller.resume(),
            Err(AudioError::InvalidState(_))
        ));
        // Cannot begin_loading again while already Loading
        assert!(matches!(
            controller.begin_loading(),
            Err(AudioError::InvalidState(_))
        ));

        // Playing -> Paused -> Playing -> Paused -> Playing
        controller.set_state_for_test(PlaybackState::Playing);
        assert!(controller.is_playing());
        assert!(matches!(
            controller.resume(),
            Err(AudioError::InvalidState(_))
        ));

        controller.pause().unwrap();
        assert!(controller.is_paused());
        assert_eq!(controller.state(), PlaybackState::Paused);
        assert!(matches!(
            controller.pause(),
            Err(AudioError::InvalidState(_))
        ));

        controller.resume().unwrap();
        assert!(controller.is_playing());

        assert_eq!(controller.toggle_pause().unwrap(), PlaybackState::Paused);
        assert_eq!(controller.toggle_pause().unwrap(), PlaybackState::Playing);

        let mut emitted = Vec::new();
        controller.drain_pending_events(&mut |event| {
            if let PlaybackEvent::StateChanged(s) = event {
                emitted.push(s);
            }
        });
        assert_eq!(
            emitted,
            vec![
                PlaybackState::Paused,
                PlaybackState::Playing,
                PlaybackState::Paused,
                PlaybackState::Playing,
            ]
        );

        // Terminal states reject pause/resume
        controller.set_state_for_test(PlaybackState::Ended);
        assert!(matches!(
            controller.pause(),
            Err(AudioError::InvalidState(_))
        ));
        assert!(matches!(
            controller.resume(),
            Err(AudioError::InvalidState(_))
        ));
        assert!(matches!(
            controller.toggle_pause(),
            Err(AudioError::InvalidState(_))
        ));

        controller.set_state_for_test(PlaybackState::Error);
        assert!(matches!(
            controller.pause(),
            Err(AudioError::InvalidState(_))
        ));
        assert!(matches!(
            controller.resume(),
            Err(AudioError::InvalidState(_))
        ));
    }

    #[test]
    fn controller_excludes_paused_time_from_elapsed_duration() {
        let controller = PlaybackController::new();
        controller.set_state_for_test(PlaybackState::Playing);
        thread::sleep(Duration::from_millis(30));
        controller.pause().unwrap();

        let elapsed_at_pause = controller.elapsed();
        assert!(elapsed_at_pause >= Duration::from_millis(20));

        thread::sleep(Duration::from_millis(60));
        let elapsed_while_paused = controller.elapsed();
        assert_eq!(elapsed_while_paused, elapsed_at_pause);

        controller.resume().unwrap();
        thread::sleep(Duration::from_millis(25));
        assert!(controller.elapsed() > elapsed_at_pause);
    }

    #[test]
    fn controller_stop_resets_position_releases_signals_and_rejects_already_stopped() {
        let controller = PlaybackController::new();
        assert!(matches!(
            controller.stop(),
            Err(AudioError::InvalidState(_))
        ));

        // Stop while Playing resets position and signals decoder worker
        let stop_signal = Arc::new(AtomicBool::new(false));
        controller.begin_loading().unwrap();
        assert!(controller.attach_stop_signal(Arc::clone(&stop_signal)));
        controller.set_state_for_test(PlaybackState::Playing);
        thread::sleep(Duration::from_millis(25));
        assert!(controller.position() >= Duration::from_millis(15));

        controller.stop().unwrap();
        assert_eq!(controller.state(), PlaybackState::Stopped);
        assert_eq!(controller.position(), Duration::ZERO);
        assert_eq!(controller.elapsed(), Duration::ZERO);
        assert!(stop_signal.load(Ordering::Acquire));
        assert!(!controller.has_active_player());

        // Stopping again when already Stopped is rejected
        assert!(matches!(
            controller.stop(),
            Err(AudioError::InvalidState(_))
        ));

        // Stopping from Paused or Ended also resets to Stopped at Duration::ZERO
        controller.set_state_for_test(PlaybackState::Ended);
        controller.stop().unwrap();
        assert_eq!(controller.state(), PlaybackState::Stopped);
        assert_eq!(controller.position(), Duration::ZERO);
    }

    #[test]
    fn controller_stop_during_loading_prevents_attaching_stop_signal_afterward() {
        let controller = PlaybackController::new();
        controller.begin_loading().unwrap();
        controller.stop().unwrap();
        assert_eq!(controller.state(), PlaybackState::Stopped);
        assert_eq!(controller.position(), Duration::ZERO);

        let late_signal = Arc::new(AtomicBool::new(false));
        assert!(!controller.attach_stop_signal(Arc::clone(&late_signal)));
        assert!(late_signal.load(Ordering::Acquire));
    }
}
