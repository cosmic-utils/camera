// SPDX-License-Identifier: MPL-2.0

//! PipeWire camera enumeration and format detection
//!
//! This module provides camera discovery and format enumeration using PipeWire.
//! PipeWire handles all camera access, format negotiation, and decoding internally.

use super::super::types::{CameraDevice, CameraFormat};
use crate::constants::formats;
use tracing::{debug, info, warn};

/// Enumerate cameras using PipeWire
/// Returns list of available cameras discovered through PipeWire
pub fn enumerate_pipewire_cameras() -> Option<Vec<CameraDevice>> {
    debug!("Attempting to enumerate cameras via PipeWire");

    // Check if PipeWire is available
    if gstreamer::init().is_err() {
        warn!("GStreamer init failed");
        return None;
    }

    // Check if pipewiresrc element exists
    if gstreamer::ElementFactory::make("pipewiresrc")
        .build()
        .is_err()
    {
        debug!("pipewiresrc not available");
        return None;
    }

    info!("✓ PipeWire available for camera enumeration");

    // PipeWire camera enumeration strategy:
    // 1. Try to discover cameras through pw-cli/pactl (if available)
    // 2. Otherwise, provide generic "Default Camera" that lets PipeWire auto-select

    let cameras = try_enumerate_with_pw_cli().or_else(|| try_enumerate_with_pactl());

    if let Some(ref cams) = cameras {
        info!(count = cams.len(), "Found PipeWire cameras");
        return Some(cams.clone());
    }

    // Fallback: Let PipeWire use its default camera
    info!("Using PipeWire auto-selection (default camera)");
    Some(vec![CameraDevice {
        name: "Default Camera (PipeWire)".to_string(),
        path: String::new(), // Empty path = PipeWire auto-selects
        metadata_path: None,
    }])
}

