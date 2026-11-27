//! Embedded album artwork extraction, format detection, and terminal preparation.

use std::fmt;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

use lofty::config::ParseOptions;
use lofty::file::TaggedFileExt;
use lofty::picture::{MimeType, Picture, PictureType};
use lofty::probe::Probe;

use super::{MetadataError, map_io_error, map_lofty_error};

/// Default maximum embedded artwork payload size (8 MiB) loaded into memory.
pub const DEFAULT_MAX_ARTWORK_BYTES: usize = 8 * 1024 * 1024;

/// FLAC metadata block type identifier for embedded `PICTURE` blocks.
const FLAC_BLOCK_TYPE_PICTURE: u8 = 6;

/// Options controlling embedded artwork extraction.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ArtworkExtractOptions {
    /// Maximum allowed artwork payload size in bytes.
    pub max_bytes: usize,
}

impl Default for ArtworkExtractOptions {
    fn default() -> Self {
        Self {
            max_bytes: DEFAULT_MAX_ARTWORK_BYTES,
        }
    }
}

impl ArtworkExtractOptions {
    /// Creates options with a custom maximum payload size in bytes.
    pub const fn with_max_bytes(max_bytes: usize) -> Self {
        Self { max_bytes }
    }
}

/// Identified image container format for embedded artwork.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ArtworkFormat {
    /// Portable Network Graphics (`image/png`).
    Png,
    /// JPEG (`image/jpeg`).
    Jpeg,
    /// Graphics Interchange Format (`image/gif`).
    Gif,
    /// WebP (`image/webp`).
    Webp,
    /// Windows Bitmap (`image/bmp`).
    Bmp,
    /// Tagged Image File Format (`image/tiff`).
    Tiff,
    /// Unrecognized or unsupported image format.
    #[default]
    Unknown,
}

impl ArtworkFormat {
    /// Identifies an artwork format by inspecting magic bytes first, then falling
    /// back to the container MIME type tag.
    pub fn detect(data: &[u8], mime: Option<&MimeType>) -> Self {
        Self::from_magic_bytes(data)
            .or_else(|| mime.map(Self::from_mime_type))
            .unwrap_or(Self::Unknown)
    }

    /// Identifies an image format from leading magic bytes.
    pub fn from_magic_bytes(data: &[u8]) -> Option<Self> {
        if data.starts_with(&[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A]) {
            return Some(Self::Png);
        }
        if data.starts_with(&[0xFF, 0xD8, 0xFF]) {
            return Some(Self::Jpeg);
        }
        if data.starts_with(b"GIF87a") || data.starts_with(b"GIF89a") {
            return Some(Self::Gif);
        }
        if data.len() >= 12 && data.starts_with(b"RIFF") && &data[8..12] == b"WEBP" {
            return Some(Self::Webp);
        }
        if data.starts_with(b"BM") {
            return Some(Self::Bmp);
        }
        if data.starts_with(&[b'I', b'I', 0x2A, 0x00])
            || data.starts_with(&[b'M', b'M', 0x00, 0x2A])
        {
            return Some(Self::Tiff);
        }
        None
    }

    /// Maps a Lofty [`MimeType`] to [`ArtworkFormat`].
    pub fn from_mime_type(mime: &MimeType) -> Self {
        match mime {
            MimeType::Png => Self::Png,
            MimeType::Jpeg => Self::Jpeg,
            MimeType::Gif => Self::Gif,
            MimeType::Bmp => Self::Bmp,
            MimeType::Tiff => Self::Tiff,
            MimeType::Unknown(raw) => {
                let lower = raw.trim().to_ascii_lowercase();
                if lower == "image/webp" {
                    Self::Webp
                } else if lower == "image/png" {
                    Self::Png
                } else if lower == "image/jpeg" || lower == "image/jpg" {
                    Self::Jpeg
                } else if lower == "image/gif" {
                    Self::Gif
                } else if lower == "image/bmp" {
                    Self::Bmp
                } else if lower == "image/tiff" {
                    Self::Tiff
                } else {
                    Self::Unknown
                }
            }
            _ => Self::Unknown,
        }
    }

