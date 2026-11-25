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
│   └── validation.rs  # Local audio file existence, permission, and format validation
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
- Owns local file validation (`validation.rs`), playback lifecycle (`PlaybackState`: `Stopped`, `Loading`, `Playing`, `Paused`, `Buffering`, `Seeking`, `Ended`, `Error`), and future audio engine layers.
- Target internal layering (implemented incrementally across Stages 1–6):
  `Playback Manager -> Audio Source Abstraction -> Audio Decoder -> Audio Processing Pipeline -> Audio Output Backend -> System Audio Device`.
- Decoders must remain decoupled from filesystem paths so local files, HTTP streams, and network sources share the same playback pipeline.
- Seeking capability must be represented explicitly rather than assumed for every source.
- Audio callbacks must remain non-blocking and free of disk/network I/O or UI synchronization locks.

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
