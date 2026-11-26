//! Integration tests for FLAC decoding across supported and error cases.

use std::fs;
use std::io::Cursor;
use std::path::{Path, PathBuf};

use termwave::audio::{AudioError, FlacDecoder};

fn fixture_path(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join(name)
}

fn decode_all(decoder: &mut FlacDecoder) -> Result<(usize, usize, f32), AudioError> {
    let mut packets = 0_usize;
    let mut total_samples = 0_usize;
    let mut peak = 0.0_f32;

    while let Some(samples) = decoder.decode_next()? {
        packets += 1;
        total_samples += samples.len();
        for &sample in samples {
            assert!(
                sample.is_finite() && (-1.0..=1.0).contains(&sample),
                "decoded PCM sample out of normalized range: {sample}"
            );
            peak = peak.max(sample.abs());
        }
    }

    // Calling decode_next again after EOF remains cleanly finished.
    assert!(decoder.decode_next()?.is_none());

    Ok((packets, total_samples, peak))
}

#[test]
fn decodes_16bit_44100hz_stereo_flac() {
    let mut decoder = FlacDecoder::open(fixture_path("sine_16bit_44100_stereo.flac"))
        .expect("open 16-bit/44.1kHz stereo fixture");

    let info = decoder.stream_info().clone();
    assert_eq!(info.codec, "FLAC");
    assert_eq!(info.sample_rate, 44_100);
    assert_eq!(info.bits_per_sample, Some(16));
    assert_eq!(info.channels, 2);
    assert_eq!(info.total_frames, Some(4_410));

    let (packets, total_samples, peak) = decode_all(&mut decoder).expect("decode full stream");
    assert!(packets >= 1);
    assert_eq!(decoder.decoded_frames(), 4_410);
    assert_eq!(total_samples, 4_410 * 2);
    assert!(peak > 0.05, "expected audible sine energy, got peak {peak}");
}

#[test]
fn decodes_24bit_48000hz_stereo_flac() {
    let mut decoder = FlacDecoder::open(fixture_path("sine_24bit_48000_stereo.flac"))
        .expect("open 24-bit/48kHz stereo fixture");

    let info = decoder.stream_info().clone();
    assert_eq!(info.sample_rate, 48_000);
    assert_eq!(info.bits_per_sample, Some(24));
    assert_eq!(info.channels, 2);
    assert_eq!(info.total_frames, Some(4_800));

    let (_, total_samples, peak) = decode_all(&mut decoder).expect("decode full stream");
    assert_eq!(decoder.decoded_frames(), 4_800);
    assert_eq!(total_samples, 4_800 * 2);
    assert!(peak > 0.05);
}

#[test]
fn decodes_24bit_96000hz_mono_flac() {
    let mut decoder = FlacDecoder::open(fixture_path("sine_24bit_96000_mono.flac"))
        .expect("open 24-bit/96kHz mono fixture");

    let info = decoder.stream_info().clone();
    assert_eq!(info.sample_rate, 96_000);
    assert_eq!(info.bits_per_sample, Some(24));
    assert_eq!(info.channels, 1);
    assert_eq!(info.total_frames, Some(9_600));

    let (_, total_samples, peak) = decode_all(&mut decoder).expect("decode full stream");
    assert_eq!(decoder.decoded_frames(), 9_600);
    assert_eq!(total_samples, 9_600);
    assert!(peak > 0.05);
}

#[test]
fn decodes_24bit_192000hz_stereo_flac_across_multiple_packets() {
    let mut decoder = FlacDecoder::open(fixture_path("sine_24bit_192000_stereo.flac"))
        .expect("open 24-bit/192kHz stereo fixture");

    let info = decoder.stream_info().clone();
    assert_eq!(info.sample_rate, 192_000);
    assert_eq!(info.bits_per_sample, Some(24));
    assert_eq!(info.channels, 2);
    assert_eq!(info.total_frames, Some(19_200));

    let (packets, total_samples, peak) = decode_all(&mut decoder).expect("decode full stream");
    assert!(packets >= 2, "expected multi-packet stream, got {packets}");
    assert_eq!(decoder.decoded_frames(), 19_200);
    assert_eq!(total_samples, 19_200 * 2);
    assert!(peak > 0.05);
}