    /// Canonical MIME type string for this format.
    pub const fn mime_type(self) -> &'static str {
        match self {
            Self::Png => "image/png",
            Self::Jpeg => "image/jpeg",
            Self::Gif => "image/gif",
            Self::Webp => "image/webp",
            Self::Bmp => "image/bmp",
            Self::Tiff => "image/tiff",
            Self::Unknown => "application/octet-stream",
        }
    }

    /// Canonical lowercase file extension without leading dot.
    pub const fn extension(self) -> &'static str {
        match self {
            Self::Png => "png",
            Self::Jpeg => "jpg",
            Self::Gif => "gif",
            Self::Webp => "webp",
            Self::Bmp => "bmp",
            Self::Tiff => "tiff",
            Self::Unknown => "bin",
        }
    }

    /// Returns `true` for formats commonly supported by terminal image protocols
    /// and image decoders.
    pub const fn is_terminal_supported(self) -> bool {
        matches!(
            self,
            Self::Png | Self::Jpeg | Self::Gif | Self::Webp | Self::Bmp
        )
    }
}

impl fmt::Display for ArtworkFormat {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let label = match self {
            Self::Png => "PNG",
            Self::Jpeg => "JPEG",
            Self::Gif => "GIF",
            Self::Webp => "WebP",
            Self::Bmp => "BMP",
            Self::Tiff => "TIFF",
            Self::Unknown => "Unknown",
        };
        f.write_str(label)
    }
}

/// Semantic role of an embedded picture within an audio file.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ArtworkRole {
    /// Front album cover.
    CoverFront,
    /// Back album cover.
    CoverBack,
    /// Media label (CD/vinyl).
    Media,
    /// Artist or band photo.
    Artist,
    /// Other or unspecified picture role.
    #[default]
    Other,
}

impl From<PictureType> for ArtworkRole {
    fn from(value: PictureType) -> Self {
        match value {
            PictureType::CoverFront => Self::CoverFront,
            PictureType::CoverBack => Self::CoverBack,
            PictureType::Media => Self::Media,
            PictureType::Artist | PictureType::LeadArtist => Self::Artist,
            _ => Self::Other,
        }
    }
}

impl fmt::Display for ArtworkRole {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let label = match self {
            Self::CoverFront => "Front Cover",
            Self::CoverBack => "Back Cover",
            Self::Media => "Media",
            Self::Artist => "Artist",
            Self::Other => "Other",
        };
        f.write_str(label)
    }
}

/// Pixel dimensions of an embedded artwork image.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ArtworkDimensions {
    /// Image width in pixels.
    pub width: u32,
    /// Image height in pixels.
    pub height: u32,
}

impl fmt::Display for ArtworkDimensions {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}x{}", self.width, self.height)
    }
}

/// Lightweight summary of embedded artwork without holding raw image bytes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArtworkSummary {
    /// Detected image container format.
    pub format: ArtworkFormat,
    /// Semantic picture role.
    pub role: ArtworkRole,
    /// Encoded image payload size in bytes.
    pub byte_len: usize,
    /// Parsed pixel dimensions if the image header is valid and supported.
    pub dimensions: Option<ArtworkDimensions>,
    /// Optional picture description from the tag.
    pub description: Option<String>,
    /// Total number of embedded pictures present in the primary tag.
    pub picture_count: usize,
}

impl ArtworkSummary {
    /// Formats a compact human-readable label (e.g. `"PNG 600x600, 42 KB"`).
    pub fn display_label(&self) -> String {
        let size = format_byte_size(self.byte_len);
        match self.dimensions {
            Some(dims) => format!("{} {dims} ({size})", self.format),
            None => format!("{} ({size})", self.format),
        }
    }
}

/// Extracted embedded album artwork payload and metadata.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EmbeddedArtwork {
    /// Lightweight summary of the artwork.
    pub summary: ArtworkSummary,
    /// Raw encoded image bytes.
    pub data: Vec<u8>,
}

