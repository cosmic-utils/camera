// SPDX-License-Identifier: GPL-3.0-only

//! V4L2 Depth Camera Controls
//!
//! This module provides detection and access to depth cameras using the new
//! V4L2 depth control class extensions (V4L2_CTRL_CLASS_DEPTH).
//!
//! When the kernel driver supports these controls, we can use V4L2 directly
//! instead of the freedepth userspace library.

use std::fs::File;
use std::os::unix::io::AsRawFd;
use tracing::{debug, info};

// V4L2 Depth Control Class and IDs
// These match the kernel header definitions from include/uapi/linux/v4l2-controls.h
const V4L2_CTRL_CLASS_DEPTH: u32 = 0x00a6_0000;
const V4L2_CID_DEPTH_CLASS_BASE: u32 = V4L2_CTRL_CLASS_DEPTH | 0x900;

/// V4L2 Depth Control IDs
#[allow(dead_code)]
pub mod cid {
    use super::V4L2_CID_DEPTH_CLASS_BASE;

    pub const DEPTH_SENSOR_TYPE: u32 = V4L2_CID_DEPTH_CLASS_BASE;
    pub const DEPTH_UNITS: u32 = V4L2_CID_DEPTH_CLASS_BASE + 1;
    pub const DEPTH_MIN_DISTANCE: u32 = V4L2_CID_DEPTH_CLASS_BASE + 2;
    pub const DEPTH_MAX_DISTANCE: u32 = V4L2_CID_DEPTH_CLASS_BASE + 3;
    pub const DEPTH_INTRINSICS: u32 = V4L2_CID_DEPTH_CLASS_BASE + 4;
    pub const DEPTH_EXTRINSICS: u32 = V4L2_CID_DEPTH_CLASS_BASE + 5;
    pub const DEPTH_CALIBRATION_VERSION: u32 = V4L2_CID_DEPTH_CLASS_BASE + 6;
    pub const DEPTH_ILLUMINATOR_ENABLE: u32 = V4L2_CID_DEPTH_CLASS_BASE + 7;
    pub const DEPTH_ILLUMINATOR_POWER: u32 = V4L2_CID_DEPTH_CLASS_BASE + 8;
    pub const DEPTH_INVALID_VALUE: u32 = V4L2_CID_DEPTH_CLASS_BASE + 9;
}

/// Depth sensor types (V4L2_DEPTH_SENSOR_*)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u32)]
pub enum DepthSensorType {
    StructuredLight = 0,
    TimeOfFlight = 1,
    Stereo = 2,
    ActiveStereo = 3,
}

impl TryFrom<u32> for DepthSensorType {
    type Error = ();

    fn try_from(value: u32) -> Result<Self, Self::Error> {
        match value {
            0 => Ok(Self::StructuredLight),
            1 => Ok(Self::TimeOfFlight),
            2 => Ok(Self::Stereo),
            3 => Ok(Self::ActiveStereo),
            _ => Err(()),
        }
    }
}

/// Camera intrinsic calibration parameters
/// Values are in Q16.16 fixed-point format (divide by 65536 for floating point)
#[derive(Debug, Clone, Copy, Default)]
#[repr(C)]
pub struct DepthIntrinsics {
    pub width: u32,
    pub height: u32,
    pub fx: i32, // Q16.16 focal length X
    pub fy: i32, // Q16.16 focal length Y
    pub cx: i32, // Q16.16 principal point X
    pub cy: i32, // Q16.16 principal point Y
    pub model: u32,
    pub k1: i32,
    pub k2: i32,
    pub k3: i32,
    pub p1: i32,
    pub p2: i32,
    pub reserved: [u32; 4],
}

impl DepthIntrinsics {
    /// Convert Q16.16 fixed-point values to floating point
    pub fn fx_f32(&self) -> f32 {
        self.fx as f32 / 65536.0
    }

    pub fn fy_f32(&self) -> f32 {
        self.fy as f32 / 65536.0
    }

    pub fn cx_f32(&self) -> f32 {
        self.cx as f32 / 65536.0
    }

    pub fn cy_f32(&self) -> f32 {
        self.cy as f32 / 65536.0
    }
}

/// Camera extrinsic calibration parameters (depth-to-RGB transform)
/// Rotation is Q2.30 fixed-point, translation is in micrometers
#[derive(Debug, Clone, Copy, Default)]
#[repr(C)]
pub struct DepthExtrinsics {
    pub rotation: [i32; 9],    // 3x3 rotation matrix (Q2.30)
    pub translation: [i32; 3], // Translation in micrometers
    pub reserved: [u32; 4],
}

