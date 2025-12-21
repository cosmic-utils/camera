// SPDX-License-Identifier: GPL-3.0-only

//! Depth camera device controller
//!
//! Manages freedepth integration for depth cameras, providing:
//! - Device detection (by V4L2 driver name)
//! - Motor/tilt control (via global USB device reference)
//! - Depth-to-mm conversion (for V4L2 mode)
//!
//! Note: LED is automatically managed by freedepth based on device state.

use std::sync::{Arc, Mutex};
use tracing::info;

use super::types::CameraDevice;

// =============================================================================
// Motor Control via Global USB Device Reference
// =============================================================================

/// Global USB device for motor control
/// Set when native depth backend starts, cleared when it stops
static MOTOR_USB_DEVICE: std::sync::OnceLock<Mutex<Option<Arc<Mutex<freedepth::UsbDevice>>>>> =
    std::sync::OnceLock::new();

fn get_motor_usb() -> &'static Mutex<Option<Arc<Mutex<freedepth::UsbDevice>>>> {
    MOTOR_USB_DEVICE.get_or_init(|| Mutex::new(None))
}

/// Set the USB device for motor control (called when native backend starts)
pub fn set_motor_usb_device(usb: Arc<Mutex<freedepth::UsbDevice>>) {
    if let Ok(mut guard) = get_motor_usb().lock() {
        *guard = Some(usb);
        info!("Motor control USB device set");
    }
}

/// Clear the USB device for motor control (called when native backend stops)
pub fn clear_motor_usb_device() {
    if let Ok(mut guard) = get_motor_usb().lock() {
        *guard = None;
        info!("Motor control USB device cleared");
    }
}

/// Execute a closure with the motor USB device
///
/// Handles the nested lock pattern internally, returning appropriate errors
/// if the device is unavailable or locks fail.
fn with_motor_usb<T, F>(f: F) -> Result<T, String>
where
    F: FnOnce(&mut freedepth::UsbDevice) -> freedepth::Result<T>,
{
    let guard = get_motor_usb()
        .lock()
        .map_err(|e| format!("Lock error: {}", e))?;
    let usb_arc = guard.as_ref().ok_or("Motor USB device not available")?;
    let mut usb = usb_arc
        .lock()
        .map_err(|e| format!("USB lock error: {}", e))?;
    f(&mut usb).map_err(|e| e.to_string())
}

/// Set motor tilt via global USB device
pub fn set_motor_tilt(degrees: i8) -> Result<(), String> {
    with_motor_usb(|usb| freedepth::Motor::new(usb).set_tilt(degrees))
}

/// Get motor tilt via global USB device
pub fn get_motor_tilt() -> Result<i8, String> {
    with_motor_usb(|usb| freedepth::Motor::new(usb).get_tilt())
}

/// Get motor/accelerometer state via global USB device
pub fn get_motor_tilt_state() -> Result<freedepth::TiltState, String> {
    with_motor_usb(|usb| freedepth::Motor::new(usb).get_state())
}

/// Check if motor control is available
pub fn is_motor_available() -> bool {
    get_motor_usb()
        .lock()
        .map(|guard| guard.is_some())
        .unwrap_or(false)
}

// =============================================================================
// Device Detection
// =============================================================================

/// Check if a camera device is a depth camera supported by freedepth
///
/// Detection is based on:
/// - Device path prefix (freedepth-enumerated devices)
/// - V4L2 driver name (kernel drivers)
/// - Device name patterns
pub fn is_depth_camera(device: &CameraDevice) -> bool {
    // Check for freedepth-enumerated device (path starts with depth path prefix)
    if device
        .path
        .starts_with(super::depth_native::DEPTH_PATH_PREFIX)
    {
        return true;
    }

    if let Some(ref info) = device.device_info {
        // Check for known depth camera kernel drivers
        // "gspca_kinect" and "kinect" are the kernel drivers for Xbox 360 Kinect
        // "freedepth" is used when enumerated via freedepth
        if info.driver == "gspca_kinect" || info.driver == "kinect" || info.driver == "freedepth" {
            return true;
        }
    }
    // Also check by name as fallback for depth cameras
    let name_lower = device.name.to_lowercase();
    name_lower.contains("kinect") || name_lower.contains("xbox nui")
}

// =============================================================================
// Depth Conversion for V4L2/PipeWire mode
// =============================================================================

