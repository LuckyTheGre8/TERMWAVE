# TERMWAVE — MASTER AGENT INSTRUCTIONS

**Project:** Termwave  
**Document:** AGENTS.md  
**Version:** 1.0  
**Language:** Rust  
**Application Type:** Terminal-based Hi-Fi Music Player  
**Target Platforms:** Linux and Windows  
**Primary Audio Format:** FLAC  
**Development Methodology:** Incremental, milestone-driven development

---

# 1. PROJECT VISION

You are the principal software engineer responsible for developing Termwave.

Termwave is a modern, lightweight, cross-platform terminal-based music player focused on high-quality audio playback, local music libraries, and eventually network-based audio streaming.

The application should combine:

- The simplicity and efficiency of cmus.
- The library management capabilities of modern music players.
- The interactive terminal experience of ncmpcpp.
- The audio-quality focus of dedicated Hi-Fi music players.
- The extensibility of a modular audio engine.

Termwave must be a genuinely useful application, not a demonstration project.

## Core philosophy

1. Local-first: Local playback must work entirely offline.
2. Lossless-first: FLAC support and audio integrity are central priorities.
3. Lightweight: Minimize unnecessary CPU and memory consumption.
4. Modular: Audio decoding, playback, sources, library management and UI must remain separate.
5. Extensible: Future streaming and additional audio formats must not require rewriting the application.
6. Cross-platform: Support Linux and Windows.
7. Terminal-native: The primary interface must work inside a terminal.
8. Reliable: Playback stability takes priority over decorative features.
9. Maintainable: Prefer clean architecture and understandable code over unnecessary abstractions.

# 2. PRODUCT REQUIREMENTS

## 2.1 Primary functionality

Termwave must eventually support:

- Local FLAC playback.
- MP3, WAV, OGG, AAC and other practical formats.
- Music library indexing.
- Album and artist browsing.
- Track search.
- Playlists and playback queues.
- Shuffle and repeat.
- Seeking and volume adjustment.
- Embedded album artwork.
- Playback history.
- Favorites.
- Customizable terminal themes.
- Audio device selection.
- Detailed audio information.
- Equalizer.
- Audio visualization.
- HTTP/HTTPS audio streaming.
- NAS and network music libraries.

## 2.2 Offline requirements

Local playback must never depend on:

- Internet connectivity.
- An external music service.
- An online authentication service.
- A remote database.
- A cloud API.

The application must remain fully functional when disconnected from the internet.

Internet connectivity should only be necessary for optional network features.

# 3. TECHNOLOGY STACK

Use stable, maintained Rust libraries. Verify current crate versions and feature compatibility before introducing dependencies.

## Core technologies

| Component | Preferred technology |
|---|---|
| Programming language | Rust |
| Audio playback | Rodio initially |
| Audio decoding | Symphonia-compatible decoding |
| Low-level audio backend | CPAL where required |
| Terminal interface | Ratatui |
| Terminal input | Crossterm |
| CLI argument parsing | Clap |
| Error handling | Anyhow and Thiserror |
| Metadata extraction | Lofty |
| Database | SQLite |
| Database access | SQLx or rusqlite |
| HTTP networking | Reqwest |
| Asynchronous operations | Tokio |
| Logging | Tracing |
| Configuration | TOML |
| Directory traversal | Walkdir |
| Serialization | Serde |

## Dependency policy

- Do not introduce dependencies without a reason.
- Avoid multiple libraries performing the same responsibility.
- Prefer stable, maintained libraries.
- Check Rust version requirements.
- Check Windows and Linux compatibility.
- Pin compatible dependency versions through Cargo.lock.
- Document platform-specific dependencies.

Rodio is the preferred starting point for playback because it provides a higher-level interface over audio decoding and output.

Do not prematurely replace it with a custom audio engine.

If advanced streaming, decoding or output requirements exceed the capabilities of the initial abstraction, evaluate a lower-level Symphonia and CPAL implementation.

# 4. AUDIO ENGINE DESIGN

The audio engine is the most important subsystem.

Its architecture must accommodate local files and future remote streams.

## 4.1 Conceptual architecture

