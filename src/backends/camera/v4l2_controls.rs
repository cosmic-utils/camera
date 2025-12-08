// SPDX-License-Identifier: GPL-3.0-only

//! V4L2 camera control interface
//!
//! Provides functions to query and set V4L2 camera controls for exposure,
//! gain, ISO, and metering settings.
//!
//! Inspired by [cameractrls](https://github.com/soyersoyer/cameractrls).

use std::fs::File;
use std::os::unix::io::AsRawFd;
use tracing::{debug, warn};

// ===== V4L2 Control Class Bases =====
const V4L2_CTRL_CLASS_USER: u32 = 0x00980000;
const V4L2_CTRL_CLASS_CAMERA: u32 = 0x009a0000;
const V4L2_CTRL_CLASS_IMAGE_SOURCE: u32 = 0x009e0000;

const V4L2_CID_BASE: u32 = V4L2_CTRL_CLASS_USER | 0x900;
const V4L2_CID_CAMERA_CLASS_BASE: u32 = V4L2_CTRL_CLASS_CAMERA | 0x900;
const V4L2_CID_IMAGE_SOURCE_CLASS_BASE: u32 = V4L2_CTRL_CLASS_IMAGE_SOURCE | 0x900;

// ===== V4L2 Control IDs (User Class) =====

/// Brightness control
pub const V4L2_CID_BRIGHTNESS: u32 = V4L2_CID_BASE + 0;
/// Contrast control
pub const V4L2_CID_CONTRAST: u32 = V4L2_CID_BASE + 1;
/// Saturation control
pub const V4L2_CID_SATURATION: u32 = V4L2_CID_BASE + 2;
/// Hue control
pub const V4L2_CID_HUE: u32 = V4L2_CID_BASE + 3;
/// Automatic white balance
pub const V4L2_CID_AUTO_WHITE_BALANCE: u32 = V4L2_CID_BASE + 12;
/// Automatic gain control
pub const V4L2_CID_AUTOGAIN: u32 = V4L2_CID_BASE + 18;
/// Gain control
pub const V4L2_CID_GAIN: u32 = V4L2_CID_BASE + 19;
/// White balance temperature in Kelvin
pub const V4L2_CID_WHITE_BALANCE_TEMPERATURE: u32 = V4L2_CID_BASE + 26;
/// Sharpness control
pub const V4L2_CID_SHARPNESS: u32 = V4L2_CID_BASE + 27;
/// Backlight compensation - helps with backlit subjects
pub const V4L2_CID_BACKLIGHT_COMPENSATION: u32 = V4L2_CID_BASE + 28;

// ===== V4L2 Control IDs (Camera Class) =====

/// Exposure mode: Auto, Manual, Shutter Priority, Aperture Priority
pub const V4L2_CID_EXPOSURE_AUTO: u32 = V4L2_CID_CAMERA_CLASS_BASE + 1;
/// Absolute exposure time in 100µs units
pub const V4L2_CID_EXPOSURE_ABSOLUTE: u32 = V4L2_CID_CAMERA_CLASS_BASE + 2;
/// Allow frame rate variation during auto exposure
pub const V4L2_CID_EXPOSURE_AUTO_PRIORITY: u32 = V4L2_CID_CAMERA_CLASS_BASE + 3;
/// Focus control (manual focus position)
pub const V4L2_CID_FOCUS_ABSOLUTE: u32 = V4L2_CID_CAMERA_CLASS_BASE + 10;
/// Auto focus enable
pub const V4L2_CID_FOCUS_AUTO: u32 = V4L2_CID_CAMERA_CLASS_BASE + 12;
/// Exposure compensation (EV bias) in 0.001 EV units
pub const V4L2_CID_AUTO_EXPOSURE_BIAS: u32 = V4L2_CID_CAMERA_CLASS_BASE + 19;
/// ISO sensitivity value
pub const V4L2_CID_ISO_SENSITIVITY: u32 = V4L2_CID_CAMERA_CLASS_BASE + 23;
/// Auto ISO control
pub const V4L2_CID_ISO_SENSITIVITY_AUTO: u32 = V4L2_CID_CAMERA_CLASS_BASE + 24;
/// Exposure metering mode
pub const V4L2_CID_EXPOSURE_METERING: u32 = V4L2_CID_CAMERA_CLASS_BASE + 25;

// ===== V4L2 Control IDs (Image Source Class) =====

/// Analogue gain (image source class)
pub const V4L2_CID_ANALOGUE_GAIN: u32 = V4L2_CID_IMAGE_SOURCE_CLASS_BASE + 3;

// ===== V4L2 Exposure Auto Menu Values =====

