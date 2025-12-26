// SPDX-License-Identifier: GPL-3.0-only

//! Exposure picker module
//!
//! This module handles camera exposure controls:
//! - V4L2 exposure control queries
//! - iOS-style picker UI overlay
//! - Essential (mode + EV) and advanced control tiers
//!
//! Inspired by [cameractrls](https://github.com/soyersoyer/cameractrls).

pub mod types;
pub mod view;

pub use types::{
    AvailableExposureControls, ColorSettings, ControlRange, ExposureMode, ExposureSettings,
    MeteringMode,
};

use crate::backends::camera::v4l2_controls::{self, ControlInfo};
use tracing::{debug, info};

/// Query a range control and return ControlRange if available
fn query_range_control(device_path: &str, control_id: u32) -> ControlRange {
    if let Some(info) = v4l2_controls::query_control(device_path, control_id)
        && !info.is_disabled()
    {
        return ControlRange::new(info.minimum, info.maximum, info.step, info.default_value);
    }
    ControlRange::unavailable()
}

/// Query a boolean control and return if it's available
fn query_bool_control(device_path: &str, control_id: u32) -> bool {
    v4l2_controls::query_control(device_path, control_id)
        .map(|info| !info.is_disabled())
        .unwrap_or(false)
}

/// Query all available exposure controls for a camera device
pub fn query_exposure_controls(device_path: &str) -> AvailableExposureControls {
    info!(device_path, "Querying exposure controls");

    let mut controls = AvailableExposureControls {
        device_path: Some(device_path.to_string()),
        ..Default::default()
    };

    // Query exposure auto mode
    if let Some(info) =
        v4l2_controls::query_control(device_path, v4l2_controls::V4L2_CID_EXPOSURE_AUTO)
        && !info.is_disabled()
    {
        controls.has_exposure_auto = true;
        controls.exposure_auto_modes = query_exposure_modes(device_path, &info);
        debug!(device_path, modes = ?controls.exposure_auto_modes, "Exposure auto modes available");
    }

    // Query exposure compensation (EV bias)
    controls.exposure_bias =
        query_range_control(device_path, v4l2_controls::V4L2_CID_AUTO_EXPOSURE_BIAS);
    if controls.exposure_bias.available {
        debug!(device_path, range = ?controls.exposure_bias, "Exposure bias available");
    }

    // Query absolute exposure time
    controls.exposure_time =
        query_range_control(device_path, v4l2_controls::V4L2_CID_EXPOSURE_ABSOLUTE);
    if controls.exposure_time.available {
        debug!(device_path, range = ?controls.exposure_time, "Exposure time available");
    }

    // Query gain control (try multiple gain IDs)
    for gain_id in [
        v4l2_controls::V4L2_CID_GAIN,
        v4l2_controls::V4L2_CID_ANALOGUE_GAIN,
    ] {
        controls.gain = query_range_control(device_path, gain_id);
        if controls.gain.available {
            debug!(device_path, control_id = gain_id, range = ?controls.gain, "Gain available");
            break;
        }
    }

    // Query ISO sensitivity
    controls.iso = query_range_control(device_path, v4l2_controls::V4L2_CID_ISO_SENSITIVITY);
    if controls.iso.available {
        debug!(device_path, range = ?controls.iso, "ISO sensitivity available");
    }

    // Query exposure metering mode
    if let Some(info) =
        v4l2_controls::query_control(device_path, v4l2_controls::V4L2_CID_EXPOSURE_METERING)
        && !info.is_disabled()
    {
        controls.has_metering = true;
        controls.metering_modes = query_metering_modes(device_path, &info);
        debug!(device_path, modes = ?controls.metering_modes, "Metering modes available");
    }

    // Query boolean controls
    controls.has_auto_priority =
        query_bool_control(device_path, v4l2_controls::V4L2_CID_EXPOSURE_AUTO_PRIORITY);
    controls.has_autogain = query_bool_control(device_path, v4l2_controls::V4L2_CID_AUTOGAIN);
    controls.has_white_balance_auto =
        query_bool_control(device_path, v4l2_controls::V4L2_CID_AUTO_WHITE_BALANCE);
    controls.has_focus_auto = query_bool_control(device_path, v4l2_controls::V4L2_CID_FOCUS_AUTO);

    // Query range controls
    controls.backlight_compensation =
        query_range_control(device_path, v4l2_controls::V4L2_CID_BACKLIGHT_COMPENSATION);
    controls.contrast = query_range_control(device_path, v4l2_controls::V4L2_CID_CONTRAST);
    controls.saturation = query_range_control(device_path, v4l2_controls::V4L2_CID_SATURATION);
    controls.sharpness = query_range_control(device_path, v4l2_controls::V4L2_CID_SHARPNESS);
    controls.hue = query_range_control(device_path, v4l2_controls::V4L2_CID_HUE);
    controls.white_balance_temperature = query_range_control(
        device_path,
        v4l2_controls::V4L2_CID_WHITE_BALANCE_TEMPERATURE,
    );
    controls.focus = query_range_control(device_path, v4l2_controls::V4L2_CID_FOCUS_ABSOLUTE);

    // Query privacy control (hardware privacy switch)
    controls.has_privacy = query_bool_control(device_path, v4l2_controls::V4L2_CID_PRIVACY);

    // Query PTZ (pan/tilt/zoom) controls
    controls.pan_absolute = query_range_control(device_path, v4l2_controls::V4L2_CID_PAN_ABSOLUTE);
    controls.tilt_absolute =
        query_range_control(device_path, v4l2_controls::V4L2_CID_TILT_ABSOLUTE);
    controls.zoom_absolute =
        query_range_control(device_path, v4l2_controls::V4L2_CID_ZOOM_ABSOLUTE);
    controls.has_pan_relative =
        query_bool_control(device_path, v4l2_controls::V4L2_CID_PAN_RELATIVE);
    controls.has_tilt_relative =
        query_bool_control(device_path, v4l2_controls::V4L2_CID_TILT_RELATIVE);
    controls.has_pan_reset = query_bool_control(device_path, v4l2_controls::V4L2_CID_PAN_RESET);
    controls.has_tilt_reset = query_bool_control(device_path, v4l2_controls::V4L2_CID_TILT_RESET);

    if controls.has_any_ptz() {
        debug!(
            device_path,
            pan_abs = controls.pan_absolute.available,
            tilt_abs = controls.tilt_absolute.available,
            zoom_abs = controls.zoom_absolute.available,
            pan_rel = controls.has_pan_relative,
            tilt_rel = controls.has_tilt_relative,
            "PTZ controls available"
        );
    }

    info!(
        device_path,
        has_mode = controls.has_exposure_auto,
        has_ev = controls.exposure_bias.available,
        has_time = controls.exposure_time.available,
        has_gain = controls.gain.available,
        has_autogain = controls.has_autogain,
        has_iso = controls.iso.available,
        has_metering = controls.has_metering,
        has_auto_priority = controls.has_auto_priority,
        has_backlight = controls.backlight_compensation.available,
        has_contrast = controls.contrast.available,
        has_saturation = controls.saturation.available,
        has_sharpness = controls.sharpness.available,
        has_hue = controls.hue.available,
        has_wb_auto = controls.has_white_balance_auto,
        has_wb_temp = controls.white_balance_temperature.available,
        has_focus_auto = controls.has_focus_auto,
        has_focus_manual = controls.focus.available,
        has_privacy = controls.has_privacy,
        "Exposure controls query complete"
    );

    controls
}

