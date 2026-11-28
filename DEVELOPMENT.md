# Termwave Development Log & Feature Inventory

**Last Updated:** 2026-11-28  
**Current Roadmap Position:** Phase 29 / 57 (Stage 4 — Terminal User Interface)  
**Latest Commit:** `f6e8439` (`feat: add playback progress visualization`)

---

## 1. Executive Summary

Termwave is a lightweight, cross-platform (Windows & Linux), offline-first, lossless-first terminal Hi-Fi music player written in Rust (Edition 2024, MSRV 1.87).

### Completed Milestones
- **Milestone 1 — Working Local FLAC Player** (Stage 1, Phases 01–08): Complete
- **Milestone 2 — Functional Audio Engine** (Stage 2, Phases 09–15): Complete
- **Milestone 3 — Persistent Music Library** (Stage 3, Phases 16–23): Complete
- **Milestone 4 — Full Interactive Terminal Music Player** (Stage 4, Phases 24–33): In Progress (Phases 24–29 complete)

### Verification Baseline
- `cargo fmt --check`: Passing
- `cargo clippy --all-targets -- -D warnings`: 0 warnings
- `cargo test`: **143 tests passing** (108 unit tests, 11 `flac_decoding` integration tests, 24 `playback_pipeline` integration tests)
- `cargo doc --no-deps`: Clean
- `cargo build` & `cargo build --release`: Clean

---

## 2. Phase-by-Phase Development History

### Stage 1 — Project Foundation and Core Playback (Milestone 1)

| Phase | Title | Commit | Key Deliverables & Features |
|---|---|---|---|
| **01** | Project initialization | `bd899e3` `chore: initialize termwave project` | Rust 2024 package setup, `Cargo.toml`, `.gitignore`, initial `README.md`. |
| **02** | Modular architecture | `8ed9133` `refactor: establish modular application architecture` | Separated `cli`, `audio`, `metadata`, `library`, `config`, and `error` (`TermwaveError` via `thiserror`, CLI `anyhow`) documented in `docs/ARCHITECTURE.md`. |
| **03** | CLI interface | `3ac4f29` `feat: add initial CLI interface` | Integrated `clap` (`--help`, `--version`, `play <file>...`, and subcommand scaffolding). |
| **04** | File validation | `2b128e2` `feat: add audio file validation` | Implemented `src/audio/validation.rs`: existence, regular-file, non-zero size, permission/Windows lock checks, `.flac` extension & `"fLaC"` magic header checks. |
| **05** | FLAC decoding | `6cc1285` `feat: add FLAC decoding support` | Integrated `symphonia` (`src/audio/decoder.rs`) supporting 16/24-bit and 44.1–192 kHz mono/stereo FLAC streams plus `tests/flac_decoding.rs`. |
| **06** | Audio output initialization | `7e69ad2` `feat: initialize system audio output` | Encapsulated `rodio` / `cpal` (`src/audio/output.rs`) with default device detection, missing-device errors, and cross-platform WASAPI/ALSA handling. |
| **07** | Basic playback | `5cc0814` `feat: implement local FLAC playback` | Connected decoder worker thread and bounded ring-buffer channel to `AudioOutput` (`src/audio/playback.rs`) with clean Ctrl+C shutdown and `tests/playback_pipeline.rs`. |
| **08** | Playback error handling | `7deb0d1` `fix: improve playback error handling` | Trapped corrupted frames, truncated streams, non-finite PCM samples, and asynchronous CPAL device loss (`AudioError::DeviceLost`). |

### Stage 2 — Playback Engine (Milestone 2)

| Phase | Title | Commit | Key Deliverables & Features |
|---|---|---|---|
| **09** | Play and pause | `b3ed178` `feat: add playback controls` | Added `PlaybackController` (`pause`, `resume`, `toggle_pause`) with validated `PlaybackState` transitions (`InvalidState` guards) and interactive stdin controls. |
| **10** | Stop functionality | `6da63d6` `feat: add playback stop control` | Added `stop()` releasing player/sink/decoder resources, resetting position, and aborting cleanly during `Loading`. |
| **11** | Track navigation | `990f206` `feat: add track navigation` | Added `TrackQueue` (`src/audio/queue.rs`) with `next_track()`, `previous_track()`, visited back-history stack, and playlist boundary enforcement. |
| **12** | Volume management | `894e36d` `feat: add volume management` | Added `VolumeState` (`src/audio/volume.rs`) with `[0.0, 1.0]` unity-capped gain, `0..=100%` control, mute preservation, and `prevent_clipping` sample clamping. |
| **13** | Seeking | `46cb207` `feat: add playback seeking` | Added `SeekCapabilities` and off-audio-thread `SeekCoordinator` supporting forward, backward, and absolute seeking during `Playing` and `Paused` states. |
| **14** | Playback state tracking | `57574bf` `feat: add playback state tracking` | Added centralized `PlaybackSnapshot`, monotonic `revision()`, bounded `subscribe()` channels, `wait_for_update()`, `last_error()`, and paused-time exclusion from elapsed position. |
| **15** | Gapless playback groundwork | `5089248` `feat: prepare gapless playback support` | Added `TrackQueue::peek_next()`, background `PreloadedTrack::preload()`, `AudioOutput` stream reuse across tracks, and preserved volume/mute/session state across transitions. |

