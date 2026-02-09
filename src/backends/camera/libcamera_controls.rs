// SPDX-License-Identifier: GPL-3.0-only

//! libcamera control adapter
//!
//! This module provides proper camera control integration using libcamera-rs,
//! replacing direct V4L2 ioctl calls with the libcamera control system.
//!
//! Benefits over V4L2 ioctl:
//! - Proper ISP coordination (3A algorithms work correctly)
//! - Per-frame control application with metadata feedback
//! - Access to libcamera-specific controls (AF zones, FrameDurationLimits)
//! - Correct mode transition protocol (wait for metadata confirmation)

use super::types::{AeState, AfState, FrameMetadata};
use libcamera::{
    camera_manager::CameraManager,
    control::ControlList,
    control_value::ControlValue,
    controls::ControlId,
    utils::UniquePtr,
};
use std::collections::HashMap;
use tracing::{debug, info, warn};

/// Exposure mode for libcamera
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExposureMode {
    /// Automatic exposure (AE enabled)
    Auto,
    /// Manual exposure (AE disabled, user sets exposure time and gain)
    Manual,
}

/// Autofocus mode
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AfMode {
    /// Autofocus disabled
    Manual,
    /// Continuous autofocus
    Continuous,
    /// Single-shot autofocus
    Auto,
}

/// Auto white balance mode
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AwbMode {
    Auto,
    Incandescent,
    Tungsten,
    Fluorescent,
    Indoor,
    Daylight,
    Cloudy,
    Custom,
}

/// Information about an available control
#[derive(Debug, Clone)]
pub struct ControlInfo {
    /// Control ID
    pub id: u32,
    /// Control name
    pub name: String,
    /// Minimum value (for numeric controls)
    pub min: Option<i64>,
    /// Maximum value (for numeric controls)
    pub max: Option<i64>,
    /// Default value
    pub default: Option<i64>,
    /// Whether this is an input control (can be set)
    pub is_input: bool,
    /// Whether this is an output control (returned in metadata)
    pub is_output: bool,
}

/// libcamera control manager
///
/// Provides a high-level interface for setting camera controls and
/// extracting metadata from completed frames.
pub struct LibcameraControlManager {
    /// Camera manager (kept alive for lifetime)
    _manager: CameraManager,
    /// Camera ID for reconnection
    camera_id: String,
    /// Available controls for the camera
    available_controls: HashMap<u32, ControlInfo>,
    /// Pending controls to apply on next request
    pending_controls: UniquePtr<ControlList>,
    /// Current exposure mode
    exposure_mode: ExposureMode,
    /// Whether a mode transition is pending
    mode_transition_pending: bool,
}

impl LibcameraControlManager {
    /// Create a new control manager for the specified camera
    ///
    /// # Arguments
    /// * `camera_id` - The libcamera camera ID (from CameraDevice.path)
    ///
    /// # Returns
    /// A new control manager, or an error if the camera cannot be opened
    pub fn new(camera_id: &str) -> Result<Self, String> {
        let manager = CameraManager::new()
            .map_err(|e| format!("Failed to create camera manager: {e:?}"))?;

        let cameras = manager.cameras();
        let cam = cameras
            .iter()
            .find(|c| c.id() == camera_id)
            .ok_or_else(|| format!("Camera not found: {camera_id}"))?;

        // Enumerate available controls
        let mut available_controls = HashMap::new();
        for (id, info) in cam.controls() {
            if let Ok(control_id) = ControlId::try_from(id) {
                let min = extract_numeric_bound(&info.min());
                let max = extract_numeric_bound(&info.max());
                let default = extract_numeric_bound(&info.def());

                debug!(
                    id,
                    name = ?control_id,
                    min = ?min,
                    max = ?max,
                    "Found libcamera control"
                );

                let control_info = ControlInfo {
                    id,
                    name: format!("{:?}", control_id),
                    min,
                    max,
                    default,
                    is_input: true,  // Controls on camera are input controls
                    is_output: true, // Most controls are also returned in metadata
                };
                available_controls.insert(id, control_info);
            }
        }

        info!(
            camera_id,
            control_count = available_controls.len(),
            "Created libcamera control manager"
        );

        Ok(Self {
            _manager: manager,
            camera_id: camera_id.to_string(),
            available_controls,
            pending_controls: ControlList::new(),
            exposure_mode: ExposureMode::Auto,
            mode_transition_pending: false,
        })
    }

    /// Get available controls for the camera
    pub fn available_controls(&self) -> &HashMap<u32, ControlInfo> {
        &self.available_controls
    }

    /// Check if a specific control is available
    pub fn has_control(&self, control_id: u32) -> bool {
        self.available_controls.contains_key(&control_id)
    }