/// Query available exposure modes from menu items
fn query_exposure_modes(device_path: &str, info: &ControlInfo) -> Vec<ExposureMode> {
    let menu_items = v4l2_controls::query_menu_items(
        device_path,
        v4l2_controls::V4L2_CID_EXPOSURE_AUTO,
        info.maximum,
    );
    let modes: Vec<_> = menu_items
        .iter()
        .map(|item| ExposureMode::from_v4l2_value(item.index))
        .collect();
    if modes.is_empty() {
        vec![ExposureMode::Auto, ExposureMode::Manual]
    } else {
        modes
    }
}

/// Query available metering modes from menu items
fn query_metering_modes(device_path: &str, info: &ControlInfo) -> Vec<MeteringMode> {
    let menu_items = v4l2_controls::query_menu_items(
        device_path,
        v4l2_controls::V4L2_CID_EXPOSURE_METERING,
        info.maximum,
    );
    let modes: Vec<_> = menu_items
        .iter()
        .map(|item| MeteringMode::from_v4l2_value(item.index))
        .collect();
    if modes.is_empty() {
        vec![
            MeteringMode::Average,
            MeteringMode::CenterWeighted,
            MeteringMode::Spot,
            MeteringMode::Matrix,
        ]
    } else {
        modes
    }
}

/// Helper to reset a control to its default value
fn reset_control_to_default(
    device_path: &str,
    control_id: u32,
    range: &ControlRange,
) -> Option<i32> {
    if !range.available {
        return None;
    }
    if let Err(e) = v4l2_controls::set_control(device_path, control_id, range.default) {
        tracing::warn!("Failed to reset control {control_id} to default: {e}");
    }
    Some(range.default)
}

