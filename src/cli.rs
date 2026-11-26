//! Command-line interface boundary.
//!
//! Coordinates user-facing CLI parsing via `clap` and delegates execution
//! without owning audio decoding, playback, or library internals.

use std::io::{self, BufRead, IsTerminal};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, OnceLock};
use std::thread;
use std::time::Duration;

use clap::{Args, CommandFactory, Parser, Subcommand};

use crate::audio::{PlaybackController, PlaybackEvent, PlaybackState, PlaybackTrackInfo};

/// Top-level command-line parser for Termwave.
#[derive(Debug, Parser)]
#[command(
    name = "termwave",
    version,
    about = "A lightweight, cross-platform, lossless-first terminal Hi-Fi music player",
    long_about = None
)]
pub struct Cli {
    /// Subcommand to execute.
    #[command(subcommand)]
    pub command: Option<Commands>,
}

/// Top-level Termwave subcommands.
#[derive(Debug, Subcommand, PartialEq, Eq)]
pub enum Commands {
    /// Play a local audio file.
    Play(PlayArgs),

    /// (planned) Inspect audio file metadata and stream properties.
    Info(InfoArgs),

    /// (planned) Scan a directory into the local music library.
    Scan(ScanArgs),

    /// (planned) Launch the interactive terminal user interface.
    Tui,

    /// (planned) List available audio output devices.
    Devices,
}

/// Arguments for the `play` subcommand.
#[derive(Debug, Args, PartialEq, Eq)]
pub struct PlayArgs {
    /// Path to the local audio file to play.
    #[arg(value_name = "FILE", value_parser = parse_non_empty_path)]
    pub file: PathBuf,
}

/// Arguments for the planned `info` subcommand.
#[derive(Debug, Args, PartialEq, Eq)]
pub struct InfoArgs {
    /// Path to the audio file to inspect.
    #[arg(value_name = "FILE", value_parser = parse_non_empty_path)]
    pub file: PathBuf,
}

/// Arguments for the planned `scan` subcommand.
#[derive(Debug, Args, PartialEq, Eq)]
pub struct ScanArgs {
    /// Directory path to scan for music files.
    #[arg(value_name = "DIR", value_parser = parse_non_empty_path)]
    pub path: PathBuf,
}

/// Validates that a CLI path argument is not empty or whitespace-only.
fn parse_non_empty_path(raw: &str) -> Result<PathBuf, String> {
    if raw.trim().is_empty() {
        return Err("path argument cannot be empty or whitespace-only".to_string());
    }
    Ok(PathBuf::from(raw))
}

/// Returns the application name and package version.
pub fn app_banner() -> String {
    format!("{} v{}", env!("CARGO_PKG_NAME"), env!("CARGO_PKG_VERSION"))
}

