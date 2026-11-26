//! FLAC audio decoding powered by Symphonia.
//!
//! The decoder operates on [`MediaSource`] streams rather than filesystem paths
//! so local files and future network sources share the same decoding path.

use std::fs::File;
use std::io::{ErrorKind, Seek as _, SeekFrom};
use std::path::Path;
use std::time::Duration;

use symphonia::core::audio::SampleBuffer;
use symphonia::core::codecs::{CODEC_TYPE_FLAC, CodecParameters, Decoder, DecoderOptions};
use symphonia::core::errors::Error as SymphoniaError;
use symphonia::core::formats::{FormatOptions, FormatReader, SeekMode, SeekTo};
use symphonia::core::io::{MediaSource, MediaSourceStream};
use symphonia::core::meta::MetadataOptions;
use symphonia::core::probe::Hint;
use symphonia::core::units::Time;

use super::AudioError;

/// Maximum valid sample rate defined by the FLAC specification (655,350 Hz).
const MAX_FLAC_SAMPLE_RATE: u32 = 655_350;

/// Maximum valid channel count defined by the FLAC specification.
const MAX_FLAC_CHANNELS: usize = 8;

/// Default seek jump interval (`5 seconds`).
pub const DEFAULT_SEEK_STEP: Duration = Duration::from_secs(5);

/// Explicit seeking capability descriptor for an audio source or stream.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct SeekCapabilities {
    /// Whether the underlying source supports seeking operations.
    pub is_seekable: bool,
    /// Total stream duration, if known.
    pub duration: Option<Duration>,
}

/// Technical properties reported by the decoded source audio stream.
///
/// These describe the encoded source stream and are distinct from the output
/// representation (`f32` PCM) or any operating-system mixer sample rate.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AudioStreamInfo {
    /// Codec name (e.g. `"FLAC"`).
    pub codec: &'static str,
    /// Source sample rate in Hz (e.g. `44100`, `48000`, `96000`, `192000`).
    pub sample_rate: u32,
    /// Source bit depth per sample when reported by the container/codec.
    pub bits_per_sample: Option<u32>,
    /// Number of channels in the source stream.
    pub channels: u16,
    /// Total number of frames (samples per channel), if known.
    pub total_frames: Option<u64>,
    /// Stream duration derived from `total_frames` and `sample_rate`, if known.
    pub duration: Option<Duration>,
    /// Whether the underlying source supports random-access seeking.
    pub seekable: bool,
}

impl AudioStreamInfo {
    /// Placeholder stream metadata used when playback is stopped before decoding starts.
    pub(crate) fn uninitialized() -> Self {
        Self {
            codec: "FLAC",
            sample_rate: 0,
            bits_per_sample: None,
            channels: 0,
            total_frames: None,
            duration: None,
            seekable: false,
        }
    }
}

/// Streaming FLAC decoder backed by any [`MediaSource`].
pub struct FlacDecoder {
    format: Option<Box<dyn FormatReader>>,
    decoder: Box<dyn Decoder>,
    track_id: u32,
    extension_hint: Option<String>,
    info: AudioStreamInfo,
    sample_buf: Option<SampleBuffer<f32>>,
    decoded_frames: u64,
    pending_skip_frames: u64,
    finished: bool,
}

impl FlacDecoder {
    /// Opens a local FLAC file and prepares it for streaming decode.
    pub fn open(path: impl AsRef<Path>) -> Result<Self, AudioError> {
        let path = path.as_ref();
        let file = File::open(path).map_err(|err| match err.kind() {
            ErrorKind::NotFound => AudioError::FileNotFound {
                path: path.to_path_buf(),
            },
            ErrorKind::PermissionDenied => AudioError::PermissionDenied {
                path: path.to_path_buf(),
            },
            _ => AudioError::Io(err),
        })?;
        let ext = path.extension().and_then(|e| e.to_str());
        Self::from_source(Box::new(file), ext)
    }

    /// Creates a FLAC decoder from any [`MediaSource`] implementation.
    pub fn from_source(
        source: Box<dyn MediaSource>,
        extension_hint: Option<&str>,
    ) -> Result<Self, AudioError> {
        let is_seekable = source.is_seekable();
        let mss = MediaSourceStream::new(source, Default::default());
        let mut hint = Hint::new();
        if let Some(ext) = extension_hint {
            hint.with_extension(ext);
        } else {
            hint.with_extension("flac");
        }

        let probed = symphonia::default::get_probe()
            .format(
                &hint,
                mss,
                &FormatOptions::default(),
                &MetadataOptions::default(),
            )
            .map_err(map_probe_error)?;

        let format = probed.format;
        let track = select_flac_track(&*format)?;
        let track_id = track.id;
        let mut info = extract_stream_info(&track.codec_params)?;
        info.seekable = is_seekable;

        let decoder = symphonia::default::get_codecs()
            .make(&track.codec_params, &DecoderOptions::default())
            .map_err(map_decode_error)?;

        Ok(Self {
            format: Some(format),
            decoder,
            track_id,
            extension_hint: extension_hint.map(str::to_owned),
            info,
            sample_buf: None,
            decoded_frames: 0,
            pending_skip_frames: 0,
            finished: false,
        })
    }