/// Try to enumerate cameras using pw-cli command
fn try_enumerate_with_pw_cli() -> Option<Vec<CameraDevice>> {
    debug!("Trying pw-cli for camera enumeration");

    let output = std::process::Command::new("pw-cli")
        .args(&["ls", "Node"])
        .output()
        .ok()?;

    if !output.status.success() {
        debug!("pw-cli command failed");
        return None;
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let mut cameras = Vec::new();
    let mut current_id: Option<String> = None;
    let mut current_serial: Option<String> = None;
    let mut current_name: Option<String> = None;
    let mut is_video_source = false;

    for line in stdout.lines() {
        let trimmed = line.trim();

        // Look for node ID (format: "id 76, type PipeWire:Interface:Node/3")
        if trimmed.starts_with("id ") && trimmed.contains("type PipeWire:Interface:Node") {
            // Save previous camera if valid
            if is_video_source {
                if let (Some(id), Some(name)) = (current_id.as_ref(), current_name.as_ref()) {
                    // Priority: use object.serial for target-object, fallback to node ID
                    let path = if let Some(serial) = current_serial.as_ref() {
                        format!("pipewire-serial-{}", serial)
                    } else {
                        format!("pipewire-{}", id)
                    };
                    debug!(id = %id, serial = ?current_serial, name = %name, path = %path, "Found video camera");
                    cameras.push(CameraDevice {
                        name: name.clone(),
                        path,
                        metadata_path: Some(id.clone()), // Store node ID in metadata_path for format enumeration
                    });
                }
            }

            // Parse new ID (extract number between "id " and ",")
            if let Some(id_str) = trimmed.strip_prefix("id ") {
                if let Some(id_num) = id_str.split(',').next() {
                    let id_clean = id_num.trim().trim_end_matches(',');
                    current_id = Some(id_clean.to_string());
                    current_serial = None;
                    current_name = None;
                    is_video_source = false;
                }
            }
        }

        // Look for media.class property indicating video source
        // Format: media.class = "Video/Source"
        if trimmed.contains("media.class") && trimmed.contains("\"Video/Source\"") {
            is_video_source = true;
        }

        // Look for object.serial for PipeWire target-object property
        // Format: object.serial = "2146"
        if trimmed.contains("object.serial") {
            if let Some(serial_start) = trimmed.find('"') {
                if let Some(serial_end) = trimmed[serial_start + 1..].find('"') {
                    let serial = &trimmed[serial_start + 1..serial_start + 1 + serial_end];
                    current_serial = Some(serial.to_string());
                    debug!(serial = %serial, "Found object.serial");
                }
            }
        }

        // Look for node.description for camera name
        // Format: node.description = "Laptop Webcam Module (2nd Gen) (V4L2)"
        if trimmed.contains("node.description") {
            if let Some(desc_start) = trimmed.find('"') {
                if let Some(desc_end) = trimmed[desc_start + 1..].find('"') {
                    let name = &trimmed[desc_start + 1..desc_start + 1 + desc_end];
                    current_name = Some(name.to_string());
                    debug!(name = %name, "Found node description");
                }
            }
        }
    }

    // Don't forget the last camera
    if is_video_source {
        if let (Some(id), Some(name)) = (current_id.as_ref(), current_name.as_ref()) {
            let path = if let Some(serial) = current_serial.as_ref() {
                format!("pipewire-serial-{}", serial)
            } else {
                format!("pipewire-{}", id)
            };
            debug!(id = %id, serial = ?current_serial, name = %name, path = %path, "Found video camera (last)");
            cameras.push(CameraDevice {
                name: name.clone(),
                path,
                metadata_path: Some(id.clone()), // Store node ID in metadata_path for format enumeration
            });
        }
    }

    if cameras.is_empty() {
        debug!("No cameras found via pw-cli");
        None
    } else {
        info!(count = cameras.len(), "Enumerated cameras via pw-cli");
        Some(cameras)
    }
}

/// Try to enumerate cameras using pactl command (PipeWire)
fn try_enumerate_with_pactl() -> Option<Vec<CameraDevice>> {
    debug!("Trying pactl for camera enumeration");

    let output = std::process::Command::new("pactl")
        .args(&["list", "sources"])
        .output()
        .ok()?;

    if !output.status.success() {
        debug!("pactl command failed");
        return None;
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let mut cameras = Vec::new();

    // Simple parsing - look for video sources
    // This is a basic implementation, may need refinement
    for line in stdout.lines() {
        if line.contains("Name:") && line.contains("video") {
            if let Some(name) = line.split(':').nth(1) {
                cameras.push(CameraDevice {
                    name: name.trim().to_string(),
                    path: name.trim().to_string(),
                    metadata_path: None,
                });
            }
        }
    }

    if cameras.is_empty() {
        None
    } else {
        info!(count = cameras.len(), "Enumerated cameras via pactl");
        Some(cameras)
    }
}

/// Get supported formats for a PipeWire camera
/// Queries actual supported formats from PipeWire using pw-cli enum-params
pub fn get_pipewire_formats(device_path: &str, metadata_path: Option<&str>) -> Vec<CameraFormat> {
    debug!(device_path, metadata_path = ?metadata_path, "Getting PipeWire formats");

    // metadata_path contains the node ID for PipeWire cameras
    if let Some(node_id) = metadata_path {
        if let Some(formats) = try_enumerate_formats_from_node(node_id) {
            info!(count = formats.len(), node_id = %node_id, "Enumerated formats via pw-cli");
            return formats;
        } else {
            warn!(node_id = %node_id, "Failed to enumerate formats from node, using fallback");
        }
    } else {
        warn!(
            device_path,
            "No node ID provided for format enumeration, using fallback"
        );
    }

    // Fallback: return common formats if we can't query PipeWire
    get_fallback_formats()
}

/// Fallback formats when PipeWire enumeration fails
fn get_fallback_formats() -> Vec<CameraFormat> {
    let mut formats = Vec::new();
    let resolutions = [
        (3840, 2160), // 4K
        (1920, 1080), // 1080p
        (1280, 720),  // 720p
        (640, 480),   // VGA
    ];

    for &(width, height) in &resolutions {
        for &fps in formats::COMMON_FRAMERATES {
            formats.push(CameraFormat {
                width,
                height,
                framerate: Some(fps),
                hardware_accelerated: true,
                pixel_format: "MJPG".to_string(),
            });
        }
    }
    formats
}

/// Try to enumerate formats from a PipeWire node using pw-cli
fn try_enumerate_formats_from_node(node_id: &str) -> Option<Vec<CameraFormat>> {
    debug!(node_id, "Enumerating formats via pw-cli enum-params");

    let output = std::process::Command::new("pw-cli")
        .args(&["enum-params", node_id, "EnumFormat"])
        .output()
        .ok()?;

    if !output.status.success() {
        debug!("pw-cli enum-params failed");
        return None;
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let mut formats = Vec::new();
    let mut current_width: Option<u32> = None;
    let mut current_height: Option<u32> = None;
    let mut current_framerates: Vec<u32> = Vec::new();
    let mut current_subtype: Option<String> = None;
    let mut current_video_format: Option<String> = None;

    for line in stdout.lines() {
        let trimmed = line.trim();

        // Look for mediaSubtype (format type: raw, mjpg, h264, etc.)
        // Format: Id 1   (Spa:Enum:MediaSubtype:raw)
        // Format: Id 131074   (Spa:Enum:MediaSubtype:mjpg)
        if trimmed.contains("Spa:Enum:MediaSubtype:") {
            if let Some(subtype_start) = trimmed.rfind(':') {
                let subtype = &trimmed[subtype_start + 1..].trim_end_matches(')');
                current_subtype = Some(subtype.to_lowercase());
                debug!(subtype = %subtype, "Found media subtype");
            }
        }

        // Look for VideoFormat (only present for raw formats)
        // Format: Id 4   (Spa:Enum:VideoFormat:YUY2)
        if trimmed.contains("Spa:Enum:VideoFormat:") {
            if let Some(format_start) = trimmed.rfind(':') {
                let video_format = &trimmed[format_start + 1..].trim_end_matches(')');
                current_video_format = Some(video_format.to_uppercase());
                debug!(video_format = %video_format, "Found video format");
            }
        }

        // Look for resolution
        // Format: Rectangle 1920x1080
        if trimmed.starts_with("Rectangle ") {
            if let Some(res_str) = trimmed.strip_prefix("Rectangle ") {
                if let Some((w_str, h_str)) = res_str.split_once('x') {
                    current_width = w_str.parse().ok();
                    current_height = h_str.parse().ok();
                    debug!(width = ?current_width, height = ?current_height, "Found resolution");
                }
            }
        }

        // Look for framerate
        // Format: Fraction 60/1
        if trimmed.starts_with("Fraction ") {
            if let Some(frac_str) = trimmed.strip_prefix("Fraction ") {
                if let Some((num_str, denom_str)) = frac_str.split_once('/') {
                    if let (Ok(num), Ok(denom)) = (num_str.parse::<u32>(), denom_str.parse::<u32>())
                    {
                        if denom > 0 {
                            let fps = num / denom;
                            if !current_framerates.contains(&fps) {
                                current_framerates.push(fps);
                            }
                        }
                    }
                }
            }
        }

        // When we hit a new Object, save the previous format
        if trimmed.starts_with("Object:") && !current_framerates.is_empty() {
            if let (Some(w), Some(h), Some(subtype)) =
                (current_width, current_height, &current_subtype)
            {
                // Determine the pixel format string:
                // - For raw formats: use VideoFormat (YUY2, NV12, etc.)
                // - For compressed formats: use MediaSubtype (MJPG, H264, etc.)
                let pixel_format = if subtype == "raw" {
                    current_video_format
                        .clone()
                        .unwrap_or_else(|| "YUY2".to_string())
                } else {
                    subtype.to_uppercase()
                };

                // Create a format for each framerate
                for &fps in &current_framerates {
                    formats.push(CameraFormat {
                        width: w,
                        height: h,
                        framerate: Some(fps),
                        hardware_accelerated: pixel_format == "MJPG", // MJPEG is hardware accelerated
                        pixel_format: pixel_format.clone(),
                    });
                }
                debug!(width = w, height = h, pixel_format = %pixel_format, framerates = current_framerates.len(), "Completed format group");
            }
            current_width = None;
            current_height = None;
            current_framerates.clear();
            current_subtype = None;
            current_video_format = None;
        }
    }

    // Don't forget the last format
    if !current_framerates.is_empty() {
        if let (Some(w), Some(h), Some(subtype)) = (current_width, current_height, &current_subtype)
        {
            let pixel_format = if subtype == "raw" {
                current_video_format
                    .clone()
                    .unwrap_or_else(|| "YUY2".to_string())
            } else {
                subtype.to_uppercase()
            };

            for &fps in &current_framerates {
                formats.push(CameraFormat {
                    width: w,
                    height: h,
                    framerate: Some(fps),
                    hardware_accelerated: pixel_format == "MJPG",
                    pixel_format: pixel_format.clone(),
                });
            }
        }
    }

    if formats.is_empty() {
        None
    } else {
        Some(formats)
    }
}

/// Test if PipeWire is available and working
pub fn is_pipewire_available() -> bool {
    if gstreamer::init().is_err() {
        return false;
    }

    gstreamer::ElementFactory::make("pipewiresrc")
        .build()
        .is_ok()
}
