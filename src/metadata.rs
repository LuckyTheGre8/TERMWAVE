//! Audio metadata and embedded artwork boundary.
//!
//! Responsible for extracting tags and technical stream properties while
//! distinguishing source audio properties from output device properties.

use std::fs::File;
use std::io::{Read, Seek};
use std::path::Path;
use std::time::Duration;

use lofty::file::{AudioFile, TaggedFileExt};
use lofty::probe::Probe;
use lofty::tag::{Accessor, ItemKey};
use thiserror::Error;

/// Errors produced during metadata or embedded artwork extraction.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum MetadataError {
    /// File not found.
    #[error("audio file not found at path: {path}")]
    FileNotFound { path: std::path::PathBuf },

    /// Access permission denied.
    #[error("permission denied reading audio file: {path}")]
    PermissionDenied { path: std::path::PathBuf },

    /// Failed to read metadata tags from an audio source.
    #[error("failed to read metadata: {0}")]
    ReadFailed(String),

    /// Underlying I/O error while inspecting metadata.
    #[error("metadata I/O error: {0}")]
    Io(#[from] std::io::Error),
}

/// Extracted audio track tags and technical stream metadata.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct TrackMetadata {
    /// Track title.
    pub title: Option<String>,
    /// Track artist(s).
    pub artist: Option<String>,
    /// Album title.
    pub album: Option<String>,
    /// Album artist.
    pub album_artist: Option<String>,
    /// Track number within the album/disc.
    pub track_number: Option<u32>,
    /// Total tracks within the album/disc.
    pub track_total: Option<u32>,
    /// Disc number.
    pub disc_number: Option<u32>,
    /// Total discs in the release.
    pub disc_total: Option<u32>,
    /// Music genre.
    pub genre: Option<String>,
    /// Release year.
    pub year: Option<u32>,
    /// Release date string if more specific than year (e.g. "2024-05-18").
    pub date: Option<String>,
    /// Duration of the audio track derived from file properties.
    pub duration: Option<Duration>,
    /// Source sample rate in Hz.
    pub sample_rate: Option<u32>,
    /// Source bit depth per sample when available.
    pub bits_per_sample: Option<u32>,
    /// Number of audio channels.
    pub channels: Option<u16>,
    /// Overall or audio bitrate in kbps.
    pub bitrate_kbps: Option<u32>,
    /// Whether embedded artwork was detected.
    pub has_artwork: bool,
}

impl TrackMetadata {
    /// Formats a human-readable display string combining title and artist,
    /// falling back to the filename if title is missing.
    pub fn display_title(&self, fallback_path: &Path) -> String {
        let fallback_name = fallback_path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| fallback_path.display().to_string());

        match (&self.title, &self.artist) {
            (Some(title), Some(artist))
                if !title.trim().is_empty() && !artist.trim().is_empty() =>
            {
                format!("{title} — {artist}")
            }
            (Some(title), _) if !title.trim().is_empty() => title.clone(),
            (_, Some(artist)) if !artist.trim().is_empty() => {
                format!("{fallback_name} — {artist}")
            }
            _ => fallback_name,
        }
    }

    /// Formats track number information, e.g. `"03"` or `"03/12"`.
    pub fn formatted_track_number(&self) -> Option<String> {
        match (self.track_number, self.track_total) {
            (Some(num), Some(total)) => Some(format!("{num:02}/{total:02}")),
            (Some(num), None) => Some(format!("{num:02}")),
            _ => None,
        }
    }

    /// Formats disc number information, e.g. `"1"` or `"1/2"`.
    pub fn formatted_disc_number(&self) -> Option<String> {
        match (self.disc_number, self.disc_total) {
            (Some(num), Some(total)) => Some(format!("{num}/{total}")),
            (Some(num), None) => Some(format!("{num}")),
            _ => None,
        }
    }
}

/// Extracts metadata from a local audio file path using Lofty.
///
/// Supports missing metadata, malformed tags, and tracks with partial tag blocks.
pub fn extract_metadata(path: impl AsRef<Path>) -> Result<TrackMetadata, MetadataError> {
    let path = path.as_ref();
    let tagged_file = Probe::open(path)
        .map_err(|err| map_lofty_error(err, path))?
        .read()
        .map_err(|err| map_lofty_error(err, path))?;

    Ok(parse_tagged_file(&tagged_file))
}

/// Extracts metadata from an open local file and path hint.
pub fn extract_metadata_from_file(file: File, path: &Path) -> Result<TrackMetadata, MetadataError> {
    let tagged_file = Probe::new(file)
        .guess_file_type()
        .map_err(|err| map_io_error(err, path))?
        .read()
        .map_err(|err| map_lofty_error(err, path))?;

    Ok(parse_tagged_file(&tagged_file))
}