    /// Set exposure mode (Auto or Manual)
    ///
    /// When switching from Auto to Manual, the mode transition protocol is:
    /// 1. Set AeEnable=false
    /// 2. Wait for metadata to confirm mode change
    /// 3. Then set ExposureTime/AnalogueGain values
    pub fn set_exposure_mode(&mut self, mode: ExposureMode) -> Result<(), String> {
        if mode == self.exposure_mode {
            return Ok(());
        }

        
        // Use ControlId enum value as u32
        let ae_enable_id = ControlId::AeEnable as u32;

        match mode {
            ExposureMode::Auto => {
                // Enable AE
                self.pending_controls
                    .set_raw(ae_enable_id, ControlValue::from(true))
                    .map_err(|e| format!("Failed to set AeEnable: {e:?}"))?;
                self.mode_transition_pending = false;
            }
            ExposureMode::Manual => {
                // Disable AE - this requires waiting for confirmation
                self.pending_controls
                    .set_raw(ae_enable_id, ControlValue::from(false))
                    .map_err(|e| format!("Failed to set AeEnable: {e:?}"))?;
                self.mode_transition_pending = true;
            }
        }

        self.exposure_mode = mode;
        info!(mode = ?mode, pending_transition = self.mode_transition_pending, "Set exposure mode");
        Ok(())
    }

    /// Set exposure time (microseconds)
    ///
    /// Only effective when exposure mode is Manual.
    pub fn set_exposure_time(&mut self, microseconds: u64) -> Result<(), String> {
        if self.exposure_mode != ExposureMode::Manual {
            warn!("Setting exposure time in Auto mode - may be ignored");
        }

                self.pending_controls
            .set_raw(ControlId::ExposureTime as u32, ControlValue::from(microseconds as i32))
            .map_err(|e| format!("Failed to set ExposureTime: {e:?}"))?;

        debug!(microseconds, "Set exposure time");
        Ok(())
    }

    /// Set analogue gain
    ///
    /// Only effective when exposure mode is Manual.
    pub fn set_analogue_gain(&mut self, gain: f32) -> Result<(), String> {
        if self.exposure_mode != ExposureMode::Manual {
            warn!("Setting analogue gain in Auto mode - may be ignored");
        }

                self.pending_controls
            .set_raw(ControlId::AnalogueGain as u32, ControlValue::from(gain))
            .map_err(|e| format!("Failed to set AnalogueGain: {e:?}"))?;

        debug!(gain, "Set analogue gain");
        Ok(())
    }

    /// Set digital gain
    pub fn set_digital_gain(&mut self, gain: f32) -> Result<(), String> {
                self.pending_controls
            .set_raw(ControlId::DigitalGain as u32, ControlValue::from(gain))
            .map_err(|e| format!("Failed to set DigitalGain: {e:?}"))?;

        debug!(gain, "Set digital gain");
        Ok(())
    }

    /// Set exposure compensation (EV, typically -2.0 to +2.0)
    pub fn set_exposure_value(&mut self, ev: f32) -> Result<(), String> {
                self.pending_controls
            .set_raw(ControlId::ExposureValue as u32, ControlValue::from(ev))
            .map_err(|e| format!("Failed to set ExposureValue: {e:?}"))?;

        debug!(ev, "Set exposure value");
        Ok(())
    }

    /// Set autofocus mode
    pub fn set_af_mode(&mut self, mode: AfMode) -> Result<(), String> {
        let value: i32 = match mode {
            AfMode::Manual => 0,
            AfMode::Auto => 1,
            AfMode::Continuous => 2,
        };

                self.pending_controls
            .set_raw(ControlId::AfMode as u32, ControlValue::from(value))
            .map_err(|e| format!("Failed to set AfMode: {e:?}"))?;

        debug!(mode = ?mode, "Set AF mode");
        Ok(())
    }

    /// Set manual lens position (for manual focus)
    pub fn set_lens_position(&mut self, position: f32) -> Result<(), String> {
                self.pending_controls
            .set_raw(ControlId::LensPosition as u32, ControlValue::from(position))
            .map_err(|e| format!("Failed to set LensPosition: {e:?}"))?;

        debug!(position, "Set lens position");
        Ok(())
    }

    /// Trigger single-shot autofocus
    pub fn trigger_autofocus(&mut self) -> Result<(), String> {
                // AfTrigger::Start = 1
        self.pending_controls
            .set_raw(ControlId::AfTrigger as u32, ControlValue::from(1i32))
            .map_err(|e| format!("Failed to trigger AF: {e:?}"))?;

        debug!("Triggered autofocus");
        Ok(())
    }

