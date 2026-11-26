//! Audio volume management, range validation, muting, and clipping prevention.
//!
//! Normalizes volume on a `[0.0, 1.0]` unity-capped amplitude scale so the audio
//! engine never introduces digital clipping above full-scale lossless PCM (`0 dB`).

use super::AudioError;

/// Minimum allowed normalized volume level (`0.0`, silent).
pub const MIN_VOLUME: f32 = 0.0;

/// Maximum allowed normalized volume level (`1.0`, unity gain without clipping).
pub const MAX_VOLUME: f32 = 1.0;

/// Default normalized volume level (`1.0`, unity gain).
pub const DEFAULT_VOLUME: f32 = 1.0;

/// Default volume adjustment step (`5%`).
pub const DEFAULT_VOLUME_STEP: f32 = 0.05;

/// Validates a normalized volume level within `[MIN_VOLUME, MAX_VOLUME]`.
///
/// Rejects `NaN`, infinite values, negative levels, and gain above `1.0` to
/// preserve signal integrity and prevent clipping.
pub fn validate_volume(level: f32) -> Result<f32, AudioError> {
    if !level.is_finite() {
        return Err(AudioError::InvalidVolume(
            "volume level must be a finite number".to_string(),
        ));
    }
    if !(MIN_VOLUME..=MAX_VOLUME).contains(&level) {
        return Err(AudioError::InvalidVolume(format!(
            "volume level {level:.3} is out of range ({MIN_VOLUME:.1}..={MAX_VOLUME:.1})"
        )));
    }
    if level == 0.0 { Ok(0.0) } else { Ok(level) }
}

/// Validates and converts an integer volume percentage (`0..=100`) to `[0.0, 1.0]`.
pub fn volume_from_percent(percent: u8) -> Result<f32, AudioError> {
    if percent > 100 {
        return Err(AudioError::InvalidVolume(format!(
            "volume percentage {percent}% exceeds maximum 100%"
        )));
    }
    Ok(f32::from(percent) / 100.0)
}

/// Ensures an audio sample stays strictly within full-scale `[-1.0, 1.0]`
/// and replaces non-finite values safely with silence (`0.0`).
#[inline]
pub fn prevent_clipping(sample: f32) -> f32 {
    if !sample.is_finite() {
        0.0
    } else {
        sample.clamp(-1.0, 1.0)
    }
}

/// Scales a PCM sample by a validated gain capped at unity (`1.0`) and prevents clipping.
#[inline]
pub fn scale_sample_safely(sample: f32, gain: f32) -> f32 {
    if !sample.is_finite() || !gain.is_finite() {
        return 0.0;
    }
    let safe_gain = gain.clamp(MIN_VOLUME, MAX_VOLUME);
    prevent_clipping(sample * safe_gain)
}

/// Stateful volume and mute model preserving user level independently of mute status.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct VolumeState {
    level: f32,
    muted: bool,
}

impl Default for VolumeState {
    fn default() -> Self {
        Self {
            level: DEFAULT_VOLUME,
            muted: false,
        }
    }
}

impl VolumeState {
    /// Creates a new unmuted [`VolumeState`] with a validated volume level.
    pub fn new(level: f32) -> Result<Self, AudioError> {
        Ok(Self {
            level: validate_volume(level)?,
            muted: false,
        })
    }

    /// Returns the configured volume level (`0.0..=1.0`), independent of mute status.
    pub fn level(&self) -> f32 {
        self.level
    }

    /// Returns the configured volume rounded to the nearest integer percentage (`0..=100`).
    pub fn percent(&self) -> u8 {
        let pct = (self.level * 100.0).round();
        pct.clamp(0.0, 100.0) as u8
    }

    /// Returns `true` if output is currently muted.
    pub fn is_muted(&self) -> bool {
        self.muted
    }

    /// Returns the effective output gain (`0.0` when muted, otherwise [`Self::level`]).
    pub fn effective_level(&self) -> f32 {
        if self.muted { 0.0 } else { self.level }
    }

    /// Sets the configured volume level after validating its range (`0.0..=1.0`).
    ///
    /// Preserves the current mute flag cleanly so adjusting volume while muted
    /// does not unexpectedly unmute playback.
    pub fn set_level(&mut self, level: f32) -> Result<f32, AudioError> {
        self.level = validate_volume(level)?;
        Ok(self.level)
    }

    /// Sets the configured volume using a percentage (`0..=100`).
    pub fn set_percent(&mut self, percent: u8) -> Result<f32, AudioError> {
        let normalized = volume_from_percent(percent)?;
        self.level = normalized;
        Ok(self.level)
    }

