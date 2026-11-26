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

use crate::audio::{
    DEFAULT_SEEK_STEP, PlaybackController, PlaybackEvent, PlaybackState, PlaybackTrackInfo,
};

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
    /// Path(s) to one or more local audio files to play sequentially.
    #[arg(value_name = "FILE", required = true, num_args = 1.., value_parser = parse_non_empty_path)]
    pub files: Vec<PathBuf>,

    /// Initial playback volume percentage (0..=100).
    #[arg(long, default_value_t = 100, value_parser = clap::value_parser!(u8).range(0..=100))]
    pub volume: u8,

    /// Start playback muted (preserves configured volume level).
    #[arg(long, default_value_t = false)]
    pub mute: bool,
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

#[derive(Debug, Clone, PartialEq, Eq)]
enum PlaybackControlAction {
    TogglePause,
    Pause,
    Resume,
    NextTrack,
    PreviousTrack,
    SeekForward,
    SeekBackward,
    SeekAbsolute(String),
    VolumeUp,
    VolumeDown,
    SetVolume(String),
    ToggleMute,
    Unmute,
    Stop,
}

fn parse_playback_control_command(line: &str) -> Option<PlaybackControlAction> {
    let trimmed = line.trim();
    let lower = trimmed.to_ascii_lowercase();
    if let Some(arg) = lower
        .strip_prefix("vol ")
        .or_else(|| lower.strip_prefix("volume "))
    {
        return Some(PlaybackControlAction::SetVolume(arg.trim().to_string()));
    }
    if let Some(arg) = lower.strip_prefix("seek ") {
        return Some(PlaybackControlAction::SeekAbsolute(arg.trim().to_string()));
    }

    match lower.as_str() {
        "" | "space" | "pp" | "toggle" => Some(PlaybackControlAction::TogglePause),
        "pause" => Some(PlaybackControlAction::Pause),
        "resume" | "play" => Some(PlaybackControlAction::Resume),
        "n" | "next" => Some(PlaybackControlAction::NextTrack),
        "p" | "prev" | "previous" => Some(PlaybackControlAction::PreviousTrack),
        "f" | "ff" | "fwd" | "right" | ">" => Some(PlaybackControlAction::SeekForward),
        "b" | "bb" | "rew" | "back" | "left" | "<" => Some(PlaybackControlAction::SeekBackward),
        "+" | "up" | "vol+" => Some(PlaybackControlAction::VolumeUp),
        "-" | "down" | "vol-" => Some(PlaybackControlAction::VolumeDown),
        "m" | "mute" => Some(PlaybackControlAction::ToggleMute),
        "unmute" => Some(PlaybackControlAction::Unmute),
        "s" | "stop" | "q" | "quit" => Some(PlaybackControlAction::Stop),
        _ => None,
    }
}

fn parse_seek_position(raw: &str) -> Result<Duration, crate::audio::AudioError> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err(crate::audio::AudioError::SeekError(
            "seek target cannot be empty".to_string(),
        ));
    }

    if let Some((mins_str, secs_str)) = trimmed.split_once(':') {
        let mins: u64 = mins_str.parse().map_err(|_| {
            crate::audio::AudioError::SeekError(format!("invalid seek timestamp '{trimmed}'"))
        })?;
        let secs: f64 = secs_str.parse().map_err(|_| {
            crate::audio::AudioError::SeekError(format!("invalid seek timestamp '{trimmed}'"))
        })?;
        if !secs.is_finite() || !(0.0..60.0).contains(&secs) {
            return Err(crate::audio::AudioError::SeekError(format!(
                "seconds component in '{trimmed}' must be in 0..60"
            )));
        }
        return Ok(Duration::from_secs_f64((mins as f64) * 60.0 + secs));
    }

    let secs: f64 = trimmed.parse().map_err(|_| {
        crate::audio::AudioError::SeekError(format!(
            "expected seconds or mm:ss timestamp, got '{trimmed}'"
        ))
    })?;
    if !secs.is_finite() || secs < 0.0 {
        return Err(crate::audio::AudioError::SeekError(format!(
            "seek position must be a non-negative finite number, got '{trimmed}'"
        )));
    }
    Ok(Duration::from_secs_f64(secs))
}