impl EmbeddedArtwork {
    /// Prepares this artwork for terminal rendering within a target character cell box.
    pub fn prepare_for_terminal(&self, max_cols: u16, max_rows: u16) -> TerminalArtwork {
        let layout = TerminalArtworkLayout::fit_within(self.summary.dimensions, max_cols, max_rows);
        let placeholder_lines =
            render_placeholder_lines(Some(&self.summary), layout.cols, layout.rows);
        TerminalArtwork {
            summary: self.summary.clone(),
            layout,
            supports_image_protocol: self.summary.format.is_terminal_supported(),
            placeholder_lines,
        }
    }
}

/// Aspect-ratio-aware terminal cell dimensions prepared for rendering artwork.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TerminalArtworkLayout {
    /// Target width in terminal columns.
    pub cols: u16,
    /// Target height in terminal rows.
    pub rows: u16,
}

impl TerminalArtworkLayout {
    /// Computes terminal cell dimensions preserving aspect ratio within `(max_cols, max_rows)`.
    ///
    /// Assumes standard terminal character cells are approximately twice as tall as they
    /// are wide (`cell_aspect ~= 2.0`), so a square image fits cleanly into `2N cols x N rows`.
    pub fn fit_within(dimensions: Option<ArtworkDimensions>, max_cols: u16, max_rows: u16) -> Self {
        if max_cols == 0 || max_rows == 0 {
            return Self { cols: 0, rows: 0 };
        }

        let (img_w, img_h) = match dimensions {
            Some(d) if d.width > 0 && d.height > 0 => (d.width as f64, d.height as f64),
            _ => (1.0, 1.0),
        };

        // Target cell ratio: cols / rows == (img_w / img_h) * 2.0
        let desired_cols_per_row = (img_w / img_h) * 2.0;
        let max_c = max_cols as f64;
        let max_r = max_rows as f64;

        let (cols, rows) = if max_c / max_r > desired_cols_per_row {
            let rows = max_r;
            let cols = (rows * desired_cols_per_row).round().clamp(1.0, max_c);
            (cols as u16, rows as u16)
        } else {
            let cols = max_c;
            let rows = (cols / desired_cols_per_row).round().clamp(1.0, max_r);
            (cols as u16, rows as u16)
        };

        Self { cols, rows }
    }
}

/// Terminal-ready artwork descriptor containing layout, protocol readiness, and fallback text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TerminalArtwork {
    /// Summary of the underlying artwork.
    pub summary: ArtworkSummary,
    /// Computed terminal cell layout.
    pub layout: TerminalArtworkLayout,
    /// Whether the format is suitable for terminal image protocols (Kitty/iTerm2/Sixel).
    pub supports_image_protocol: bool,
    /// Pre-rendered textual fallback lines for terminals without image protocol support.
    pub placeholder_lines: Vec<String>,
}

/// Renders a textual placeholder box for terminals without image protocol support.
pub fn render_placeholder_lines(
    summary: Option<&ArtworkSummary>,
    cols: u16,
    rows: u16,
) -> Vec<String> {
    if cols == 0 || rows == 0 {
        return Vec::new();
    }

    let w = cols as usize;
    let h = rows as usize;

    let label = match summary {
        Some(s) => match s.dimensions {
            Some(d) => format!("{} {}", s.format, d),
            None => s.format.to_string(),
        },
        None => "No Artwork".to_string(),
    };

    if w < 4 || h < 3 {
        let truncated: String = label.chars().take(w).collect();
        return vec![format!("{truncated:<w$}")];
    }

    let inner_w = w - 2;
    let mut lines = Vec::with_capacity(h);
    lines.push(format!("+{}+", "-".repeat(inner_w)));

    let mid_row = h / 2;
    for row in 1..(h - 1) {
        if row == mid_row {
            let text: String = label.chars().take(inner_w).collect();
            let text_len = text.chars().count();
            let left_pad = (inner_w.saturating_sub(text_len)) / 2;
            let right_pad = inner_w.saturating_sub(text_len + left_pad);
            lines.push(format!(
                "|{}{text}{}|",
                " ".repeat(left_pad),
                " ".repeat(right_pad)
            ));
        } else {
            lines.push(format!("|{}|", " ".repeat(inner_w)));
        }
    }

    lines.push(format!("+{}+", "-".repeat(inner_w)));
    lines
}

