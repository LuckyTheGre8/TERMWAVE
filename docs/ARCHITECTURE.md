# Termwave Architecture

Termwave separates presentation, playback, metadata extraction, library persistence, and configuration into independent Rust modules so local playback remains reliable, offline-capable, and extensible.

## Crate Layout

```text
src/
├── main.rs            # Thin binary entry point (propagates anyhow::Result)
├── lib.rs             # Library root exposing top-level module boundaries
├── cli.rs             # CLI command interface and user-facing entry flow
├── audio/             # Audio engine boundary
│   ├── mod.rs         # PlaybackState lifecycle and AudioError definitions
│   ├── validation.rs  # Local audio file existence, permission, and format validation
│   ├── decoder.rs     # Source-agnostic Symphonia FLAC decoder and AudioStreamInfo
│   ├── output.rs      # Encapsulated rodio/cpal system audio output initialization
│   ├── queue.rs       # Ordered track queue, history stacks, and boundary guards
│   ├── volume.rs      # Volume normalization, range validation, mute, and clipping guard
│   └── playback.rs    # Bounded-channel decoder worker and rodio playback pipeline
├── metadata.rs        # Tag and embedded artwork extraction boundary
├── library.rs         # Music library scanning, search, and SQLite boundary
├── config.rs          # User preferences and platform-aware defaults
└── error.rs           # Unified TermwaveError and crate Result<T> alias
```

## Module Responsibilities and Boundaries

### `cli` (`src/cli.rs`)
- Parses command-line arguments and subcommands using `clap` (`play <file>` plus planned `info`, `scan`, `tui`, and `devices` subcommands).
- Handles command-line entry, `--help` / `--version`, and delegates work to lower-level subsystems.
- Must not own decoding, audio device management, or database logic.
- Planned companion UI layer (`tui` in later phases) will follow the same rule: presentation reads state and dispatches commands without blocking playback.

### `audio` (`src/audio/`)
- Owns local file validation (`validation.rs`), source-agnostic FLAC decoding (`decoder.rs` powered by Symphonia), system audio output initialization (`output.rs` encapsulating `rodio`/`cpal`), bounded-channel playback execution and control (`playback.rs` with `PlaybackController`), and the validated `PlaybackState` lifecycle (`mod.rs`).
- Target internal layering (implemented incrementally across Stages 1–6):
  `Playback Manager -> Audio Source Abstraction -> Audio Decoder -> Audio Processing Pipeline -> Audio Output Backend -> System Audio Device`.
- Decoders must remain decoupled from filesystem paths so local files, HTTP streams, and network sources share the same playback pipeline.
- Seeking capability must be represented explicitly rather than assumed for every source.
- Audio callbacks must remain non-blocking and free of disk/network I/O or UI synchronization locks.
- State machine & playback control:
  - `PlaybackState::can_transition_to` and `PlaybackState::transition_to` enforce valid engine lifecycle transitions and reject invalid operations via `AudioError::InvalidState`.
  - `PlaybackController` provides centralized state synchronization (`snapshot()`, monotonic `revision()`, non-blocking bounded `subscribe()` channels, `wait_for_update()`, active `current_track_info()`, and `last_error()`), alongside thread-safe `pause()`, `resume()`, `toggle_pause()`, `stop()`, `next_track()`, `previous_track()`, bounded queue/history inspection, active playback position tracking (resetting on stop or track change), cancellation during loading, and immediate release of audio player, output sink, and decoder worker resources.
  - Seek wakeup synchronization checks coordinator readiness alongside pending events and stop/switch flags to avoid race conditions or missed wakeups across UI, worker, and controller threads.
  - `TrackQueue` (`queue.rs`) tracks current queue position, visited back-navigation stack, chronological played history (bounded to avoid unbounded memory growth), and start/end boundary guards.
  - `VolumeState` (`volume.rs`) validates gain strictly within `[0.0, 1.0]` (unity-capped to prevent overdrive clipping), preserves configured volume across mute toggles and track transitions, and `prevent_clipping` bounds PCM output samples within `[-1.0, 1.0]`.
  - Seeking (`decoder.rs` + `SeekCoordinator` in `playback.rs`) represents stream seekability explicitly via `SeekCapabilities`, executes forward/backward/absolute FLAC seeks on the decoder worker thread (never inside real-time audio callbacks), trims partial leading frames accurately, and preserves `Playing` or `Paused` state across seeks.
  - Gapless playback groundwork & decoder preloading (`PreloadedTrack`, `TrackQueue::peek_next`):
    - `TrackQueue` provides forward lookahead (`peek_next`, `peek_next_index`, `remaining_tracks`, `is_consecutive`) without advancing the queue cursor.
    - `PreloadedTrack` validates upcoming audio files and probes Symphonia FLAC headers off the audio thread while the current track's ring buffer drains, eliminating filesystem and header parsing latency when the track advances.
    - `AudioOutput` stream reuse keeps the OS mixer/device stream open across playlist tracks, avoiding hardware re-initialization and audio clicks.
    - Playback state (configured volume, mute state, session listening duration, and queue history) is strictly preserved across consecutive track transitions.