/// Installs a process-wide Ctrl+C handler once and returns the shared shutdown flag.
fn install_shutdown_handler() -> Arc<AtomicBool> {
    static SHUTDOWN_FLAG: OnceLock<Arc<AtomicBool>> = OnceLock::new();
    let flag = SHUTDOWN_FLAG.get_or_init(|| {
        let shutdown = Arc::new(AtomicBool::new(false));
        let signal_flag = Arc::clone(&shutdown);
        let _ = ctrlc::set_handler(move || {
            signal_flag.store(true, Ordering::SeqCst);
        });
        shutdown
    });
    Arc::clone(flag)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PlaybackControlAction {
    TogglePause,
    Pause,
    Resume,
    Stop,
}

fn parse_playback_control_command(line: &str) -> Option<PlaybackControlAction> {
    match line.trim().to_ascii_lowercase().as_str() {
        "" | "p" | "toggle" => Some(PlaybackControlAction::TogglePause),
        "pause" => Some(PlaybackControlAction::Pause),
        "resume" | "play" => Some(PlaybackControlAction::Resume),
        "s" | "stop" | "q" | "quit" => Some(PlaybackControlAction::Stop),
        _ => None,
    }
}

fn apply_playback_control_action(
    controller: &PlaybackController,
    action: PlaybackControlAction,
) -> Result<PlaybackState, crate::audio::AudioError> {
    match action {
        PlaybackControlAction::TogglePause => controller.toggle_pause(),
        PlaybackControlAction::Pause => {
            controller.pause()?;
            Ok(PlaybackState::Paused)
        }
        PlaybackControlAction::Resume => {
            controller.resume()?;
            Ok(PlaybackState::Playing)
        }
        PlaybackControlAction::Stop => {
            controller.stop()?;
            Ok(PlaybackState::Stopped)
        }
    }
}

fn spawn_stdin_control_thread(controller: PlaybackController, active: Arc<AtomicBool>) {
    thread::spawn(move || {
        let stdin = io::stdin();
        let mut reader = stdin.lock();
        let mut line = String::new();
        while active.load(Ordering::Acquire) {
            line.clear();
            match reader.read_line(&mut line) {
                Ok(0) => break,
                Ok(_) => {
                    if !active.load(Ordering::Acquire) {
                        break;
                    }
                    if let Some(action) = parse_playback_control_command(&line) {
                        match apply_playback_control_action(&controller, action) {
                            Ok(PlaybackState::Stopped) => break,
                            Ok(_) => {}
                            Err(err) => eprintln!("Note:    {err}"),
                        }
                    } else {
                        eprintln!(
                            "Note:    unknown control '{}' (use Enter/'p' to toggle, 'pause', 'resume', 's'/'stop', or Ctrl+C to quit)",
                            line.trim()
                        );
                    }
                }
                Err(_) => break,
            }
        }
    });
}

/// Parses arguments from the process environment and executes the selected command.
pub fn run() -> anyhow::Result<()> {
    let cli = Cli::parse();
    let shutdown = install_shutdown_handler();
    match &cli.command {
        Some(Commands::Play(args)) if io::stdin().is_terminal() => {
            let controller = PlaybackController::new();
            let active = Arc::new(AtomicBool::new(true));
            spawn_stdin_control_thread(controller.clone(), Arc::clone(&active));
            let result = execute_play(&args.file, controller, shutdown, true);
            active.store(false, Ordering::Release);
            result
        }
        _ => execute_with_shutdown(cli, shutdown),
    }
}

/// Executes a parsed [`Cli`] command.
pub fn execute(cli: Cli) -> anyhow::Result<()> {
    execute_with_shutdown(cli, Arc::new(AtomicBool::new(false)))
}

/// Executes a parsed [`Cli`] command with an explicit shutdown signal flag.
pub fn execute_with_shutdown(cli: Cli, shutdown: Arc<AtomicBool>) -> anyhow::Result<()> {
    match cli.command {
        Some(Commands::Play(args)) => {
            execute_play(&args.file, PlaybackController::new(), shutdown, false)
        }
        Some(Commands::Info(_)) => {
            anyhow::bail!("the 'info' subcommand is planned for a future phase")
        }
        Some(Commands::Scan(_)) => {
            anyhow::bail!("the 'scan' subcommand is planned for a future phase")
        }
        Some(Commands::Tui) => {
            anyhow::bail!("the 'tui' subcommand is planned for a future phase")
        }
        Some(Commands::Devices) => {
            anyhow::bail!("the 'devices' subcommand is planned for a future phase")
        }
        None => {
            println!("{} — terminal Hi-Fi music player\n", app_banner());
            Cli::command().print_help()?;
            println!();
            Ok(())
        }
    }
}

fn execute_play(
    file: &Path,
    controller: PlaybackController,
    shutdown: Arc<AtomicBool>,
    show_controls_hint: bool,
) -> anyhow::Result<()> {
    println!("{}", app_banner());
    let summary = crate::audio::play_local_flac_with_controller(
        file,
        controller,
        shutdown,
        |event| match event {
            PlaybackEvent::StateChanged(state) => {
                println!("Status:  {state}");
            }
            PlaybackEvent::TrackLoaded(info) => {
                print_track_info(&info);
                if show_controls_hint {
                    println!(
                        "Controls: press Enter/'p' to pause/resume, 's' to stop, or Ctrl+C to quit"
                    );
                }
            }
        },
    )?;

    if summary.final_state == PlaybackState::Stopped {
        println!(
            "Playback stopped after {}",
            format_duration(summary.elapsed)
        );
    }
    Ok(())
}

fn print_track_info(info: &PlaybackTrackInfo) {
    let display_name = file_display_name(&info.file.path);
    let bit_depth = info
        .stream
        .bits_per_sample
        .map_or_else(|| "unknown".to_string(), |bits| format!("{bits}-bit"));
    let duration = info
        .stream
        .duration
        .map_or_else(|| "unknown".to_string(), format_duration);

    println!("Track:   {display_name} ({} bytes)", info.file.size_bytes);
    println!(
        "Source:  {} | {} Hz | {} | {} | {}",
        info.stream.codec,
        info.stream.sample_rate,
        bit_depth,
        format_channels(info.stream.channels),
        duration
    );
    println!(
        "Output:  {} | {} Hz | {} ch | {}",
        info.output.name, info.output.sample_rate, info.output.channels, info.output.sample_format
    );
}

fn file_display_name(path: &Path) -> String {
    path.file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.display().to_string())
}

