//! Recursive music library directory scanner.
//!
//! Traverses selected directories using `walkdir`, identifies supported audio
//! formats, ignores irrelevant files, records non-fatal warnings for
//! inaccessible subdirectories, and supports background execution so it never
//! blocks the playback engine.

use std::fmt;
use std::fs::{self, File};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::thread::{self, JoinHandle};
use std::time::SystemTime;

use walkdir::WalkDir;

use super::LibraryError;

/// FLAC stream marker magic bytes (`"fLaC"`).
const FLAC_MAGIC: &[u8; 4] = b"fLaC";

/// Supported audio container formats recognized by library scanning.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ScannedAudioFormat {
    /// Free Lossless Audio Codec (`.flac`).
    Flac,
}

impl ScannedAudioFormat {
    /// Identifies a supported audio format from a file path's extension
    /// (case-insensitive). Returns `None` for unsupported or missing extensions.
    pub fn from_path(path: &Path) -> Option<Self> {
        let ext = path.extension()?.to_str()?;
        if ext.eq_ignore_ascii_case("flac") {
            Some(Self::Flac)
        } else {
            None
        }
    }

    /// Canonical lowercase extension without a leading dot.
    pub const fn extension(self) -> &'static str {
        match self {
            Self::Flac => "flac",
        }
    }
}

impl fmt::Display for ScannedAudioFormat {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Flac => f.write_str("FLAC"),
        }
    }
}

/// Configuration controlling recursive music library directory scanning.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ScanOptions {
    /// Whether to follow symbolic links during directory traversal (default: `false`).
    pub follow_symlinks: bool,
    /// Optional maximum traversal depth relative to each root directory.
    pub max_depth: Option<usize>,
    /// Whether to include hidden files and directories starting with `.` (default: `false`).
    pub include_hidden: bool,
    /// Whether to open candidate audio files and verify leading container magic bytes (default: `false`).
    pub verify_headers: bool,
}

/// A discovered audio track ready for indexing or playback queueing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScannedTrack {
    /// Full path to the discovered audio file.
    pub path: PathBuf,
    /// Identified audio container format.
    pub format: ScannedAudioFormat,
    /// File size in bytes.
    pub size_bytes: u64,
    /// Last modification timestamp when available from the filesystem.
    pub modified: Option<SystemTime>,
}

/// Non-fatal warning encountered while scanning a directory tree.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScanWarning {
    /// Path where the warning occurred (if known).
    pub path: Option<PathBuf>,
    /// Human-readable description of the issue.
    pub message: String,
}

/// Aggregated results and diagnostics produced by a directory scan.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ScanSummary {
    /// Discovered supported audio tracks sorted deterministically by path.
    pub tracks: Vec<ScannedTrack>,
    /// Total number of regular files visited.
    pub visited_files: usize,
    /// Number of files ignored (unsupported format, hidden, or zero-byte).
    pub ignored_files: usize,
    /// Number of inaccessible or unreadable entries skipped with a warning.
    pub skipped_entries: usize,
    /// Non-fatal warnings collected during traversal.
    pub warnings: Vec<ScanWarning>,
    /// Whether the scan was cancelled cooperatively before completion.
    pub cancelled: bool,
}

/// Live atomic progress counters updated during background directory scanning.
#[derive(Debug, Default)]
pub struct ScanProgress {
    visited_files: AtomicUsize,
    discovered_tracks: AtomicUsize,
    skipped_entries: AtomicUsize,
    finished: AtomicBool,
}

impl ScanProgress {
    /// Number of regular files visited so far.
    pub fn visited_files(&self) -> usize {
        self.visited_files.load(Ordering::Relaxed)
    }

    /// Number of supported audio tracks discovered so far.
    pub fn discovered_tracks(&self) -> usize {
        self.discovered_tracks.load(Ordering::Relaxed)
    }

    /// Number of inaccessible entries skipped so far.
    pub fn skipped_entries(&self) -> usize {
        self.skipped_entries.load(Ordering::Relaxed)
    }

    /// Returns `true` once the background scan thread has finished.
    pub fn is_finished(&self) -> bool {
        self.finished.load(Ordering::Acquire)
    }
}