/// Get current exposure settings from a camera device
///
/// This also sets the camera to auto exposure mode on initialization
/// to ensure a consistent starting state.
pub fn get_exposure_settings(
    device_path: &str,
    available: &AvailableExposureControls,
) -> ExposureSettings {
    let mut settings = ExposureSettings::default();

    // Set camera to aperture priority mode on initialization
    if available.has_exposure_auto {
        let aperture_priority_value = ExposureMode::AperturePriority.to_v4l2_value();
        if let Err(e) = v4l2_controls::set_control(
            device_path,
            v4l2_controls::V4L2_CID_EXPOSURE_AUTO,
            aperture_priority_value,
        ) {
            tracing::warn!("Failed to set aperture priority mode on init: {}", e);
        }
        settings.mode = ExposureMode::AperturePriority;
    }

    // Reset exposure compensation to default
    if let Some(default) = reset_control_to_default(
        device_path,
        v4l2_controls::V4L2_CID_AUTO_EXPOSURE_BIAS,
        &available.exposure_bias,
    ) {
        settings.exposure_compensation = default;
    }

    // Get exposure time (read current value, camera controls this in auto modes)
    if available.exposure_time.available {
        settings.exposure_time =
            v4l2_controls::get_control(device_path, v4l2_controls::V4L2_CID_EXPOSURE_ABSOLUTE);
    }

    // Get gain (read current value, camera controls this in auto modes)
    if available.gain.available {
        settings.gain = v4l2_controls::get_control(device_path, v4l2_controls::V4L2_CID_GAIN)
            .or_else(|| {
                v4l2_controls::get_control(device_path, v4l2_controls::V4L2_CID_ANALOGUE_GAIN)
            });
    }

    // Get ISO (read current value, camera controls this in auto modes)
    if available.iso.available {
        settings.iso =
            v4l2_controls::get_control(device_path, v4l2_controls::V4L2_CID_ISO_SENSITIVITY);
    }

    // Get metering mode
    if available.has_metering
        && let Some(value) =
            v4l2_controls::get_control(device_path, v4l2_controls::V4L2_CID_EXPOSURE_METERING)
    {
        settings.metering_mode = Some(MeteringMode::from_v4l2_value(value));
    }

    // Get auto priority
    if available.has_auto_priority
        && let Some(value) =
            v4l2_controls::get_control(device_path, v4l2_controls::V4L2_CID_EXPOSURE_AUTO_PRIORITY)
    {
        settings.auto_priority = Some(value != 0);
    }

    // Reset backlight compensation to default
    settings.backlight_compensation = reset_control_to_default(
        device_path,
        v4l2_controls::V4L2_CID_BACKLIGHT_COMPENSATION,
        &available.backlight_compensation,
    );

    // Get autogain
    if available.has_autogain
        && let Some(value) =
            v4l2_controls::get_control(device_path, v4l2_controls::V4L2_CID_AUTOGAIN)
    {
        settings.autogain = Some(value != 0);
    }

    // Get auto focus
    if available.has_focus_auto
        && let Some(value) =
            v4l2_controls::get_control(device_path, v4l2_controls::V4L2_CID_FOCUS_AUTO)
    {
        settings.focus_auto = Some(value != 0);
    }

    // Get focus position
    if available.focus.available {
        settings.focus_absolute =
            v4l2_controls::get_control(device_path, v4l2_controls::V4L2_CID_FOCUS_ABSOLUTE);
    }

    settings
}

/// Get current color settings from a camera device
///
/// This resets color controls to their defaults on initialization
/// to ensure a consistent starting state.
pub fn get_color_settings(
    device_path: &str,
    available: &AvailableExposureControls,
) -> ColorSettings {
    let mut settings = ColorSettings::default();

    // Reset color controls to defaults
    settings.contrast = reset_control_to_default(
        device_path,
        v4l2_controls::V4L2_CID_CONTRAST,
        &available.contrast,
    );
    settings.saturation = reset_control_to_default(
        device_path,
        v4l2_controls::V4L2_CID_SATURATION,
        &available.saturation,
    );
    settings.sharpness = reset_control_to_default(
        device_path,
        v4l2_controls::V4L2_CID_SHARPNESS,
        &available.sharpness,
    );
    settings.hue =
        reset_control_to_default(device_path, v4l2_controls::V4L2_CID_HUE, &available.hue);

    // Reset white balance to auto mode
    if available.has_white_balance_auto {
        if let Err(e) =
            v4l2_controls::set_control(device_path, v4l2_controls::V4L2_CID_AUTO_WHITE_BALANCE, 1)
        {
            tracing::warn!("Failed to set auto white balance to default: {}", e);
        }
        settings.white_balance_auto = Some(true);
    }

    // Get white balance temperature
    if available.white_balance_temperature.available {
        settings.white_balance_temperature = v4l2_controls::get_control(
            device_path,
            v4l2_controls::V4L2_CID_WHITE_BALANCE_TEMPERATURE,
        );
    }

    settings
}
