# Termwave

Termwave is a lightweight, cross-platform, lossless-first terminal Hi-Fi music player written in Rust.

## Project Philosophy

- **Local-first:** Local playback operates completely offline without external services.
- **Lossless-first:** FLAC decoding and audio signal integrity are primary design goals.
- **Modular:** Audio sources, decoding, playback, library persistence, and UI remain decoupled.
- **Cross-platform:** Targets Linux and Windows.

## Status

Currently at **Phase 04 (File validation)** of the development roadmap in [AGENTS.md](AGENTS.md). The `clap`-based CLI interface and local FLAC file validation (path syntax, existence, regular-file type, readability/permissions, and `fLaC` header signature checks) are implemented; FLAC decoding and audio playback will be implemented incrementally in Phases 05–08.

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
- `src/audio/` — Audio engine boundary (`PlaybackState` and local file `validation`)
- `src/metadata.rs` — Metadata and embedded artwork boundary
- `src/library.rs` — Music library and SQLite persistence boundary
- `src/config.rs` — Configuration boundary
- `src/error.rs` — Unified `TermwaveError` (`thiserror`) and `Result<T>`

See [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md) for architecture boundaries and error-handling conventions.

## Prerequisites

- Rust toolchain (`rustc` 1.85+ / Edition 2024) and `cargo`
- On Windows: Visual Studio C++ Build Tools (MSVC + Windows SDK)
- On Linux: Standard C toolchain and system audio development libraries when audio output is introduced in later phases

## Building and Verification

```sh
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test
cargo build
```