### Stage 3 — Metadata and Music Library (Milestone 3)

| Phase | Title | Commit | Key Deliverables & Features |
|---|---|---|---|
| **16** | Metadata extraction | `76c508e` `feat: extract audio metadata` | Integrated `lofty` (`src/metadata/`) extracting Vorbis comments (`title`, `artist`, `album`, `album_artist`, `track`, `disc`, `genre`, `year`, `date`), stream properties, and missing/malformed tag fallbacks. |
| **17** | Embedded artwork | `32cac39` `feat: support embedded album artwork` | Added `src/metadata/artwork.rs`: front/back cover prioritization, magic-byte & MIME format detection (`PNG`/`JPEG`/`GIF`/`WebP`/`BMP`/`TIFF`), header dimension parsing, FLAC `PICTURE` block pre-scan memory cap (8 MiB default), and terminal cell layout / textual placeholder preparation. |
| **18** | Directory scanning | `1c519c4` `feat: add recursive music library scanning` | Integrated `walkdir` (`src/library/scanner.rs`) with recursive traversal, `.flac` detection, hidden/empty/irrelevant file filtering, non-fatal `ScanWarning`s for inaccessible subdirs, and non-blocking `spawn_directory_scan` with `ScanProgress` & cooperative cancellation. |
| **19** | SQLite database | `a6be7ff` `feat: add persistent music library` | Integrated `rusqlite` (bundled SQLite) in `src/library/db.rs` with WAL mode, `PRAGMA foreign_keys = ON`, transactional `schema_migrations`, `artists`/`albums`/`tracks` tables, indexes, and typed CRUD/upsert APIs. |
| **20** | Library indexing | `9c1c73d` `feat: index scanned music files` | Added `src/library/indexer.rs`: transactional batch indexing, path normalization & deduplication, unchanged-file skipping (`file_size` + `last_modified`), changed-file metadata updates, move/rename in-place reconciliation, missing-file pruning, and orphan cleanup. |
| **21** | Music search | `ad4cc37` `feat: add library search` | Added Migration 2 (`COLLATE NOCASE` indexes + FTS5 `tracks_fts` with sync triggers) and `src/library/search.rs` (`search_tracks`, `search_artists`, `search_albums`, `search_library`) with partial matching, wildcard escaping, relevance ranking, and pagination. |
| **22** | Favorites | `d94ace2` `feat: add favorite tracks` | Added Migration 3 (`favorites` table with `ON DELETE CASCADE`) and `src/library/favorites.rs` (`add`, `remove`, `toggle`, `is_favorite`, `list_favorites`, `list_available_favorites`, `prune_missing_favorites`) with missing-file detection. |
| **23** | Playback history | `16e9f00` `feat: track playback history` | Added Migration 4 (`playback_history` table) and `src/library/history.rs` (`record_playback`, `recently_played`, `recently_played_unique_tracks`, plus write-throttled `HistoryRecorder` batching entries and filtering short skips). |

### Stage 4 — Terminal User Interface (In Progress)

| Phase | Title | Commit | Key Deliverables & Features |
|---|---|---|---|
| **24** | TUI integration | `281f7d6` `feat: initialize terminal UI` | Integrated `ratatui` + `crossterm` (`src/tui/`), RAII `TuiTerminal` with raw-mode/alternate-screen/panic-hook restoration, event-driven `run_tui_loop` with dirty-redraw tracking, and wired `termwave tui`. |
| **25** | Main layout | `138852d` `feat: add terminal dashboard layout` | Added `src/tui/layout.rs`: responsive `DashboardLayout` (header, 10-section navigation sidebar, main content panel, bottom playback bar) adapting across wide, medium, and ultra-compact viewports. |
| **26** | Library browser | `597db5b` `feat: add interactive music library` | Added `src/tui/browser.rs` (`LibraryBrowser`, `ListCursor`) connecting SQLite queries (`tracks`, `albums`, `artists`, `favorites`, `recently_played`) to Ratatui stateful `List` widgets with per-section keyboard navigation. |
| **27** | Now-playing interface | `e3d7db5` `feat: add now playing interface` | Added `src/tui/now_playing.rs` (`NowPlayingInfo`) displaying live track, artist, album, playback state badge, volume, and source-vs-output audio properties with missing-metadata fallbacks. |
| **28** | Keyboard controls | `002143a` `feat: implement keyboard controls` | Added `src/tui/keymap.rs` (`resolve_key_action`, `dispatch_playback_action`, `DOCUMENTED_SHORTCUTS`) with conflict-free global & focus-scoped shortcuts, Shuffle/Repeat toggles, and the `?` Help overlay. |
| **29** | Playback progress | `f6e8439` `feat: add playback progress visualization` | Added Ratatui `LineGauge` progress bar with elapsed/total duration (`MM:SS` / `HH:MM:SS`), `0`–`9` percentage seeking + `seek_target_from_column`, and sub-second redraw throttling. |

---

## 3. Feature Inventory by Subsystem