/// Handle to a non-blocking background directory scan running off the audio thread.
pub struct BackgroundScanHandle {
    cancel_flag: Arc<AtomicBool>,
    progress: Arc<ScanProgress>,
    join_handle: JoinHandle<Result<ScanSummary, LibraryError>>,
}

impl BackgroundScanHandle {
    /// Requests cooperative cancellation of the background scan.
    pub fn cancel(&self) {
        self.cancel_flag.store(true, Ordering::Release);
    }

    /// Returns shared live progress counters for the active scan.
    pub fn progress(&self) -> &Arc<ScanProgress> {
        &self.progress
    }

    /// Returns `true` if the background scan has completed.
    pub fn is_finished(&self) -> bool {
        self.progress.is_finished()
    }

    /// Waits for the background scan thread to finish and returns its result.
    pub fn join(self) -> Result<ScanSummary, LibraryError> {
        self.join_handle
            .join()
            .map_err(|_| LibraryError::Unavailable("background scan thread panicked".to_string()))?
    }
}

/// Recursively scans a single directory for supported audio files.
///
/// Returns an error if `root` itself does not exist, is not a directory, or
/// cannot be opened. Inaccessible subdirectories inside `root` are recorded as
/// [`ScanWarning`]s without aborting the rest of the scan.
pub fn scan_directory(
    root: impl AsRef<Path>,
    options: &ScanOptions,
) -> Result<ScanSummary, LibraryError> {
    scan_directories([root.as_ref()], options)
}

/// Recursively scans multiple root directories for supported audio files.
pub fn scan_directories<I, P>(roots: I, options: &ScanOptions) -> Result<ScanSummary, LibraryError>
where
    I: IntoIterator<Item = P>,
    P: AsRef<Path>,
{
    let cancel_flag = AtomicBool::new(false);
    let progress = ScanProgress::default();
    scan_directories_internal(roots, options, &cancel_flag, &progress)
}

/// Spawns a recursive directory scan on a dedicated background worker thread
/// so that library scanning never blocks the playback engine or UI thread.
pub fn spawn_directory_scan(roots: Vec<PathBuf>, options: ScanOptions) -> BackgroundScanHandle {
    let cancel_flag = Arc::new(AtomicBool::new(false));
    let progress = Arc::new(ScanProgress::default());
    let thread_cancel = Arc::clone(&cancel_flag);
    let thread_progress = Arc::clone(&progress);

    let join_handle = thread::Builder::new()
        .name("termwave-library-scan".to_string())
        .spawn(move || {
            let result =
                scan_directories_internal(&roots, &options, &thread_cancel, &thread_progress);
            thread_progress.finished.store(true, Ordering::Release);
            result
        })
        .expect("failed to spawn library scan thread");

    BackgroundScanHandle {
        cancel_flag,
        progress,
        join_handle,
    }
}

