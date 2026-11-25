//! System audio output initialization and backend isolation.
//!
//! Encapsulates `rodio` and `cpal` behind a platform-agnostic interface so
//! higher-level playback and UI code never depend directly on OS audio APIs.

use std::fmt;

use rodio::cpal::traits::{DeviceTrait, HostTrait};
use rodio::{DeviceSinkBuilder, DeviceSinkError, MixerDeviceSink};

use super::AudioError;

/// Properties of the initialized system audio output device and stream.
///
/// These values reflect the active OS audio output sink (e.g. WASAPI on
/// Windows, ALSA/PipeWire on Linux), which may differ from the decoded
/// source file's sample rate or bit depth.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OutputDeviceInfo {
    /// Human-readable name of the selected output device.
    pub name: String,
    /// Active output stream sample rate in Hz.
    pub sample_rate: u32,
    /// Active output stream channel count.
    pub channels: u16,
    /// Active output sample format reported by the backend.
    pub sample_format: String,
}

impl fmt::Display for OutputDeviceInfo {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{} ({} Hz, {} ch, {})",
            self.name, self.sample_rate, self.channels, self.sample_format
        )
    }
}

/// Handle to an initialized system audio output stream.
pub struct AudioOutput {
    sink: MixerDeviceSink,
    device_info: OutputDeviceInfo,
}

impl fmt::Debug for AudioOutput {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AudioOutput")
            .field("device_info", &self.device_info)
            .finish_non_exhaustive()
    }
}

impl AudioOutput {
    /// Initializes system audio output using the default output device.
    ///
    /// Falls back through available host output devices if the default device
    /// cannot be opened directly, and returns [`AudioError::NoOutputDevice`]
    /// if no usable output device exists.
    pub fn open_default() -> Result<Self, AudioError> {
        let detected_name = detect_default_output_device().ok();

        let mut sink = DeviceSinkBuilder::open_default_sink().map_err(map_device_sink_error)?;
        sink.log_on_drop(false);

        let config = sink.config();
        let device_info = OutputDeviceInfo {
            name: detected_name.unwrap_or_else(|| "Default Output Device".to_string()),
            sample_rate: config.sample_rate().get(),
            channels: config.channel_count().get(),
            sample_format: format!("{:?}", config.sample_format()),
        };

        Ok(Self { sink, device_info })
    }

    /// Returns properties of the initialized audio output device.
    pub fn device_info(&self) -> &OutputDeviceInfo {
        &self.device_info
    }

    /// Returns a reference to the underlying mixer for audio playback scheduling.
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn mixer(&self) -> &rodio::mixer::Mixer {
        self.sink.mixer()
    }
}

/// Detects the system's default audio output device and returns its name.
pub fn detect_default_output_device() -> Result<String, AudioError> {
    let host = rodio::cpal::default_host();
    let device = host
        .default_output_device()
        .ok_or(AudioError::NoOutputDevice)?;

    let name = device
        .description()
        .map(|desc| desc.name().to_string())
        .unwrap_or_else(|_| "Default Output Device".to_string());

    Ok(name)
}

/// Maps `rodio::DeviceSinkError` variants into structured [`AudioError`] values.
fn map_device_sink_error(err: DeviceSinkError) -> AudioError {
    match err {
        DeviceSinkError::NoDevice => AudioError::NoOutputDevice,
        DeviceSinkError::DefaultSinkConfigError(cause) => {
            AudioError::Backend(format!("failed to query default output config: {cause}"))
        }
        DeviceSinkError::BuildError(cause) => {
            AudioError::Backend(format!("failed to open audio output stream: {cause}"))
        }
        DeviceSinkError::PlayError(cause) => {
            AudioError::Backend(format!("failed to start audio output stream: {cause}"))
        }
        DeviceSinkError::SupportedConfigsError(cause) => {
            AudioError::Backend(format!("failed to query supported output configs: {cause}"))
        }
        DeviceSinkError::UnsupportedSampleFormat => {
            AudioError::Backend("audio device uses an unsupported sample format".to_string())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_no_device_error_to_no_output_device() {
        let err = map_device_sink_error(DeviceSinkError::NoDevice);
        assert!(matches!(err, AudioError::NoOutputDevice));
        assert_eq!(err.to_string(), "no audio output device available");
    }

    #[test]
    fn maps_unsupported_sample_format_to_backend_error() {
        let err = map_device_sink_error(DeviceSinkError::UnsupportedSampleFormat);
        assert!(matches!(err, AudioError::Backend(_)));
        assert!(err.to_string().contains("unsupported sample format"));
    }

    #[test]
    fn formats_output_device_info_cleanly() {
        let info = OutputDeviceInfo {
            name: "Speakers (Realtek Audio)".to_string(),
            sample_rate: 48_000,
            channels: 2,
            sample_format: "F32".to_string(),
        };
        assert_eq!(
            info.to_string(),
            "Speakers (Realtek Audio) (48000 Hz, 2 ch, F32)"
        );
    }

    #[test]
    fn opens_default_system_audio_output_or_reports_no_device() {
        match AudioOutput::open_default() {
            Ok(output) => {
                let info = output.device_info();
                assert!(!info.name.is_empty());
                assert!(info.sample_rate > 0);
                assert!(info.channels > 0);
                assert!(!info.sample_format.is_empty());
                let _mixer = output.mixer();
                eprintln!("Detected audio output: {info}");
            }
            Err(AudioError::NoOutputDevice) => {
                // Acceptable in headless CI environments without audio hardware.
                eprintln!("No audio output device present in current environment");
            }
            Err(other) => {
                panic!("unexpected audio output initialization error: {other}");
            }
        }
    }
}
