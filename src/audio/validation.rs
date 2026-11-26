//! Local audio file validation.
//!
//! Verifies path validity, existence, regular-file type, readability,
//! and supported container signatures before handing a file to decoders.

use std::fmt;
use std::fs::{self, File};
use std::io::{ErrorKind, Read};
use std::path::{Path, PathBuf};

use super::AudioError;

/// FLAC stream marker magic bytes (`"fLaC"`).
const FLAC_MAGIC: &[u8; 4] = b"fLaC";

/// Supported audio container formats recognized by file validation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AudioFormat {
    /// Free Lossless Audio Codec (`.flac`).
    Flac,
}

impl fmt::Display for AudioFormat {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Flac => f.write_str("FLAC"),
        }
    }
}

/// Metadata produced by validating a local audio file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValidatedAudioFile {
    /// Validated path to the audio file.
    pub path: PathBuf,
    /// Detected audio format.
    pub format: AudioFormat,
    /// File size in bytes.
    pub size_bytes: u64,
}

/// Validates a local audio file for existence, readability, and supported format.
///
/// Checks are ordered to surface actionable diagnostics:
/// 1. Non-empty, non-whitespace path check.
/// 2. Filesystem metadata lookup (existence, permissions, path validity).
/// 3. Regular file and non-zero size checks.
/// 4. Open and header-read verification (read permissions and container signature).
pub fn validate_audio_file(path: impl AsRef<Path>) -> Result<ValidatedAudioFile, AudioError> {
    let path = path.as_ref();
    validate_path_syntax(path)?;

    let metadata = fs::metadata(path).map_err(|err| map_io_error(path, "access", err))?;

    if !metadata.is_file() {
        return Err(AudioError::NotAFile {
            path: path.to_path_buf(),
        });
    }

    let size_bytes = metadata.len();
    if size_bytes == 0 {
        return Err(AudioError::EmptyFile {
            path: path.to_path_buf(),
        });
    }

    let format = detect_format_by_extension(path)?;
    verify_readable_header(path, format)?;

    Ok(ValidatedAudioFile {
        path: path.to_path_buf(),
        format,
        size_bytes,
    })
}

/// Rejects obviously invalid path strings before touching the filesystem.
fn validate_path_syntax(path: &Path) -> Result<(), AudioError> {
    let raw = path.as_os_str();
    if raw.is_empty() || path.to_string_lossy().trim().is_empty() {
        return Err(AudioError::InvalidPath {
            path: path.to_path_buf(),
            reason: "path cannot be empty or whitespace-only".to_string(),
        });
    }
    Ok(())
}

/// Detects the expected audio format from the file extension.
fn detect_format_by_extension(path: &Path) -> Result<AudioFormat, AudioError> {
    let ext = path
        .extension()
        .and_then(|value| value.to_str())
        .map(str::to_ascii_lowercase);

    match ext.as_deref() {
        Some("flac") => Ok(AudioFormat::Flac),
        Some(other) => Err(AudioError::UnsupportedFormat {
            path: path.to_path_buf(),
            reason: format!("extension '.{other}' is not supported (supported: .flac)"),
        }),
        None => Err(AudioError::UnsupportedFormat {
            path: path.to_path_buf(),
            reason: "missing file extension (supported: .flac)".to_string(),
        }),
    }
}

/// Opens the file to verify OS read access and validates the container header.
fn verify_readable_header(path: &Path, format: AudioFormat) -> Result<(), AudioError> {
    let mut file = File::open(path).map_err(|err| map_io_error(path, "open", err))?;

    let mut header = [0_u8; 4];
    let bytes_read = read_header_prefix(&mut file, &mut header)
        .map_err(|err| map_io_error(path, "read", err))?;

    match format {
        AudioFormat::Flac => {
            if bytes_read < FLAC_MAGIC.len() || &header != FLAC_MAGIC {
                return Err(AudioError::UnsupportedFormat {
                    path: path.to_path_buf(),
                    reason: "file does not begin with a valid 'fLaC' stream marker".to_string(),
                });
            }
        }
    }

    Ok(())
}

