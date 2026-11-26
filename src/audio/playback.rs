//! Local FLAC playback pipeline connecting decoding and system audio output.
//!
//! Decoding runs on a dedicated background worker thread feeding a bounded
//! channel so real-time audio callbacks never perform disk I/O or block.

use std::collections::VecDeque;
use std::fmt;
use std::num::{NonZeroU16, NonZeroU32};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, SyncSender, TryRecvError, TrySendError};
use std::sync::{Arc, Condvar, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use rodio::{ChannelCount, Player, SampleRate, Source};

use super::{
    AudioError, AudioOutput, AudioStreamInfo, DEFAULT_VOLUME_STEP, FlacDecoder, OutputDeviceInfo,
    PlaybackState, TrackQueue, ValidatedAudioFile, VolumeState, prevent_clipping,
    validate_audio_file,
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
    queue: Option<TrackQueue>,
    volume: VolumeState,
    stop_signal: Option<Arc<AtomicBool>>,
    stop_requested: bool,
    track_switch_requested: bool,
    accumulated_elapsed: Duration,
    active_since: Option<Instant>,
    total_session_elapsed: Duration,
    last_stopped_elapsed: Duration,
    pending_events: VecDeque<PlaybackState>,
}

enum TrackNavigationDirection {
    Next,
    Previous,
}

enum TrackEndOutcome {
    StillPlaying,
    NextTrack,
    Ended(Duration),
}

enum TrackWaitOutcome {
    SwitchTrack,
    Stopped(PlaybackSummary),
    Ended(PlaybackSummary),
}

impl fmt::Debug for ControllerInner {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ControllerInner")
            .field("state", &self.state)
            .field("has_player", &self.player.is_some())
            .field(
                "queue_index",
                &self.queue.as_ref().and_then(TrackQueue::current_index),
            )
            .field("stop_requested", &self.stop_requested)
            .field("track_switch_requested", &self.track_switch_requested)
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

        let track_elapsed = self.settle_active_elapsed();
        self.total_session_elapsed += track_elapsed;
        self.last_stopped_elapsed = self.total_session_elapsed;
        // Reset active playback position on stop.
        self.accumulated_elapsed = Duration::ZERO;
        self.active_since = None;
        self.track_switch_requested = false;

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
        Ok(self.last_stopped_elapsed)
    }

    fn apply_navigation(
        &mut self,
        direction: TrackNavigationDirection,
    ) -> Result<PathBuf, AudioError> {
        if !matches!(
            self.state,
            PlaybackState::Loading | PlaybackState::Playing | PlaybackState::Paused
        ) {
            return Err(AudioError::InvalidState(format!(
                "cannot navigate tracks while in state {}",
                self.state
            )));
        }

        let queue = self.queue.as_mut().ok_or_else(|| {
            AudioError::InvalidState(
                "cannot navigate tracks: no playback queue attached".to_string(),
            )
        })?;

        let target = match direction {
            TrackNavigationDirection::Next => queue.advance_next()?.to_path_buf(),
            TrackNavigationDirection::Previous => queue.step_previous()?.to_path_buf(),
        };

        let track_elapsed = self.settle_active_elapsed();
        self.total_session_elapsed += track_elapsed;
        self.accumulated_elapsed = Duration::ZERO;
        self.active_since = None;

        if let Some(player) = self.player.take() {
            player.stop();
            drop(player);
        }
        if let Some(signal) = &self.stop_signal {
            signal.store(true, Ordering::Release);
        }
        if self.state != PlaybackState::Loading {
            self.state.transition_to(PlaybackState::Loading)?;
        }
        self.track_switch_requested = true;
        Ok(target)
    }

    fn settle_active_elapsed(&mut self) -> Duration {
        if let Some(started) = self.active_since.take() {
            self.accumulated_elapsed += started.elapsed();
        }
        self.accumulated_elapsed
    }

    fn apply_player_volume(&self) {
        if let Some(player) = &self.player {
            player.set_volume(self.volume.effective_level());
        }
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
    /// to [`Duration::ZERO`] when stopped, loading, or switching tracks.
    pub fn position(&self) -> Duration {
        let inner = self.lock_inner();
        let current_span = inner.active_since.map_or(Duration::ZERO, |t| t.elapsed());
        inner.accumulated_elapsed + current_span
    }

    /// Returns the active playback time elapsed for the current track
    /// (excluding paused durations), reset to [`Duration::ZERO`] when stopped
    /// or switching tracks.
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

    /// Skips to the next track in the active queue and records the current track in history.
    ///
    /// Returns the newly selected track path, or [`AudioError::InvalidState`] if playback
    /// is inactive or already at the last track boundary.
    pub fn next_track(&self) -> Result<PathBuf, AudioError> {
        let mut inner = self.lock_inner();
        let path = inner.apply_navigation(TrackNavigationDirection::Next)?;
        self.shared.state_changed.notify_all();
        Ok(path)
    }

    /// Returns to the previous track from playback history.
    ///
    /// Returns the newly selected track path, or [`AudioError::InvalidState`] if playback
    /// is inactive or already at the start boundary with no previous track history.
    pub fn previous_track(&self) -> Result<PathBuf, AudioError> {
        let mut inner = self.lock_inner();
        let path = inner.apply_navigation(TrackNavigationDirection::Previous)?;
        self.shared.state_changed.notify_all();
        Ok(path)
    }

    /// Returns `true` if a next track exists in the active queue.
    pub fn has_next_track(&self) -> bool {
        self.lock_inner()
            .queue
            .as_ref()
            .is_some_and(TrackQueue::has_next)
    }

    /// Returns `true` if navigating to a previous track is possible.
    pub fn has_previous_track(&self) -> bool {
        self.lock_inner()
            .queue
            .as_ref()
            .is_some_and(TrackQueue::has_previous)
    }

    /// Returns the currently selected track path in the active queue, if any.
    pub fn current_track(&self) -> Option<PathBuf> {
        self.lock_inner()
            .queue
            .as_ref()
            .and_then(|q| q.current_track().map(Path::to_path_buf))
    }

    /// Returns the 0-based index of the current track in the active queue, if any.
    pub fn current_track_index(&self) -> Option<usize> {
        self.lock_inner()
            .queue
            .as_ref()
            .and_then(TrackQueue::current_index)
    }

    /// Returns the number of tracks in the active queue (`0` if no queue is loaded).
    pub fn track_count(&self) -> usize {
        self.lock_inner().queue.as_ref().map_or(0, TrackQueue::len)
    }

    /// Returns the chronological paths of tracks entered during the current session.
    pub fn played_history(&self) -> Vec<PathBuf> {
        self.lock_inner()
            .queue
            .as_ref()
            .map_or_else(Vec::new, TrackQueue::played_history)
    }

    /// Returns the paths currently held on the back-navigation stack.
    pub fn back_history(&self) -> Vec<PathBuf> {
        self.lock_inner()
            .queue
            .as_ref()
            .map_or_else(Vec::new, TrackQueue::back_history)
    }

    /// Returns the configured volume level (`0.0..=1.0`), independent of mute status.
    pub fn volume(&self) -> f32 {
        self.lock_inner().volume.level()
    }

    /// Returns the configured volume rounded to an integer percentage (`0..=100`).
    pub fn volume_percent(&self) -> u8 {
        self.lock_inner().volume.percent()
    }

    /// Returns the effective output volume gain (`0.0` when muted, otherwise [`Self::volume`]).
    pub fn effective_volume(&self) -> f32 {
        self.lock_inner().volume.effective_level()
    }

    /// Returns `true` if audio output is currently muted.
    pub fn is_muted(&self) -> bool {
        self.lock_inner().volume.is_muted()
    }

    /// Sets the volume level within `[0.0, 1.0]`, applying immediately to any active player.
    ///
    /// Rejects negative, `NaN`, infinite, or clipping (`> 1.0`) values with
    /// [`AudioError::InvalidVolume`].
    pub fn set_volume(&self, level: f32) -> Result<f32, AudioError> {
        let mut inner = self.lock_inner();
        let updated = inner.volume.set_level(level)?;
        inner.apply_player_volume();
        Ok(updated)
    }

    /// Sets the volume level as a percentage (`0..=100`), returning the normalized level.
    pub fn set_volume_percent(&self, percent: u8) -> Result<f32, AudioError> {
        let mut inner = self.lock_inner();
        let updated = inner.volume.set_percent(percent)?;
        inner.apply_player_volume();
        Ok(updated)
    }

    /// Adjusts volume by `delta`, clamping safely within `[0.0, 1.0]` to prevent clipping.
    ///
    /// Returns [`AudioError::InvalidVolume`] if `delta` is non-finite.
    pub fn adjust_volume(&self, delta: f32) -> Result<f32, AudioError> {
        let mut inner = self.lock_inner();
        let updated = inner.volume.adjust(delta)?;
        inner.apply_player_volume();
        Ok(updated)
    }

    /// Increases volume by the default 5% step, clamped safely at unity gain (`1.0`).
    pub fn increase_volume(&self) -> f32 {
        self.adjust_volume(DEFAULT_VOLUME_STEP)
            .expect("DEFAULT_VOLUME_STEP is finite")
    }

    /// Decreases volume by the default 5% step, clamped cleanly at silence (`0.0`).
    pub fn decrease_volume(&self) -> f32 {
        self.adjust_volume(-DEFAULT_VOLUME_STEP)
            .expect("DEFAULT_VOLUME_STEP is finite")
    }

    /// Mutes playback while preserving the configured volume level.
    pub fn mute(&self) {
        let mut inner = self.lock_inner();
        inner.volume.mute();
        inner.apply_player_volume();
    }

    /// Unmutes playback and restores output at the configured volume level.
    pub fn unmute(&self) {
        let mut inner = self.lock_inner();
        inner.volume.unmute();
        inner.apply_player_volume();
    }

    /// Toggles mute status and returns the resulting muted state (`true` if muted).
    pub fn toggle_mute(&self) -> bool {
        let mut inner = self.lock_inner();
        let muted = inner.volume.toggle_mute();
        inner.apply_player_volume();
        muted
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
        inner.track_switch_requested = false;
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

    fn begin_loading_with_queue(&self, queue: TrackQueue) -> Result<(), AudioError> {
        let mut inner = self.lock_inner();
        inner.state.transition_to(PlaybackState::Loading)?;
        inner.player = None;
        inner.queue = Some(queue);
        inner.stop_signal = None;
        inner.stop_requested = false;
        inner.track_switch_requested = false;
        inner.accumulated_elapsed = Duration::ZERO;
        inner.active_since = None;
        inner.total_session_elapsed = Duration::ZERO;
        inner.last_stopped_elapsed = Duration::ZERO;
        inner.pending_events.clear();
        Ok(())
    }

    #[cfg(test)]
    fn begin_loading(&self) -> Result<(), AudioError> {
        self.begin_loading_with_queue(TrackQueue::new(Vec::new()))
    }

    fn take_track_switch(&self) -> bool {
        let mut inner = self.lock_inner();
        std::mem::take(&mut inner.track_switch_requested)
    }

    fn current_queue_track(&self) -> Option<PathBuf> {
        self.lock_inner()
            .queue
            .as_ref()
            .and_then(|q| q.current_track().map(Path::to_path_buf))
    }

    fn attach_stop_signal(&self, signal: Arc<AtomicBool>) -> bool {
        let mut inner = self.lock_inner();
        if inner.stop_requested || inner.track_switch_requested {
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

    fn is_switch_or_stop_requested(&self, shutdown: &AtomicBool) -> bool {
        if shutdown.load(Ordering::Acquire) {
            return true;
        }
        let inner = self.lock_inner();
        inner.stop_requested || inner.track_switch_requested
    }

    fn start_playing(&self, player: Player, shutdown: &AtomicBool) -> Result<bool, AudioError> {
        let mut inner = self.lock_inner();
        if inner.stop_requested || inner.track_switch_requested || shutdown.load(Ordering::Acquire)
        {
            player.stop();
            drop(player);
            return Ok(false);
        }
        inner.state.transition_to(PlaybackState::Playing)?;
        player.set_volume(inner.volume.effective_level());
        inner.player = Some(player);
        inner.accumulated_elapsed = Duration::ZERO;
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

    fn finish_ended_or_advance(&self) -> TrackEndOutcome {
        let mut inner = self.lock_inner();
        if inner.stop_requested || inner.track_switch_requested {
            return TrackEndOutcome::StillPlaying;
        }
        if inner.state != PlaybackState::Playing
            || !inner.player.as_ref().is_some_and(Player::empty)
        {
            return TrackEndOutcome::StillPlaying;
        }

        let track_elapsed = inner.settle_active_elapsed();
        inner.total_session_elapsed += track_elapsed;
        drop(inner.player.take());
        inner.stop_signal = None;

        if inner
            .queue
            .as_mut()
            .is_some_and(|queue| queue.advance_next().is_ok())
        {
            inner.accumulated_elapsed = Duration::ZERO;
            let _ = inner.state.transition_to(PlaybackState::Loading);
            self.shared.state_changed.notify_all();
            return TrackEndOutcome::NextTrack;
        }

        let total_elapsed = inner.total_session_elapsed;
        let _ = inner.state.transition_to(PlaybackState::Ended);
        self.shared.state_changed.notify_all();
        TrackEndOutcome::Ended(total_elapsed)
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
            inner.track_switch_requested = false;
        }
        inner.stop_signal = None;
        self.shared.state_changed.notify_all();
        inner.last_stopped_elapsed
    }

    fn fail_active(&self) {
        let mut inner = self.lock_inner();
        let track_elapsed = inner.settle_active_elapsed();
        inner.total_session_elapsed += track_elapsed;
        if let Some(player) = inner.player.take() {
            player.stop();
            drop(player);
        }
        inner.stop_signal = None;
        inner.track_switch_requested = false;
        let _ = inner.state.transition_to(PlaybackState::Error);
        self.shared.state_changed.notify_all();
    }

    fn wait_poll_or_event(&self, timeout: Duration) {
        let inner = self.lock_inner();
        if inner.pending_events.is_empty() && !inner.stop_requested && !inner.track_switch_requested
        {
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
    on_event: impl FnMut(PlaybackEvent),
) -> Result<PlaybackSummary, AudioError> {
    play_local_playlist_with_controller(&[path.as_ref()], controller, shutdown, on_event)
}

/// Plays a sequence of local FLAC files using the default system audio device.
///
/// Advances sequentially through the playlist, tracks playback history, and stops
/// cleanly at the end of the queue or when `shutdown` is set to `true`.
pub fn play_local_playlist<P: AsRef<Path>>(
    tracks: &[P],
    shutdown: Arc<AtomicBool>,
    on_event: impl FnMut(PlaybackEvent),
) -> Result<PlaybackSummary, AudioError> {
    play_local_playlist_with_controller(tracks, PlaybackController::new(), shutdown, on_event)
}

/// Plays a sequence of local FLAC files using a caller-supplied [`PlaybackController`].
///
/// Supports track navigation (`next_track`, `previous_track`), bounded playback history,
/// boundary validation, pause/resume, and stop controls.
pub fn play_local_playlist_with_controller<P: AsRef<Path>>(
    tracks: &[P],
    controller: PlaybackController,
    shutdown: Arc<AtomicBool>,
    mut on_event: impl FnMut(PlaybackEvent),
) -> Result<PlaybackSummary, AudioError> {
    let queue = TrackQueue::new(tracks.iter().map(|p| p.as_ref().to_path_buf()));
    if queue.is_empty() {
        return Err(AudioError::InvalidState(
            "cannot start playback with an empty track list".to_string(),
        ));
    }

    controller.begin_loading_with_queue(queue)?;

    loop {
        // Clear any pending switch flag before loading the newly selected track.
        let _ = controller.take_track_switch();
        let Some(track_path) = controller.current_queue_track() else {
            return Err(AudioError::InvalidState(
                "playback queue has no active track".to_string(),
            ));
        };

        match play_single_queue_track(&track_path, &controller, &shutdown, &mut on_event)? {
            TrackWaitOutcome::SwitchTrack => continue,
            TrackWaitOutcome::Stopped(summary) | TrackWaitOutcome::Ended(summary) => {
                return Ok(summary);
            }
        }
    }
}

fn play_single_queue_track(
    path: &Path,
    controller: &PlaybackController,
    shutdown: &AtomicBool,
    on_event: &mut impl FnMut(PlaybackEvent),
) -> Result<TrackWaitOutcome, AudioError> {
    on_event(PlaybackEvent::StateChanged(PlaybackState::Loading));

    if should_stop_now(controller, shutdown) {
        return Ok(TrackWaitOutcome::Stopped(finish_with_stop(
            controller,
            None,
            None,
            None,
            AudioStreamInfo::uninitialized(),
            OutputDeviceInfo::uninitialized(),
            on_event,
        )));
    }
    if controller.take_track_switch() {
        return Ok(TrackWaitOutcome::SwitchTrack);
    }

    let validated = match validate_audio_file(path) {
        Ok(file) => file,
        Err(err) => {
            if should_stop_now(controller, shutdown) {
                return Ok(TrackWaitOutcome::Stopped(finish_with_stop(
                    controller,
                    None,
                    None,
                    None,
                    AudioStreamInfo::uninitialized(),
                    OutputDeviceInfo::uninitialized(),
                    on_event,
                )));
            }
            if controller.take_track_switch() {
                return Ok(TrackWaitOutcome::SwitchTrack);
            }
            return Err(fail_and_emit(controller, err, on_event));
        }
    };

    if should_stop_now(controller, shutdown) {
        return Ok(TrackWaitOutcome::Stopped(finish_with_stop(
            controller,
            None,
            None,
            None,
            AudioStreamInfo::uninitialized(),
            OutputDeviceInfo::uninitialized(),
            on_event,
        )));
    }
    if controller.take_track_switch() {
        return Ok(TrackWaitOutcome::SwitchTrack);
    }

    let decoder = match FlacDecoder::open(&validated.path) {
        Ok(dec) => dec,
        Err(err) => {
            if should_stop_now(controller, shutdown) {
                return Ok(TrackWaitOutcome::Stopped(finish_with_stop(
                    controller,
                    None,
                    None,
                    None,
                    AudioStreamInfo::uninitialized(),
                    OutputDeviceInfo::uninitialized(),
                    on_event,
                )));
            }
            if controller.take_track_switch() {
                return Ok(TrackWaitOutcome::SwitchTrack);
            }
            return Err(fail_and_emit(controller, err, on_event));
        }
    };

    let stream_info = decoder.stream_info().clone();
    if should_stop_now(controller, shutdown) {
        drop(decoder);
        return Ok(TrackWaitOutcome::Stopped(finish_with_stop(
            controller,
            None,
            None,
            None,
            stream_info,
            OutputDeviceInfo::uninitialized(),
            on_event,
        )));
    }
    if controller.take_track_switch() {
        drop(decoder);
        return Ok(TrackWaitOutcome::SwitchTrack);
    }

    let channels = match NonZeroU16::new(stream_info.channels) {
        Some(ch) => ch,
        None => {
            return Err(fail_and_emit(
                controller,
                AudioError::UnsupportedConfiguration("channel count cannot be zero".to_string()),
                on_event,
            ));
        }
    };
    let sample_rate = match NonZeroU32::new(stream_info.sample_rate) {
        Some(rate) => rate,
        None => {
            return Err(fail_and_emit(
                controller,
                AudioError::UnsupportedConfiguration("sample rate cannot be zero".to_string()),
                on_event,
            ));
        }
    };

    let shared_error = Arc::new(Mutex::new(None));

    let output = match AudioOutput::open_default_with_error_sink(Some(Arc::clone(&shared_error))) {
        Ok(out) => out,
        Err(err) => {
            if should_stop_now(controller, shutdown) {
                drop(decoder);
                return Ok(TrackWaitOutcome::Stopped(finish_with_stop(
                    controller,
                    None,
                    None,
                    None,
                    stream_info,
                    OutputDeviceInfo::uninitialized(),
                    on_event,
                )));
            }
            if controller.take_track_switch() {
                drop(decoder);
                return Ok(TrackWaitOutcome::SwitchTrack);
            }
            return Err(fail_and_emit(controller, err, on_event));
        }
    };
    let output_info = output.device_info().clone();

    if should_stop_now(controller, shutdown) {
        drop(decoder);
        return Ok(TrackWaitOutcome::Stopped(finish_with_stop(
            controller,
            Some(output),
            None,
            None,
            stream_info,
            output_info,
            on_event,
        )));
    }
    if controller.take_track_switch() {
        drop(decoder);
        drop(output);
        return Ok(TrackWaitOutcome::SwitchTrack);
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
        if controller.take_track_switch() && !should_stop_now(controller, shutdown) {
            drop(output);
            return Ok(TrackWaitOutcome::SwitchTrack);
        }
        return Ok(TrackWaitOutcome::Stopped(finish_with_stop(
            controller,
            Some(output),
            None,
            None,
            stream_info,
            output_info,
            on_event,
        )));
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
        controller,
        shutdown,
        &stop_decoder,
        &decoder_done,
        &shared_error,
    ) {
        Ok(Some(chunk)) => chunk,
        Ok(None) => {
            if controller.take_track_switch() && !should_stop_now(controller, shutdown) {
                drop(output);
                join_decoder_worker(decoder_handle, &stop_decoder);
                return Ok(TrackWaitOutcome::SwitchTrack);
            }
            return Ok(TrackWaitOutcome::Stopped(finish_with_stop(
                controller,
                Some(output),
                Some(decoder_handle),
                Some(&stop_decoder),
                stream_info,
                output_info,
                on_event,
            )));
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
    match controller.start_playing(player, shutdown) {
        Ok(true) => {}
        Ok(false) => {
            if controller.take_track_switch() && !should_stop_now(controller, shutdown) {
                drop(output);
                join_decoder_worker(decoder_handle, &stop_decoder);
                return Ok(TrackWaitOutcome::SwitchTrack);
            }
            return Ok(TrackWaitOutcome::Stopped(finish_with_stop(
                controller,
                Some(output),
                Some(decoder_handle),
                Some(&stop_decoder),
                stream_info,
                output_info,
                on_event,
            )));
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
    controller: &PlaybackController,
    output: AudioOutput,
    decoder_handle: JoinHandle<()>,
    stream_info: AudioStreamInfo,
    output_info: OutputDeviceInfo,
    shutdown: &AtomicBool,
    stop_decoder: Arc<AtomicBool>,
    decoder_done: Arc<AtomicBool>,
    shared_error: Arc<Mutex<Option<AudioError>>>,
    on_event: &mut impl FnMut(PlaybackEvent),
) -> Result<TrackWaitOutcome, AudioError> {
    loop {
        controller.drain_pending_events(on_event);

        if should_stop_now(controller, shutdown) {
            return Ok(TrackWaitOutcome::Stopped(finish_with_stop(
                controller,
                Some(output),
                Some(decoder_handle),
                Some(&stop_decoder),
                stream_info,
                output_info,
                on_event,
            )));
        }

        if controller.take_track_switch() {
            drop(output);
            join_decoder_worker(decoder_handle, &stop_decoder);
            return Ok(TrackWaitOutcome::SwitchTrack);
        }

        if let Some(err) = take_shared_error(&shared_error) {
            controller.fail_active();
            drop(output);
            join_decoder_worker(decoder_handle, &stop_decoder);
            controller.drain_pending_events(on_event);
            on_event(PlaybackEvent::StateChanged(PlaybackState::Error));
            return Err(err);
        }

        if controller.is_ready_to_finish_playing() && decoder_done.load(Ordering::Acquire) {
            // Allow the hardware ring buffer a brief drain window unless interrupted.
            if !controller.is_switch_or_stop_requested(shutdown) {
                controller.wait_poll_or_event(Duration::from_millis(50));
            }
            controller.drain_pending_events(on_event);

            if should_stop_now(controller, shutdown) {
                return Ok(TrackWaitOutcome::Stopped(finish_with_stop(
                    controller,
                    Some(output),
                    Some(decoder_handle),
                    Some(&stop_decoder),
                    stream_info,
                    output_info,
                    on_event,
                )));
            }

            if controller.take_track_switch() {
                drop(output);
                join_decoder_worker(decoder_handle, &stop_decoder);
                return Ok(TrackWaitOutcome::SwitchTrack);
            }

            if let Some(err) = take_shared_error(&shared_error) {
                controller.fail_active();
                drop(output);
                join_decoder_worker(decoder_handle, &stop_decoder);
                on_event(PlaybackEvent::StateChanged(PlaybackState::Error));
                return Err(err);
            }

            match controller.finish_ended_or_advance() {
                TrackEndOutcome::StillPlaying => {}
                TrackEndOutcome::NextTrack => {
                    drop(output);
                    join_decoder_worker(decoder_handle, &stop_decoder);
                    return Ok(TrackWaitOutcome::SwitchTrack);
                }
                TrackEndOutcome::Ended(elapsed) => {
                    drop(output);
                    join_decoder_worker(decoder_handle, &stop_decoder);
                    controller.drain_pending_events(on_event);
                    on_event(PlaybackEvent::StateChanged(PlaybackState::Ended));
                    return Ok(TrackWaitOutcome::Ended(PlaybackSummary {
                        final_state: PlaybackState::Ended,
                        elapsed,
                        stream: stream_info,
                        output: output_info,
                    }));
                }
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
        if controller.is_switch_or_stop_requested(shutdown) {
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
                let sample = prevent_clipping(self.current_chunk[self.offset]);
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

    #[test]
    fn controller_navigates_next_previous_tracks_with_history_and_boundaries() {
        let controller = PlaybackController::new();
        assert!(matches!(
            controller.next_track(),
            Err(AudioError::InvalidState(_))
        ));
        assert!(matches!(
            controller.previous_track(),
            Err(AudioError::InvalidState(_))
        ));

        let queue = TrackQueue::new(vec![
            PathBuf::from("a.flac"),
            PathBuf::from("b.flac"),
            PathBuf::from("c.flac"),
        ]);
        controller.begin_loading_with_queue(queue).unwrap();
        controller.set_state_for_test(PlaybackState::Playing);
        assert_eq!(controller.track_count(), 3);
        assert_eq!(controller.current_track_index(), Some(0));
        assert_eq!(controller.current_track(), Some(PathBuf::from("a.flac")));
        assert!(controller.has_next_track());
        assert!(!controller.has_previous_track());

        // Start boundary keeps active playback untouched
        assert!(matches!(
            controller.previous_track(),
            Err(AudioError::InvalidState(_))
        ));
        assert_eq!(controller.state(), PlaybackState::Playing);
        assert_eq!(controller.current_track_index(), Some(0));

        // Next -> b.flac
        assert_eq!(controller.next_track().unwrap(), PathBuf::from("b.flac"));
        assert_eq!(controller.state(), PlaybackState::Loading);
        assert_eq!(controller.current_track_index(), Some(1));
        assert_eq!(controller.position(), Duration::ZERO);
        assert!(controller.has_previous_track());
        assert!(controller.has_next_track());

        // Pause then Next -> c.flac
        controller.set_state_for_test(PlaybackState::Paused);
        assert_eq!(controller.next_track().unwrap(), PathBuf::from("c.flac"));
        assert_eq!(controller.current_track_index(), Some(2));
        assert!(!controller.has_next_track());

        // End boundary preserves active state cleanly
        controller.set_state_for_test(PlaybackState::Playing);
        assert!(matches!(
            controller.next_track(),
            Err(AudioError::InvalidState(_))
        ));
        assert_eq!(controller.state(), PlaybackState::Playing);
        assert_eq!(controller.current_track_index(), Some(2));

        // Previous -> b.flac -> a.flac
        assert_eq!(
            controller.previous_track().unwrap(),
            PathBuf::from("b.flac")
        );
        assert_eq!(
            controller.previous_track().unwrap(),
            PathBuf::from("a.flac")
        );
        assert!(!controller.has_previous_track());
        assert!(matches!(
            controller.previous_track(),
            Err(AudioError::InvalidState(_))
        ));

        assert_eq!(
            controller.played_history(),
            vec![
                PathBuf::from("a.flac"),
                PathBuf::from("b.flac"),
                PathBuf::from("c.flac"),
                PathBuf::from("b.flac"),
                PathBuf::from("a.flac"),
            ]
        );
        assert!(controller.back_history().is_empty());
    }

    #[test]
    fn channel_audio_source_prevents_sample_clipping() {
        let (_tx, rx) = mpsc::sync_channel(2);
        let done = Arc::new(AtomicBool::new(true));
        let channels = NonZeroU16::new(2).unwrap();
        let rate = NonZeroU32::new(44_100).unwrap();

        let src = ChannelAudioSource::new(
            vec![1.5, -1.75, 0.5, f32::NAN],
            rx,
            channels,
            rate,
            None,
            done,
        );
        let samples: Vec<f32> = src.collect();
        assert_eq!(samples, vec![1.0, -1.0, 0.5, 0.0]);
    }

    #[test]
    fn controller_manages_volume_ranges_mute_and_clamping() {
        let controller = PlaybackController::new();
        assert_eq!(controller.volume(), 1.0);
        assert_eq!(controller.volume_percent(), 100);
        assert!(!controller.is_muted());
        assert_eq!(controller.effective_volume(), 1.0);

        // Out-of-range and non-finite inputs are rejected cleanly
        for bad in [-0.1, 1.05, f32::NAN, f32::INFINITY] {
            assert!(matches!(
                controller.set_volume(bad),
                Err(AudioError::InvalidVolume(_))
            ));
        }
        assert!(matches!(
            controller.set_volume_percent(101),
            Err(AudioError::InvalidVolume(_))
        ));
        assert!(matches!(
            controller.adjust_volume(f32::NEG_INFINITY),
            Err(AudioError::InvalidVolume(_))
        ));
        assert_eq!(controller.volume(), 1.0);

        // Stepping down and clamping at 0.0 / 1.0 prevents clipping
        assert!((controller.decrease_volume() - 0.95).abs() < 1e-6);
        assert_eq!(controller.increase_volume(), 1.0);
        assert_eq!(controller.increase_volume(), 1.0);

        controller.set_volume(0.02).unwrap();
        assert_eq!(controller.decrease_volume(), 0.0);

        // Mute / unmute preserves configured volume
        controller.set_volume_percent(60).unwrap();
        controller.mute();
        assert!(controller.is_muted());
        assert_eq!(controller.effective_volume(), 0.0);
        assert_eq!(controller.volume_percent(), 60);

        controller.unmute();
        assert!(!controller.is_muted());
        assert!((controller.effective_volume() - 0.6).abs() < 1e-6);

        assert!(controller.toggle_mute());
        assert!(!controller.toggle_mute());
    }
}
