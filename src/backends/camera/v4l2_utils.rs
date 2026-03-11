// SPDX-License-Identifier: GPL-3.0-only

//! Shared V4L2 utility functions
//!
//! This module provides common V4L2 operations used by multiple camera backends,
//! avoiding code duplication between PipeWire enumeration and native bindings.

use super::types::DeviceInfo;
use std::os::unix::io::{AsRawFd, RawFd};
use tracing::debug;

/// VIDIOC_QUERYCAP ioctl number
const VIDIOC_QUERYCAP: libc::c_ulong = 0x80685600;

/// V4L2 capability structure for VIDIOC_QUERYCAP ioctl
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

/// Query V4L2 capabilities for an open file descriptor.
///
/// Issues the `VIDIOC_QUERYCAP` ioctl and returns the capability struct,
/// or `None` if the ioctl fails.
fn query_v4l2_cap(fd: RawFd) -> Option<V4l2Capability> {
    let mut cap: V4l2Capability = unsafe { std::mem::zeroed() };
    let result = unsafe { libc::ioctl(fd, VIDIOC_QUERYCAP as _, &mut cap as *mut V4l2Capability) };
    if result < 0 { None } else { Some(cap) }
}

/// Get V4L2 driver name using ioctl
///
/// Opens the device and queries its capabilities to get the driver name.
/// Returns None if the device cannot be opened or the ioctl fails.
pub fn get_v4l2_driver(device_path: &str) -> Option<String> {
    let file = std::fs::File::open(device_path).ok()?;
    let cap = query_v4l2_cap(file.as_raw_fd())?;

    // Find null terminator or use full length
    let len = cap.driver.iter().position(|&c| c == 0).unwrap_or(16);
    let driver = String::from_utf8_lossy(&cap.driver[..len]).to_string();

    debug!(device_path, driver = %driver, "Got V4L2 driver name");
    Some(driver)
}

/// Build DeviceInfo from V4L2 device path and optional card name
///
/// Resolves symlinks to get the real device path and queries the driver name.
pub fn build_device_info(v4l2_path: &str, card: Option<&str>) -> DeviceInfo {
    // Get real path by resolving symlinks
    let real_path = std::fs::canonicalize(v4l2_path)
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_else(|_| v4l2_path.to_string());

    // Get driver name using V4L2 ioctl
    let driver = get_v4l2_driver(v4l2_path).unwrap_or_default();

    DeviceInfo {
        card: card.unwrap_or_default().to_string(),
        driver,
        path: v4l2_path.to_string(),
        real_path,
    }
}

/// Discover V4L2 subdevices that support focus control (lens actuators)
///
/// Scans `/dev/v4l-subdev*` for devices that support `V4L2_CID_FOCUS_ABSOLUTE`.
/// Returns a list of (device_path, name) tuples for discovered actuators.
pub fn discover_lens_actuators() -> Vec<(String, String)> {
    use super::v4l2_controls;

    let mut actuators = Vec::new();

    // Scan /dev/v4l-subdev* devices
    let entries = match std::fs::read_dir("/dev") {
        Ok(entries) => entries,
        Err(_) => return actuators,
    };

    for entry in entries.flatten() {
        let name = entry.file_name();
        let name_str = name.to_string_lossy();
        if !name_str.starts_with("v4l-subdev") {
            continue;
        }

        let path = format!("/dev/{}", name_str);

        // Check if this subdevice supports V4L2_CID_FOCUS_ABSOLUTE
        if let Some(info) =
            v4l2_controls::query_control(&path, v4l2_controls::V4L2_CID_FOCUS_ABSOLUTE)
            && !info.is_disabled()
        {
            // Read name from sysfs for logging
            let sysfs_name =
                std::fs::read_to_string(format!("/sys/class/video4linux/{}/name", name_str))
                    .unwrap_or_default()
                    .trim()
                    .to_string();

            let display_name = if sysfs_name.is_empty() {
                name_str.to_string()
            } else {
                sysfs_name
            };

            debug!(
                path = %path,
                name = %display_name,
                range = format!("{}-{}", info.minimum, info.maximum),
                "Discovered lens actuator with focus control"
            );
            actuators.push((path, display_name));
        }
    }

    actuators
}