/// Extracts the primary embedded album artwork from a local audio file path.
///
/// Returns `Ok(None)` when the file contains no embedded artwork.
/// Enforces `options.max_bytes` before and during extraction to avoid loading
/// excessively large images into memory.
pub fn extract_artwork(
    path: impl AsRef<Path>,
    options: ArtworkExtractOptions,
) -> Result<Option<EmbeddedArtwork>, MetadataError> {
    let path = path.as_ref();
    let mut file = File::open(path).map_err(|err| map_io_error(err, path))?;
    check_flac_picture_block_sizes(&mut file, options.max_bytes)?;

    let tagged_file = Probe::new(file)
        .guess_file_type()
        .map_err(|err| map_io_error(err, path))?
        .options(ParseOptions::new())
        .read()
        .map_err(|err| map_lofty_error(err, path))?;

    extract_from_tagged_file(&tagged_file, options)
}

/// Extracts the primary embedded album artwork, returning `None` if missing,
/// unreadable, or exceeding `options.max_bytes`.
pub fn extract_artwork_or_none(
    path: impl AsRef<Path>,
    options: ArtworkExtractOptions,
) -> Option<EmbeddedArtwork> {
    extract_artwork(path, options).ok().flatten()
}

/// Extracts the primary embedded album artwork from an arbitrary seekable reader.
///
/// Returns `Ok(None)` when no embedded artwork is present.
pub fn extract_artwork_from_reader<R: Read + Seek>(
    reader: &mut R,
    options: ArtworkExtractOptions,
) -> Result<Option<EmbeddedArtwork>, MetadataError> {
    check_flac_picture_block_sizes(reader, options.max_bytes)?;

    let tagged_file = Probe::new(reader)
        .guess_file_type()
        .map_err(|err| MetadataError::ReadFailed(err.to_string()))?
        .options(ParseOptions::new())
        .read()
        .map_err(|err| MetadataError::ReadFailed(err.to_string()))?;

    extract_from_tagged_file(&tagged_file, options)
}

/// Extracts only the lightweight [`ArtworkSummary`] if artwork is present,
/// or `Ok(None)` if missing or if extraction fails/exceeds size limits.
pub(crate) fn summarize_tagged_file_artwork(
    tagged_file: &lofty::file::TaggedFile,
) -> Option<ArtworkSummary> {
    let pictures = collect_pictures(tagged_file);
    let picture = select_primary_picture(&pictures)?;
    Some(build_summary(picture, pictures.len()))
}

fn extract_from_tagged_file(
    tagged_file: &lofty::file::TaggedFile,
    options: ArtworkExtractOptions,
) -> Result<Option<EmbeddedArtwork>, MetadataError> {
    let pictures = collect_pictures(tagged_file);
    let Some(picture) = select_primary_picture(&pictures) else {
        return Ok(None);
    };

    let data = picture.data();
    if data.len() > options.max_bytes {
        return Err(MetadataError::ArtworkTooLarge {
            size_bytes: data.len(),
            max_bytes: options.max_bytes,
        });
    }

    let summary = build_summary(picture, pictures.len());
    Ok(Some(EmbeddedArtwork {
        summary,
        data: data.to_vec(),
    }))
}

fn collect_pictures(tagged_file: &lofty::file::TaggedFile) -> Vec<&Picture> {
    if let Some(primary) = tagged_file.primary_tag()
        && !primary.pictures().is_empty()
    {
        return primary.pictures().iter().collect();
    }

    for tag in tagged_file.tags() {
        if !tag.pictures().is_empty() {
            return tag.pictures().iter().collect();
        }
    }

    Vec::new()
}

fn select_primary_picture<'a>(pictures: &[&'a Picture]) -> Option<&'a Picture> {
    pictures
        .iter()
        .copied()
        .filter(|p| !p.data().is_empty())
        .find(|p| p.pic_type() == PictureType::CoverFront)
        .or_else(|| {
            pictures
                .iter()
                .copied()
                .filter(|p| !p.data().is_empty())
                .find(|p| p.pic_type() == PictureType::CoverBack)
        })
        .or_else(|| pictures.iter().copied().find(|p| !p.data().is_empty()))
}

