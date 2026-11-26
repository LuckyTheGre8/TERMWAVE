# Termwave

Termwave is a lightweight, cross-platform, lossless-first terminal Hi-Fi music player written in Rust.

## Project Philosophy

- **Local-first:** Local playback operates completely offline without external services.
- **Lossless-first:** FLAC decoding and audio signal integrity are primary design goals.
- **Modular:** Audio sources, decoding, playback, library persistence, and UI remain decoupled.
- **Cross-platform:** Targets Linux and Windows.

## Status

Currently at **Phase 13 (Seeking)** in Stage 2 (Playback Engine) of the development roadmap in [AGENTS.md](AGENTS.md). `termwave play` supports forward, backward, and absolute seeking across FLAC sample rates/bit depths (`FlacDecoder::seek`, `seek_forward`, `seek_backward`), explicit source `SeekCapabilities` enforcement, seeking while both `Playing` and `Paused` (preserving pause state via `PlaybackController` and performing all I/O off the audio callback), alongside playlist navigation, validated volume/mute controls with clipping prevention, and interactive terminal controls. Centralized state tracking follows in Phase 14.

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