Application CLI / TUI  
↓  
Application Controller  
↓  
Playback Manager  
↓  
Audio Source Abstraction  
↓  
Audio Decoder  
↓  
Audio Processing Pipeline  
↓  
Audio Output Backend  
↓  
System Audio Device

## 4.2 Audio source abstraction

Do not couple the audio decoder directly to filesystem paths.

Plan for the following implementations:

### LocalFileSource
- Opens local audio files.
- Supports seeking.
- Supports reading metadata.
- Reports file errors.
- Works without internet.

### HttpStreamSource
- Retrieves audio from HTTP/HTTPS endpoints.
- Supports progressive playback.
- Handles partial network reads.
- Supports buffering.
- Handles network interruptions.
- Supports HTTP Range requests when available.

### NetworkFileSource
- Provides access to network-hosted music.
- Supports NAS and network libraries.
- Handles unavailable network resources.

All source implementations must integrate with a shared decoding and playback architecture.

Important: Do not assume every audio source is seekable. Represent seeking capabilities explicitly.

## 4.3 Playback state management

Maintain a clearly defined playback state:

- Stopped
- Loading
- Playing
- Paused
- Buffering
- Seeking
- Ended
- Error

Playback state changes must be communicated safely between the audio engine and UI.

Do not allow terminal UI operations to block the audio processing thread.

## 4.4 Audio thread safety

- Audio callbacks must avoid blocking operations.
- Avoid disk access inside real-time output callbacks.
- Avoid network operations inside audio callbacks.
- Avoid unnecessary heap allocations in real-time audio paths.
- Use appropriate bounded channels and buffers.
- Keep audio processing isolated from terminal rendering.
- Handle device disconnections gracefully.

# 5. FLAC AND HI-FI AUDIO REQUIREMENTS

FLAC is the primary audio format.

Understand the following concepts and preserve their meaning in the implementation.

## 5.1 FLAC characteristics

FLAC is a lossless audio compression format.

It preserves the original decoded PCM audio samples.

Common FLAC configurations include:

- 16-bit / 44.1 kHz.
- 24-bit / 44.1 kHz.
- 24-bit / 48 kHz.
- 24-bit / 96 kHz.
- 24-bit / 192 kHz.

The application must correctly decode supported configurations.

Do not assume all FLAC files are 16-bit or 44.1 kHz.

## 5.2 Audio metadata

Extract and display:

- Track title.
- Artist.
- Album.
- Album artist.
- Track number.
- Disc number.
- Genre.
- Release year.
- Duration.
- Sample rate.
- Bit depth where available.
- Channel count.
- Codec.
- Bitrate where meaningful.
- Embedded artwork.

Distinguish between source metadata and actual output properties.

For example, a 24-bit FLAC file may be decoded into a floating-point representation and then sent through an operating-system mixer.

Do not claim bit-perfect output simply because the source file is FLAC.

## 5.3 Audio quality

Prioritize:

- Correct decoding.
- Avoiding unintended resampling.
- Avoiding unnecessary lossy conversions.
- Preserving channel information.
- Accurate sample-rate reporting.
- Correct volume handling.
- Proper clipping prevention.

Implement ReplayGain as an optional feature.

Implement equalization as an optional feature.

Both must be disabled or neutral by default to preserve the original signal.

## 5.4 Bit-perfect playback

Investigate platform-specific exclusive audio output.

On Windows, investigate WASAPI exclusive mode.

On Linux, investigate appropriate ALSA and PipeWire output configurations.

Do not promise bit-perfect playback unless the actual output path and device configuration have been verified.

Allow users to select the output backend when supported.

# 6. TERMINAL USER INTERFACE

The eventual interface should be polished, responsive and keyboard-driven.

Use Ratatui and Crossterm.

## 6.1 Main interface

The interface should contain:

1. Header with application name and status.
2. Sidebar navigation.
3. Main music library panel.
4. Track information.
5. Now-playing section.
6. Playback controls.
7. Progress indicator.
8. Audio information.
9. Optional visualizer.

## 6.2 Navigation sections

- Home
- Library
- Artists
- Albums
- Playlists
- Queue
- Favorites
- Recently Played
- Search
- Settings

## 6.3 Keyboard shortcuts

Provide configurable defaults:

