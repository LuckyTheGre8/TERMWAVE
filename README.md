# Termwave

Termwave is a lightweight, cross-platform, lossless-first terminal Hi-Fi music player written in Rust.

## Project Philosophy

- **Local-first:** Local playback operates completely offline without external services.
- **Lossless-first:** FLAC decoding and audio signal integrity are primary design goals.
- **Modular:** Audio sources, decoding, playback, library persistence, and UI remain decoupled.
- **Cross-platform:** Targets Linux and Windows.

## Status

Completed **Phase 08 (Playback error handling)**, concluding **Milestone 1: Working Local FLAC Player** of the development roadmap in [AGENTS.md](AGENTS.md). `termwave play <file>` provides robust offline local FLAC playback: validating inputs, rejecting invalid/corrupt audio and non-finite samples, catching asynchronous audio output device disconnections and backend errors, and gracefully handling unexpected I/O or permission failures with actionable user feedback. Stage 2 (Playback Engine) will introduce interactive playback controls (pause/resume, stop, seeking, volume).

## Usage

```sh
termwave --help
termwave --version
termwave play <file>
```

## Module Structure

- `src/main.rs` — Thin binary entry point
- `src/lib.rs` — Core library exports
- `src/cli.rs` — Command-line interface boundary
- `src/audio/` — Audio engine boundary (`PlaybackState`, `validation`, `decoder`, `output`, and `playback`)
- `src/metadata.rs` — Metadata and embedded artwork boundary
- `src/library.rs` — Music library and SQLite persistence boundary
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