    /// Re-opens the underlying FLAC demuxer from byte 0 so Symphonia resets any internal parser header state.
    fn reopen_reader_at_start(&mut self) -> Result<(), AudioError> {
        let format = self
            .format
            .take()
            .ok_or_else(|| AudioError::SeekError("FLAC demuxer unavailable".to_string()))?;
        let mut mss = format.into_inner();
        mss.seek(SeekFrom::Start(0)).map_err(AudioError::Io)?;

        let mut hint = Hint::new();
        if let Some(ext) = &self.extension_hint {
            hint.with_extension(ext);
        } else {
            hint.with_extension("flac");
        }

        let probed = symphonia::default::get_probe()
            .format(
                &hint,
                mss,
                &FormatOptions::default(),
                &MetadataOptions::default(),
            )
            .map_err(map_probe_error)?;

        let track = select_flac_track(&*probed.format)?;
        self.track_id = track.id;
        self.format = Some(probed.format);
        self.decoder.reset();
        Ok(())
    }

    /// Returns technical properties of the source FLAC stream.
    pub fn stream_info(&self) -> &AudioStreamInfo {
        &self.info
    }

    /// Returns explicit seeking capabilities for this stream.
    pub fn seek_capabilities(&self) -> SeekCapabilities {
        SeekCapabilities {
            is_seekable: self.info.seekable,
            duration: self.info.duration,
        }
    }

    /// Returns `true` if the underlying source supports seeking.
    pub fn is_seekable(&self) -> bool {
        self.info.seekable
    }

    /// Returns the total number of audio frames (samples per channel) decoded so far.
    pub fn decoded_frames(&self) -> u64 {
        self.decoded_frames
    }

    /// Returns the current logical stream position derived from frame progress and sample rate.
    pub fn current_position(&self) -> Duration {
        frames_to_duration(
            self.decoded_frames.saturating_add(self.pending_skip_frames),
            self.info.sample_rate,
        )
    }

    /// Seeks to an absolute `position` within the FLAC stream, returning the actual landed position.
    ///
    /// Returns [`AudioError::SeekUnsupported`] if the source is non-seekable.
    pub fn seek(&mut self, position: Duration) -> Result<Duration, AudioError> {
        if !self.info.seekable {
            return Err(AudioError::SeekUnsupported(
                "audio source does not support seeking".to_string(),
            ));
        }

        let clamped = match self.info.duration {
            Some(dur) if position > dur => dur,
            _ => position,
        };

        let sample_rate = self.info.sample_rate;
        let requested_frames = duration_to_frames(clamped, sample_rate);
        let (target_ts, extra_skip) = match self.info.total_frames {
            Some(total) if total > 0 && requested_frames >= total => (total - 1, 1_u64),
            _ => (requested_frames, 0_u64),
        };

        // Re-opening cleanly before seeking backward or after EOF clears Symphonia's
        // cached last-frame header state, which otherwise breaks Frame 0 re-reads.
        if requested_frames <= self.decoded_frames || self.finished {
            self.reopen_reader_at_start()?;
        }

        if target_ts == 0 {
            self.decoder.reset();
            self.finished = false;
            self.decoded_frames = 0;
            self.pending_skip_frames = extra_skip;
            return Ok(self.current_position());
        }

        let target_time = duration_to_symphonia_time(frames_to_duration(target_ts, sample_rate));
        let format = self
            .format
            .as_mut()
            .ok_or_else(|| AudioError::SeekError("FLAC demuxer unavailable".to_string()))?;
        let seeked = format
            .seek(
                SeekMode::Accurate,
                SeekTo::Time {
                    time: target_time,
                    track_id: Some(self.track_id),
                },
            )
            .map_err(map_seek_error)?;

        self.decoder.reset();
        self.finished = false;
        self.decoded_frames = seeked.actual_ts;
        self.pending_skip_frames = seeked
            .required_ts
            .saturating_sub(seeked.actual_ts)
            .saturating_add(extra_skip);

        Ok(self.current_position())
    }