| Key | Function |
|---|---|
| Space | Play / Pause |
| N | Next track |
| P | Previous track |
| Left | Seek backward |
| Right | Seek forward |
| Up | Increase volume |
| Down | Decrease volume |
| S | Shuffle |
| R | Repeat |
| Q | Quit |
| / | Search |
| Enter | Select / Play |
| Esc | Back / Close |
| L | Library |
| F | Favorite |
| ? | Help |

The interface must remain usable on terminals without image protocol support.

For artwork, support compatible terminal image protocols where possible, with a fallback to a textual placeholder.

## 6.4 UI design principles

- Avoid excessive visual clutter.
- Use consistent spacing and alignment.
- Provide clear focus indicators.
- Support keyboard-only navigation.
- Handle terminal resizing.
- Avoid flickering during updates.
- Avoid excessive terminal redraws.
- Support customizable themes.
- Preserve terminal state when exiting.

# 7. DATABASE AND LIBRARY DESIGN

Use SQLite for persistent library information.

The database should eventually contain:

### Tracks
- ID
- File path or source identifier
- Title
- Artist
- Album
- Album artist
- Genre
- Duration
- Codec
- Sample rate
- Bit depth
- Channel count
- File size
- Track number
- Disc number
- Date added
- Last modified

### Artists
- ID
- Name

### Albums
- ID
- Name
- Artist ID
- Release year
- Artwork reference

### Playlists
- ID
- Name
- Creation timestamp

### Playlist Tracks
- Playlist ID
- Track ID
- Position

### Playback History
- Track ID
- Played timestamp
- Playback duration

### Favorites
- Track ID
- Date added

## Database requirements

- Use migrations.
- Add appropriate indexes.
- Avoid duplicate tracks during scanning.
- Handle renamed and moved files.
- Support library rescanning.
- Avoid storing unnecessary copies of music files.
- Store artwork efficiently.
- Use transactions where appropriate.

The database must never become a requirement for basic single-file playback.

# 8. COMPLETE DEVELOPMENT ROADMAP

The following roadmap contains 57 independently scoped development phases.

Implement them sequentially.

A phase is complete only when its requirements have been implemented and validated.

Do not skip phases or silently merge unrelated phases.

---

# STAGE 1 — PROJECT FOUNDATION AND CORE PLAYBACK

## Phase 01 — Project initialization

Tasks:
- Initialize the Rust project.
- Configure edition and package metadata.
- Set up Cargo dependencies.
- Create the initial README.
- Add .gitignore.
- Verify the development environment.

Deliverable: A clean Rust project that compiles.

Commit:
`chore: initialize termwave project`

## Phase 02 — Modular architecture

Tasks:
- Establish module boundaries.
- Separate CLI, audio, library, metadata and configuration responsibilities.
- Establish error-handling conventions.
- Document the architecture.
- Avoid unnecessary abstraction.

Deliverable: A modular, compiling application skeleton.

Commit:
`refactor: establish modular application architecture`

## Phase 03 — CLI interface

Tasks:
- Integrate Clap.
- Add application version and help.
- Implement the initial play command.
- Define the future command structure.
- Validate command-line arguments.

Deliverable:
`termwave play <file>`

Commit:
`feat: add initial CLI interface`

## Phase 04 — File validation

Tasks:
- Validate file existence.
- Validate file readability.
- Detect unsupported formats.
- Handle incorrect paths.
- Handle permission errors.
- Provide meaningful error messages.

Deliverable: Reliable local file validation.

Commit:
`feat: add audio file validation`

## Phase 05 — FLAC decoding

Tasks:
- Integrate the decoding implementation.
- Decode FLAC files.
- Handle supported sample rates and channel configurations.
- Detect corrupted files.
- Handle unsupported configurations.
- Add decoding tests.

Deliverable: Working FLAC decoding.

Commit:
`feat: add FLAC decoding support`

## Phase 06 — Audio output initialization

Tasks:
- Initialize system audio output.
- Detect default output devices.
- Handle missing devices.
- Handle initialization errors.
- Verify Linux and Windows output.

Deliverable: Audio output initialization.

Commit:
`feat: initialize system audio output`

## Phase 07 — Basic playback