fn format_channels(channels: u16) -> String {
    match channels {
        1 => "1 ch (Mono)".to_string(),
        2 => "2 ch (Stereo)".to_string(),
        count => format!("{count} ch"),
    }
}

fn format_duration(duration: Duration) -> String {
    let total_millis = duration.as_millis();
    let minutes = total_millis / 60_000;
    let seconds = (total_millis % 60_000) / 1_000;
    let hundredths = (total_millis % 1_000) / 10;
    format!("{minutes:02}:{seconds:02}.{hundredths:02}")
}

#[cfg(test)]
mod tests {
    use clap::error::ErrorKind;

    use super::*;

    #[test]
    fn clap_configuration_is_valid() {
        Cli::command().debug_assert();
    }

    #[test]
    fn app_banner_includes_name_and_version() {
        let banner = app_banner();
        assert!(banner.starts_with("termwave v"));
        assert!(banner.contains(env!("CARGO_PKG_VERSION")));
    }

    #[test]
    fn version_flag_reports_package_version() {
        let err = Cli::try_parse_from(["termwave", "--version"]).unwrap_err();
        assert_eq!(err.kind(), ErrorKind::DisplayVersion);
        let rendered = err.to_string();
        assert!(rendered.contains("termwave"));
        assert!(rendered.contains(env!("CARGO_PKG_VERSION")));
    }

    #[test]
    fn help_flag_lists_play_and_planned_commands() {
        let err = Cli::try_parse_from(["termwave", "--help"]).unwrap_err();
        assert_eq!(err.kind(), ErrorKind::DisplayHelp);
        let rendered = err.to_string();
        assert!(rendered.contains("play"));
        assert!(rendered.contains("info"));
        assert!(rendered.contains("scan"));
        assert!(rendered.contains("tui"));
        assert!(rendered.contains("devices"));
    }

    #[test]
    fn parses_play_command_with_file_argument() {
        let cli = Cli::try_parse_from(["termwave", "play", "music/sample.flac"]).unwrap();
        assert_eq!(
            cli.command,
            Some(Commands::Play(PlayArgs {
                file: PathBuf::from("music/sample.flac"),
            }))
        );
    }

    #[test]
    fn execute_play_plays_valid_flac_and_rejects_missing_file() {
        let missing = Cli::try_parse_from(["termwave", "play", "does_not_exist_12345.flac"])
            .expect("CLI syntax is valid");
        assert!(execute(missing).is_err());

        let fixture = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests")
            .join("fixtures")
            .join("sine_16bit_44100_stereo.flac");
        let valid = Cli::try_parse_from(["termwave", "play", &fixture.to_string_lossy()])
            .expect("CLI syntax is valid");
        match execute(valid) {
            Ok(()) => {}
            Err(err) => {
                // Allow headless CI without audio hardware, fail on any other error.
                let is_no_device = err
                    .downcast_ref::<crate::audio::AudioError>()
                    .is_some_and(|e| matches!(e, crate::audio::AudioError::NoOutputDevice));
                assert!(is_no_device, "unexpected CLI play error: {err}");
            }
        }
    }