    /// Adjusts the volume by `delta`, clamping safely within `[MIN_VOLUME, MAX_VOLUME]`
    /// to prevent clipping.
    ///
    /// Returns [`AudioError::InvalidVolume`] if `delta` is `NaN` or infinite.
    pub fn adjust(&mut self, delta: f32) -> Result<f32, AudioError> {
        if !delta.is_finite() {
            return Err(AudioError::InvalidVolume(
                "volume adjustment delta must be finite".to_string(),
            ));
        }
        let adjusted = (self.level + delta).clamp(MIN_VOLUME, MAX_VOLUME);
        self.level = if adjusted == 0.0 { 0.0 } else { adjusted };
        Ok(self.level)
    }

    /// Mutes output while retaining the configured [`Self::level`].
    pub fn mute(&mut self) {
        self.muted = true;
    }

    /// Restores audio output at the retained [`Self::level`].
    pub fn unmute(&mut self) {
        self.muted = false;
    }

    /// Toggles mute status and returns the new muted state (`true` if muted).
    pub fn toggle_mute(&mut self) -> bool {
        self.muted = !self.muted;
        self.muted
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_to_unity_gain_unmuted() {
        let vol = VolumeState::default();
        assert_eq!(vol.level(), 1.0);
        assert_eq!(vol.percent(), 100);
        assert!(!vol.is_muted());
        assert_eq!(vol.effective_level(), 1.0);
    }

    #[test]
    fn validates_volume_ranges_and_rejects_out_of_bounds_or_non_finite() {
        assert_eq!(validate_volume(0.0).unwrap(), 0.0);
        assert_eq!(validate_volume(-0.0).unwrap(), 0.0);
        assert_eq!(validate_volume(0.5).unwrap(), 0.5);
        assert_eq!(validate_volume(1.0).unwrap(), 1.0);

        for invalid in [
            -0.01,
            -1.0,
            1.01,
            2.0,
            f32::NAN,
            f32::INFINITY,
            f32::NEG_INFINITY,
        ] {
            assert!(
                matches!(validate_volume(invalid), Err(AudioError::InvalidVolume(_))),
                "expected InvalidVolume for {invalid:?}"
            );
        }

        assert_eq!(volume_from_percent(0).unwrap(), 0.0);
        assert_eq!(volume_from_percent(50).unwrap(), 0.5);
        assert_eq!(volume_from_percent(100).unwrap(), 1.0);
        assert!(matches!(
            volume_from_percent(101),
            Err(AudioError::InvalidVolume(_))
        ));
    }

    #[test]
    fn adjusts_volume_with_clamping_and_preserves_mute_state() {
        let mut vol = VolumeState::default();
        vol.adjust(0.25).unwrap();
        assert_eq!(vol.level(), 1.0);

        vol.adjust(-0.4).unwrap();
        assert!((vol.level() - 0.6).abs() < 1e-6);
        assert_eq!(vol.percent(), 60);

        vol.mute();
        assert!(vol.is_muted());
        assert_eq!(vol.effective_level(), 0.0);

        // Adjusting while muted retains mute and updates target volume
        vol.set_percent(35).unwrap();
        assert!((vol.level() - 0.35).abs() < 1e-6);
        assert!(vol.is_muted());
        assert_eq!(vol.effective_level(), 0.0);

        vol.unmute();
        assert!(!vol.is_muted());
        assert!((vol.effective_level() - 0.35).abs() < 1e-6);

        assert!(vol.toggle_mute());
        assert!(!vol.toggle_mute());

        assert!(matches!(
            vol.adjust(f32::NAN),
            Err(AudioError::InvalidVolume(_))
        ));
    }

    #[test]
    fn prevents_clipping_and_handles_non_finite_samples() {
        assert_eq!(prevent_clipping(0.5), 0.5);
        assert_eq!(prevent_clipping(-0.75), -0.75);
        assert_eq!(prevent_clipping(1.25), 1.0);
        assert_eq!(prevent_clipping(-1.5), -1.0);
        assert_eq!(prevent_clipping(f32::NAN), 0.0);
        assert_eq!(prevent_clipping(f32::INFINITY), 0.0);

        assert_eq!(scale_sample_safely(0.8, 1.5), 0.8);
        assert_eq!(scale_sample_safely(1.2, 1.0), 1.0);
        assert_eq!(scale_sample_safely(-1.3, 0.9), -1.0);
        assert_eq!(scale_sample_safely(0.5, -0.5), 0.0);
        assert_eq!(scale_sample_safely(0.5, f32::NAN), 0.0);
    }
}