Tasks:
- Connect decoder and audio output.
- Play local FLAC files.
- Display track information.
- Display playback status.
- Wait for playback completion.
- Support clean shutdown.

Deliverable: Fully working local FLAC playback.

Commit:
`feat: implement local FLAC playback`

## Phase 08 — Playback error handling

Tasks:
- Handle decoder errors.
- Handle device errors.
- Handle invalid audio data.
- Handle unexpected file access errors.
- Improve error reporting.
- Test failure scenarios.

Deliverable: Robust basic playback.

Commit:
`fix: improve playback error handling`

**MILESTONE 1: WORKING LOCAL FLAC PLAYER**

---

# STAGE 2 — PLAYBACK ENGINE

## Phase 09 — Play and pause

Tasks:
- Implement playback pause.
- Implement playback resume.
- Track playback state.
- Prevent invalid state transitions.

Commit:
`feat: add playback controls`

## Phase 10 — Stop functionality

Tasks:
- Implement stop.
- Release playback resources.
- Reset playback position appropriately.
- Handle stop during loading.

Commit:
`feat: add playback stop control`

## Phase 11 — Track navigation

Tasks:
- Implement next track.
- Implement previous track.
- Handle playback history.
- Handle track boundaries.

Commit:
`feat: add track navigation`

## Phase 12 — Volume management

Tasks:
- Implement volume adjustment.
- Implement mute.
- Validate volume ranges.
- Prevent clipping where appropriate.

Commit:
`feat: add volume management`

## Phase 13 — Seeking

Tasks:
- Implement forward and backward seeking.
- Support absolute seeking.
- Respect source seeking capabilities.
- Handle seeking during playback and pause.
- Test different FLAC configurations.

Commit:
`feat: add playback seeking`

## Phase 14 — Playback state tracking

Tasks:
- Implement centralized playback state.
- Synchronize state with application components.
- Handle track completion.
- Handle playback errors.
- Prevent race conditions.

Commit:
`feat: add playback state tracking`

## Phase 15 — Gapless playback groundwork

Tasks:
- Prepare the playback queue for consecutive tracks.
- Investigate decoder preloading.
- Reduce transition delays.
- Preserve playback state across track changes.
- Document gapless limitations.

Commit:
`feat: prepare gapless playback support`

**MILESTONE 2: FUNCTIONAL AUDIO ENGINE**

---

# STAGE 3 — METADATA AND MUSIC LIBRARY

## Phase 16 — Metadata extraction

Tasks:
- Integrate Lofty.
- Extract FLAC metadata.
- Support missing metadata.
- Handle malformed tags.
- Extract track and disc information.

Commit:
`feat: extract audio metadata`

## Phase 17 — Embedded artwork

Tasks:
- Extract embedded album artwork.
- Identify artwork formats.
- Handle missing artwork.
- Avoid loading excessively large artwork into memory.
- Prepare terminal-compatible rendering.

Commit:
`feat: support embedded album artwork`

## Phase 18 — Directory scanning

Tasks:
- Recursively scan selected directories.
- Identify supported formats.
- Ignore irrelevant files.
- Handle inaccessible directories.
- Avoid blocking the playback engine.

Commit:
`feat: add recursive music library scanning`

## Phase 19 — SQLite database

Tasks:
- Integrate SQLite.
- Establish database schema.
- Implement migrations.
- Create track, album and artist tables.
- Implement database access layer.

Commit:
`feat: add persistent music library`

## Phase 20 — Library indexing

Tasks:
- Index discovered tracks.
- Detect duplicate entries.
- Detect changed files.
- Support rescanning.
- Update metadata.
- Preserve library consistency.

Commit:
`feat: index scanned music files`

## Phase 21 — Music search

Tasks:
- Search tracks.
- Search artists.
- Search albums.
- Support partial matching.
- Add indexed database queries.

Commit:
`feat: add library search`

## Phase 22 — Favorites

Tasks:
- Add and remove favorites.
- Persist favorite status.
- Query favorites.
- Handle missing files.

Commit:
`feat: add favorite tracks`

## Phase 23 — Playback history

Tasks:
- Record played tracks.
- Track playback timestamps.
- Track playback duration.
- Implement recently played queries.
- Avoid excessive database writes.

Commit:
`feat: track playback history`