    #[test]
    fn formats_duration_and_channels_cleanly() {
        assert_eq!(format_duration(Duration::from_millis(100)), "00:00.10");
        assert_eq!(format_duration(Duration::from_secs(125)), "02:05.00");
        assert_eq!(format_channels(1), "1 ch (Mono)");
        assert_eq!(format_channels(2), "2 ch (Stereo)");
        assert_eq!(format_channels(6), "6 ch");
    }

    #[test]
    fn rejects_play_command_without_file_argument() {
        let err = Cli::try_parse_from(["termwave", "play"]).unwrap_err();
        assert_eq!(err.kind(), ErrorKind::MissingRequiredArgument);
    }

    #[test]
    fn rejects_empty_or_whitespace_file_argument() {
        for invalid in ["", "   ", "\t"] {
            let err = Cli::try_parse_from(["termwave", "play", invalid]).unwrap_err();
            assert_eq!(err.kind(), ErrorKind::ValueValidation);
        }
    }

    #[test]
    fn rejects_unknown_flags() {
        let err = Cli::try_parse_from(["termwave", "--not-a-flag"]).unwrap_err();
        assert_eq!(err.kind(), ErrorKind::UnknownArgument);
    }

    #[test]
    fn parses_planned_commands_and_reports_unimplemented() {
        let scan = Cli::try_parse_from(["termwave", "scan", "/music"]).unwrap();
        assert_eq!(
            scan.command,
            Some(Commands::Scan(ScanArgs {
                path: PathBuf::from("/music"),
            }))
        );
        assert!(execute(scan).is_err());

        let tui = Cli::try_parse_from(["termwave", "tui"]).unwrap();
        assert_eq!(tui.command, Some(Commands::Tui));
        assert!(execute(tui).is_err());

        let info = Cli::try_parse_from(["termwave", "info", "track.flac"]).unwrap();
        assert!(execute(info).is_err());

        let devices = Cli::try_parse_from(["termwave", "devices"]).unwrap();
        assert!(execute(devices).is_err());
    }

    #[test]
    fn execute_with_no_subcommand_succeeds() {
        let cli = Cli::try_parse_from(["termwave"]).unwrap();
        assert_eq!(cli.command, None);
        assert!(execute(cli).is_ok());
    }

    #[test]
    fn parses_interactive_playback_control_commands() {
        assert_eq!(
            parse_playback_control_command("\n"),
            Some(PlaybackControlAction::TogglePause)
        );
        assert_eq!(
            parse_playback_control_command("  p "),
            Some(PlaybackControlAction::TogglePause)
        );
        assert_eq!(
            parse_playback_control_command("TOGGLE"),
            Some(PlaybackControlAction::TogglePause)
        );
        assert_eq!(
            parse_playback_control_command("pause"),
            Some(PlaybackControlAction::Pause)
        );
        assert_eq!(
            parse_playback_control_command("RESUME"),
            Some(PlaybackControlAction::Resume)
        );
        assert_eq!(
            parse_playback_control_command("play"),
            Some(PlaybackControlAction::Resume)
        );
        assert_eq!(
            parse_playback_control_command("s"),
            Some(PlaybackControlAction::Stop)
        );
        assert_eq!(
            parse_playback_control_command("STOP"),
            Some(PlaybackControlAction::Stop)
        );
        assert_eq!(
            parse_playback_control_command("quit"),
            Some(PlaybackControlAction::Stop)
        );
        assert_eq!(parse_playback_control_command("seek"), None);
    }

    #[test]
    fn applies_playback_control_actions_with_state_validation() {
        let controller = PlaybackController::new();
        assert!(matches!(
            apply_playback_control_action(&controller, PlaybackControlAction::TogglePause),
            Err(crate::audio::AudioError::InvalidState(_))
        ));
        assert!(matches!(
            apply_playback_control_action(&controller, PlaybackControlAction::Stop),
            Err(crate::audio::AudioError::InvalidState(_))
        ));
    }
}