    /// Seeks forward by `delta` relative to [`Self::current_position`], clamped at stream duration.
    pub fn seek_forward(&mut self, delta: Duration) -> Result<Duration, AudioError> {
        let target = self.current_position().saturating_add(delta);
        self.seek(target)
    }

    /// Seeks backward by `delta` relative to [`Self::current_position`], clamped at `Duration::ZERO`.
    pub fn seek_backward(&mut self, delta: Duration) -> Result<Duration, AudioError> {
        let target = self.current_position().saturating_sub(delta);
        self.seek(target)
    }

    /// Decodes the next FLAC frame and returns interleaved `f32` PCM samples.
    ///
    /// Returns `Ok(None)` when the stream reaches end-of-stream cleanly.
    pub fn decode_next(&mut self) -> Result<Option<&[f32]>, AudioError> {
        if self.finished {
            return Ok(None);
        }

        loop {
            let packet = {
                let format = self.format.as_mut().ok_or_else(|| {
                    AudioError::DecodeError("FLAC demuxer unavailable".to_string())
                })?;
                match format.next_packet() {
                    Ok(packet) => packet,
                    Err(SymphoniaError::IoError(err)) if err.kind() == ErrorKind::UnexpectedEof => {
                        self.finished = true;
                        self.verify_clean_eof()?;
                        return Ok(None);
                    }
                    Err(err) => {
                        self.finished = true;
                        return Err(map_decode_error(err));
                    }
                }
            };

            if packet.track_id() != self.track_id {
                continue;
            }

            let decoded = match self.decoder.decode(&packet) {
                Ok(decoded) => decoded,
                Err(err) => {
                    self.finished = true;
                    return Err(map_decode_error(err));
                }
            };

            let spec = *decoded.spec();
            let frames = decoded.frames();
            if spec.rate != self.info.sample_rate
                || spec.channels.count() != usize::from(self.info.channels)
            {
                self.finished = true;
                return Err(AudioError::UnsupportedConfiguration(format!(
                    "mid-stream audio parameter change ({} Hz, {} ch -> {} Hz, {} ch) is unsupported",
                    self.info.sample_rate,
                    self.info.channels,
                    spec.rate,
                    spec.channels.count()
                )));
            }

            if frames == 0 {
                continue;
            }

            let frames_u64 = frames as u64;
            self.decoded_frames = self.decoded_frames.saturating_add(frames_u64);

            if self.pending_skip_frames >= frames_u64 {
                self.pending_skip_frames -= frames_u64;
                continue;
            }

            let skip_frames = usize::try_from(self.pending_skip_frames).unwrap_or(0);
            self.pending_skip_frames = 0;

            let capacity = decoded.capacity() as u64;
            let needs_new_buf = match &self.sample_buf {
                Some(buf) => buf.capacity() < decoded.capacity(),
                None => true,
            };
            if needs_new_buf {
                self.sample_buf = Some(SampleBuffer::<f32>::new(capacity, spec));
            }

            let buf = self
                .sample_buf
                .as_mut()
                .expect("sample buffer initialized above");
            buf.copy_interleaved_ref(decoded);

            let channels = usize::from(self.info.channels);
            let skip_samples = skip_frames.saturating_mul(channels);
            let samples = &buf.samples()[skip_samples..];

            for &sample in samples {
                if !sample.is_finite() {
                    self.finished = true;
                    return Err(AudioError::DecodeError(
                        "corrupted audio data: non-finite sample encountered in decoded frame"
                            .to_string(),
                    ));
                }
            }

            return Ok(Some(samples));
        }
    }

    /// Verifies that end-of-stream occurred after decoding all expected frames.
    fn verify_clean_eof(&self) -> Result<(), AudioError> {
        if let Some(expected) = self.info.total_frames
            && expected > 0
            && self.decoded_frames < expected
        {
            return Err(AudioError::DecodeError(format!(
                "truncated FLAC stream: decoded {} of {} expected frames",
                self.decoded_frames, expected
            )));
        }

        if self.decoded_frames == 0 {
            return Err(AudioError::DecodeError(
                "FLAC stream ended with no decodable audio frames".to_string(),
            ));
        }

        Ok(())
    }
}

/// Selects the primary FLAC audio track from the probed container.
fn select_flac_track(
    format: &dyn FormatReader,
) -> Result<&symphonia::core::formats::Track, AudioError> {
    if let Some(track) = format
        .default_track()
        .filter(|track| track.codec_params.codec == CODEC_TYPE_FLAC)
    {
        return Ok(track);
    }

    format
        .tracks()
        .iter()
        .find(|track| track.codec_params.codec == CODEC_TYPE_FLAC)
        .ok_or_else(|| {
            AudioError::UnsupportedConfiguration("no FLAC audio track found in stream".to_string())
        })
}