**MILESTONE 3: PERSISTENT MUSIC LIBRARY**

---

# STAGE 4 — TERMINAL USER INTERFACE

## Phase 24 — TUI integration

Tasks:
- Integrate Ratatui.
- Integrate Crossterm.
- Initialize terminal.
- Implement terminal restoration.
- Establish rendering loop.

Commit:
`feat: initialize terminal UI`

## Phase 25 — Main layout

Tasks:
- Design the application dashboard.
- Add header.
- Add navigation sidebar.
- Add main content area.
- Add bottom playback panel.
- Support terminal resizing.

Commit:
`feat: add terminal dashboard layout`

## Phase 26 — Library browser

Tasks:
- Display tracks.
- Display albums.
- Display artists.
- Implement selection.
- Implement keyboard navigation.
- Connect library queries.

Commit:
`feat: add interactive music library`

## Phase 27 — Now-playing interface

Tasks:
- Display current track.
- Display artist and album.
- Display audio information.
- Display playback state.
- Handle missing metadata.

Commit:
`feat: add now playing interface`

## Phase 28 — Keyboard controls

Tasks:
- Implement global keyboard shortcuts.
- Implement contextual controls.
- Implement focus management.
- Prevent conflicting shortcuts.
- Document shortcuts.

Commit:
`feat: implement keyboard controls`

## Phase 29 — Playback progress

Tasks:
- Add progress bar.
- Display elapsed time.
- Display total duration.
- Support interactive seeking.
- Update without excessive redraws.

Commit:
`feat: add playback progress visualization`

## Phase 30 — Playback queue

Tasks:
- Display upcoming tracks.
- Add and remove queue items.
- Reorder tracks.
- Clear queue.
- Implement queue navigation.

Commit:
`feat: add interactive playback queue`

## Phase 31 — Search interface

Tasks:
- Implement interactive search.
- Display matching results.
- Navigate search results.
- Support keyboard-driven selection.

Commit:
`feat: add interactive library search`

## Phase 32 — Themes and settings

Tasks:
- Add theme configuration.
- Support color customization.
- Configure keyboard shortcuts.
- Persist preferences.
- Provide sensible defaults.

Commit:
`feat: add customizable themes and settings`

## Phase 33 — Terminal artwork

Tasks:
- Investigate terminal image protocols.
- Support compatible terminals.
- Render album artwork.
- Implement fallback placeholders.
- Handle terminal resizing and cleanup.

Commit:
`feat: render album artwork in terminal`

**MILESTONE 4: FULL INTERACTIVE TERMINAL MUSIC PLAYER**

---

# STAGE 5 — ADVANCED AUDIO FEATURES

## Phase 34 — Codec and bit-depth information

Tasks:
- Display codec.
- Display source bit depth when available.
- Distinguish source format from decoded representation.
- Display channel configuration.

Commit:
`feat: display audio format information`

## Phase 35 — Audio statistics

Tasks:
- Display source sample rate.
- Display bitrate when meaningful.
- Display file size.
- Display duration.
- Display output sample rate separately where available.

Commit:
`feat: add detailed audio statistics`

## Phase 36 — ReplayGain

Tasks:
- Read ReplayGain metadata.
- Implement track gain.
- Implement album gain.
- Implement preamp.
- Prevent clipping.
- Allow disabling ReplayGain.

Commit:
`feat: add ReplayGain support`

## Phase 37 — Equalizer

Tasks:
- Design configurable equalizer.
- Implement frequency bands.
- Add gain controls.
- Add presets.
- Preserve neutral output by default.
- Test processing stability.

Commit:
`feat: add configurable equalizer`

## Phase 38 — Audio visualizer

Tasks:
- Implement spectrum analysis.
- Add FFT-based visualization.
- Display frequency bands.
- Keep visualization separate from the audio callback.
- Minimize CPU consumption.
- Add visualization configuration.

Commit:
`feat: add real-time spectrum visualizer`

## Phase 39 — Audio device selection

Tasks:
- Enumerate audio output devices.
- Allow device selection.
- Detect device disconnection.
- Handle missing devices.
- Persist preferences.

Commit:
`feat: support output device selection`

## Phase 40 — Exclusive output investigation

