// SPDX-License-Identifier: GPL-3.0-only

//! Libcamera camera enumeration and format detection
//!
//! This module provides camera discovery and format enumeration using libcamera.
//! Libcamera is the modern camera stack for Linux mobile devices.

use super::super::types::{CameraDevice, CameraFormat};
use crate::constants::formats;
use tracing::{debug, info, warn};

/// Check if libcamera is available on this system
///
/// Returns true if the libcamerasrc GStreamer element is available
pub fn is_libcamera_available() -> bool {
    if gstreamer::init().is_err() {
        return false;
    }

    gstreamer::ElementFactory::make("libcamerasrc")
        .build()
        .is_ok()
}

/// Enumerate cameras using libcamera
/// Returns list of available cameras discovered through libcamera
pub fn enumerate_libcamera_cameras() -> Option<Vec<CameraDevice>> {
    debug!("Attempting to enumerate cameras via libcamera");

    // Check if libcamera is available
    if gstreamer::init().is_err() {
        warn!("GStreamer init failed");
        return None;
    }

    // Check if libcamerasrc element exists
    if gstreamer::ElementFactory::make("libcamerasrc")
        .build()
        .is_err()
    {
        debug!("libcamerasrc not available");
        return None;
    }

    info!("libcamera available for camera enumeration");

    // Libcamera camera enumeration strategy:
    // 1. Try to discover cameras through cam CLI tool (if available)
    // 2. Otherwise, provide generic "Default Camera" that lets libcamera auto-select

    let cameras = try_enumerate_with_cam_cli();

    if let Some(ref cams) = cameras {
        info!(count = cams.len(), "Found libcamera cameras");
        return Some(cams.clone());
    }

    // Fallback: Let libcamera use its default camera
    info!("Using libcamera auto-selection (default camera)");
    Some(vec![CameraDevice {
        name: "Default Camera (libcamera)".to_string(),
        path: String::new(), // Empty path = libcamera auto-selects
        metadata_path: None,
        device_info: None,
    }])
}