    /// Set auto white balance mode
    pub fn set_awb_mode(&mut self, mode: AwbMode) -> Result<(), String> {
        let value: i32 = match mode {
            AwbMode::Auto => 0,
            AwbMode::Incandescent => 1,
            AwbMode::Tungsten => 2,
            AwbMode::Fluorescent => 3,
            AwbMode::Indoor => 4,
            AwbMode::Daylight => 5,
            AwbMode::Cloudy => 6,
            AwbMode::Custom => 7,
        };

                self.pending_controls
            .set_raw(ControlId::AwbMode as u32, ControlValue::from(value))
            .map_err(|e| format!("Failed to set AwbMode: {e:?}"))?;

        debug!(mode = ?mode, "Set AWB mode");
        Ok(())
    }

    /// Set manual color temperature (Kelvin)
    pub fn set_colour_temperature(&mut self, kelvin: u32) -> Result<(), String> {
                self.pending_controls
            .set_raw(ControlId::ColourTemperature as u32, ControlValue::from(kelvin as i32))
            .map_err(|e| format!("Failed to set ColourTemperature: {e:?}"))?;

        debug!(kelvin, "Set colour temperature");
        Ok(())
    }

    /// Set brightness (-1.0 to 1.0)
    pub fn set_brightness(&mut self, brightness: f32) -> Result<(), String> {
                self.pending_controls
            .set_raw(ControlId::Brightness as u32, ControlValue::from(brightness))
            .map_err(|e| format!("Failed to set Brightness: {e:?}"))?;

        debug!(brightness, "Set brightness");
        Ok(())
    }

    /// Set contrast (0.0 to 2.0, 1.0 = normal)
    pub fn set_contrast(&mut self, contrast: f32) -> Result<(), String> {
                self.pending_controls
            .set_raw(ControlId::Contrast as u32, ControlValue::from(contrast))
            .map_err(|e| format!("Failed to set Contrast: {e:?}"))?;

        debug!(contrast, "Set contrast");
        Ok(())
    }

    /// Set saturation (0.0 to 2.0, 1.0 = normal)
    pub fn set_saturation(&mut self, saturation: f32) -> Result<(), String> {
                self.pending_controls
            .set_raw(ControlId::Saturation as u32, ControlValue::from(saturation))
            .map_err(|e| format!("Failed to set Saturation: {e:?}"))?;

        debug!(saturation, "Set saturation");
        Ok(())
    }

    /// Set sharpness (0.0 to 2.0, 1.0 = normal)
    pub fn set_sharpness(&mut self, sharpness: f32) -> Result<(), String> {
                self.pending_controls
            .set_raw(ControlId::Sharpness as u32, ControlValue::from(sharpness))
            .map_err(|e| format!("Failed to set Sharpness: {e:?}"))?;

        debug!(sharpness, "Set sharpness");
        Ok(())
    }

    /// Set frame duration limits (min, max in microseconds)
    ///
    /// This is useful for:
    /// - Allowing longer exposures in low light (increase max)
    /// - Maintaining consistent video framerate (set min = max)
    pub fn set_frame_duration_limits(
        &mut self,
        min_us: u64,
        max_us: u64,
    ) -> Result<(), String> {
        
        // FrameDurationLimits is an array of [min, max]
        let limits: Vec<i64> = vec![min_us as i64, max_us as i64];
        self.pending_controls
            .set_raw(ControlId::FrameDurationLimits as u32, ControlValue::from(limits))
            .map_err(|e| format!("Failed to set FrameDurationLimits: {e:?}"))?;

        debug!(min_us, max_us, "Set frame duration limits");
        Ok(())
    }

    /// Get pending controls to merge into a request
    pub fn take_pending_controls(&mut self) -> UniquePtr<ControlList> {
        std::mem::replace(&mut self.pending_controls, ControlList::new())
    }

    /// Check if a mode transition is pending
    ///
    /// When true, the caller should wait for metadata confirmation before
    /// setting exposure/gain values.
    pub fn is_mode_transition_pending(&self) -> bool {
        self.mode_transition_pending
    }

    /// Called when metadata confirms mode transition is complete
    pub fn confirm_mode_transition(&mut self, metadata: &FrameMetadata) {
        if !self.mode_transition_pending {
            return;
        }

        // Check if AE state indicates mode change is complete
        match self.exposure_mode {
            ExposureMode::Manual => {
                // In manual mode, AE should be inactive
                if metadata.ae_state == Some(AeState::Inactive) {
                    self.mode_transition_pending = false;
                    debug!("Mode transition to Manual confirmed");
                }
            }
            ExposureMode::Auto => {
                // In auto mode, AE should be active (searching or converged)
                if matches!(
                    metadata.ae_state,
                    Some(AeState::Searching) | Some(AeState::Converged)
                ) {
                    self.mode_transition_pending = false;
                    debug!("Mode transition to Auto confirmed");
                }
            }
        }
    }

