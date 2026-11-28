# Termwave

Termwave is a lightweight, cross-platform, lossless-first terminal Hi-Fi music player written in Rust.

## Project Philosophy

- **Local-first:** Local playback operates completely offline without external services.
- **Lossless-first:** FLAC decoding and audio signal integrity are primary design goals.
- **Modular:** Audio sources, decoding, playback, library persistence, and UI remain decoupled.
- **Cross-platform:** Targets Linux and Windows.

## Status

Currently at **Phase 26 (Library browser)** in **Stage 4 (Terminal User Interface)** of the development roadmap in [AGENTS.md](AGENTS.md). `termwave play` plays local FLAC files with extracted Vorbis comments and artwork metadata, backed by a persistent SQLite library (`rusqlite`) with directory scanning (`walkdir`), indexing, FTS5/indexed search, favorites, and batched playback history, while `termwave tui` initializes Ratatui + Crossterm with a responsive dashboard layout and an interactive library browser connected to SQLite queries for tracks, albums, artists, favorites, and recently played history. Now-playing details follow in Phase 27.

## Usage

```sh
termwave --help
termwave --version
termwave play <file> [<file>...]
termwave play --volume 80 <file>
termwave tui
```

## Module Structure

- `src/main.rs` — Thin binary entry point
- `src/lib.rs` — Core library exports
- `src/cli.rs` — Command-line interface boundary
- `src/tui/` — Terminal user interface (`mod.rs` loop, `terminal.rs` lifecycle, `layout.rs` dashboard, `browser.rs` library browser)
- `src/audio/` — Audio engine boundary (`PlaybackState`, `validation`, `decoder`, `output`, `queue`, `volume`, and `playback`)
- `src/metadata/` — Metadata (`mod.rs`) and embedded artwork (`artwork.rs`) boundary
- `src/library/` — Music library directory scanning (`scanner.rs`), SQLite database/migrations (`db.rs`), indexing (`indexer.rs`), search (`search.rs`), favorites (`favorites.rs`), and playback history (`history.rs`) boundary
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