/// Reads up to `buf.len()` bytes while retrying `ErrorKind::Interrupted`.
fn read_header_prefix(file: &mut File, buf: &mut [u8]) -> std::io::Result<usize> {
    let mut total = 0;
    while total < buf.len() {
        match file.read(&mut buf[total..]) {
            Ok(0) => break,
            Ok(n) => total += n,
            Err(err) if err.kind() == ErrorKind::Interrupted => continue,
            Err(err) => return Err(err),
        }
    }
    Ok(total)
}

/// Maps filesystem I/O errors into descriptive [`AudioError`] variants.
fn map_io_error(path: &Path, action: &str, err: std::io::Error) -> AudioError {
    #[cfg(windows)]
    if err.raw_os_error() == Some(32) || err.raw_os_error() == Some(33) {
        return AudioError::InvalidPath {
            path: path.to_path_buf(),
            reason: "file is locked by another process (sharing violation)".to_string(),
        };
    }

    match err.kind() {
        ErrorKind::NotFound => AudioError::FileNotFound {
            path: path.to_path_buf(),
        },
        ErrorKind::PermissionDenied => AudioError::PermissionDenied {
            path: path.to_path_buf(),
        },
        ErrorKind::InvalidInput | ErrorKind::NotADirectory => AudioError::InvalidPath {
            path: path.to_path_buf(),
            reason: format!("cannot {action} path: {err}"),
        },
        _ => AudioError::InvalidPath {
            path: path.to_path_buf(),
            reason: format!("failed to {action} file: {err}"),
        },
    }
}

#[cfg(test)]
mod tests {
    use std::io::Write;
    use std::sync::atomic::{AtomicU64, Ordering};

    use super::*;

    static NEXT_ID: AtomicU64 = AtomicU64::new(1);

    struct TempDirGuard {
        path: PathBuf,
    }

    impl TempDirGuard {
        fn new(prefix: &str) -> Self {
            let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
            let path = std::env::temp_dir().join(format!(
                "termwave_test_{prefix}_{}_{id}",
                std::process::id()
            ));
            fs::create_dir_all(&path).expect("create temporary test directory");
            Self { path }
        }

        fn write_file(&self, name: &str, bytes: &[u8]) -> PathBuf {
            let file_path = self.path.join(name);
            let mut file = File::create(&file_path).expect("create test file");
            file.write_all(bytes).expect("write test bytes");
            file_path
        }
    }