impl DepthExtrinsics {
    /// Get baseline (depth-to-RGB distance) in millimeters
    pub fn baseline_mm(&self) -> f32 {
        // Translation[0] is typically the X offset (baseline) in micrometers
        self.translation[0] as f32 / 1000.0
    }
}

/// Depth camera capabilities detected via V4L2 controls
#[derive(Debug, Clone)]
pub struct DepthCapabilities {
    pub sensor_type: DepthSensorType,
    pub units_um: u32, // Depth units in micrometers (1000 = mm)
    pub min_distance_mm: u32,
    pub max_distance_mm: u32,
    pub invalid_value: u32,
    pub intrinsics: Option<DepthIntrinsics>,
    pub extrinsics: Option<DepthExtrinsics>,
    pub has_illuminator: bool,
}

// V4L2 ioctl definitions
const VIDIOC_QUERYCAP: libc::c_ulong = 0x8068_5600; // _IOR('V', 0, struct v4l2_capability)
const VIDIOC_QUERYCTRL: libc::c_ulong = 0xc044_5624; // _IOWR('V', 36, struct v4l2_queryctrl) - 68 bytes struct
const VIDIOC_G_CTRL: libc::c_ulong = 0xc008_561b; // _IOWR('V', 27, struct v4l2_control)
const VIDIOC_G_EXT_CTRLS: libc::c_ulong = 0xc040_5647; // _IOWR('V', 71, struct v4l2_ext_controls)

/// V4L2 device capability structure for QUERYCAP
#[repr(C)]
struct V4l2Capability {
    driver: [u8; 16],
    card: [u8; 32],
    bus_info: [u8; 32],
    version: u32,
    capabilities: u32,
    device_caps: u32,
    reserved: [u32; 3],
}

/// Device information from V4L2 QUERYCAP
#[derive(Debug, Clone)]
pub struct V4l2DeviceInfo {
    pub driver: String,
    pub card: String,
    pub bus_info: String,
}

/// Query device information (driver, card name, bus_info) via QUERYCAP
pub fn query_device_info(device_path: &str) -> Option<V4l2DeviceInfo> {
    let file = File::open(device_path).ok()?;
    let fd = file.as_raw_fd();

    let mut caps = V4l2Capability {
        driver: [0; 16],
        card: [0; 32],
        bus_info: [0; 32],
        version: 0,
        capabilities: 0,
        device_caps: 0,
        reserved: [0; 3],
    };

    let result = unsafe { libc::ioctl(fd, VIDIOC_QUERYCAP, &mut caps as *mut _) };

    if result == 0 {
        let driver = std::str::from_utf8(&caps.driver)
            .unwrap_or("")
            .trim_end_matches('\0')
            .to_string();
        let card = std::str::from_utf8(&caps.card)
            .unwrap_or("")
            .trim_end_matches('\0')
            .to_string();
        let bus_info = std::str::from_utf8(&caps.bus_info)
            .unwrap_or("")
            .trim_end_matches('\0')
            .to_string();

        Some(V4l2DeviceInfo {
            driver,
            card,
            bus_info,
        })
    } else {
        None
    }
}

#[repr(C)]
struct V4l2Queryctrl {
    id: u32,
    type_: u32,
    name: [u8; 32],
    minimum: i32,
    maximum: i32,
    step: i32,
    default_value: i32,
    flags: u32,
    reserved: [u32; 2],
}

#[repr(C)]
struct V4l2Control {
    id: u32,
    value: i32,
}

#[repr(C)]
struct V4l2ExtControl {
    id: u32,
    size: u32,
    reserved2: [u32; 1],
    value_or_ptr: u64, // Union: value (i32/i64) or pointer
}

#[repr(C)]
struct V4l2ExtControls {
    which: u32,
    count: u32,
    error_idx: u32,
    request_fd: i32,
    reserved: [u32; 1],
    controls: *mut V4l2ExtControl,
}

/// Check if a V4L2 device supports depth camera controls
///
/// Returns true if the device has V4L2_CID_DEPTH_SENSOR_TYPE control,
/// indicating it's a depth camera with kernel driver support.
pub fn has_depth_controls(device_path: &str) -> bool {
    let file = match File::open(device_path) {
        Ok(f) => f,
        Err(e) => {
            debug!(path = %device_path, error = %e, "Failed to open device for depth control check");
            return false;
        }
    };

    let fd = file.as_raw_fd();

    // Query if DEPTH_SENSOR_TYPE control exists
    let mut query = V4l2Queryctrl {
        id: cid::DEPTH_SENSOR_TYPE,
        type_: 0,
        name: [0; 32],
        minimum: 0,
        maximum: 0,
        step: 0,
        default_value: 0,
        flags: 0,
        reserved: [0; 2],
    };

    let result = unsafe { libc::ioctl(fd, VIDIOC_QUERYCTRL, &mut query as *mut _) };

    if result == 0 {
        let name = std::str::from_utf8(&query.name)
            .unwrap_or("unknown")
            .trim_end_matches('\0');
        info!(
            path = %device_path,
            control_name = %name,
            "Device has V4L2 depth controls (kernel driver)"
        );
        true
    } else {
        debug!(path = %device_path, "Device does not have V4L2 depth controls");
        false
    }
}