fn build_summary(picture: &Picture, picture_count: usize) -> ArtworkSummary {
    let data = picture.data();
    let format = ArtworkFormat::detect(data, picture.mime_type());
    let dimensions = parse_image_dimensions(format, data);
    let description = picture
        .description()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string);

    ArtworkSummary {
        format,
        role: ArtworkRole::from(picture.pic_type()),
        byte_len: data.len(),
        dimensions,
        description,
        picture_count,
    }
}

/// Pre-scans FLAC metadata block headers so oversized `PICTURE` blocks are
/// rejected before Lofty allocates or reads them into memory.
pub(crate) fn check_flac_picture_block_sizes<R: Read + Seek>(
    reader: &mut R,
    max_bytes: usize,
) -> Result<(), MetadataError> {
    let start_pos = reader.stream_position()?;
    let mut magic = [0u8; 4];
    if reader.read_exact(&mut magic).is_err() || &magic != b"fLaC" {
        reader.seek(SeekFrom::Start(start_pos))?;
        return Ok(());
    }

    let mut header = [0u8; 4];
    loop {
        if reader.read_exact(&mut header).is_err() {
            break;
        }
        let is_last = (header[0] & 0x80) != 0;
        let block_type = header[0] & 0x7F;
        let block_len =
            ((header[1] as usize) << 16) | ((header[2] as usize) << 8) | (header[3] as usize);

        if block_type == FLAC_BLOCK_TYPE_PICTURE && block_len > max_bytes {
            reader.seek(SeekFrom::Start(start_pos))?;
            return Err(MetadataError::ArtworkTooLarge {
                size_bytes: block_len,
                max_bytes,
            });
        }

        if reader.seek(SeekFrom::Current(block_len as i64)).is_err() {
            break;
        }

        if is_last {
            break;
        }
    }

    reader.seek(SeekFrom::Start(start_pos))?;
    Ok(())
}

/// Parses image pixel dimensions from encoded header bytes without decoding the full image.
pub fn parse_image_dimensions(format: ArtworkFormat, data: &[u8]) -> Option<ArtworkDimensions> {
    match format {
        ArtworkFormat::Png => parse_png_dimensions(data),
        ArtworkFormat::Gif => parse_gif_dimensions(data),
        ArtworkFormat::Bmp => parse_bmp_dimensions(data),
        ArtworkFormat::Jpeg => parse_jpeg_dimensions(data),
        ArtworkFormat::Webp => parse_webp_dimensions(data),
        ArtworkFormat::Tiff | ArtworkFormat::Unknown => None,
    }
}

fn parse_png_dimensions(data: &[u8]) -> Option<ArtworkDimensions> {
    if data.len() < 24 || &data[12..16] != b"IHDR" {
        return None;
    }
    let width = u32::from_be_bytes(data[16..20].try_into().ok()?);
    let height = u32::from_be_bytes(data[20..24].try_into().ok()?);
    (width > 0 && height > 0).then_some(ArtworkDimensions { width, height })
}

fn parse_gif_dimensions(data: &[u8]) -> Option<ArtworkDimensions> {
    if data.len() < 10 {
        return None;
    }
    let width = u16::from_le_bytes(data[6..8].try_into().ok()?) as u32;
    let height = u16::from_le_bytes(data[8..10].try_into().ok()?) as u32;
    (width > 0 && height > 0).then_some(ArtworkDimensions { width, height })
}

fn parse_bmp_dimensions(data: &[u8]) -> Option<ArtworkDimensions> {
    if data.len() < 26 {
        return None;
    }
    let dib_size = u32::from_le_bytes(data[14..18].try_into().ok()?);
    if dib_size == 12 {
        let width = u16::from_le_bytes(data[18..20].try_into().ok()?) as u32;
        let height = u16::from_le_bytes(data[20..22].try_into().ok()?) as u32;
        (width > 0 && height > 0).then_some(ArtworkDimensions { width, height })
    } else if dib_size >= 40 {
        let width = i32::from_le_bytes(data[18..22].try_into().ok()?).unsigned_abs();
        let height = i32::from_le_bytes(data[22..26].try_into().ok()?).unsigned_abs();
        (width > 0 && height > 0).then_some(ArtworkDimensions { width, height })
    } else {
        None
    }
}