/// Find the V4L2 video capture device path for a libcamera camera ID.
///
/// libcamera camera IDs for UVC cameras contain the USB VID:PID as the last
/// segment (e.g., `\_SB_.PCI0.GP17.XHC1.RHUB.PRT4-2.3:1.0-3564:fef8`).
/// This function extracts the VID:PID, then scans `/sys/class/video4linux/`
/// to find a matching `/dev/videoX` device.
pub fn find_v4l2_device_for_libcamera(camera_id: &str) -> Option<String> {
    // V4L2 capability flag for single-planar video capture
    const V4L2_CAP_VIDEO_CAPTURE: u32 = 0x00000001;

    // Extract VID:PID from camera ID.
    // UVC IDs end with "-VVVV:PPPP" (4-hex-digit vendor : 4-hex-digit product).
    let vid_pid = camera_id.rsplit('-').next()?;
    let (vid_str, pid_str) = vid_pid.split_once(':')?;
    if vid_str.len() != 4 || pid_str.len() != 4 {
        return None;
    }
    // Verify they're valid hex
    u16::from_str_radix(vid_str, 16).ok()?;
    u16::from_str_radix(pid_str, 16).ok()?;

    debug!(
        camera_id,
        vid = vid_str,
        pid = pid_str,
        "Looking for V4L2 device"
    );

    let entries = std::fs::read_dir("/sys/class/video4linux").ok()?;

    for entry in entries.flatten() {
        let name = entry.file_name();
        let name_str = name.to_string_lossy();
        if !name_str.starts_with("video") {
            continue;
        }

        // Read the device symlink to find the USB device hierarchy
        let device_link = format!("/sys/class/video4linux/{}/device", name_str);
        let resolved = match std::fs::canonicalize(&device_link) {
            Ok(p) => p.to_string_lossy().to_string(),
            Err(_) => continue,
        };

        // Walk up the sysfs tree to find idVendor/idProduct files
        let mut path = std::path::PathBuf::from(&resolved);
        let mut matched = false;
        for _ in 0..5 {
            let vendor_file = path.join("idVendor");
            let product_file = path.join("idProduct");
            if vendor_file.exists() && product_file.exists() {
                let vendor = std::fs::read_to_string(&vendor_file)
                    .unwrap_or_default()
                    .trim()
                    .to_string();
                let product = std::fs::read_to_string(&product_file)
                    .unwrap_or_default()
                    .trim()
                    .to_string();
                if vendor == vid_str && product == pid_str {
                    matched = true;
                }
                break;
            }
            if !path.pop() {
                break;
            }
        }

        if !matched {
            continue;
        }

        // Verify this is a video capture device (not metadata)
        let dev_path = format!("/dev/{}", name_str);
        let file = match std::fs::File::open(&dev_path) {
            Ok(f) => f,
            Err(_) => continue,
        };

        let cap = match query_v4l2_cap(file.as_raw_fd()) {
            Some(c) => c,
            None => continue,
        };

        // Use device_caps if available, otherwise capabilities
        let caps = if cap.device_caps != 0 {
            cap.device_caps
        } else {
            cap.capabilities
        };

        if caps & V4L2_CAP_VIDEO_CAPTURE == 0 {
            continue;
        }

        debug!(camera_id, device = %dev_path, "Found V4L2 device for libcamera camera");
        return Some(dev_path);
    }

    debug!(camera_id, "No V4L2 device found for libcamera camera");
    None
}

/// Check if a libcamera pipeline handler supports multi-stream capture
///
/// With native libcamera-rs bindings, all known pipeline handlers support
/// ViewFinder + Raw dual-stream configuration:
/// - "simple" handler: ViewFinder (Software ISP) + Raw (bypass ISP)
/// - Hardware ISP handlers ("vc4", "ipu3", "rkisp1"): native multi-stream
pub fn supports_multistream(pipeline_handler: Option<&str>) -> bool {
    // All known pipeline handlers (both "simple" Software ISP and hardware ISP
    // handlers like "vc4", "ipu3", "rkisp1") support ViewFinder + Raw dual-stream.
    // If no handler is known, assume single-stream to be safe.
    pipeline_handler.is_some()
}