/// Extracts metadata from an arbitrary seekable reader.
pub fn extract_metadata_from_reader<R: Read + Seek>(
    reader: &mut R,
) -> Result<TrackMetadata, MetadataError> {
    let tagged_file = Probe::new(reader)
        .guess_file_type()
        .map_err(|err| MetadataError::ReadFailed(err.to_string()))?
        .read()
        .map_err(|err| MetadataError::ReadFailed(err.to_string()))?;

    Ok(parse_tagged_file(&tagged_file))
}

fn map_io_error(err: std::io::Error, path: &Path) -> MetadataError {
    match err.kind() {
        std::io::ErrorKind::NotFound => MetadataError::FileNotFound {
            path: path.to_path_buf(),
        },
        std::io::ErrorKind::PermissionDenied => MetadataError::PermissionDenied {
            path: path.to_path_buf(),
        },
        _ => MetadataError::Io(err),
    }
}

fn map_lofty_error(err: lofty::error::LoftyError, path: &Path) -> MetadataError {
    match err.kind() {
        lofty::error::ErrorKind::Io(io_err) => match io_err.kind() {
            std::io::ErrorKind::NotFound => MetadataError::FileNotFound {
                path: path.to_path_buf(),
            },
            std::io::ErrorKind::PermissionDenied => MetadataError::PermissionDenied {
                path: path.to_path_buf(),
            },
            _ => MetadataError::ReadFailed(io_err.to_string()),
        },
        _ => MetadataError::ReadFailed(err.to_string()),
    }
}

/// Extracts metadata from an audio file, returning default empty metadata on failure
/// so playback can proceed even if tag extraction fails.
pub fn extract_metadata_or_default(path: impl AsRef<Path>) -> TrackMetadata {
    extract_metadata(path).unwrap_or_default()
}

fn parse_tagged_file(tagged_file: &lofty::file::TaggedFile) -> TrackMetadata {
    let mut meta = TrackMetadata::default();

    // 1. Audio stream properties
    let props = tagged_file.properties();
    let duration = props.duration();
    if !duration.is_zero() {
        meta.duration = Some(duration);
    }
    meta.sample_rate = props.sample_rate();
    meta.channels = props.channels().map(u16::from);
    meta.bits_per_sample = props.bit_depth().map(u32::from);
    meta.bitrate_kbps = props.audio_bitrate();

    // 2. Metadata tags (Vorbis Comments for FLAC, ID3, etc.)
    let tag = tagged_file
        .primary_tag()
        .or_else(|| tagged_file.first_tag());

    if let Some(tag) = tag {
        meta.title = clean_tag_string(tag.title().as_deref());
        meta.artist = clean_tag_string(tag.artist().as_deref());
        meta.album = clean_tag_string(tag.album().as_deref());
        meta.genre = clean_tag_string(tag.genre().as_deref());
        meta.track_number = tag.track();
        meta.track_total = tag.track_total();
        meta.disc_number = tag.disk();
        meta.disc_total = tag.disk_total();

        // Year and date
        meta.year = tag.get_string(ItemKey::Year).and_then(parse_year_from_date);

        // Extended keys
        meta.album_artist = clean_tag_string(tag.get_string(ItemKey::AlbumArtist));
        meta.date = clean_tag_string(
            tag.get_string(ItemKey::RecordingDate)
                .or_else(|| tag.get_string(ItemKey::OriginalReleaseDate))
                .or_else(|| tag.get_string(ItemKey::Year)),
        );

        // Fallback year parsing from date string if year was missing
        if meta.year.is_none()
            && let Some(date_str) = &meta.date
            && let Some(year) = parse_year_from_date(date_str)
        {
            meta.year = Some(year);
        }

        meta.has_artwork = !tag.pictures().is_empty();
    }

    meta
}

fn clean_tag_string(val: Option<&str>) -> Option<String> {
    val.map(|s| s.trim())
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string())
}