### Audio Engine (`src/audio/`)
- **FLAC Decoding (`decoder.rs`)**: Lossless 16-bit and 24-bit PCM decoding across 44.1 kHz, 48 kHz, 96 kHz, and 192 kHz (mono/stereo) via Symphonia; explicit `SeekCapabilities`.
- **File Validation (`validation.rs`)**: Pre-decode validation for missing files, directories, 0-byte files, permissions/locks, extensions, and `"fLaC"` magic bytes.
- **Output Backend (`output.rs`)**: `rodio` + `cpal` output stream initialization, device info reporting, and async device-disconnection detection.
- **Playback Pipeline (`playback.rs`)**: Non-blocking real-time audio callback fed by bounded channels; pause/resume/stop/seek/volume/mute; centralized `PlaybackController` snapshots & subscribers; background `PreloadedTrack` and stream reuse.
- **Queue & Volume (`queue.rs`, `volume.rs`)**: Multi-track queue navigation with history stack, lookahead, unity-capped volume, and anti-clipping sample clamping.

### Metadata & Artwork (`src/metadata/`)
- **Tag Extraction (`mod.rs`)**: Lofty-based Vorbis comment and stream property extraction with graceful fallback (`extract_metadata_or_default`).
- **Embedded Artwork (`artwork.rs`)**: Front/back cover selection, magic-byte & MIME detection, header pixel dimension parsing, FLAC `PICTURE` block size pre-scan guard, and terminal aspect-ratio layout + textual placeholder rendering.

### Persistent Music Library (`src/library/`)
- **Directory Scanner (`scanner.rs`)**: `walkdir` recursive traversal, `.flac` recognition, hidden/empty file filtering, non-fatal error warnings, and cancellable background scanning.
- **SQLite Database (`db.rs`)**: Bundled SQLite with WAL, foreign keys, and 4 versioned transactional migrations (`artists`, `albums`, `tracks`, `tracks_fts`, `favorites`, `playback_history`).
- **Indexer (`indexer.rs`)**: Deduplication, incremental rescanning, metadata updates, move/rename reconciliation preserving track IDs, deletion pruning, and orphan cleanup.
- **Search (`search.rs`)**: Indexed `COLLATE NOCASE` + FTS5 partial matching across tracks, artists, and albums with relevance sorting and pagination.
- **Favorites & History (`favorites.rs`, `history.rs`)**: Persistent favorites with missing-file handling; recently played & unique-track queries with write-throttled `HistoryRecorder` batching.

### Terminal User Interface (`src/tui/`) & CLI (`src/cli.rs`)
- **CLI (`cli.rs`)**: `termwave play [--volume N] [--mute] <FILE>...` with interactive stdin playback controls; `termwave tui` launcher; `--help` / `--version`.
- **Terminal Lifecycle (`terminal.rs`)**: Raw mode, alternate screen, cursor hiding, and idempotent `Drop` + panic-hook restoration.
- **Responsive Dashboard (`layout.rs`)**: Header, 10-section navigation sidebar, scrollable main library panel, 3-row playback bar with `LineGauge`, and centered `?` Help overlay.
- **Keyboard & Progress (`keymap.rs`, `now_playing.rs`, `browser.rs`)**: Conflict-free global/contextual shortcuts, `0`–`9` / `[` / `]` seeking, `NowPlayingInfo` with source-vs-output audio stats, and sub-second redraw throttling.

---

## 4. Core Dependencies (`Cargo.toml`)

| Crate | Version | Role |
|---|---|---|
| `anyhow` | `1.0` | Application/CLI error propagation |
| `clap` | `4.6` (`derive`) | CLI argument parsing |
| `crossterm` | `0.28` | Cross-platform terminal input & raw mode |
| `ctrlc` | `3.5` | Process Ctrl+C signal handling |
| `lofty` | `0.24.0` | Audio tag and embedded artwork parsing |
| `ratatui` | `0.29` | Terminal UI layout and widgets |
| `rodio` | `0.23` (`playback`, `symphonia-flac`) | Audio playback output abstraction |
| `rusqlite` | `0.34` (`bundled`) | Embedded SQLite database |
| `symphonia` | `0.5` (`flac`) | Source-agnostic FLAC decoding |
| `thiserror` | `2.0` | Structured subsystem error enums |
| `walkdir` | `2.5` | Cross-platform recursive directory traversal |

---

## 5. Remaining Roadmap

- **Stage 4 (Remaining — Phases 30–33)**: Playback queue UI (`30`), interactive search UI (`31`), customizable themes & settings (`32`), terminal album artwork protocols (`33`).
- **Stage 5 (Phases 34–41)**: Codec/bit-depth details, audio statistics, ReplayGain, equalizer, FFT spectrum visualizer, audio device selection, exclusive output (WASAPI / ALSA / PipeWire), and gapless playback.
- **Stage 6 (Phases 42–57)**: Generalized audio sources, streaming buffer, HTTP/HTTPS FLAC streaming, Range seeking, NAS/network libraries, reconnect resilience, and final release polish.

---

## 6. Build & Verification Commands

```sh
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test
cargo doc --no-deps
cargo build --release
```