/// Get a simple V4L2 control value
fn get_control(fd: i32, id: u32) -> Option<i32> {
    let mut ctrl = V4l2Control { id, value: 0 };

    let result = unsafe { libc::ioctl(fd, VIDIOC_G_CTRL, &mut ctrl as *mut _) };

    if result == 0 { Some(ctrl.value) } else { None }
}

/// Query depth camera capabilities via V4L2 controls
///
/// This reads all available depth controls to build a complete
/// picture of the camera's capabilities.
pub fn query_depth_capabilities(device_path: &str) -> Option<DepthCapabilities> {
    let file = File::open(device_path).ok()?;
    let fd = file.as_raw_fd();

    // Check for depth sensor type (required)
    let sensor_type_val = get_control(fd, cid::DEPTH_SENSOR_TYPE)?;
    let sensor_type = DepthSensorType::try_from(sensor_type_val as u32).ok()?;

    info!(
        path = %device_path,
        sensor_type = ?sensor_type,
        "Querying depth camera capabilities"
    );

    // Get other standard controls
    let units_um = get_control(fd, cid::DEPTH_UNITS).unwrap_or(1000) as u32;
    let min_distance_mm = get_control(fd, cid::DEPTH_MIN_DISTANCE).unwrap_or(500) as u32;
    let max_distance_mm = get_control(fd, cid::DEPTH_MAX_DISTANCE).unwrap_or(4000) as u32;
    let invalid_value = get_control(fd, cid::DEPTH_INVALID_VALUE).unwrap_or(2047) as u32;

    // Check for illuminator control
    let has_illuminator = {
        let mut query = V4l2Queryctrl {
            id: cid::DEPTH_ILLUMINATOR_ENABLE,
            type_: 0,
            name: [0; 32],
            minimum: 0,
            maximum: 0,
            step: 0,
            default_value: 0,
            flags: 0,
            reserved: [0; 2],
        };
        let result = unsafe { libc::ioctl(fd, VIDIOC_QUERYCTRL, &mut query as *mut _) };
        result == 0
    };

    // Try to get intrinsics (compound control - may not be available yet)
    let intrinsics = get_intrinsics(fd);
    let extrinsics = get_extrinsics(fd);

    Some(DepthCapabilities {
        sensor_type,
        units_um,
        min_distance_mm,
        max_distance_mm,
        invalid_value,
        intrinsics,
        extrinsics,
        has_illuminator,
    })
}

/// Get depth intrinsics via extended control
fn get_intrinsics(fd: i32) -> Option<DepthIntrinsics> {
    let mut intrinsics = DepthIntrinsics::default();
    let size = std::mem::size_of::<DepthIntrinsics>() as u32;

    let mut ext_ctrl = V4l2ExtControl {
        id: cid::DEPTH_INTRINSICS,
        size,
        reserved2: [0],
        value_or_ptr: &mut intrinsics as *mut _ as u64,
    };

    let mut ext_ctrls = V4l2ExtControls {
        which: V4L2_CTRL_CLASS_DEPTH,
        count: 1,
        error_idx: 0,
        request_fd: 0,
        reserved: [0],
        controls: &mut ext_ctrl,
    };

    let result = unsafe { libc::ioctl(fd, VIDIOC_G_EXT_CTRLS, &mut ext_ctrls as *mut _) };

    if result == 0 {
        debug!(
            fx = intrinsics.fx_f32(),
            fy = intrinsics.fy_f32(),
            cx = intrinsics.cx_f32(),
            cy = intrinsics.cy_f32(),
            "Got depth intrinsics from kernel"
        );
        Some(intrinsics)
    } else {
        debug!("Depth intrinsics control not available");
        None
    }
}