/// Try to enumerate cameras using the cam CLI tool
fn try_enumerate_with_cam_cli() -> Option<Vec<CameraDevice>> {
    debug!("Trying cam CLI for camera enumeration");

    let output = std::process::Command::new("cam")
        .args(["--list"])
        .output()
        .ok()?;

    if !output.status.success() {
        debug!("cam --list command failed");
        return None;
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let mut cameras = Vec::new();

    // Parse cam --list output
    // Example format:
    // Available cameras:
    // 1: 'ov5640 2-003c' (/base/soc/i2c@fdd40000/ov5640@3c)
    // 2: 'gc2145 4-003c' (/base/soc/i2c@fe5d0000/gc2145@3c)
    for line in stdout.lines() {
        let trimmed = line.trim();

        // Look for lines starting with a number followed by ':'
        if let Some(colon_pos) = trimmed.find(':') {
            let before_colon = trimmed[..colon_pos].trim();
            if before_colon.parse::<u32>().is_ok() {
                // This is a camera line
                let rest = &trimmed[colon_pos + 1..].trim();

                // Extract camera name between single quotes
                if let Some(name_start) = rest.find('\'') {
                    if let Some(name_end) = rest[name_start + 1..].find('\'') {
                        let name = &rest[name_start + 1..name_start + 1 + name_end];

                        // Extract camera ID between parentheses
                        let camera_id = if let Some(id_start) = rest.find('(') {
                            if let Some(id_end) = rest.rfind(')') {
                                rest[id_start + 1..id_end].to_string()
                            } else {
                                name.to_string()
                            }
                        } else {
                            name.to_string()
                        };

                        debug!(name = %name, camera_id = %camera_id, "Found libcamera camera");
                        cameras.push(CameraDevice {
                            name: name.to_string(),
                            path: camera_id.clone(),
                            metadata_path: Some(camera_id),
                            device_info: None,
                        });
                    }
                }
            }
        }
    }

    if cameras.is_empty() {
        debug!("No cameras found via cam CLI");
        None
    } else {
        info!(count = cameras.len(), "Enumerated cameras via cam CLI");
        Some(cameras)
    }
}

/// Get supported formats for a libcamera camera
/// Queries actual supported formats from libcamera
pub fn get_libcamera_formats(device_path: &str, metadata_path: Option<&str>) -> Vec<CameraFormat> {
    debug!(device_path, metadata_path = ?metadata_path, "Getting libcamera formats");

    // Try to enumerate formats from the camera
    if let Some(camera_id) = metadata_path.or(if device_path.is_empty() {
        None
    } else {
        Some(device_path)
    }) {
        if let Some(formats) = try_enumerate_formats_from_camera(camera_id) {
            info!(count = formats.len(), camera_id = %camera_id, "Enumerated formats via cam CLI");
            return formats;
        } else {
            warn!(camera_id = %camera_id, "Failed to enumerate formats from camera, using fallback");
        }
    } else {
        warn!(
            device_path,
            "No camera ID provided for format enumeration, using fallback"
        );
    }

    // Fallback: return common mobile camera formats
    get_fallback_formats()
}

/// Fallback formats for mobile cameras when enumeration fails
fn get_fallback_formats() -> Vec<CameraFormat> {
    let mut formats = Vec::new();

    // Common mobile camera resolutions (lower than desktop due to ISP constraints)
    let resolutions = [
        (1920, 1080), // 1080p
        (1280, 720),  // 720p
        (640, 480),   // VGA
    ];

    // Mobile cameras typically output NV12 from the ISP
    for &(width, height) in &resolutions {
        for &fps in formats::COMMON_FRAMERATES {
            // Only include reasonable framerates for mobile
            if fps <= 30 {
                formats.push(CameraFormat {
                    width,
                    height,
                    framerate: Some(fps),
                    hardware_accelerated: true,
                    pixel_format: "NV12".to_string(),
                });
            }
        }
    }
    formats
}

/// Try to enumerate formats from a libcamera camera using cam CLI
fn try_enumerate_formats_from_camera(camera_id: &str) -> Option<Vec<CameraFormat>> {
    debug!(camera_id, "Enumerating formats via cam CLI");

    // cam -c <camera_id> --list-properties shows camera info
    // cam -c <camera_id> --capture=1 shows supported formats during capture
    // For now, we'll use common mobile formats and let GStreamer negotiate

    // Try to get some info via cam -c <id> -I
    let output = std::process::Command::new("cam")
        .args(["-c", camera_id, "-I"])
        .output()
        .ok()?;

    if !output.status.success() {
        debug!("cam -c {} -I failed", camera_id);
        return None;
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let mut formats = Vec::new();
    let mut current_width: Option<u32> = None;
    let mut current_height: Option<u32> = None;

    // Parse the output for resolution information
    // Format varies, but look for lines containing resolution info
    for line in stdout.lines() {
        let trimmed = line.trim();

        // Look for resolution patterns like "1920x1080" or "Size: 1920x1080"
        if let Some(res_match) = find_resolution_in_line(trimmed) {
            current_width = Some(res_match.0);
            current_height = Some(res_match.1);
        }
    }

    // If we found a resolution, create formats for common framerates
    if let (Some(w), Some(h)) = (current_width, current_height) {
        for &fps in &[30u32, 15, 10] {
            formats.push(CameraFormat {
                width: w,
                height: h,
                framerate: Some(fps),
                hardware_accelerated: true,
                pixel_format: "NV12".to_string(),
            });
        }
    }

    // If no formats found from parsing, return None to use fallback
    if formats.is_empty() {
        None
    } else {
        Some(formats)
    }
}

/// Find resolution pattern (WxH) in a line
fn find_resolution_in_line(line: &str) -> Option<(u32, u32)> {
    // Look for patterns like "1920x1080" or "1920 x 1080"
    for word in line.split_whitespace() {
        if let Some((w_str, h_str)) = word.split_once('x') {
            if let (Ok(w), Ok(h)) = (w_str.parse::<u32>(), h_str.parse::<u32>()) {
                if w >= 320 && h >= 240 {
                    return Some((w, h));
                }
            }
        }
    }
    None
}