Tasks:
- Investigate WASAPI exclusive output.
- Investigate ALSA/PipeWire output capabilities.
- Support platform-specific configuration.
- Preserve normal shared output as a fallback.
- Report actual output capabilities.
- Test resampling behavior.

Commit:
`feat: add platform-specific exclusive output support`

## Phase 41 — Gapless playback

Tasks:
- Implement decoder preloading.
- Improve consecutive track transitions.
- Minimize transition gaps.
- Handle differing sample rates and channel layouts.
- Test with consecutive FLAC tracks.

Commit:
`feat: implement gapless track transitions`

**MILESTONE 5: ADVANCED HI-FI AUDIO PLAYER**

---

# STAGE 6 — STREAMING INFRASTRUCTURE

Streaming must be an extension of the existing player, not a separate application.

Local playback must continue working independently.

## Phase 42 — Generalize audio source architecture

Tasks:
- Refactor source handling.
- Remove unnecessary filesystem assumptions.
- Define source capabilities.
- Define seekability behavior.
- Define source lifecycle.
- Preserve local playback.

Commit:
`refactor: decouple playback from local files`

## Phase 43 — Streaming buffer architecture

Tasks:
- Implement buffered input.
- Handle partial reads.
- Implement bounded buffering.
- Separate networking from audio callbacks.
- Handle buffer underruns.
- Support configurable buffer sizes.

Commit:
`feat: add streaming buffer architecture`

## Phase 44 — HTTP source

Tasks:
- Integrate Reqwest.
- Support HTTP and HTTPS.
- Retrieve remote audio data.
- Handle response errors.
- Handle connection timeouts.
- Handle interrupted transfers.

Commit:
`feat: add HTTP audio sources`

## Phase 45 — Remote FLAC decoding

Tasks:
- Connect HTTP source to decoder.
- Support progressive FLAC decoding.
- Start playback before the complete file downloads.
- Handle partial network data.
- Validate metadata availability.
- Test with large FLAC files.

Commit:
`feat: support remote FLAC playback`

## Phase 46 — HTTP seeking

Tasks:
- Investigate HTTP Range requests.
- Detect server range support.
- Implement range-based seeking where possible.
- Handle servers without range support.
- Provide clear seeking capability indicators.

Commit:
`feat: support remote audio seeking`

## Phase 47 — Network recovery

Tasks:
- Detect network disconnections.
- Implement bounded retries.
- Resume where supported.
- Handle buffering states.
- Prevent indefinite blocking.
- Provide useful error messages.

Commit:
`feat: add streaming recovery`

## Phase 48 — NAS and network libraries

Tasks:
- Support network-hosted libraries.
- Investigate SMB and WebDAV integration.
- Support network directory access where practical.
- Handle unavailable network locations.
- Support remote metadata indexing.
- Preserve local playback when network resources fail.

Commit:
`feat: support network music libraries`

**MILESTONE 6: LOCAL AND NETWORK MUSIC PLAYBACK**

---

# STAGE 7 — RELIABILITY, PERFORMANCE AND RELEASE

## Phase 49 — Unit testing

Tasks:
- Test source handling.
- Test metadata extraction.
- Test playback state transitions.
- Test file validation.
- Test queue behavior.
- Test error conditions.

Commit:
`test: expand audio engine test coverage`

## Phase 50 — Integration testing

Tasks:
- Test complete playback pipelines.
- Test supported FLAC configurations.
- Test invalid files.
- Test local file navigation.
- Test streaming where implemented.
- Test shutdown behavior.

Commit:
`test: add end-to-end playback tests`

## Phase 51 — Performance optimization

Tasks:
- Profile CPU consumption.
- Profile memory usage.
- Identify unnecessary allocations.
- Optimize library scanning.
- Optimize metadata processing.
- Reduce UI rendering overhead.
- Verify playback stability under load.

Commit:
`perf: optimize audio processing`

## Phase 52 — Cross-platform compatibility

Tasks:
- Validate Linux builds.
- Validate Windows builds.
- Fix platform-specific issues.
- Test audio backend selection.
- Document platform dependencies.
- Test terminal compatibility.

Commit:
`fix: improve Windows and Linux compatibility`

## Phase 53 — Structured logging

