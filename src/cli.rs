//! Command-line interface boundary.
//!
//! Coordinates user-facing CLI parsing via `clap` and delegates execution
//! without owning audio decoding, playback, or library internals.

use std::path::PathBuf;

use clap::{Args, CommandFactory, Parser, Subcommand};

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

/// Parses arguments from the process environment and executes the selected command.
pub fn run() -> anyhow::Result<()> {
    let cli = Cli::parse();
    execute(cli)
}

/// Executes a parsed [`Cli`] command.
pub fn execute(cli: Cli) -> anyhow::Result<()> {
    match cli.command {
        Some(Commands::Play(args)) => {
            let validated = crate::audio::validate_audio_file(&args.file)?;
            println!(
                "{} — validated {} file: {} ({} bytes)",
                app_banner(),
                validated.format,
                validated.path.display(),
                validated.size_bytes
            );
            Ok(())
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
    fn execute_play_validates_existing_and_missing_files() {
        let missing = Cli::try_parse_from(["termwave", "play", "does_not_exist_12345.flac"])
            .expect("CLI syntax is valid");
        assert!(execute(missing).is_err());

        let temp_file = std::env::temp_dir().join(format!(
            "termwave_cli_play_test_{}.flac",
            std::process::id()
        ));
        std::fs::write(&temp_file, b"fLaC\x00\x00\x00\x22payload").expect("write temp flac");
        let valid = Cli::try_parse_from(["termwave", "play", &temp_file.to_string_lossy()])
            .expect("CLI syntax is valid");
        let result = execute(valid);
        let _ = std::fs::remove_file(&temp_file);
        assert!(result.is_ok());
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
}