fn parse_jpeg_dimensions(data: &[u8]) -> Option<ArtworkDimensions> {
    if data.len() < 4 || data[0] != 0xFF || data[1] != 0xD8 {
        return None;
    }

    let mut pos = 2usize;
    while pos + 1 < data.len() {
        while pos < data.len() && data[pos] != 0xFF {
            pos += 1;
        }
        while pos < data.len() && data[pos] == 0xFF {
            pos += 1;
        }
        if pos >= data.len() {
            break;
        }

        let marker = data[pos];
        pos += 1;

        if marker == 0x00 || marker == 0x01 || (0xD0..=0xD8).contains(&marker) {
            continue;
        }
        if marker == 0xD9 || marker == 0xDA {
            break;
        }

        if pos + 2 > data.len() {
            break;
        }
        let seg_len = u16::from_be_bytes(data[pos..pos + 2].try_into().ok()?) as usize;
        if seg_len < 2 || pos + seg_len > data.len() {
            break;
        }

        let is_sof = matches!(
            marker,
            0xC0..=0xC3 | 0xC5..=0xC7 | 0xC9..=0xCB | 0xCD..=0xCF
        );
        if is_sof && seg_len >= 7 {
            let height = u16::from_be_bytes(data[pos + 3..pos + 5].try_into().ok()?) as u32;
            let width = u16::from_be_bytes(data[pos + 5..pos + 7].try_into().ok()?) as u32;
            return (width > 0 && height > 0).then_some(ArtworkDimensions { width, height });
        }

        pos += seg_len;
    }

    None
}

fn parse_webp_dimensions(data: &[u8]) -> Option<ArtworkDimensions> {
    if data.len() < 30 {
        return None;
    }
    match &data[12..16] {
        b"VP8 " => {
            if data[23..26] != [0x9D, 0x01, 0x2A] {
                return None;
            }
            let width = (u16::from_le_bytes(data[26..28].try_into().ok()?) & 0x3FFF) as u32;
            let height = (u16::from_le_bytes(data[28..30].try_into().ok()?) & 0x3FFF) as u32;
            (width > 0 && height > 0).then_some(ArtworkDimensions { width, height })
        }
        b"VP8L" => {
            if data[20] != 0x2F {
                return None;
            }
            let bits = u32::from_le_bytes(data[21..25].try_into().ok()?);
            let width = (bits & 0x3FFF) + 1;
            let height = ((bits >> 14) & 0x3FFF) + 1;
            Some(ArtworkDimensions { width, height })
        }
        b"VP8X" => {
            let width =
                1 + (data[24] as u32 | ((data[25] as u32) << 8) | ((data[26] as u32) << 16));
            let height =
                1 + (data[27] as u32 | ((data[28] as u32) << 8) | ((data[29] as u32) << 16));
            Some(ArtworkDimensions { width, height })
        }
        _ => None,
    }
}

fn format_byte_size(bytes: usize) -> String {
    if bytes >= 1024 * 1024 {
        format!("{:.1} MB", bytes as f64 / (1024.0 * 1024.0))
    } else if bytes >= 1024 {
        format!("{:.1} KB", bytes as f64 / 1024.0)
    } else {
        format!("{bytes} B")
    }
}

#[cfg(test)]
mod tests {
    use std::io::Cursor;
    use std::path::PathBuf;

    use lofty::file::AudioFile;
    use lofty::flac::FlacFile;
    use lofty::ogg::OggPictureStorage;
    use lofty::picture::{MimeType, Picture, PictureInformation, PictureType};

    use super::*;