    impl Drop for TempDirGuard {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.path);
        }
    }

    #[test]
    fn validates_existing_flac_file_case_insensitively() {
        let dir = TempDirGuard::new("valid_flac");
        let path = dir.write_file("Track01.FLAC", b"fLaC\x00\x00\x00\x22payload");

        let validated = validate_audio_file(&path).expect("valid FLAC file should pass");
        assert_eq!(validated.path, path);
        assert_eq!(validated.format, AudioFormat::Flac);
        assert_eq!(validated.size_bytes, 15);
    }

    #[test]
    fn rejects_empty_or_whitespace_path() {
        for raw in ["", "   ", "\t"] {
            let err = validate_audio_file(raw).unwrap_err();
            assert!(matches!(err, AudioError::InvalidPath { .. }));
        }
    }

    #[test]
    fn rejects_missing_file() {
        let dir = TempDirGuard::new("missing");
        let missing = dir.path.join("does_not_exist.flac");

        let err = validate_audio_file(&missing).unwrap_err();
        assert!(matches!(err, AudioError::FileNotFound { .. }));
        assert!(err.to_string().contains("does_not_exist.flac"));
    }

    #[test]
    fn rejects_directory_path() {
        let dir = TempDirGuard::new("dir_not_file");

        let err = validate_audio_file(&dir.path).unwrap_err();
        assert!(matches!(err, AudioError::NotAFile { .. }));
    }

    #[test]
    fn rejects_empty_file() {
        let dir = TempDirGuard::new("empty_file");
        let empty = dir.write_file("empty.flac", b"");

        let err = validate_audio_file(&empty).unwrap_err();
        assert!(matches!(err, AudioError::EmptyFile { .. }));
    }

    #[test]
    fn rejects_unsupported_extension_and_missing_extension() {
        let dir = TempDirGuard::new("unsupported_ext");
        let txt = dir.write_file("notes.txt", b"hello world");
        let mp3 = dir.write_file("song.mp3", b"ID3\x03\x00");
        let no_ext = dir.write_file("README", b"fLaC1234");

        let err_txt = validate_audio_file(&txt).unwrap_err();
        assert!(matches!(err_txt, AudioError::UnsupportedFormat { .. }));
        assert!(err_txt.to_string().contains(".txt"));

        let err_mp3 = validate_audio_file(&mp3).unwrap_err();
        assert!(matches!(err_mp3, AudioError::UnsupportedFormat { .. }));
        assert!(err_mp3.to_string().contains(".mp3"));

        let err_no_ext = validate_audio_file(&no_ext).unwrap_err();
        assert!(matches!(err_no_ext, AudioError::UnsupportedFormat { .. }));
        assert!(err_no_ext.to_string().contains("missing file extension"));
    }

    #[test]
    fn rejects_flac_extension_with_non_flac_header() {
        let dir = TempDirGuard::new("bad_magic");
        let fake = dir.write_file("fake.flac", b"not actually flac audio");
        let short = dir.write_file("short.flac", b"fLa");

        let err_fake = validate_audio_file(&fake).unwrap_err();
        assert!(matches!(err_fake, AudioError::UnsupportedFormat { .. }));
        assert!(err_fake.to_string().contains("fLaC"));

        let err_short = validate_audio_file(&short).unwrap_err();
        assert!(matches!(err_short, AudioError::UnsupportedFormat { .. }));
    }

    #[test]
    fn maps_permission_denied_cleanly() {
        let err = map_io_error(
            Path::new("locked.flac"),
            "open",
            std::io::Error::from(ErrorKind::PermissionDenied),
        );
        assert!(matches!(err, AudioError::PermissionDenied { .. }));
        assert!(err.to_string().contains("permission denied"));
        assert!(err.to_string().contains("locked.flac"));
    }

    #[cfg(windows)]
    #[test]
    fn rejects_exclusively_locked_file_on_windows() {
        use std::fs::OpenOptions;
        use std::os::windows::fs::OpenOptionsExt;

        let dir = TempDirGuard::new("locked_win");
        let path = dir.write_file("locked.flac", b"fLaC\x00\x00\x00\x22payload");

        let _lock = OpenOptions::new()
            .read(true)
            .share_mode(0)
            .open(&path)
            .expect("open file with exclusive share_mode(0)");

        let err = validate_audio_file(&path).unwrap_err();
        assert!(matches!(err, AudioError::InvalidPath { .. }));
    }

    #[cfg(unix)]
    #[test]
    fn rejects_unreadable_file_on_unix() {
        use std::os::unix::fs::PermissionsExt;

        let dir = TempDirGuard::new("chmod_unix");
        let path = dir.write_file("secret.flac", b"fLaC\x00\x00\x00\x22payload");
        let mut perms = fs::metadata(&path).unwrap().permissions();
        perms.set_mode(0o000);
        fs::set_permissions(&path, perms).unwrap();

        let result = validate_audio_file(&path);

        let mut restore = fs::metadata(&path).unwrap().permissions();
        restore.set_mode(0o644);
        let _ = fs::set_permissions(&path, restore);

        if let Err(err) = result {
            assert!(matches!(err, AudioError::PermissionDenied { .. }));
        }
    }
}