#[test]
fn decodes_from_in_memory_media_source() {
    let bytes = fs::read(fixture_path("sine_16bit_44100_stereo.flac")).expect("read fixture bytes");
    let mut decoder = FlacDecoder::from_source(Box::new(Cursor::new(bytes)), Some("flac"))
        .expect("decode from memory cursor");

    let (_, total_samples, _) = decode_all(&mut decoder).expect("decode memory stream");
    assert_eq!(total_samples, 4_410 * 2);
}

#[test]
fn detects_truncated_flac_stream() {
    let full = fs::read(fixture_path("sine_24bit_192000_stereo.flac")).expect("read fixture bytes");

    // Truncation inside metadata header is detected while opening.
    let header_truncated = full[..128].to_vec();
    let header_err =
        FlacDecoder::from_source(Box::new(Cursor::new(header_truncated)), Some("flac"))
            .err()
            .expect("truncated header must fail");
    assert!(
        matches!(header_err, AudioError::DecodeError(_)),
        "expected DecodeError for truncated header, got {header_err:?}"
    );

    // Truncation mid-audio stream is detected while decoding frames.
    let audio_truncated = full[..full.len() - 512].to_vec();
    let mut decoder =
        FlacDecoder::from_source(Box::new(Cursor::new(audio_truncated)), Some("flac"))
            .expect("header still parses before truncated audio frames");

    let err = decode_all(&mut decoder).unwrap_err();
    assert!(
        matches!(err, AudioError::DecodeError(_)),
        "expected DecodeError for truncated audio stream, got {err:?}"
    );
}

#[test]
fn detects_corrupted_flac_frame_data() {
    let mut bytes =
        fs::read(fixture_path("sine_16bit_44100_stereo.flac")).expect("read fixture bytes");
    // Corrupt bytes near the end of the file inside encoded audio frame data.
    let start = bytes.len().saturating_sub(256);
    for byte in &mut bytes[start..start + 64] {
        *byte ^= 0xFF;
    }

    let mut decoder = FlacDecoder::from_source(Box::new(Cursor::new(bytes)), Some("flac"))
        .expect("container header remains intact");

    let err = decode_all(&mut decoder).unwrap_err();
    assert!(
        matches!(err, AudioError::DecodeError(_)),
        "expected DecodeError for corrupted FLAC frame, got {err:?}"
    );
}

#[test]
fn rejects_unsupported_non_flac_container() {
    // Minimal RIFF/WAVE header (unsupported because only FLAC decoding is enabled).
    let wav_header = b"RIFF\x24\x00\x00\x00WAVEfmt \x10\x00\x00\x00\x01\x00\x02\x00\x44\xAC\x00\x00\x10\xB1\x02\x00\x04\x00\x10\x00data\x00\x00\x00\x00";
    let err = FlacDecoder::from_source(Box::new(Cursor::new(wav_header.to_vec())), Some("wav"));
    assert!(err.is_err());
}

#[test]
fn detects_stream_truncated_before_audio_frames() {
    let full = fs::read(fixture_path("sine_16bit_44100_stereo.flac")).expect("read fixture bytes");
    // Truncate right around end of metadata blocks so no full frame can be decoded.
    let truncated = full[..350.min(full.len())].to_vec();
    if let Ok(mut decoder) =
        FlacDecoder::from_source(Box::new(Cursor::new(truncated)), Some("flac"))
    {
        let err = decode_all(&mut decoder).unwrap_err();
        assert!(matches!(err, AudioError::DecodeError(_)));
    }
}

struct NonSeekableSource {
    inner: Cursor<Vec<u8>>,
}

impl std::io::Read for NonSeekableSource {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        self.inner.read(buf)
    }
}

impl std::io::Seek for NonSeekableSource {
    fn seek(&mut self, _pos: std::io::SeekFrom) -> std::io::Result<u64> {
        Err(std::io::Error::new(
            std::io::ErrorKind::Unsupported,
            "stream does not support seeking",
        ))
    }
}

impl symphonia::core::io::MediaSource for NonSeekableSource {
    fn is_seekable(&self) -> bool {
        false
    }

    fn byte_len(&self) -> Option<u64> {
        None
    }
}

