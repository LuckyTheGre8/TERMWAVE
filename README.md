# Termwave

Termwave is a lightweight, cross-platform, lossless-first terminal Hi-Fi music player written in Rust.

## Project Philosophy

- **Local-first:** Local playback operates completely offline without external services.
- **Lossless-first:** FLAC decoding and audio signal integrity are primary design goals.
- **Modular:** Audio sources, decoding, playback, library persistence, and UI remain decoupled.
- **Cross-platform:** Targets Linux and Windows.

## Status

Currently at **Phase 20 (Library indexing)** in Stage 3 (Metadata and Music Library) of the development roadmap in [AGENTS.md](AGENTS.md). `termwave play` integrates `lofty` to extract FLAC Vorbis comments and embedded album artwork while gracefully supporting missing tags and malformed metadata without playback failure, alongside consecutive track lookahead, decoder preloading, hardware audio stream reuse, volume/mute state preservation across tracks, centralized state tracking, non-blocking recursive directory scanning (`walkdir`), and transactional SQLite library indexing (`rusqlite` with deduplication, change detection, rescanning, move/rename reconciliation, and orphan cleanup). Music search follows in Phase 21.

## Usage

```sh
termwave --help
termwave --version
termwave play <file> [<file>...]
termwave play --volume 80 <file>
```

## Module Structure

- `src/main.rs` — Thin binary entry point
- `src/lib.rs` — Core library exports
- `src/cli.rs` — Command-line interface boundary
- `src/audio/` — Audio engine boundary (`PlaybackState`, `validation`, `decoder`, `output`, `queue`, `volume`, and `playback`)
- `src/metadata/` — Metadata (`mod.rs`) and embedded artwork (`artwork.rs`) boundary
- `src/library/` — Music library directory scanning (`scanner.rs`), SQLite database/migrations (`db.rs`), and library indexing (`indexer.rs`) boundary
- `src/config.rs` — Configuration boundary
- `src/error.rs` — Unified `TermwaveError` (`thiserror`) and `Result<T>`

See [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md) for architecture boundaries and error-handling conventions.

## Prerequisites

- Rust toolchain (`rustc` 1.87+ / Edition 2024) and `cargo`
- On Windows: Visual Studio C++ Build Tools (MSVC + Windows SDK; uses WASAPI for audio output)
- On Linux: Standard C toolchain and ALSA development headers (`alsa-lib` / `libasound2-dev`) or PipeWire/ALSA compatibility layer for `cpal` audio output

## Building and Verification

```sh
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test
cargo build
```