/// Extracts and validates source stream parameters from [`CodecParameters`].
pub(crate) fn extract_stream_info(params: &CodecParameters) -> Result<AudioStreamInfo, AudioError> {
    if params.codec != CODEC_TYPE_FLAC {
        return Err(AudioError::UnsupportedConfiguration(
            "expected FLAC codec stream".to_string(),
        ));
    }

    let sample_rate = params.sample_rate.ok_or_else(|| {
        AudioError::UnsupportedConfiguration("FLAC stream is missing sample rate".to_string())
    })?;
    if sample_rate == 0 || sample_rate > MAX_FLAC_SAMPLE_RATE {
        return Err(AudioError::UnsupportedConfiguration(format!(
            "unsupported FLAC sample rate: {sample_rate} Hz"
        )));
    }

    let channels_layout = params.channels.ok_or_else(|| {
        AudioError::UnsupportedConfiguration("FLAC stream is missing channel layout".to_string())
    })?;
    let channel_count = channels_layout.count();
    if channel_count == 0 || channel_count > MAX_FLAC_CHANNELS {
        return Err(AudioError::UnsupportedConfiguration(format!(
            "unsupported FLAC channel count: {channel_count}"
        )));
    }
    let channels = u16::try_from(channel_count).map_err(|_| {
        AudioError::UnsupportedConfiguration(format!(
            "unsupported FLAC channel count: {channel_count}"
        ))
    })?;

    if let Some(bps) = params.bits_per_sample
        && !(4..=32).contains(&bps)
    {
        return Err(AudioError::UnsupportedConfiguration(format!(
            "unsupported FLAC bit depth: {bps}-bit"
        )));
    }

    let total_frames = params.n_frames;
    let duration = total_frames.map(|frames| frames_to_duration(frames, sample_rate));

    Ok(AudioStreamInfo {
        codec: "FLAC",
        sample_rate,
        bits_per_sample: params.bits_per_sample,
        channels,
        total_frames,
        duration,
        seekable: false,
    })
}

fn frames_to_duration(frames: u64, sample_rate: u32) -> Duration {
    if sample_rate == 0 {
        return Duration::ZERO;
    }
    let nanos = (u128::from(frames) * 1_000_000_000) / u128::from(sample_rate);
    let clamped = u64::try_from(nanos).unwrap_or(u64::MAX);
    Duration::from_nanos(clamped)
}

fn duration_to_frames(duration: Duration, sample_rate: u32) -> u64 {
    if sample_rate == 0 {
        return 0;
    }
    let frames = (duration.as_nanos() * u128::from(sample_rate)) / 1_000_000_000;
    u64::try_from(frames).unwrap_or(u64::MAX)
}

fn duration_to_symphonia_time(duration: Duration) -> Time {
    Time::new(
        duration.as_secs(),
        f64::from(duration.subsec_nanos()) / 1_000_000_000.0,
    )
}

fn map_seek_error(err: SymphoniaError) -> AudioError {
    match err {
        SymphoniaError::SeekError(symphonia::core::errors::SeekErrorKind::Unseekable) => {
            AudioError::SeekUnsupported("audio source does not support seeking".to_string())
        }
        SymphoniaError::SeekError(kind) => AudioError::SeekError(format!("seek failed: {kind:?}")),
        SymphoniaError::Unsupported(msg) => AudioError::SeekUnsupported(msg.to_string()),
        SymphoniaError::IoError(io_err) => AudioError::Io(io_err),
        other => AudioError::SeekError(format!("seek failure: {other}")),
    }
}

fn map_probe_error(err: SymphoniaError) -> AudioError {
    match err {
        SymphoniaError::Unsupported(msg) => AudioError::UnsupportedConfiguration(msg.to_string()),
        SymphoniaError::IoError(io_err) if io_err.kind() == ErrorKind::UnexpectedEof => {
            AudioError::DecodeError(
                "unexpected end of FLAC stream while reading header".to_string(),
            )
        }
        SymphoniaError::IoError(io_err) => AudioError::Io(io_err),
        other => AudioError::DecodeError(format!("failed to probe FLAC container: {other}")),
    }
}