Tasks:
- Integrate tracing.
- Add configurable log levels.
- Log audio initialization.
- Log decoding failures.
- Log network failures.
- Avoid logging sensitive information.
- Support diagnostic reports.

Commit:
`feat: add application logging`

## Phase 54 — Configuration persistence

Tasks:
- Implement configuration loading.
- Implement configuration saving.
- Store user preferences.
- Handle invalid configurations.
- Provide default configuration.
- Support configuration migration.

Commit:
`feat: persist user preferences`

## Phase 55 — Documentation

Tasks:
- Complete README.
- Document installation.
- Document dependencies.
- Document keyboard shortcuts.
- Document audio architecture.
- Document supported formats.
- Document streaming capabilities.
- Document troubleshooting.
- Document contribution guidelines.

Commit:
`docs: complete project documentation`

## Phase 56 — Continuous integration

Tasks:
- Configure GitHub Actions.
- Run cargo fmt.
- Run cargo clippy.
- Run cargo test.
- Build Linux targets.
- Build Windows targets.
- Cache dependencies.
- Validate pull requests.

Commit:
`ci: add automated build and test workflows`

## Phase 57 — Release preparation

Tasks:
- Finalize version number.
- Verify release build.
- Check dependency licenses.
- Review security.
- Verify documentation.
- Prepare release notes.
- Prepare distribution binaries.
- Tag the first stable release.

Commit:
`chore: prepare v1.0.0 release`

**MILESTONE 7: TERMWAVE V1.0**

---

# 9. GIT AND VERSION CONTROL POLICY

Git history must reflect actual development progress.

Each phase should produce one meaningful commit when its scope is complete.

## Conventional commit format

Use:

- feat: New functionality.
- fix: Bug fixes.
- refactor: Architecture improvements.
- perf: Performance improvements.
- test: Tests.
- docs: Documentation.
- chore: Project maintenance.
- ci: Continuous integration.

## Git workflow

For each phase:

1. Inspect current branch and repository status.
2. Inspect existing implementation.
3. Implement the assigned phase.
4. Add or update tests.
5. Run formatting.
6. Run Clippy.
7. Run tests.
8. Inspect the complete diff.
9. Check for secrets and unrelated changes.
10. Commit the completed phase.
11. Report the commit hash.

Suggested validation commands:

`cargo fmt --check`

`cargo clippy --all-targets -- -D warnings`

`cargo test`

`cargo build --release`

Never:
- Force-push.
- Delete existing commits.
- Rewrite Git history.
- Commit generated build artifacts.
- Commit personal music files.
- Commit credentials or API keys.
- Create meaningless commits.
- Claim a commit exists without verifying it.

If a phase is incomplete, do not create a completion commit.

Do not automatically push to GitHub unless explicitly authorized.

# 10. TESTING AND QUALITY STANDARDS

Every new feature must have appropriate tests.

## FLAC test coverage

Test:

- 16-bit FLAC.
- 24-bit FLAC.
- 44.1 kHz.
- 48 kHz.
- 96 kHz.
- Stereo audio.
- Mono audio.
- Large files.
- Corrupted files.
- Missing files.
- Files with missing metadata.
- Files with embedded artwork.

Use legally distributable or self-generated test fixtures.

Do not commit large commercial music files.

## Playback testing

Verify:
- Correct playback initialization.
- Correct channel count.
- Playback completion.
- Pause and resume.
- Seeking.
- Volume changes.
- Device disconnection handling.
- Graceful shutdown.
- No unexpected application hangs.

Compilation success alone is not proof of working playback.

Actual audio playback must be tested on real hardware where applicable.

# 11. PERFORMANCE REQUIREMENTS

Termwave should remain lightweight.

Prioritize:

- Low idle CPU usage.
- Low memory consumption.
- Efficient decoding.
- Efficient database queries.
- Responsive terminal rendering.
- Non-blocking background scanning.
- Stable playback under heavy UI activity.

Do not introduce unnecessary background services.

Do not load an entire music collection into memory.

Avoid unnecessary audio conversions.

Performance changes must be supported by measurements, not assumptions.

# 12. SECURITY REQUIREMENTS