- Robust error propagation:
  - Input validation traps nonexistent, empty, non-file, unsupported, and process-locked audio files before playback begins.
  - Decoder traps corrupted headers, truncated packets, unsupported sample formats/rates, and non-finite (`NaN`/`Inf`) PCM audio samples.
  - Audio output hooks CPAL stream error callbacks into a thread-safe error sink to report asynchronous device loss (`AudioError::DeviceLost`) and fatal backend faults without dropping audio thread safety.

### `metadata` (`src/metadata.rs`)
- Extracts track tags, technical stream properties, and bounded embedded artwork.
- Explicitly distinguishes source audio format properties (codec, sample rate, bit depth, channels) from actual output device properties.

### `library` (`src/library.rs`)
- Owns recursive directory scanning, SQLite indexing, search, favorites, and playback history.
- Basic single-file playback must work independently without requiring the library database.

### `config` (`src/config.rs`)
- Owns application settings (`AppConfig`), defaults, validation, and preference persistence.

## Error-Handling Conventions

1. **Domain errors (`thiserror`):** Each subsystem defines a `#[non_exhaustive]` error enum using `thiserror::Error` (e.g., `AudioError`, `MetadataError`, `LibraryError`, `ConfigError`). Subsystems do not depend on each other's error types.
2. **Library boundary (`TermwaveError`):** `src/error.rs` unifies subsystem errors via `#[from]` conversions into `TermwaveError` and exposes `termwave::error::Result<T>`.
3. **Application / CLI boundary (`anyhow`):** `cli::run()` and `main()` return `anyhow::Result<()>` so user-facing entry points can attach contextual messages with `.context(...)` without erasing structured errors inside the core library.

## Cross-Platform and Offline-First Rules

- **Offline-first:** Local playback, metadata reading, and library browsing must never require internet connectivity or external services.
- **Linux and Windows:** Platform-specific audio backend logic (WASAPI on Windows, PipeWire/ALSA on Linux) and terminal behavior must stay isolated behind clean subsystem boundaries.

## Gapless Playback and Preloading Architecture (Phase 15 & Phase 41)

In Phase 15, Termwave establishes the architectural foundation for seamless transitions between consecutive playlist tracks:

1. **Queue Lookahead:** `TrackQueue::peek_next()` and `remaining_tracks()` allow the engine and UI to inspect upcoming tracks without perturbing the active track index or visited history.
2. **Decoder Preloading:** While the current track's ring buffer drains to the hardware device, `PreloadedTrack::preload()` validates the next file and initializes the Symphonia FLAC format reader and codec ahead of time. When the track advances, startup I/O is already complete.
3. **Audio Stream Reuse:** Reusing `AudioOutput` across playlist items eliminates CPAL/WASAPI/ALSA stream teardown and recreation delays, reducing transition time from ~100–200ms to under 2ms.
4. **State Preservation:** Volume, mute state, session listening duration, and visited history stacks are preserved across track boundaries.

### Current Gapless Limitations and Phase 41 Roadmap

While Phase 15 drastically reduces transition delays, full bit-perfect gapless transitions require addressing specific hardware and format constraints:

- **Sample Rate & Channel Layout Differences:** In shared output mode, the OS audio engine resamples streams to the device rate. In exclusive mode (WASAPI Exclusive / ALSA direct in Phase 40/41), differing sample rates (e.g. 44.1 kHz to 96 kHz) necessitate reconfiguring the DAC clock, which introduces an unavoidable hardware resync delay. True gapless requires identical stream sample formats.
- **Buffer Starvation vs. Direct Sample Stitching:** Current architecture instantiates a new `Player` on the shared output mixer once the previous player drains. Phase 41 will implement continuous sample stitching feeding multiple tracks into a single uninterrupted audio source buffer to prevent mixer starvation.
- **FLAC Padding & Encoder Delay:** Unlike lossy MP3/AAC files with priming/padding samples, FLAC audio frames preserve original PCM sample boundaries. Non-standard variable block sizes or padding blocks are handled by Symphonia frame decoding.