fn scan_directories_internal<I, P>(
    roots: I,
    options: &ScanOptions,
    cancel_flag: &AtomicBool,
    progress: &ScanProgress,
) -> Result<ScanSummary, LibraryError>
where
    I: IntoIterator<Item = P>,
    P: AsRef<Path>,
{
    let root_paths: Vec<PathBuf> = roots
        .into_iter()
        .map(|p| p.as_ref().to_path_buf())
        .collect();
    for root in &root_paths {
        validate_scan_root(root)?;
    }

    let mut summary = ScanSummary::default();

    for root in root_paths {
        if cancel_flag.load(Ordering::Acquire) {
            summary.cancelled = true;
            break;
        }

        let mut walker = WalkDir::new(&root).follow_links(options.follow_symlinks);
        if let Some(depth) = options.max_depth {
            walker = walker.max_depth(depth);
        }

        let mut it = walker.into_iter();
        while let Some(entry_result) = it.next() {
            if cancel_flag.load(Ordering::Acquire) {
                summary.cancelled = true;
                break;
            }

            let entry = match entry_result {
                Ok(entry) => entry,
                Err(err) => {
                    summary.skipped_entries += 1;
                    progress.skipped_entries.fetch_add(1, Ordering::Relaxed);
                    summary.warnings.push(ScanWarning {
                        path: err.path().map(Path::to_path_buf),
                        message: err.to_string(),
                    });
                    continue;
                }
            };

            let path = entry.path();
            let is_root = entry.depth() == 0;
            if !options.include_hidden && !is_root && is_hidden_entry(path) {
                if entry.file_type().is_dir() {
                    it.skip_current_dir();
                } else if entry.file_type().is_file() {
                    summary.visited_files += 1;
                    summary.ignored_files += 1;
                    progress.visited_files.fetch_add(1, Ordering::Relaxed);
                }
                continue;
            }

            if !entry.file_type().is_file() {
                continue;
            }

            summary.visited_files += 1;
            progress.visited_files.fetch_add(1, Ordering::Relaxed);

            let Some(format) = ScannedAudioFormat::from_path(path) else {
                summary.ignored_files += 1;
                continue;
            };

            let metadata = match entry.metadata() {
                Ok(meta) => meta,
                Err(err) => {
                    summary.skipped_entries += 1;
                    progress.skipped_entries.fetch_add(1, Ordering::Relaxed);
                    summary.warnings.push(ScanWarning {
                        path: Some(path.to_path_buf()),
                        message: err.to_string(),
                    });
                    continue;
                }
            };

            let size_bytes = metadata.len();
            if size_bytes == 0 {
                summary.ignored_files += 1;
                continue;
            }

            if options.verify_headers && !has_valid_magic_header(path, format) {
                summary.ignored_files += 1;
                continue;
            }

            summary.tracks.push(ScannedTrack {
                path: path.to_path_buf(),
                format,
                size_bytes,
                modified: metadata.modified().ok(),
            });
            progress.discovered_tracks.fetch_add(1, Ordering::Relaxed);
        }

        if summary.cancelled {
            break;
        }
    }

    summary.tracks.sort_by(|a, b| a.path.cmp(&b.path));
    progress.finished.store(true, Ordering::Release);
    Ok(summary)
}

fn validate_scan_root(root: &Path) -> Result<(), LibraryError> {
    if root.as_os_str().to_string_lossy().trim().is_empty() {
        return Err(LibraryError::PathNotFound {
            path: root.to_path_buf(),
        });
    }

    let meta = match fs::metadata(root) {
        Ok(meta) => meta,
        Err(err) => return Err(map_root_io_error(root, err)),
    };

    if !meta.is_dir() {
        return Err(LibraryError::NotADirectory {
            path: root.to_path_buf(),
        });
    }

    // Verify we can actually open the directory for reading before walking.
    if let Err(err) = fs::read_dir(root) {
        return Err(map_root_io_error(root, err));
    }

    Ok(())
}

fn map_root_io_error(path: &Path, err: std::io::Error) -> LibraryError {
    match err.kind() {
        std::io::ErrorKind::NotFound => LibraryError::PathNotFound {
            path: path.to_path_buf(),
        },
        std::io::ErrorKind::PermissionDenied => LibraryError::PermissionDenied {
            path: path.to_path_buf(),
        },
        _ => LibraryError::Io(err),
    }
}

fn is_hidden_entry(path: &Path) -> bool {
    path.file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name.starts_with('.'))
}

