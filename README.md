# Termwave

Termwave is a lightweight, cross-platform, lossless-first terminal Hi-Fi music player written in Rust.

## Project Philosophy

- **Local-first:** Local playback operates completely offline without external services.
- **Lossless-first:** FLAC decoding and audio signal integrity are primary design goals.
- **Modular:** Audio sources, decoding, playback, library persistence, and UI remain decoupled.
- **Cross-platform:** Targets Linux and Windows.

## Status

Currently at **Phase 10 (Stop functionality)** in Stage 2 (Playback Engine) of the development roadmap in [AGENTS.md](AGENTS.md). `termwave play <file>` supports offline local FLAC playback with thread-safe `pause()`, `resume()`, `toggle_pause()`, and `stop()` controls via `PlaybackController` — immediately releasing decoder/device resources, resetting playback position to zero, handling stops cleanly during loading, and offering interactive terminal commands (`Enter`/`p` to toggle pause/resume, `s`/`stop` to stop, or `Ctrl+C` to quit). Track navigation follows in Phase 11.

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