/// Global depth converter using default calibration (for V4L2 mode)
///
/// Note: In V4L2 mode, we cannot access the USB device to fetch device-specific
/// calibration because the kernel driver holds the device. Using default
/// calibration values provides reasonable accuracy for most Kinect sensors.
static V4L2_DEPTH_CONVERTER: std::sync::OnceLock<freedepth::DepthToMm> = std::sync::OnceLock::new();

fn get_v4l2_depth_converter() -> &'static freedepth::DepthToMm {
    V4L2_DEPTH_CONVERTER.get_or_init(|| freedepth::DepthToMm::with_defaults())
}

/// Depth camera controller for V4L2/PipeWire mode
///
/// Uses default calibration values since we cannot access the USB device
/// while the kernel driver is active.
pub struct DepthController;

impl DepthController {
    /// Initialize is a no-op now - depth conversion uses default calibration
    pub fn initialize() -> Result<(), String> {
        // Pre-initialize the converter
        let _ = get_v4l2_depth_converter();
        info!("Depth converter initialized with default calibration");
        Ok(())
    }

    /// Shutdown is a no-op
    pub fn shutdown() {
        // Nothing to do
    }

    /// Always returns true since we use default calibration
    pub fn is_initialized() -> bool {
        true
    }

    /// Convert raw 11-bit depth values to millimeters
    ///
    /// Takes raw depth values and converts them to millimeters
    /// using default calibration data.
    pub fn convert_depth_to_mm(raw_depth: &[u16]) -> Option<Vec<u16>> {
        let converter = get_v4l2_depth_converter();

        Some(
            raw_depth
                .iter()
                .map(|&raw| {
                    // If the value was left-shifted by 6 (from GPU), shift it back
                    // Values from GPU processor are: 10-bit << 6
                    let raw_10bit = raw >> 6;
                    // Convert to mm (calibration expects raw 11-bit values 0-2047)
                    // For 10-bit input, scale up to 11-bit range
                    let raw_11bit = (raw_10bit as u32 * 2) as u16;
                    converter.convert(raw_11bit.min(2047))
                })
                .collect(),
        )
    }

    /// Convert a single raw depth value to millimeters
    pub fn convert_single_depth(raw_10bit: u16) -> Option<u16> {
        let converter = get_v4l2_depth_converter();

        // Scale 10-bit to 11-bit and convert
        let raw_11bit = ((raw_10bit as u32) * 2).min(2047) as u16;
        Some(converter.convert(raw_11bit))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_is_depth_camera() {
        use super::super::types::DeviceInfo;

        // Test with gspca_kinect driver (depth camera kernel driver)
        let depth_device = CameraDevice {
            name: "Some Camera".to_string(),
            path: "pipewire-123".to_string(),
            metadata_path: None,
            device_info: Some(DeviceInfo {
                card: "Depth Camera".to_string(),
                driver: "gspca_kinect".to_string(),
                path: "/dev/video0".to_string(),
                real_path: "/dev/video0".to_string(),
            }),
        };
        assert!(is_depth_camera(&depth_device));

        // Test with different driver (not a depth camera)
        let other_device = CameraDevice {
            name: "Regular Webcam".to_string(),
            path: "pipewire-456".to_string(),
            metadata_path: None,
            device_info: Some(DeviceInfo {
                card: "Webcam".to_string(),
                driver: "uvcvideo".to_string(),
                path: "/dev/video2".to_string(),
                real_path: "/dev/video2".to_string(),
            }),
        };
        assert!(!is_depth_camera(&other_device));

        // Test with name-based detection
        let depth_by_name = CameraDevice {
            name: "Xbox NUI Kinect Camera".to_string(),
            path: "pipewire-789".to_string(),
            metadata_path: None,
            device_info: None,
        };
        assert!(is_depth_camera(&depth_by_name));

        // Test with Xbox NUI name
        let xbox_nui = CameraDevice {
            name: "Xbox NUI Camera".to_string(),
            path: "pipewire-101".to_string(),
            metadata_path: None,
            device_info: None,
        };
        assert!(is_depth_camera(&xbox_nui));

        // Test with kinect driver name
        let kinect_driver = CameraDevice {
            name: "Some Camera".to_string(),
            path: "pipewire-102".to_string(),
            metadata_path: None,
            device_info: Some(DeviceInfo {
                card: "Camera".to_string(),
                driver: "kinect".to_string(),
                path: "/dev/video11".to_string(),
                real_path: "/dev/video11".to_string(),
            }),
        };
        assert!(is_depth_camera(&kinect_driver));
    }
}