- Validate user-provided file paths.
- Treat remote URLs as untrusted input.
- Validate network responses.
- Use HTTPS where supported.
- Avoid exposing credentials in logs.
- Avoid executing downloaded content.
- Handle malformed audio safely.
- Limit untrusted metadata and artwork sizes.
- Apply sensible network timeouts.
- Avoid unsafe Rust unless necessary and documented.

Network streaming must never compromise the local playback functionality.

# 13. CROSS-PLATFORM REQUIREMENTS

Primary supported platforms:

### Linux
- Arch Linux.
- CachyOS.
- PipeWire.
- ALSA compatibility where supported.
- Common terminal emulators.

### Windows
- Windows 11.
- WASAPI.
- Windows Terminal.
- Other compatible terminal emulators.

Keep platform-specific code isolated.

Do not assume that a Linux audio backend or dependency exists on Windows.

Document platform-specific behavior and installation requirements.

# 14. AUTONOMOUS DEVELOPMENT RULES FOR ANTIGRAVITY

You are authorized to inspect the repository, edit source files, implement features, run tests, and create Git commits within the assigned phase.

However, follow these boundaries.

## Before starting any phase

1. Read this AGENTS.md completely.
2. Inspect the repository.
3. Inspect Git history and working-tree status.
4. Identify the current completed phase.
5. Review existing architecture.
6. Identify dependencies and potential conflicts.
7. State the intended implementation scope.

Never assume the repository is empty.

Never overwrite existing work without inspecting it.

## During implementation

- Implement only the assigned phase.
- Do not jump to later phases.
- Do not remove working functionality without justification.
- Keep changes scoped.
- Add tests.
- Update documentation where needed.
- Preserve platform compatibility.
- Explain significant architecture decisions.

If a future-phase requirement must be anticipated, prepare an appropriate interface without implementing the future feature.

## After implementation

Provide a completion report containing:

1. Phase number and name.
2. Implementation summary.
3. Files created.
4. Files modified.
5. Dependencies added or changed.
6. Tests executed.
7. Actual test results.
8. Build results.
9. Known limitations.
10. Git commit message.
11. Git commit hash, if committed.
12. Recommended next phase.

Do not fabricate test results or claim functionality was verified when it was only compiled.

## Phase completion

A phase is complete only when:
- Its requirements are satisfied.
- Relevant tests pass.
- Formatting passes.
- Clippy passes, except for documented and justified pre-existing issues.
- No known regression has been introduced.
- The implementation is reviewed.
- The Git commit is verified.

After completing a phase, stop and wait for the next phase instruction.

Do not automatically begin the next phase.

# 15. IMPORTANT ARCHITECTURAL DECISIONS

These decisions must guide the entire project.

1. Local playback is the foundation.
2. FLAC is the first-class format.
3. Local playback must work offline.
4. Audio source handling must be independent of UI.
5. Decoder implementation must not assume all sources are local files.
6. Seeking must account for source capabilities.
7. Network operations must never block audio callbacks.
8. SQLite is for library persistence, not mandatory single-file playback.
9. The TUI must not own playback logic.
10. Audio output must be isolated behind a backend abstraction.
11. Audio information must distinguish source properties from actual output properties.
12. Bit-perfect playback must never be falsely advertised.
13. Linux and Windows compatibility must be considered throughout development.
14. Performance and reliability take priority over unnecessary features.

# 16. CURRENT DEVELOPMENT POSITION

**Initial status: Phase 01 — Not started.**

The immediate objective is to complete Phase 01 through Phase 08.

Do not implement the complete application in one operation.

The first deliverable is a reliable command-line FLAC player that can open a local file, decode it, play it through the system audio device, display basic audio information, and exit cleanly.

After that, progressively build the playback engine, library, interface, advanced audio features and streaming capabilities.

# 17. FIRST ASSIGNMENT

Begin with Phase 01 only.

Inspect the existing repository first.

Initialize the project if necessary, establish the initial Cargo configuration, create the basic documentation and Git ignore rules, and verify that the project builds.

Do not implement playback yet.

Report the actual results and prepare the appropriate Git commit.

Wait for authorization before starting Phase 02.

---

**END OF MASTER INSTRUCTIONS**

Project codename: Termwave  
Primary objective: A lightweight, cross-platform, lossless-first terminal music player.