/// Automatic exposure time and iris
pub const V4L2_EXPOSURE_AUTO: i32 = 0;
/// Manual exposure time and iris
pub const V4L2_EXPOSURE_MANUAL: i32 = 1;
/// Manual exposure time, auto iris (shutter priority)
pub const V4L2_EXPOSURE_SHUTTER_PRIORITY: i32 = 2;
/// Auto exposure time, manual iris (aperture priority)
pub const V4L2_EXPOSURE_APERTURE_PRIORITY: i32 = 3;

// ===== V4L2 Exposure Metering Menu Values =====

/// Average metering across entire frame
pub const V4L2_EXPOSURE_METERING_AVERAGE: i32 = 0;
/// Center-weighted metering
pub const V4L2_EXPOSURE_METERING_CENTER_WEIGHTED: i32 = 1;
/// Spot metering on center point
pub const V4L2_EXPOSURE_METERING_SPOT: i32 = 2;
/// Matrix/evaluative metering
pub const V4L2_EXPOSURE_METERING_MATRIX: i32 = 3;

// ===== V4L2 Control Types =====
const V4L2_CTRL_TYPE_INTEGER: u32 = 1;
const V4L2_CTRL_TYPE_BOOLEAN: u32 = 2;
const V4L2_CTRL_TYPE_MENU: u32 = 3;
const V4L2_CTRL_TYPE_INTEGER_MENU: u32 = 9;

// ===== V4L2 Control Flags =====
const V4L2_CTRL_FLAG_DISABLED: u32 = 0x0001;
const V4L2_CTRL_FLAG_INACTIVE: u32 = 0x0010;

// ===== V4L2 ioctl Numbers =====
// Calculated as: (dir << 30) | (size << 16) | ('V' << 8) | nr
// where dir: 2=READ, 1=WRITE, 3=READ|WRITE

/// Get control value (v4l2_control: 8 bytes)
const VIDIOC_G_CTRL: libc::c_ulong = 0xC008561B;
/// Set control value (v4l2_control: 8 bytes)
const VIDIOC_S_CTRL: libc::c_ulong = 0xC008561C;
/// Query control info (v4l2_queryctrl: 68 bytes)
const VIDIOC_QUERYCTRL: libc::c_ulong = 0xC0445624;
/// Query menu item (v4l2_querymenu: 44 bytes)
const VIDIOC_QUERYMENU: libc::c_ulong = 0xC02C5625;

// ===== V4L2 ioctl Structures =====

/// V4L2 control get/set structure
#[repr(C)]
struct V4l2Control {
    id: u32,
    value: i32,
}

/// V4L2 query control structure
#[repr(C)]
struct V4l2Queryctrl {
    id: u32,
    ctrl_type: u32,
    name: [u8; 32],
    minimum: i32,
    maximum: i32,
    step: i32,
    default_value: i32,
    flags: u32,
    reserved: [u32; 2],
}

/// V4L2 query menu structure
#[repr(C)]
#[repr(packed)]
struct V4l2Querymenu {
    id: u32,
    index: u32,
    name: [u8; 32],
    reserved: u32,
}

// ===== Public Types =====

/// Information about a V4L2 control
#[derive(Debug, Clone)]
pub struct ControlInfo {
    pub id: u32,
    pub name: String,
    pub ctrl_type: ControlType,
    pub minimum: i32,
    pub maximum: i32,
    pub step: i32,
    pub default_value: i32,
    pub flags: u32,
}

/// V4L2 control type
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ControlType {
    Integer,
    Boolean,
    Menu,
    IntegerMenu,
    Unknown(u32),
}

impl From<u32> for ControlType {
    fn from(value: u32) -> Self {
        match value {
            V4L2_CTRL_TYPE_INTEGER => ControlType::Integer,
            V4L2_CTRL_TYPE_BOOLEAN => ControlType::Boolean,
            V4L2_CTRL_TYPE_MENU => ControlType::Menu,
            V4L2_CTRL_TYPE_INTEGER_MENU => ControlType::IntegerMenu,
            other => ControlType::Unknown(other),
        }
    }
}

impl ControlInfo {
    /// Check if control is disabled
    pub fn is_disabled(&self) -> bool {
        self.flags & V4L2_CTRL_FLAG_DISABLED != 0
    }

    /// Check if control is inactive (value cannot be changed)
    pub fn is_inactive(&self) -> bool {
        self.flags & V4L2_CTRL_FLAG_INACTIVE != 0
    }
}

/// Menu item for menu-type controls
#[derive(Debug, Clone)]
pub struct MenuItem {
    pub index: i32,
    pub name: String,
}

// ===== Helper Functions =====