fn parse_year_from_date(date_str: &str) -> Option<u32> {
    let trimmed = date_str.trim();
    if trimmed.len() >= 4 {
        if trimmed.len() > 4 && trimmed.as_bytes()[4].is_ascii_digit() {
            return None;
        }
        if let Ok(year) = trimmed[0..4].parse::<u32>()
            && (1000..=9999).contains(&year)
        {
            return Some(year);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use std::io::Cursor;
    use std::path::PathBuf;

    use super::*;

    fn fixture_path(name: &str) -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("tests")
            .join("fixtures")
            .join(name)
    }

    #[test]
    fn extracts_stream_properties_from_flac_fixtures() {
        let meta16 = extract_metadata(fixture_path("sine_16bit_44100_stereo.flac")).unwrap();
        assert_eq!(meta16.sample_rate, Some(44_100));
        assert_eq!(meta16.channels, Some(2));
        assert_eq!(meta16.bits_per_sample, Some(16));
        assert!(meta16.duration.is_some());
        assert!(!meta16.has_artwork);

        let meta24 = extract_metadata(fixture_path("sine_24bit_96000_mono.flac")).unwrap();
        assert_eq!(meta24.sample_rate, Some(96_000));
        assert_eq!(meta24.channels, Some(1));
        assert_eq!(meta24.bits_per_sample, Some(24));
        assert!(meta24.duration.is_some());
    }

    #[test]
    fn supports_missing_metadata_and_fallbacks() {
        let mut empty = TrackMetadata::default();
        let fallback_path = Path::new("album/01_intro.flac");
        assert_eq!(empty.display_title(fallback_path), "01_intro.flac");
        assert_eq!(empty.formatted_track_number(), None);
        assert_eq!(empty.formatted_disc_number(), None);

        empty.title = Some("Intro".to_string());
        assert_eq!(empty.display_title(fallback_path), "Intro");

        empty.artist = Some("Artist".to_string());
        assert_eq!(empty.display_title(fallback_path), "Intro — Artist");

        empty.track_number = Some(4);
        assert_eq!(empty.formatted_track_number(), Some("04".to_string()));
        empty.track_total = Some(12);
        assert_eq!(empty.formatted_track_number(), Some("04/12".to_string()));

        empty.disc_number = Some(1);
        assert_eq!(empty.formatted_disc_number(), Some("1".to_string()));
        empty.disc_total = Some(2);
        assert_eq!(empty.formatted_disc_number(), Some("1/2".to_string()));
    }

    #[test]
    fn handles_missing_file_and_malformed_readers() {
        let err = extract_metadata("nonexistent_flac_file_12345.flac").unwrap_err();
        assert!(matches!(err, MetadataError::FileNotFound { .. }));

        let mut malformed_bytes = Cursor::new(b"not a valid flac or audio header");
        let read_err = extract_metadata_from_reader(&mut malformed_bytes).unwrap_err();
        assert!(matches!(read_err, MetadataError::ReadFailed(_)));

        let fallback = extract_metadata_or_default("nonexistent_flac_file_12345.flac");
        assert_eq!(fallback, TrackMetadata::default());
    }

    #[test]
    fn extracts_vorbis_tags_and_track_disc_information() {
        use lofty::tag::{Tag, TagExt, TagType};

        let src_flac = fixture_path("sine_16bit_44100_stereo.flac");
        let temp_flac = std::env::temp_dir().join("termwave_phase16_test_tags.flac");
        std::fs::copy(&src_flac, &temp_flac).expect("copy test flac");

        let mut tag = Tag::new(TagType::VorbisComments);
        tag.set_title("Resolution".to_string());
        tag.set_artist("Hi-Fi Sound Labs".to_string());
        tag.set_album("Acoustic Reference".to_string());
        tag.set_genre("Audiophile".to_string());
        tag.set_track(5);
        tag.set_track_total(10);
        tag.set_disk(1);
        tag.set_disk_total(2);
        tag.insert_text(ItemKey::AlbumArtist, "Reference Ensemble".to_string());
        tag.insert_text(ItemKey::RecordingDate, "2024-11-20".to_string());

        tag.save_to_path(&temp_flac, lofty::config::WriteOptions::default())
            .expect("save tags to flac");

        let meta = extract_metadata(&temp_flac).expect("extract metadata with tags");
        assert_eq!(meta.title.as_deref(), Some("Resolution"));
        assert_eq!(meta.artist.as_deref(), Some("Hi-Fi Sound Labs"));
        assert_eq!(meta.album.as_deref(), Some("Acoustic Reference"));
        assert_eq!(meta.album_artist.as_deref(), Some("Reference Ensemble"));
        assert_eq!(meta.genre.as_deref(), Some("Audiophile"));
        assert_eq!(meta.track_number, Some(5));
        assert_eq!(meta.track_total, Some(10));
        assert_eq!(meta.disc_number, Some(1));
        assert_eq!(meta.disc_total, Some(2));
        assert_eq!(meta.year, Some(2024));
        assert_eq!(meta.date.as_deref(), Some("2024-11-20"));
        assert_eq!(meta.formatted_track_number(), Some("05/10".to_string()));
        assert_eq!(meta.formatted_disc_number(), Some("1/2".to_string()));
        assert_eq!(
            meta.display_title(&temp_flac),
            "Resolution — Hi-Fi Sound Labs"
        );

        let _ = std::fs::remove_file(temp_flac);
    }

    #[test]
    fn handles_malformed_date_strings_cleanly() {
        assert_eq!(parse_year_from_date("2024-11-20"), Some(2024));
        assert_eq!(parse_year_from_date("1982"), Some(1982));
        assert_eq!(parse_year_from_date("   1999   "), Some(1999));
        assert_eq!(parse_year_from_date("bad"), None);
        assert_eq!(parse_year_from_date(""), None);
        assert_eq!(parse_year_from_date("12"), None);
        assert_eq!(parse_year_from_date("99999"), None);
    }
}
