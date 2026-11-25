# Termwave

Termwave is a lightweight, cross-platform, lossless-first terminal Hi-Fi music player written in Rust.

## Project Philosophy

- **Local-first:** Local playback operates completely offline without external services.
- **Lossless-first:** FLAC decoding and audio signal integrity are primary design goals.
- **Modular:** Audio sources, decoding, playback, library persistence, and UI remain decoupled.
- **Cross-platform:** Targets Linux and Windows.

## Status

Currently at **Phase 01 (Project initialization)** of the development roadmap in [AGENTS.md](AGENTS.md). Playback, CLI commands, metadata extraction, and the terminal UI will be implemented incrementally in subsequent phases.

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