/// Extract a null-terminated string from a fixed-size byte array
fn extract_name(bytes: &[u8; 32]) -> String {
    let name_len = bytes.iter().position(|&c| c == 0).unwrap_or(32);
    String::from_utf8_lossy(&bytes[..name_len]).to_string()
}

// ===== Public Functions =====

/// Query if a control exists and get its information
pub fn query_control(device_path: &str, control_id: u32) -> Option<ControlInfo> {
    let file = File::open(device_path).ok()?;
    let fd = file.as_raw_fd();

    let mut qctrl = V4l2Queryctrl {
        id: control_id,
        ctrl_type: 0,
        name: [0; 32],
        minimum: 0,
        maximum: 0,
        step: 0,
        default_value: 0,
        flags: 0,
        reserved: [0; 2],
    };

    let result = unsafe { libc::ioctl(fd, VIDIOC_QUERYCTRL, &mut qctrl as *mut V4l2Queryctrl) };

    if result < 0 {
        return None;
    }

    Some(ControlInfo {
        id: qctrl.id,
        name: extract_name(&qctrl.name),
        ctrl_type: qctrl.ctrl_type.into(),
        minimum: qctrl.minimum,
        maximum: qctrl.maximum,
        step: qctrl.step,
        default_value: qctrl.default_value,
        flags: qctrl.flags,
    })
}

/// Get current value of a control
pub fn get_control(device_path: &str, control_id: u32) -> Option<i32> {
    let file = File::open(device_path).ok()?;
    let fd = file.as_raw_fd();

    let mut ctrl = V4l2Control {
        id: control_id,
        value: 0,
    };

    let result = unsafe { libc::ioctl(fd, VIDIOC_G_CTRL, &mut ctrl as *mut V4l2Control) };

    if result < 0 {
        debug!(device_path, control_id, "Failed to get V4L2 control");
        return None;
    }

    Some(ctrl.value)
}

/// Set value of a control
pub fn set_control(device_path: &str, control_id: u32, value: i32) -> Result<(), String> {
    let file = File::open(device_path).map_err(|e| format!("Failed to open device: {}", e))?;
    let fd = file.as_raw_fd();

    let mut ctrl = V4l2Control {
        id: control_id,
        value,
    };

    let result = unsafe { libc::ioctl(fd, VIDIOC_S_CTRL, &mut ctrl as *mut V4l2Control) };

    if result < 0 {
        let errno = std::io::Error::last_os_error();
        warn!(
            device_path,
            control_id,
            value,
            ?errno,
            "Failed to set V4L2 control"
        );
        return Err(format!("Failed to set control: {}", errno));
    }

    // Check if the driver accepted our value
    if ctrl.value != value {
        debug!(
            device_path,
            control_id,
            requested = value,
            actual = ctrl.value,
            "V4L2 control value was clamped"
        );
    }

    Ok(())
}

/// Query all menu items for a menu-type control
pub fn query_menu_items(device_path: &str, control_id: u32, max_index: i32) -> Vec<MenuItem> {
    let file = match File::open(device_path) {
        Ok(f) => f,
        Err(_) => return Vec::new(),
    };
    let fd = file.as_raw_fd();

    let mut items = Vec::new();

    for index in 0..=max_index {
        let mut qmenu = V4l2Querymenu {
            id: control_id,
            index: index as u32,
            name: [0; 32],
            reserved: 0,
        };

        let result = unsafe { libc::ioctl(fd, VIDIOC_QUERYMENU, &mut qmenu as *mut V4l2Querymenu) };

        if result >= 0 {
            items.push(MenuItem {
                index,
                name: extract_name(&qmenu.name),
            });
        }
    }

    items
}

/// Check if a control is available on the device
pub fn has_control(device_path: &str, control_id: u32) -> bool {
    query_control(device_path, control_id)
        .map(|info| !info.is_disabled())
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_control_id_values() {
        // Verify control IDs match expected values
        assert_eq!(V4L2_CID_EXPOSURE_AUTO, 0x009a0901);
        assert_eq!(V4L2_CID_EXPOSURE_ABSOLUTE, 0x009a0902);
        assert_eq!(V4L2_CID_AUTO_EXPOSURE_BIAS, 0x009a0913);
        assert_eq!(V4L2_CID_EXPOSURE_METERING, 0x009a0919);
        assert_eq!(V4L2_CID_ISO_SENSITIVITY, 0x009a0917);
        assert_eq!(V4L2_CID_GAIN, 0x00980913);
    }

    #[test]
    fn test_control_type_conversion() {
        assert_eq!(ControlType::from(1), ControlType::Integer);
        assert_eq!(ControlType::from(2), ControlType::Boolean);
        assert_eq!(ControlType::from(3), ControlType::Menu);
        assert_eq!(ControlType::from(99), ControlType::Unknown(99));
    }
}
