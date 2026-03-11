// SPDX-License-Identifier: GPL-3.0-only

//! Exposure control types
//!
//! This module defines types for V4L2 exposure controls including
//! exposure mode, metering mode, and exposure settings.

use crate::backends::camera::v4l2_controls::{
    V4L2_EXPOSURE_APERTURE_PRIORITY, V4L2_EXPOSURE_AUTO, V4L2_EXPOSURE_MANUAL,
    V4L2_EXPOSURE_METERING_AVERAGE, V4L2_EXPOSURE_METERING_CENTER_WEIGHTED,
    V4L2_EXPOSURE_METERING_MATRIX, V4L2_EXPOSURE_METERING_SPOT, V4L2_EXPOSURE_SHUTTER_PRIORITY,
};
use serde::{Deserialize, Serialize};

/// V4L2 exposure auto modes
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum ExposureMode {
    /// Automatic exposure control
    #[default]
    Auto,
    /// Manual exposure control (user sets exposure time)
    Manual,
    /// Shutter priority (user sets exposure time, camera adjusts aperture)
    ShutterPriority,
    /// Aperture priority (user sets aperture, camera adjusts exposure time)
    AperturePriority,
}

impl ExposureMode {
    /// Convert to V4L2 exposure auto value
    pub fn to_v4l2_value(self) -> i32 {
        match self {
            ExposureMode::Auto => V4L2_EXPOSURE_AUTO,
            ExposureMode::Manual => V4L2_EXPOSURE_MANUAL,
            ExposureMode::ShutterPriority => V4L2_EXPOSURE_SHUTTER_PRIORITY,
            ExposureMode::AperturePriority => V4L2_EXPOSURE_APERTURE_PRIORITY,
        }
    }

    /// Convert from V4L2 exposure auto value
    pub fn from_v4l2_value(value: i32) -> Self {
        match value {
            V4L2_EXPOSURE_AUTO => ExposureMode::Auto,
            V4L2_EXPOSURE_MANUAL => ExposureMode::Manual,
            V4L2_EXPOSURE_SHUTTER_PRIORITY => ExposureMode::ShutterPriority,
            V4L2_EXPOSURE_APERTURE_PRIORITY => ExposureMode::AperturePriority,
            _ => ExposureMode::Auto,
        }
    }

    /// Get display name for UI
    pub fn display_name(self) -> &'static str {
        match self {
            ExposureMode::Auto => "Auto",
            ExposureMode::Manual => "Manual",
            ExposureMode::ShutterPriority => "Shutter Priority",
            ExposureMode::AperturePriority => "Aperture Priority",
        }
    }
}

/// V4L2 exposure metering modes
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum MeteringMode {
    /// Average metering across entire frame
    #[default]
    Average,
    /// Center-weighted metering
    CenterWeighted,
    /// Spot metering on center point
    Spot,
    /// Matrix/evaluative metering
    Matrix,
}

impl MeteringMode {
    /// Convert to V4L2 metering value
    pub fn to_v4l2_value(self) -> i32 {
        match self {
            MeteringMode::Average => V4L2_EXPOSURE_METERING_AVERAGE,
            MeteringMode::CenterWeighted => V4L2_EXPOSURE_METERING_CENTER_WEIGHTED,
            MeteringMode::Spot => V4L2_EXPOSURE_METERING_SPOT,
            MeteringMode::Matrix => V4L2_EXPOSURE_METERING_MATRIX,
        }
    }

    /// Convert from V4L2 metering value
    pub fn from_v4l2_value(value: i32) -> Self {
        match value {
            V4L2_EXPOSURE_METERING_AVERAGE => MeteringMode::Average,
            V4L2_EXPOSURE_METERING_CENTER_WEIGHTED => MeteringMode::CenterWeighted,
            V4L2_EXPOSURE_METERING_SPOT => MeteringMode::Spot,
            V4L2_EXPOSURE_METERING_MATRIX => MeteringMode::Matrix,
            _ => MeteringMode::Average,
        }
    }

