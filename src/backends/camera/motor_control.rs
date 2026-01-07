// SPDX-License-Identifier: GPL-3.0-only

//! Unified Motor Control Abstraction
//!
//! This module provides a unified interface for controlling the Kinect motor
//! that works with both the kernel V4L2 driver and the freedepth userspace library.
//!
//! When the kernel driver is active, motor control uses V4L2 controls
//! (V4L2_CID_TILT_ABSOLUTE, V4L2_CID_TILT_RESET).
//!
//! When using freedepth, motor control goes through the freedepth USB interface.

use tracing::{debug, info, warn};

/// Tilt angle limits (in degrees)
pub const TILT_MIN_DEGREES: i8 = -27;
pub const TILT_MAX_DEGREES: i8 = 27;

/// Motor control backend type
#[derive(Debug, Clone)]
pub enum MotorBackend {
    /// V4L2 kernel driver tilt control
    KernelV4L2 {
        /// Path to the V4L2 device that has tilt controls
        device_path: String,
    },
    /// freedepth USB motor control
    #[cfg(all(target_arch = "x86_64", feature = "freedepth"))]
    Freedepth,
    /// No motor control available
    None,
}

/// Unified motor controller
///
/// Provides a consistent interface for motor control regardless of whether
/// the kernel driver or freedepth is being used.
pub struct MotorController {
    backend: MotorBackend,
}

impl MotorController {
    /// Create a motor controller for a kernel V4L2 device
    pub fn for_kernel_device(device_path: &str) -> Self {
        info!(
            device_path,
            "Creating motor controller for kernel V4L2 device"
        );
        Self {
            backend: MotorBackend::KernelV4L2 {
                device_path: device_path.to_string(),
            },
        }
    }

    /// Create a motor controller using freedepth
    #[cfg(all(target_arch = "x86_64", feature = "freedepth"))]
    pub fn for_freedepth() -> Self {
        info!("Creating motor controller for freedepth");
        Self {
            backend: MotorBackend::Freedepth,
        }
    }

    /// Create a no-op motor controller (when no motor control is available)
    pub fn none() -> Self {
        Self {
            backend: MotorBackend::None,
        }
    }

    /// Get the current backend type
    pub fn backend(&self) -> &MotorBackend {
        &self.backend
    }

    /// Set tilt angle (-27 to +27 degrees)
    pub fn set_tilt(&self, degrees: i8) -> Result<(), String> {
        let degrees = degrees.clamp(TILT_MIN_DEGREES, TILT_MAX_DEGREES);

        match &self.backend {
            MotorBackend::KernelV4L2 { device_path } => set_tilt_v4l2(device_path, degrees),
            #[cfg(all(target_arch = "x86_64", feature = "freedepth"))]
            MotorBackend::Freedepth => set_tilt_freedepth(degrees),
            MotorBackend::None => {
                warn!("Motor control not available");
                Err("Motor control not available".to_string())
            }
        }
    }

    /// Get current tilt angle
    pub fn get_tilt(&self) -> Result<i8, String> {
        match &self.backend {
            MotorBackend::KernelV4L2 { device_path } => get_tilt_v4l2(device_path),
            #[cfg(all(target_arch = "x86_64", feature = "freedepth"))]
            MotorBackend::Freedepth => get_tilt_freedepth(),
            MotorBackend::None => Err("Motor control not available".to_string()),
        }
    }

    /// Reset tilt to center position
    pub fn reset_tilt(&self) -> Result<(), String> {
        match &self.backend {
            MotorBackend::KernelV4L2 { device_path } => reset_tilt_v4l2(device_path),
            #[cfg(all(target_arch = "x86_64", feature = "freedepth"))]
            MotorBackend::Freedepth => {
                // freedepth doesn't have a dedicated reset, just set to 0
                set_tilt_freedepth(0)
            }
            MotorBackend::None => Err("Motor control not available".to_string()),
        }
    }
}

// V4L2 implementation
use super::v4l2_controls;