fn map_decode_error(err: SymphoniaError) -> AudioError {
    match err {
        SymphoniaError::Unsupported(msg) => AudioError::UnsupportedConfiguration(msg.to_string()),
        SymphoniaError::DecodeError(msg) => {
            AudioError::DecodeError(format!("corrupted FLAC frame: {msg}"))
        }
        SymphoniaError::ResetRequired => {
            AudioError::DecodeError("FLAC stream reset required mid-stream".to_string())
        }
        SymphoniaError::IoError(io_err) if io_err.kind() == ErrorKind::UnexpectedEof => {
            AudioError::DecodeError("unexpected end of file: truncated FLAC stream".to_string())
        }
        SymphoniaError::IoError(io_err) if io_err.kind() == ErrorKind::PermissionDenied => {
            AudioError::PermissionDenied {
                path: std::path::PathBuf::from("audio stream"),
            }
        }
        SymphoniaError::IoError(io_err) => AudioError::Io(io_err),
        other => AudioError::DecodeError(format!("FLAC decode failure: {other}")),
    }
}

#[cfg(test)]
mod tests {
    use std::io::Cursor;

    use symphonia::core::audio::Channels;
    use symphonia::core::codecs::CODEC_TYPE_NULL;

    use super::*;

    fn valid_flac_params() -> CodecParameters {
        let mut params = CodecParameters::new();
        params
            .for_codec(CODEC_TYPE_FLAC)
            .with_sample_rate(48_000)
            .with_bits_per_sample(24)
            .with_channels(Channels::FRONT_LEFT | Channels::FRONT_RIGHT)
            .with_n_frames(48_000);
        params
    }

    #[test]
    fn extracts_valid_flac_stream_info_and_duration() {
        let info = extract_stream_info(&valid_flac_params()).expect("valid params should succeed");
        assert_eq!(info.codec, "FLAC");
        assert_eq!(info.sample_rate, 48_000);
        assert_eq!(info.bits_per_sample, Some(24));
        assert_eq!(info.channels, 2);
        assert_eq!(info.total_frames, Some(48_000));
        assert_eq!(info.duration, Some(Duration::from_secs(1)));
    }

    #[test]
    fn rejects_non_flac_codec_configuration() {
        let mut params = valid_flac_params();
        params.for_codec(CODEC_TYPE_NULL);

        let err = extract_stream_info(&params).unwrap_err();
        assert!(matches!(err, AudioError::UnsupportedConfiguration(_)));
    }

    #[test]
    fn rejects_missing_or_invalid_sample_rate() {
        let mut missing = CodecParameters::new();
        missing
            .for_codec(CODEC_TYPE_FLAC)
            .with_channels(Channels::FRONT_LEFT);
        assert!(matches!(
            extract_stream_info(&missing).unwrap_err(),
            AudioError::UnsupportedConfiguration(_)
        ));

        let mut zero = valid_flac_params();
        zero.with_sample_rate(0);
        assert!(matches!(
            extract_stream_info(&zero).unwrap_err(),
            AudioError::UnsupportedConfiguration(_)
        ));

        let mut too_high = valid_flac_params();
        too_high.with_sample_rate(MAX_FLAC_SAMPLE_RATE + 1);
        assert!(matches!(
            extract_stream_info(&too_high).unwrap_err(),
            AudioError::UnsupportedConfiguration(_)
        ));
    }

    #[test]
    fn rejects_missing_or_invalid_channel_configuration() {
        let mut missing = CodecParameters::new();
        missing.for_codec(CODEC_TYPE_FLAC).with_sample_rate(44_100);
        assert!(matches!(
            extract_stream_info(&missing).unwrap_err(),
            AudioError::UnsupportedConfiguration(_)
        ));

        let mut empty_ch = valid_flac_params();
        empty_ch.with_channels(Channels::empty());
        assert!(matches!(
            extract_stream_info(&empty_ch).unwrap_err(),
            AudioError::UnsupportedConfiguration(_)
        ));
    }

    #[test]
    fn rejects_unsupported_bit_depth() {
        for invalid_bps in [0, 3, 33, 64] {
            let mut params = valid_flac_params();
            params.with_bits_per_sample(invalid_bps);
            let err = extract_stream_info(&params).unwrap_err();
            assert!(matches!(err, AudioError::UnsupportedConfiguration(_)));
        }
    }

    #[test]
    fn from_source_rejects_non_flac_bytes() {
        let garbage = Cursor::new(b"not a valid flac stream at all".to_vec());
        let err = FlacDecoder::from_source(Box::new(garbage), Some("flac"));
        assert!(err.is_err());
    }

    #[test]
    fn converts_frames_and_durations_accurately() {
        assert_eq!(frames_to_duration(44_100, 44_100), Duration::from_secs(1));
        assert_eq!(duration_to_frames(Duration::from_secs(2), 96_000), 192_000);
        assert_eq!(frames_to_duration(100, 0), Duration::ZERO);
        assert_eq!(duration_to_frames(Duration::from_secs(1), 0), 0);
    }
}
