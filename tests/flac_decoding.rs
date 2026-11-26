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