    /// Get display name for UI
    pub fn display_name(self) -> &'static str {
        match self {
            MeteringMode::Average => "Average",
            MeteringMode::CenterWeighted => "Center",
            MeteringMode::Spot => "Spot",
            MeteringMode::Matrix => "Matrix",
        }
    }
}

/// Current exposure settings for a camera
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExposureSettings {
    /// Exposure mode (auto/manual/priority modes)
    pub mode: ExposureMode,
    /// Exposure compensation in 0.001 EV units (e.g., 1000 = +1 EV)
    pub exposure_compensation: i32,
    /// Absolute exposure time in 100µs units (only when mode is Manual/ShutterPriority)
    pub exposure_time: Option<i32>,
    /// Gain value
    pub gain: Option<i32>,
    /// Automatic gain control enabled
    pub autogain: Option<bool>,
    /// ISO sensitivity value
    pub iso: Option<i32>,
    /// Exposure metering mode
    pub metering_mode: Option<MeteringMode>,
    /// Allow frame rate variation during auto exposure
    pub auto_priority: Option<bool>,
    /// Backlight compensation value
    pub backlight_compensation: Option<i32>,
    /// Auto focus enabled
    pub focus_auto: Option<bool>,
    /// Manual focus position (absolute)
    pub focus_absolute: Option<i32>,
}

/// Current color/image adjustment settings for a camera
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ColorSettings {
    /// Contrast level
    pub contrast: Option<i32>,
    /// Saturation level
    pub saturation: Option<i32>,
    /// Sharpness level
    pub sharpness: Option<i32>,
    /// Hue adjustment
    pub hue: Option<i32>,
    /// Automatic white balance enabled
    pub white_balance_auto: Option<bool>,
    /// White balance color temperature in Kelvin
    pub white_balance_temperature: Option<i32>,
}

/// Describes the range and availability of a V4L2 control
#[derive(Debug, Clone, Default)]
pub struct ControlRange {
    /// Whether this control is available
    pub available: bool,
    /// Minimum value
    pub min: i32,
    /// Maximum value
    pub max: i32,
    /// Step size (defaults to 1)
    pub step: i32,
    /// Default value
    pub default: i32,
}

impl ControlRange {
    /// Create a new available control range
    pub fn new(min: i32, max: i32, step: i32, default: i32) -> Self {
        Self {
            available: true,
            min,
            max,
            step: step.max(1),
            default,
        }
    }

    /// Create an unavailable control
    pub fn unavailable() -> Self {
        Self::default()
    }
}

/// Describes which exposure controls are available for a camera and their ranges
#[derive(Debug, Clone, Default)]
pub struct AvailableExposureControls {
    /// Device path for V4L2 access
    pub device_path: Option<String>,

    // === Exposure Mode ===
    /// Whether exposure auto mode control is available
    pub has_exposure_auto: bool,
    /// Available exposure modes (subset of Auto, Manual, ShutterPriority, AperturePriority)
    pub exposure_auto_modes: Vec<ExposureMode>,

    // === Exposure Compensation (EV Bias) ===
    pub exposure_bias: ControlRange,

    // === Absolute Exposure Time ===
    pub exposure_time: ControlRange,

    // === Gain ===
    pub gain: ControlRange,

    // === Auto Gain ===
    pub has_autogain: bool,

    // === ISO ===
    pub iso: ControlRange,

    // === Metering Mode ===
    pub has_metering: bool,
    /// Available metering modes
    pub metering_modes: Vec<MeteringMode>,

    // === Auto Priority ===
    pub has_auto_priority: bool,

    // === Backlight Compensation ===
    pub backlight_compensation: ControlRange,

    // === Contrast ===
    pub contrast: ControlRange,

    // === Saturation ===
    pub saturation: ControlRange,

    // === Sharpness ===
    pub sharpness: ControlRange,

    // === Hue ===
    pub hue: ControlRange,

    // === White Balance ===
    pub has_white_balance_auto: bool,
    pub white_balance_temperature: ControlRange,