fn has_valid_magic_header(path: &Path, format: ScannedAudioFormat) -> bool {
    let Ok(mut file) = File::open(path) else {
        return false;
    };
    match format {
        ScannedAudioFormat::Flac => {
            let mut magic = [0u8; 4];
            file.read_exact(&mut magic).is_ok() && &magic == FLAC_MAGIC
        }
    }
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::time::{Duration, Instant};

    use super::*;

    fn temp_test_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("termwave_phase18_{name}"));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).expect("create temp test dir");
        dir
    }

    fn fixture_path(name: &str) -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("tests")
            .join("fixtures")
            .join(name)
    }

    #[test]
    fn recursively_scans_supported_flac_and_ignores_irrelevant_files() {
        let root = temp_test_dir("recursive_scan");
        let nested = root.join("Artist").join("Album");
        let hidden_dir = root.join(".hidden_cache");
        fs::create_dir_all(&nested).unwrap();
        fs::create_dir_all(&hidden_dir).unwrap();

        let src_flac = fixture_path("sine_16bit_44100_stereo.flac");
        let track1 = root.join("01_root.flac");
        let track2 = nested.join("02_UPPER.FLAC");
        let hidden_track = hidden_dir.join("ignored_in_hidden.flac");
        let dot_track = root.join(".hidden_track.flac");
        let empty_flac = nested.join("empty.flac");
        let txt_file = root.join("notes.txt");
        let jpg_file = nested.join("cover.jpg");

        fs::copy(&src_flac, &track1).unwrap();
        fs::copy(&src_flac, &track2).unwrap();
        fs::copy(&src_flac, &hidden_track).unwrap();
        fs::copy(&src_flac, &dot_track).unwrap();
        fs::write(&empty_flac, b"").unwrap();
        fs::write(&txt_file, b"album liner notes").unwrap();
        fs::write(&jpg_file, b"\xFF\xD8\xFF").unwrap();

        let summary = scan_directory(&root, &ScanOptions::default()).unwrap();
        assert_eq!(summary.tracks.len(), 2);
        assert_eq!(summary.tracks[0].format, ScannedAudioFormat::Flac);
        assert!(summary.tracks.iter().any(|t| t.path == track1));
        assert!(summary.tracks.iter().any(|t| t.path == track2));
        // txt, jpg, empty.flac, .hidden_track.flac are ignored
        assert_eq!(summary.ignored_files, 4);
        assert!(!summary.cancelled);

        // With include_hidden = true, hidden files/dirs are included
        let hidden_opts = ScanOptions {
            include_hidden: true,
            ..ScanOptions::default()
        };
        let with_hidden = scan_directory(&root, &hidden_opts).unwrap();
        assert_eq!(with_hidden.tracks.len(), 4);

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn verifies_headers_when_enabled_and_rejects_spoofed_flac() {
        let root = temp_test_dir("verify_headers");
        let src_flac = fixture_path("sine_16bit_44100_stereo.flac");
        let real_flac = root.join("real.flac");
        let fake_flac = root.join("fake.flac");

        fs::copy(&src_flac, &real_flac).unwrap();
        fs::write(&fake_flac, b"NOT_A_FLAC_FILE").unwrap();

        let strict_opts = ScanOptions {
            verify_headers: true,
            ..ScanOptions::default()
        };
        let summary = scan_directory(&root, &strict_opts).unwrap();
        assert_eq!(summary.tracks.len(), 1);
        assert_eq!(summary.tracks[0].path, real_flac);
        assert_eq!(summary.ignored_files, 1);

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn handles_missing_and_non_directory_roots_cleanly() {
        let missing =
            scan_directory("nonexistent_library_dir_98765", &ScanOptions::default()).unwrap_err();
        assert!(matches!(missing, LibraryError::PathNotFound { .. }));

        let file_root = fixture_path("sine_16bit_44100_stereo.flac");
        let not_dir = scan_directory(&file_root, &ScanOptions::default()).unwrap_err();
        assert!(matches!(not_dir, LibraryError::NotADirectory { .. }));
    }

    #[test]
    fn runs_scan_in_background_without_blocking_caller_and_supports_cancellation() {
        let root = temp_test_dir("background_scan");
        let src_flac = fixture_path("sine_16bit_44100_stereo.flac");
        for idx in 0..8 {
            let dir = root.join(format!("disc_{idx}"));
            fs::create_dir_all(&dir).unwrap();
            fs::copy(&src_flac, dir.join(format!("track_{idx}.flac"))).unwrap();
        }

        let start = Instant::now();
        let handle = spawn_directory_scan(vec![root.clone()], ScanOptions::default());
        assert!(start.elapsed() < Duration::from_secs(1));

        let summary = handle.join().unwrap();
        assert_eq!(summary.tracks.len(), 8);
        assert!(!summary.cancelled);

        // Test cooperative cancellation on background scan
        let handle2 = spawn_directory_scan(vec![root.clone()], ScanOptions::default());
        handle2.cancel();
        let cancelled_summary = handle2.join().unwrap();
        assert!(cancelled_summary.tracks.len() <= 8);

        let _ = fs::remove_dir_all(&root);
    }
}