fn set_tilt_v4l2(device_path: &str, degrees: i8) -> Result<(), String> {
    debug!(device_path, degrees, "Setting tilt via V4L2");

    // V4L2_CID_TILT_ABSOLUTE expects the value in degrees for the Kinect driver
    v4l2_controls::set_control(
        device_path,
        v4l2_controls::V4L2_CID_TILT_ABSOLUTE,
        degrees as i32,
    )
    .map_err(|e| format!("Failed to set tilt: {}", e))
}

fn get_tilt_v4l2(device_path: &str) -> Result<i8, String> {
    debug!(device_path, "Getting tilt via V4L2");

    v4l2_controls::get_control(device_path, v4l2_controls::V4L2_CID_TILT_ABSOLUTE)
        .map(|v| v as i8)
        .ok_or_else(|| "Failed to get tilt".to_string())
}

fn reset_tilt_v4l2(device_path: &str) -> Result<(), String> {
    debug!(device_path, "Resetting tilt via V4L2");

    // V4L2_CID_TILT_RESET is a button control - write 1 to trigger
    v4l2_controls::set_control(device_path, v4l2_controls::V4L2_CID_TILT_RESET, 1)
        .map_err(|e| format!("Failed to reset tilt: {}", e))
}

// =============================================================================
// freedepth USB Device Management
// =============================================================================

#[cfg(all(target_arch = "x86_64", feature = "freedepth"))]
mod freedepth_motor {
    use std::sync::{Arc, Mutex};
    use tracing::{debug, info};

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

    pub fn set_tilt(degrees: i8) -> Result<(), String> {
        debug!(degrees, "Setting tilt via freedepth");
        with_motor_usb(|usb| freedepth::Motor::new(usb).set_tilt(degrees))
    }

    pub fn get_tilt() -> Result<i8, String> {
        debug!("Getting tilt via freedepth");
        with_motor_usb(|usb| freedepth::Motor::new(usb).get_tilt())
    }

    pub fn is_available() -> bool {
        get_motor_usb()
            .lock()
            .map(|guard| guard.is_some())
            .unwrap_or(false)
    }
}

// Re-export for external use
#[cfg(all(target_arch = "x86_64", feature = "freedepth"))]
pub use freedepth_motor::{clear_motor_usb_device, set_motor_usb_device};

/// Global function to set motor tilt via freedepth
///
/// This is a convenience function for handlers that need quick motor access
/// without maintaining a MotorController instance.
#[cfg(all(target_arch = "x86_64", feature = "freedepth"))]
pub fn set_motor_tilt(degrees: i8) -> Result<(), String> {
    let degrees = degrees.clamp(TILT_MIN_DEGREES, TILT_MAX_DEGREES);
    freedepth_motor::set_tilt(degrees)
}

/// Global function to get motor tilt via freedepth
#[cfg(all(target_arch = "x86_64", feature = "freedepth"))]
pub fn get_motor_tilt() -> Result<i8, String> {
    freedepth_motor::get_tilt()
}

/// Check if motor control is available via freedepth
#[cfg(all(target_arch = "x86_64", feature = "freedepth"))]
pub fn is_motor_available() -> bool {
    freedepth_motor::is_available()
}

// freedepth implementation
#[cfg(all(target_arch = "x86_64", feature = "freedepth"))]
fn set_tilt_freedepth(degrees: i8) -> Result<(), String> {
    freedepth_motor::set_tilt(degrees)
}

#[cfg(all(target_arch = "x86_64", feature = "freedepth"))]
fn get_tilt_freedepth() -> Result<i8, String> {
    freedepth_motor::get_tilt()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_tilt_clamping() {
        // Tilt values should be clamped to valid range
        assert_eq!(30_i8.clamp(TILT_MIN_DEGREES, TILT_MAX_DEGREES), 27);
        assert_eq!((-30_i8).clamp(TILT_MIN_DEGREES, TILT_MAX_DEGREES), -27);
        assert_eq!(0_i8.clamp(TILT_MIN_DEGREES, TILT_MAX_DEGREES), 0);
    }
}