/// Get depth extrinsics via extended control
fn get_extrinsics(fd: i32) -> Option<DepthExtrinsics> {
    let mut extrinsics = DepthExtrinsics::default();
    let size = std::mem::size_of::<DepthExtrinsics>() as u32;

    let mut ext_ctrl = V4l2ExtControl {
        id: cid::DEPTH_EXTRINSICS,
        size,
        reserved2: [0],
        value_or_ptr: &mut extrinsics as *mut _ as u64,
    };

    let mut ext_ctrls = V4l2ExtControls {
        which: V4L2_CTRL_CLASS_DEPTH,
        count: 1,
        error_idx: 0,
        request_fd: 0,
        reserved: [0],
        controls: &mut ext_ctrl,
    };

    let result = unsafe { libc::ioctl(fd, VIDIOC_G_EXT_CTRLS, &mut ext_ctrls as *mut _) };

    if result == 0 {
        debug!(
            baseline_mm = extrinsics.baseline_mm(),
            "Got depth extrinsics from kernel"
        );
        Some(extrinsics)
    } else {
        debug!("Depth extrinsics control not available");
        None
    }
}

/// Enable or disable the depth illuminator (IR projector)
pub fn set_illuminator_enabled(device_path: &str, enabled: bool) -> Result<(), String> {
    let file = File::open(device_path).map_err(|e| format!("Failed to open device: {}", e))?;
    let fd = file.as_raw_fd();

    let ctrl = V4l2Control {
        id: cid::DEPTH_ILLUMINATOR_ENABLE,
        value: if enabled { 1 } else { 0 },
    };

    // Use VIDIOC_S_CTRL for setting
    const VIDIOC_S_CTRL: libc::c_ulong = 0xc008_561c;

    let result = unsafe { libc::ioctl(fd, VIDIOC_S_CTRL, &ctrl as *const _) };

    if result == 0 {
        info!(enabled, "Set depth illuminator state");
        Ok(())
    } else {
        Err(format!(
            "Failed to set illuminator: {}",
            std::io::Error::last_os_error()
        ))
    }
}

/// Check if illuminator is currently enabled
pub fn is_illuminator_enabled(device_path: &str) -> Option<bool> {
    let file = File::open(device_path).ok()?;
    let fd = file.as_raw_fd();

    get_control(fd, cid::DEPTH_ILLUMINATOR_ENABLE).map(|v| v != 0)
}

// ============================================================================
// Registration Data Conversion (Kernel Calibration -> Shader Format)
// ============================================================================

/// Depth image dimensions (standard Kinect)
const DEPTH_WIDTH: u32 = 640;
const DEPTH_HEIGHT: u32 = 480;
const DEPTH_MM_MAX: u32 = 10000;

/// Fixed-point scale factor for x coordinates (matches freedepth)
pub const REG_X_VAL_SCALE: i32 = 256;

/// Registration data for GPU shaders
/// Matches the format expected by point_cloud and mesh shaders
#[derive(Clone)]
pub struct KernelRegistrationData {
    /// Registration table: 640*480 [x_scaled, y] pairs
    /// x is scaled by REG_X_VAL_SCALE (256)
    pub registration_table: Vec<[i32; 2]>,
    /// Depth-to-RGB shift table: 10001 i32 values indexed by depth_mm
    pub depth_to_rgb_shift: Vec<i32>,
    /// Target offset (typically 0 for kernel driver)
    pub target_offset: u32,
}

impl KernelRegistrationData {
    /// Create registration data from kernel intrinsics and extrinsics
    ///
    /// For the kernel driver, we use a simplified pinhole camera model:
    /// - Registration table maps each depth pixel to RGB space
    /// - Depth-to-RGB shift handles stereo baseline disparity
    pub fn from_kernel_calibration(
        intrinsics: &DepthIntrinsics,
        extrinsics: Option<&DepthExtrinsics>,
    ) -> Self {
        let registration_table = build_registration_table_from_intrinsics(intrinsics);
        let depth_to_rgb_shift = build_depth_to_rgb_shift_from_extrinsics(intrinsics, extrinsics);

        info!(
            table_size = registration_table.len(),
            shift_size = depth_to_rgb_shift.len(),
            fx = intrinsics.fx_f32(),
            fy = intrinsics.fy_f32(),
            cx = intrinsics.cx_f32(),
            cy = intrinsics.cy_f32(),
            baseline_mm = extrinsics.map(|e| e.baseline_mm()).unwrap_or(0.0),
            "Built registration data from kernel calibration"
        );

        Self {
            registration_table,
            depth_to_rgb_shift,
            target_offset: 0, // Kernel driver doesn't use pad offset
        }
    }

    /// Convert to the shader RegistrationData format
    pub fn to_shader_format(&self) -> crate::shaders::RegistrationData {
        crate::shaders::RegistrationData {
            registration_table: self.registration_table.clone(),
            depth_to_rgb_shift: self.depth_to_rgb_shift.clone(),
            target_offset: self.target_offset,
        }
    }
}