    /// Extract metadata from a libcamera ControlList (from completed request)
    pub fn extract_metadata(metadata_list: &mut ControlList) -> FrameMetadata {
        let mut metadata = FrameMetadata::default();

        // Extract exposure time
        if let Ok(ControlValue::Int32(v)) = metadata_list.get_raw(ControlId::ExposureTime as u32)
            && let Some(&val) = v.first() {
                metadata.exposure_time = Some(val as u64);
            }

        // Extract analogue gain
        if let Ok(ControlValue::Float(v)) = metadata_list.get_raw(ControlId::AnalogueGain as u32)
            && let Some(&val) = v.first() {
                metadata.analogue_gain = Some(val);
            }

        // Extract digital gain
        if let Ok(ControlValue::Float(v)) = metadata_list.get_raw(ControlId::DigitalGain as u32)
            && let Some(&val) = v.first() {
                metadata.digital_gain = Some(val);
            }

        // Extract colour temperature
        if let Ok(ControlValue::Int32(v)) = metadata_list.get_raw(ControlId::ColourTemperature as u32)
            && let Some(&val) = v.first() {
                metadata.colour_temperature = Some(val as u32);
            }

        // Extract lens position
        if let Ok(ControlValue::Float(v)) = metadata_list.get_raw(ControlId::LensPosition as u32)
            && let Some(&val) = v.first() {
                metadata.lens_position = Some(val);
            }

        // Extract sensor timestamp
        if let Ok(ControlValue::Int64(v)) = metadata_list.get_raw(ControlId::SensorTimestamp as u32)
            && let Some(&val) = v.first() {
                metadata.sensor_timestamp = Some(val as u64);
            }

        // Extract AF state
        if let Ok(ControlValue::Int32(v)) = metadata_list.get_raw(ControlId::AfState as u32)
            && let Some(&val) = v.first() {
                metadata.af_state = match val {
                    0 => Some(AfState::Idle),
                    1 => Some(AfState::Scanning),
                    2 => Some(AfState::Focused),
                    3 => Some(AfState::Failed),
                    _ => None,
                };
            }

        // Extract AE state
        if let Ok(ControlValue::Int32(v)) = metadata_list.get_raw(ControlId::AeState as u32)
            && let Some(&val) = v.first() {
                metadata.ae_state = match val {
                    0 => Some(AeState::Inactive),
                    1 => Some(AeState::Searching),
                    2 => Some(AeState::Converged),
                    3 => Some(AeState::Locked),
                    _ => None,
                };
            }

        // Note: AwbState control ID may not be available in all libcamera versions
        // The state can be inferred from AwbLocked control instead

        metadata
    }
}

/// Helper to extract numeric value from ControlValue for bounds
fn extract_numeric_bound(value: &ControlValue) -> Option<i64> {
    match value {
        ControlValue::Bool(v) => v.first().map(|&b| b as i64),
        ControlValue::Byte(v) => v.first().map(|&b| b as i64),
        ControlValue::Int32(v) => v.first().map(|&i| i as i64),
        ControlValue::Int64(v) => v.first().copied(),
        ControlValue::Float(v) => v.first().map(|&f| f as i64),
        _ => None,
    }
}

/// Check if libcamera is available on this system
pub fn is_libcamera_available() -> bool {
    match CameraManager::new() {
        Ok(mgr) => {
            let cameras = mgr.cameras();
            let available = !cameras.is_empty();
            debug!(camera_count = cameras.len(), "libcamera availability check");
            available
        }
        Err(e) => {
            debug!(?e, "libcamera not available");
            false
        }
    }
}

/// List available libcamera cameras
pub fn list_libcamera_cameras() -> Vec<String> {
    match CameraManager::new() {
        Ok(mgr) => {
            let cameras = mgr.cameras();
            cameras.iter().map(|c| c.id().to_string()).collect()
        }
        Err(_) => Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // These tests are ignored by default because libcamera's CameraManager
    // can have issues when created multiple times in parallel tests.
    // Run with: cargo test -- --ignored

    #[test]
    #[ignore = "libcamera CameraManager conflicts with parallel tests"]
    fn test_libcamera_available() {
        // Just verify the check doesn't panic
        let _ = is_libcamera_available();
    }

    #[test]
    #[ignore = "libcamera CameraManager conflicts with parallel tests"]
    fn test_list_cameras() {
        let cameras = list_libcamera_cameras();
        // Don't assert on count since it depends on the system
        for id in &cameras {
            assert!(!id.is_empty());
        }
    }
}