    fn fixture_path(name: &str) -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("tests")
            .join("fixtures")
            .join(name)
    }

    /// Minimal valid 1x1 PNG header + IHDR bytes (24+ bytes).
    fn sample_png_bytes(width: u32, height: u32) -> Vec<u8> {
        let mut bytes = vec![
            0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A, // PNG signature
            0x00, 0x00, 0x00, 0x0D, // IHDR length
            b'I', b'H', b'D', b'R', // IHDR chunk
        ];
        bytes.extend_from_slice(&width.to_be_bytes());
        bytes.extend_from_slice(&height.to_be_bytes());
        bytes.extend_from_slice(&[8, 2, 0, 0, 0]); // bit depth, color type, etc.
        bytes
    }

    #[test]
    fn identifies_artwork_formats_from_magic_bytes_and_mime() {
        let png = sample_png_bytes(600, 600);
        assert_eq!(ArtworkFormat::detect(&png, None), ArtworkFormat::Png);
        assert_eq!(ArtworkFormat::Png.mime_type(), "image/png");
        assert_eq!(ArtworkFormat::Png.extension(), "png");
        assert!(ArtworkFormat::Png.is_terminal_supported());

        let jpeg = [0xFF, 0xD8, 0xFF, 0xE0, 0x00, 0x10];
        assert_eq!(ArtworkFormat::detect(&jpeg, None), ArtworkFormat::Jpeg);
        assert_eq!(ArtworkFormat::Jpeg.extension(), "jpg");

        let gif = b"GIF89a\x10\x00\x20\x00";
        assert_eq!(ArtworkFormat::detect(gif, None), ArtworkFormat::Gif);

        let webp = b"RIFF\x24\x00\x00\x00WEBPVP8L\x00\x00\x00\x00\x2F\x00\x00\x00\x00";
        assert_eq!(ArtworkFormat::detect(webp, None), ArtworkFormat::Webp);

        let bmp = b"BM\x00\x00\x00\x00";
        assert_eq!(ArtworkFormat::detect(bmp, None), ArtworkFormat::Bmp);

        let tiff = [b'I', b'I', 0x2A, 0x00];
        assert_eq!(ArtworkFormat::detect(&tiff, None), ArtworkFormat::Tiff);
        assert!(!ArtworkFormat::Tiff.is_terminal_supported());

        assert_eq!(
            ArtworkFormat::detect(b"unknown", Some(&MimeType::Unknown("image/webp".into()))),
            ArtworkFormat::Webp
        );
        assert_eq!(
            ArtworkFormat::detect(b"unknown", None),
            ArtworkFormat::Unknown
        );
    }

    #[test]
    fn parses_image_dimensions_from_headers() {
        let png = sample_png_bytes(1200, 800);
        assert_eq!(
            parse_image_dimensions(ArtworkFormat::Png, &png),
            Some(ArtworkDimensions {
                width: 1200,
                height: 800
            })
        );

        let gif = b"GIF89a\x40\x01\xF0\x00"; // 320x240 LE
        assert_eq!(
            parse_image_dimensions(ArtworkFormat::Gif, gif),
            Some(ArtworkDimensions {
                width: 320,
                height: 240
            })
        );

        // Minimal JPEG with SOI + APP0 + SOF0 (640x480) + EOI
        let jpeg = [
            0xFF, 0xD8, // SOI
            0xFF, 0xE0, 0x00, 0x04, 0x00, 0x00, // APP0 len=4
            0xFF, 0xC0, 0x00, 0x0B, 0x08, 0x01, 0xE0, 0x02, 0x80, 0x03, 0x01, 0x22,
            0x00, // SOF0: 480x640
            0xFF, 0xD9, // EOI
        ];
        assert_eq!(
            parse_image_dimensions(ArtworkFormat::Jpeg, &jpeg),
            Some(ArtworkDimensions {
                width: 640,
                height: 480
            })
        );

        assert_eq!(parse_image_dimensions(ArtworkFormat::Png, b"bad"), None);
    }

    #[test]
    fn handles_missing_artwork_cleanly() {
        let path = fixture_path("sine_16bit_44100_stereo.flac");
        let artwork = extract_artwork(&path, ArtworkExtractOptions::default()).unwrap();
        assert!(artwork.is_none());
        assert!(extract_artwork_or_none(&path, ArtworkExtractOptions::default()).is_none());
    }

    #[test]
    fn extracts_embedded_artwork_and_prioritizes_front_cover() {
        let src = fixture_path("sine_16bit_44100_stereo.flac");
        let temp_flac = std::env::temp_dir().join("termwave_phase17_artwork.flac");
        std::fs::copy(&src, &temp_flac).expect("copy fixture");

        let mut flac_file = FlacFile::read_from(
            &mut std::fs::File::open(&temp_flac).unwrap(),
            ParseOptions::new(),
        )
        .unwrap();

        let back_cover = Picture::unchecked(vec![0xFF, 0xD8, 0xFF, 0xD9])
            .pic_type(PictureType::CoverBack)
            .mime_type(MimeType::Jpeg)
            .description("Back")
            .build();
        let front_cover = Picture::unchecked(sample_png_bytes(500, 500))
            .pic_type(PictureType::CoverFront)
            .mime_type(MimeType::Png)
            .description("Front Cover")
            .build();

        flac_file
            .insert_picture(
                back_cover,
                Some(PictureInformation {
                    width: 100,
                    height: 100,
                    color_depth: 24,
                    num_colors: 0,
                }),
            )
            .unwrap();
        flac_file
            .insert_picture(
                front_cover,
                Some(PictureInformation {
                    width: 500,
                    height: 500,
                    color_depth: 24,
                    num_colors: 0,
                }),
            )
            .unwrap();
        flac_file
            .save_to_path(&temp_flac, lofty::config::WriteOptions::default())
            .expect("save flac with artwork");

        let extracted = extract_artwork(&temp_flac, ArtworkExtractOptions::default())
            .expect("extract artwork")
            .expect("artwork should be present");

        assert_eq!(extracted.summary.format, ArtworkFormat::Png);
        assert_eq!(extracted.summary.role, ArtworkRole::CoverFront);
        assert_eq!(
            extracted.summary.dimensions,
            Some(ArtworkDimensions {
                width: 500,
                height: 500
            })
        );
        assert_eq!(
            extracted.summary.description.as_deref(),
            Some("Front Cover")
        );
        assert_eq!(extracted.summary.picture_count, 2);

        let meta = crate::metadata::extract_metadata(&temp_flac).unwrap();
        assert!(meta.has_artwork);
        assert_eq!(meta.artwork, Some(extracted.summary.clone()));

        let prepared = extracted.prepare_for_terminal(40, 20);
        assert!(prepared.supports_image_protocol);
        assert_eq!(
            prepared.layout,
            TerminalArtworkLayout { cols: 40, rows: 20 }
        );
        assert_eq!(prepared.placeholder_lines.len(), 20);
        assert!(
            prepared
                .placeholder_lines
                .iter()
                .any(|l| l.contains("PNG 500x500"))
        );

        let _ = std::fs::remove_file(temp_flac);
    }

    #[test]
    fn rejects_oversized_artwork_before_allocating() {
        // Synthetic FLAC stream declaring a 10 MB PICTURE block (type 6) without allocating 10 MB
        let mut synthetic_flac = Vec::new();
        synthetic_flac.extend_from_slice(b"fLaC");
        // Block header: is_last=1 (0x80) | type=6 (PICTURE) => 0x86; length = 10_000_000 (0x989680)
        synthetic_flac.extend_from_slice(&[0x86, 0x98, 0x96, 0x80]);

        let mut cursor = Cursor::new(synthetic_flac);
        let err = extract_artwork_from_reader(
            &mut cursor,
            ArtworkExtractOptions::with_max_bytes(1024 * 1024),
        )
        .unwrap_err();

        assert!(matches!(
            err,
            MetadataError::ArtworkTooLarge {
                size_bytes: 10_000_000,
                max_bytes: 1_048_576
            }
        ));
    }

    #[test]
    fn prepares_terminal_layout_and_placeholder_for_missing_artwork() {
        let wide = TerminalArtworkLayout::fit_within(
            Some(ArtworkDimensions {
                width: 1000,
                height: 500,
            }),
            40,
            20,
        );
        assert_eq!(wide, TerminalArtworkLayout { cols: 40, rows: 10 });

        let lines = render_placeholder_lines(None, 16, 5);
        assert_eq!(lines.len(), 5);
        assert!(lines[2].contains("No Artwork"));
    }
}