fn apply_playback_control_action(
    controller: &PlaybackController,
    action: &PlaybackControlAction,
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
        PlaybackControlAction::NextTrack => {
            controller.next_track()?;
            Ok(controller.state())
        }
        PlaybackControlAction::PreviousTrack => {
            controller.previous_track()?;
            Ok(controller.state())
        }
        PlaybackControlAction::SeekForward => {
            controller.seek_forward(DEFAULT_SEEK_STEP)?;
            Ok(controller.state())
        }
        PlaybackControlAction::SeekBackward => {
            controller.seek_backward(DEFAULT_SEEK_STEP)?;
            Ok(controller.state())
        }
        PlaybackControlAction::SeekAbsolute(raw) => {
            let pos = parse_seek_position(raw)?;
            controller.seek(pos)?;
            Ok(controller.state())
        }
        PlaybackControlAction::VolumeUp => {
            controller.increase_volume();
            Ok(controller.state())
        }
        PlaybackControlAction::VolumeDown => {
            controller.decrease_volume();
            Ok(controller.state())
        }
        PlaybackControlAction::SetVolume(raw) => {
            let percent: i32 = raw.parse().map_err(|_| {
                crate::audio::AudioError::InvalidVolume(format!(
                    "expected integer percentage 0..=100, got '{raw}'"
                ))
            })?;
            let percent = u8::try_from(percent).map_err(|_| {
                crate::audio::AudioError::InvalidVolume(format!(
                    "volume percentage {percent}% is out of range (0..=100)"
                ))
            })?;
            controller.set_volume_percent(percent)?;
            Ok(controller.state())
        }
        PlaybackControlAction::ToggleMute => {
            controller.toggle_mute();
            Ok(controller.state())
        }
        PlaybackControlAction::Unmute => {
            controller.unmute();
            Ok(controller.state())
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
                        let is_volume_action = matches!(
                            action,
                            PlaybackControlAction::VolumeUp
                                | PlaybackControlAction::VolumeDown
                                | PlaybackControlAction::SetVolume(_)
                                | PlaybackControlAction::ToggleMute
                                | PlaybackControlAction::Unmute
                        );
                        match apply_playback_control_action(&controller, &action) {
                            Ok(PlaybackState::Stopped) => break,
                            Ok(_) if is_volume_action => {
                                if controller.is_muted() {
                                    println!("Volume:  {}% (muted)", controller.volume_percent());
                                } else {
                                    println!("Volume:  {}%", controller.volume_percent());
                                }
                            }
                            Ok(_) => {}
                            Err(err) => eprintln!("Note:    {err}"),
                        }
                    } else {
                        eprintln!(
                            "Note:    unknown control '{}' (Enter/'toggle', 'n'/'p', 'f'/'b'/'seek <t>', '+'/'-'/'m', 's', Ctrl+C)",
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
            let result = execute_play(args, controller, shutdown, true);
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
            execute_play(&args, PlaybackController::new(), shutdown, false)
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
    args: &PlayArgs,
    controller: PlaybackController,
    shutdown: Arc<AtomicBool>,
    show_controls_hint: bool,
) -> anyhow::Result<()> {
    controller.set_volume_percent(args.volume)?;
    if args.mute {
        controller.mute();
    }

    println!("{}", app_banner());
    let observer = controller.clone();
    let mut hint_printed = false;
    let summary = crate::audio::play_local_playlist_with_controller(
        &args.files,
        controller,
        shutdown,
        |event| match event {
            PlaybackEvent::StateChanged(state) => {
                println!("Status:  {state}");
            }
            PlaybackEvent::TrackLoaded(info) => {
                print_track_info(
                    &info,
                    observer.current_track_index(),
                    observer.track_count(),
                );
                if show_controls_hint && !hint_printed {
                    hint_printed = true;
                    println!(
                        "Controls: Enter/'toggle' pause, 'n'/'p' track, 'f'/'b'/'seek <t>' seek, '+'/'-'/'m' vol, 's' stop, Ctrl+C"
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

fn print_track_info(info: &PlaybackTrackInfo, track_index: Option<usize>, track_count: usize) {
    let display_name = file_display_name(&info.file.path);
    let bit_depth = info
        .stream
        .bits_per_sample
        .map_or_else(|| "unknown".to_string(), |bits| format!("{bits}-bit"));
    let duration = info
        .stream
        .duration
        .map_or_else(|| "unknown".to_string(), format_duration);

    if track_count > 1
        && let Some(idx) = track_index
    {
        println!(
            "Track:   [{}/{}] {display_name} ({} bytes)",
            idx + 1,
            track_count,
            info.file.size_bytes
        );
    } else {
        println!("Track:   {display_name} ({} bytes)", info.file.size_bytes);
    }
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
                files: vec![PathBuf::from("music/sample.flac")],
                volume: 100,
                mute: false,
            }))
        );

        let multi = Cli::try_parse_from([
            "termwave", "play", "--volume", "75", "--mute", "a.flac", "b.flac",
        ])
        .unwrap();
        assert_eq!(
            multi.command,
            Some(Commands::Play(PlayArgs {
                files: vec![PathBuf::from("a.flac"), PathBuf::from("b.flac")],
                volume: 75,
                mute: true,
            }))
        );

        let bad_vol = Cli::try_parse_from(["termwave", "play", "--volume", "150", "a.flac"]);
        assert!(bad_vol.is_err());
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
            parse_playback_control_command("  space "),
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
            parse_playback_control_command("n"),
            Some(PlaybackControlAction::NextTrack)
        );
        assert_eq!(
            parse_playback_control_command("NEXT"),
            Some(PlaybackControlAction::NextTrack)
        );
        assert_eq!(
            parse_playback_control_command("  p "),
            Some(PlaybackControlAction::PreviousTrack)
        );
        assert_eq!(
            parse_playback_control_command("prev"),
            Some(PlaybackControlAction::PreviousTrack)
        );
        assert_eq!(
            parse_playback_control_command("previous"),
            Some(PlaybackControlAction::PreviousTrack)
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
            parse_playback_control_command("+"),
            Some(PlaybackControlAction::VolumeUp)
        );
        assert_eq!(
            parse_playback_control_command("down"),
            Some(PlaybackControlAction::VolumeDown)
        );
        assert_eq!(
            parse_playback_control_command("m"),
            Some(PlaybackControlAction::ToggleMute)
        );
        assert_eq!(
            parse_playback_control_command("unmute"),
            Some(PlaybackControlAction::Unmute)
        );
        assert_eq!(
            parse_playback_control_command("vol 65"),
            Some(PlaybackControlAction::SetVolume("65".to_string()))
        );
        assert_eq!(
            parse_playback_control_command("quit"),
            Some(PlaybackControlAction::Stop)
        );
        assert_eq!(
            parse_playback_control_command("f"),
            Some(PlaybackControlAction::SeekForward)
        );
        assert_eq!(
            parse_playback_control_command("back"),
            Some(PlaybackControlAction::SeekBackward)
        );
        assert_eq!(
            parse_playback_control_command("seek 1:30"),
            Some(PlaybackControlAction::SeekAbsolute("1:30".to_string()))
        );
        assert_eq!(parse_playback_control_command("seek"), None);

        assert_eq!(parse_seek_position("90").unwrap(), Duration::from_secs(90));
        assert_eq!(
            parse_seek_position("1:30").unwrap(),
            Duration::from_secs(90)
        );
        assert_eq!(
            parse_seek_position("0:02.5").unwrap(),
            Duration::from_millis(2500)
        );
        assert!(parse_seek_position("-5").is_err());
        assert!(parse_seek_position("1:75").is_err());
        assert!(parse_seek_position("abc").is_err());
    }

    #[test]
    fn applies_playback_control_actions_with_state_validation() {
        let controller = PlaybackController::new();
        assert!(matches!(
            apply_playback_control_action(&controller, &PlaybackControlAction::TogglePause),
            Err(crate::audio::AudioError::InvalidState(_))
        ));
        assert!(matches!(
            apply_playback_control_action(&controller, &PlaybackControlAction::NextTrack),
            Err(crate::audio::AudioError::InvalidState(_))
        ));
        assert!(matches!(
            apply_playback_control_action(&controller, &PlaybackControlAction::PreviousTrack),
            Err(crate::audio::AudioError::InvalidState(_))
        ));
        assert!(matches!(
            apply_playback_control_action(&controller, &PlaybackControlAction::SeekForward),
            Err(crate::audio::AudioError::InvalidState(_))
        ));
        assert!(matches!(
            apply_playback_control_action(
                &controller,
                &PlaybackControlAction::SeekAbsolute("10".into())
            ),
            Err(crate::audio::AudioError::InvalidState(_))
        ));
        assert!(matches!(
            apply_playback_control_action(&controller, &PlaybackControlAction::Stop),
            Err(crate::audio::AudioError::InvalidState(_))
        ));

        apply_playback_control_action(&controller, &PlaybackControlAction::SetVolume("50".into()))
            .unwrap();
        assert_eq!(controller.volume_percent(), 50);
        apply_playback_control_action(&controller, &PlaybackControlAction::VolumeUp).unwrap();
        assert_eq!(controller.volume_percent(), 55);
        apply_playback_control_action(&controller, &PlaybackControlAction::VolumeDown).unwrap();
        assert_eq!(controller.volume_percent(), 50);
        apply_playback_control_action(&controller, &PlaybackControlAction::ToggleMute).unwrap();
        assert!(controller.is_muted());
        assert!(matches!(
            apply_playback_control_action(
                &controller,
                &PlaybackControlAction::SetVolume("150".into())
            ),
            Err(crate::audio::AudioError::InvalidVolume(_))
        ));
    }
}