#[test]
fn respects_source_seeking_capabilities_when_non_seekable() {
    let bytes = fs::read(fixture_path("sine_16bit_44100_stereo.flac")).expect("read fixture bytes");
    let mut decoder = FlacDecoder::from_source(
        Box::new(NonSeekableSource {
            inner: Cursor::new(bytes),
        }),
        Some("flac"),
    )
    .expect("non-seekable FLAC stream should still open and decode sequentially");

    assert!(!decoder.is_seekable());
    assert!(!decoder.seek_capabilities().is_seekable);
    assert!(matches!(
        decoder.seek(std::time::Duration::from_millis(20)),
        Err(AudioError::SeekUnsupported(_))
    ));
    assert!(matches!(
        decoder.seek_forward(std::time::Duration::from_millis(10)),
        Err(AudioError::SeekUnsupported(_))
    ));
    assert!(matches!(
        decoder.seek_backward(std::time::Duration::from_millis(10)),
        Err(AudioError::SeekUnsupported(_))
    ));
}

#[test]
fn seeks_forward_backward_and_absolutely_across_flac_configurations() {
    let fixtures = [
        ("sine_16bit_44100_stereo.flac", 44_100_u32, 2_u16),
        ("sine_24bit_48000_stereo.flac", 48_000, 2),
        ("sine_24bit_96000_mono.flac", 96_000, 1),
        ("sine_24bit_192000_stereo.flac", 192_000, 2),
    ];

    for (name, sample_rate, channels) in fixtures {
        let mut decoder = FlacDecoder::open(fixture_path(name)).expect("open FLAC fixture");
        assert!(
            decoder.is_seekable(),
            "{name} local file should be seekable"
        );
        let duration = decoder
            .stream_info()
            .duration
            .expect("fixture has known duration");
        assert!(duration >= std::time::Duration::from_millis(50));

        // 1. Absolute seek to 50ms midpoint
        let landed_mid = decoder
            .seek(std::time::Duration::from_millis(50))
            .unwrap_or_else(|err| panic!("absolute seek failed for {name}: {err}"));
        assert!(
            landed_mid >= std::time::Duration::from_millis(45)
                && landed_mid <= std::time::Duration::from_millis(55),
            "unexpected midpoint seek landing for {name}: {landed_mid:?}"
        );

        let (_, samples_after_mid, _) = decode_all(&mut decoder)
            .unwrap_or_else(|err| panic!("post-seek decode failed for {name}: {err}"));
        let remaining_frames = samples_after_mid / usize::from(channels);
        let expected_half_frames = usize::try_from(sample_rate / 20).unwrap();
        // Expect roughly the remaining 50ms (+/- small boundary rounding)
        assert!(
            remaining_frames > 0 && remaining_frames <= expected_half_frames + 64,
            "unexpected remaining frame count for {name}: {remaining_frames} (expected ~{expected_half_frames})"
        );

        // 2. Seek backward 25ms from current end back into the stream and decode cleanly
        let landed_back = decoder
            .seek_backward(std::time::Duration::from_millis(25))
            .unwrap_or_else(|err| panic!("backward seek failed for {name}: {err}"));
        assert!(landed_back < landed_mid.saturating_add(std::time::Duration::from_millis(35)));
        let (_, rewound_samples, _) = decode_all(&mut decoder)
            .unwrap_or_else(|err| panic!("post-rewind decode failed for {name}: {err}"));
        assert!(rewound_samples > 0);

        // 3. Seek backward past zero clamps cleanly to Duration::ZERO
        let at_zero = decoder
            .seek_backward(std::time::Duration::from_secs(10))
            .expect("clamped backward seek should succeed");
        assert_eq!(at_zero, std::time::Duration::ZERO);
        let (_, full_samples, _) = decode_all(&mut decoder)
            .unwrap_or_else(|err| panic!("full decode after zero seek failed for {name}: {err}"));
        let full_frames = full_samples / usize::from(channels);
        assert_eq!(
            full_frames as u64,
            decoder.stream_info().total_frames.unwrap(),
            "seek to 0 should replay all frames cleanly for {name}"
        );

        // 4. Seek forward clamped at stream end finishes without error
        decoder
            .seek(std::time::Duration::ZERO)
            .expect("reset to zero");
        let at_end = decoder
            .seek_forward(std::time::Duration::from_secs(10))
            .expect("clamped forward seek should succeed");
        assert!(at_end <= duration);
        let (_, tail_samples, _) = decode_all(&mut decoder).unwrap_or_else(|err| {
            panic!("tail decode after forward clamp failed for {name}: {err}")
        });
        assert!(tail_samples <= usize::from(channels) * 2);
    }
}