/// Build registration table from intrinsics
///
/// For a pinhole camera model, the registration table maps each depth pixel
/// to its corresponding RGB pixel position. Since depth and RGB cameras have
/// similar intrinsics, this is mostly an identity mapping with small corrections.
fn build_registration_table_from_intrinsics(intrinsics: &DepthIntrinsics) -> Vec<[i32; 2]> {
    let mut table = Vec::with_capacity((DEPTH_WIDTH * DEPTH_HEIGHT) as usize);

    let fx = intrinsics.fx_f32();
    let fy = intrinsics.fy_f32();
    let cx = intrinsics.cx_f32();
    let cy = intrinsics.cy_f32();

    // Default intrinsics if not calibrated
    let fx = if fx > 0.0 { fx } else { 580.0 };
    let fy = if fy > 0.0 { fy } else { 580.0 };
    let cx = if cx > 0.0 { cx } else { 320.0 };
    let cy = if cy > 0.0 { cy } else { 240.0 };

    for y in 0..DEPTH_HEIGHT {
        for x in 0..DEPTH_WIDTH {
            // For a simple pinhole model, depth and RGB pixels are roughly aligned
            // The main offset is the stereo baseline (handled by depth_to_rgb_shift)
            //
            // Apply minor correction for principal point offset between cameras
            // This is usually very small for Kinect
            let depth_cx = DEPTH_WIDTH as f32 / 2.0;
            let depth_cy = DEPTH_HEIGHT as f32 / 2.0;

            // Project from depth camera to RGB camera coordinate
            // For aligned cameras, this is approximately identity with focal length scaling
            let rgb_x = (x as f32 - depth_cx) * (fx / fx) + cx;
            let rgb_y = (y as f32 - depth_cy) * (fy / fy) + cy;

            // Store scaled x and integer y (matching freedepth format)
            // The x value is multiplied by REG_X_VAL_SCALE for fixed-point precision
            let x_scaled = (rgb_x * REG_X_VAL_SCALE as f32) as i32;
            let y_int = rgb_y as i32;

            table.push([x_scaled, y_int]);
        }
    }

    table
}

/// Build depth-to-RGB shift table from extrinsics
///
/// The shift table maps depth values (in mm) to horizontal pixel offsets
/// needed to align depth with RGB, accounting for stereo baseline.
///
/// Formula: shift = baseline_mm * fx / depth_mm * REG_X_VAL_SCALE
fn build_depth_to_rgb_shift_from_extrinsics(
    intrinsics: &DepthIntrinsics,
    extrinsics: Option<&DepthExtrinsics>,
) -> Vec<i32> {
    let mut table = vec![0i32; (DEPTH_MM_MAX + 1) as usize];

    // Get baseline from extrinsics (or use default Kinect baseline of ~25mm)
    let baseline_mm = extrinsics
        .map(|e| e.baseline_mm().abs())
        .filter(|&b| b > 0.0)
        .unwrap_or(25.0);

    // Get focal length
    let fx = intrinsics.fx_f32();
    let fx = if fx > 0.0 { fx } else { 580.0 };

    debug!(
        baseline_mm,
        fx, "Building depth-to-RGB shift table from kernel extrinsics"
    );

    for depth_mm in 1..=DEPTH_MM_MAX {
        // Disparity formula: shift_pixels = baseline * focal_length / depth
        // This gives the horizontal pixel offset due to stereo baseline
        let shift_pixels = baseline_mm * fx / (depth_mm as f32);

        // Scale by REG_X_VAL_SCALE for fixed-point math
        let shift_scaled = (shift_pixels * REG_X_VAL_SCALE as f32) as i32;

        table[depth_mm as usize] = shift_scaled;
    }

    // Depth 0 has no shift
    table[0] = 0;

    table
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_depth_sensor_type_conversion() {
        assert_eq!(
            DepthSensorType::try_from(0),
            Ok(DepthSensorType::StructuredLight)
        );
        assert_eq!(
            DepthSensorType::try_from(1),
            Ok(DepthSensorType::TimeOfFlight)
        );
        assert!(DepthSensorType::try_from(99).is_err());
    }

    #[test]
    fn test_intrinsics_conversion() {
        let intrinsics = DepthIntrinsics {
            fx: 580 * 65536, // 580 pixels in Q16.16
            fy: 580 * 65536,
            cx: 320 * 65536,
            cy: 240 * 65536,
            ..Default::default()
        };

        assert!((intrinsics.fx_f32() - 580.0).abs() < 0.001);
        assert!((intrinsics.cx_f32() - 320.0).abs() < 0.001);
    }
}