    // === Focus ===
    pub has_focus_auto: bool,
    pub focus: ControlRange,
    /// Separate V4L2 subdevice path for focus control (lens actuator)
    pub focus_device_path: Option<String>,

    // === Privacy ===
    /// Whether privacy control is available (hardware privacy switch)
    pub has_privacy: bool,

    // === PTZ (Pan/Tilt/Zoom) Controls ===
    /// Pan absolute position range
    pub pan_absolute: ControlRange,
    /// Tilt absolute position range
    pub tilt_absolute: ControlRange,
    /// Zoom absolute position range
    pub zoom_absolute: ControlRange,
    /// Whether pan relative control is available
    pub has_pan_relative: bool,
    /// Whether tilt relative control is available
    pub has_tilt_relative: bool,
    /// Whether pan reset control is available
    pub has_pan_reset: bool,
    /// Whether tilt reset control is available
    pub has_tilt_reset: bool,
}

impl AvailableExposureControls {
    /// Check if any essential controls (mode or EV compensation) are available
    pub fn has_any_essential(&self) -> bool {
        self.has_exposure_auto || self.exposure_bias.available
    }

    /// Check if any advanced exposure controls are available
    pub fn has_any_advanced(&self) -> bool {
        self.exposure_time.available
            || self.gain.available
            || self.has_autogain
            || self.iso.available
            || self.has_metering
            || self.has_auto_priority
    }

    /// Check if any image adjustment controls are available
    pub fn has_any_image_controls(&self) -> bool {
        self.contrast.available
            || self.saturation.available
            || self.sharpness.available
            || self.hue.available
    }

    /// Check if any white balance controls are available
    pub fn has_any_white_balance(&self) -> bool {
        self.has_white_balance_auto || self.white_balance_temperature.available
    }

    /// Check if any focus controls are available
    pub fn has_any_focus(&self) -> bool {
        self.has_focus_auto || self.focus.available
    }

    /// Check if any PTZ (pan/tilt/zoom) controls are available
    pub fn has_any_ptz(&self) -> bool {
        self.pan_absolute.available
            || self.tilt_absolute.available
            || self.zoom_absolute.available
            || self.has_pan_relative
            || self.has_tilt_relative
    }

    /// Check if any exposure controls are available at all
    pub fn has_any(&self) -> bool {
        self.has_any_essential()
            || self.has_any_advanced()
            || self.has_any_image_controls()
            || self.has_any_white_balance()
            || self.has_any_focus()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_exposure_mode_roundtrip() {
        for mode in [
            ExposureMode::Auto,
            ExposureMode::Manual,
            ExposureMode::ShutterPriority,
            ExposureMode::AperturePriority,
        ] {
            assert_eq!(ExposureMode::from_v4l2_value(mode.to_v4l2_value()), mode);
        }
    }

    #[test]
    fn test_metering_mode_roundtrip() {
        for mode in [
            MeteringMode::Average,
            MeteringMode::CenterWeighted,
            MeteringMode::Spot,
            MeteringMode::Matrix,
        ] {
            assert_eq!(MeteringMode::from_v4l2_value(mode.to_v4l2_value()), mode);
        }
    }

    #[test]
    fn test_available_controls_checks() {
        let mut controls = AvailableExposureControls::default();
        assert!(!controls.has_any());
        assert!(!controls.has_any_essential());
        assert!(!controls.has_any_advanced());

        controls.has_exposure_auto = true;
        assert!(controls.has_any());
        assert!(controls.has_any_essential());
        assert!(!controls.has_any_advanced());

        controls.gain.available = true;
        assert!(controls.has_any_advanced());
    }

    #[test]
    fn test_control_range() {
        let range = ControlRange::new(0, 100, 1, 50);
        assert!(range.available);
        assert_eq!(range.min, 0);
        assert_eq!(range.max, 100);
        assert_eq!(range.step, 1);
        assert_eq!(range.default, 50);

        let unavailable = ControlRange::unavailable();
        assert!(!unavailable.available);
    }
}
