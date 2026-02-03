// SPDX-License-Identifier: GPL-3.0-only

//! PipeWire camera enumeration and format detection
//!
//! This module provides camera discovery and format enumeration using PipeWire.
//! PipeWire handles all camera access, format negotiation, and decoding internally.

use super::super::types::{CameraDevice, CameraFormat, DeviceInfo, Framerate, SensorRotation};
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

    debug!("PipeWire available for camera enumeration");

    // PipeWire camera enumeration strategy:
    // 1. Try to discover cameras through pw-cli/pactl (if available)
    // 2. Otherwise, provide generic "Default Camera" that lets PipeWire auto-select

    let cameras = try_enumerate_with_pw_cli().or_else(try_enumerate_with_pactl);

    if let Some(ref cams) = cameras {
        debug!(count = cams.len(), "Found PipeWire cameras");
        return Some(cams.clone());
    }

    // Fallback: Let PipeWire use its default camera
    info!("Using PipeWire auto-selection (default camera)");
    Some(vec![CameraDevice {
        name: "Default Camera (PipeWire)".to_string(),
        path: String::new(), // Empty path = PipeWire auto-selects
        metadata_path: None,
        device_info: None,
        rotation: SensorRotation::None,
    }])
}

/// Try to enumerate cameras using pw-cli command
fn try_enumerate_with_pw_cli() -> Option<Vec<CameraDevice>> {
    debug!("Trying pw-cli for camera enumeration");

    let output = std::process::Command::new("pw-cli")
        .args(["ls", "Node"])
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
    let mut current_nick: Option<String> = None;
    let mut current_object_path: Option<String> = None;
    let mut is_video_source = false;

    for line in stdout.lines() {
        let trimmed = line.trim();

        // Look for node ID (format: "id 76, type PipeWire:Interface:Node/3")
        if trimmed.starts_with("id ") && trimmed.contains("type PipeWire:Interface:Node") {
            // Save previous camera if valid
            if is_video_source
                && let (Some(id), Some(name)) = (current_id.as_ref(), current_name.as_ref())
            {
                // Skip our own virtual camera output to avoid self-detection
                if name.contains("Camera (Virtual)") {
                    debug!(name = %name, "Skipping self (virtual camera output)");
                } else {
                    // Priority: use object.serial for target-object, fallback to node ID
                    let path = if let Some(serial) = current_serial.as_ref() {
                        format!("pipewire-serial-{}", serial)
                    } else {
                        format!("pipewire-{}", id)
                    };

                    // Build device info from captured properties
                    let device_info =
                        build_device_info(current_nick.as_deref(), current_object_path.as_deref());

                    // Query rotation from pw-cli info (not available in pw-cli ls output)
                    let rotation = query_node_rotation(id);

                    debug!(id = %id, serial = ?current_serial, name = %name, path = %path, rotation = %rotation, "Found video camera");
                    cameras.push(CameraDevice {
                        name: name.clone(),
                        path,
                        metadata_path: Some(id.clone()), // Store node ID in metadata_path for format enumeration
                        device_info,
                        rotation,
                    });
                }
            }

            // Parse new ID (extract number between "id " and ",")
            if let Some(id_str) = trimmed.strip_prefix("id ")
                && let Some(id_num) = id_str.split(',').next()
            {
                let id_clean = id_num.trim().trim_end_matches(',');
                current_id = Some(id_clean.to_string());
                current_serial = None;
                current_name = None;
                current_nick = None;
                current_object_path = None;
                is_video_source = false;
            }
        }

        // Look for media.class property indicating video source
        // Format: media.class = "Video/Source"
        if trimmed.contains("media.class") && trimmed.contains("\"Video/Source\"") {
            is_video_source = true;
        }

        // Look for object.serial for PipeWire target-object property
        // Format: object.serial = "2146"
        if trimmed.contains("object.serial")
            && let Some(value) = extract_quoted_value(trimmed)
        {
            current_serial = Some(value);
            debug!(serial = %current_serial.as_ref().unwrap(), "Found object.serial");
        }

        // Look for object.path for V4L2 device path
        // Format: object.path = "v4l2:/dev/video0"
        if trimmed.contains("object.path")
            && let Some(value) = extract_quoted_value(trimmed)
        {
            current_object_path = Some(value);
            debug!(object_path = %current_object_path.as_ref().unwrap(), "Found object.path");
        }

        // Look for node.nick for card name
        // Format: node.nick = "Laptop Webcam Module (2nd Gen)"
        if trimmed.contains("node.nick")
            && let Some(value) = extract_quoted_value(trimmed)
        {
            current_nick = Some(value);
            debug!(nick = %current_nick.as_ref().unwrap(), "Found node.nick");
        }

        // Look for node.description for camera name
        // Format: node.description = "Laptop Webcam Module (2nd Gen) (V4L2)"
        if trimmed.contains("node.description")
            && let Some(value) = extract_quoted_value(trimmed)
        {
            current_name = Some(value);
            debug!(name = %current_name.as_ref().unwrap(), "Found node description");
        }
    }

    // Don't forget the last camera
    if is_video_source && let (Some(id), Some(name)) = (current_id.as_ref(), current_name.as_ref())
    {
        // Skip our own virtual camera output to avoid self-detection
        if name.contains("Camera (Virtual)") {
            debug!(name = %name, "Skipping self (virtual camera output)");
        } else {
            let path = if let Some(serial) = current_serial.as_ref() {
                format!("pipewire-serial-{}", serial)
            } else {
                format!("pipewire-{}", id)
            };

            // Build device info from captured properties
            let device_info =
                build_device_info(current_nick.as_deref(), current_object_path.as_deref());

            // Query rotation from pw-cli info (not available in pw-cli ls output)
            let rotation = query_node_rotation(id);

            debug!(id = %id, serial = ?current_serial, name = %name, path = %path, rotation = %rotation, "Found video camera (last)");
            cameras.push(CameraDevice {
                name: name.clone(),
                path,
                metadata_path: Some(id.clone()), // Store node ID in metadata_path for format enumeration
                device_info,
                rotation,
            });
        }
    }

    if cameras.is_empty() {
        debug!("No cameras found via pw-cli");
        None
    } else {
        debug!(count = cameras.len(), "Enumerated cameras via pw-cli");
        Some(cameras)
    }
}

/// Extract quoted value from a property line (e.g., 'property = "value"' -> "value")
fn extract_quoted_value(line: &str) -> Option<String> {
    let start = line.find('"')?;
    let end = line[start + 1..].find('"')?;
    Some(line[start + 1..start + 1 + end].to_string())
}

/// Query rotation for a PipeWire node using pw-cli info
/// This is needed because pw-cli ls Node doesn't include api.libcamera.rotation
fn query_node_rotation(node_id: &str) -> SensorRotation {
    let output = match std::process::Command::new("pw-cli")
        .args(["info", node_id])
        .output()
    {
        Ok(output) if output.status.success() => output,
        _ => {
            debug!(node_id, "Failed to query node info for rotation");
            return SensorRotation::default();
        }
    };

    let stdout = String::from_utf8_lossy(&output.stdout);

    for line in stdout.lines() {
        let trimmed = line.trim();
        // Look for: api.libcamera.rotation = "90"
        if trimmed.contains("api.libcamera.rotation")
            && let Some(value) = extract_quoted_value(trimmed)
        {
            debug!(node_id, rotation = %value, "Found rotation from pw-cli info");
            return SensorRotation::from_degrees(&value);
        }
    }

    SensorRotation::default()
}

/// Build DeviceInfo from PipeWire properties and V4L2 device info
fn build_device_info(nick: Option<&str>, object_path: Option<&str>) -> Option<DeviceInfo> {
    // Extract V4L2 device path from object.path (format: "v4l2:/dev/video0")
    let v4l2_path = object_path.and_then(|p| p.strip_prefix("v4l2:"));

    let v4l2_path = match v4l2_path {
        Some(p) => p.to_string(),
        None => return None,
    };

    // Get real path by resolving symlinks
    let real_path = std::fs::canonicalize(&v4l2_path)
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_else(|_| v4l2_path.clone());

    // Get driver name using V4L2 ioctl
    let driver = get_v4l2_driver(&v4l2_path).unwrap_or_default();

    // Use node.nick as the card name, fallback to empty
    let card = nick.unwrap_or_default().to_string();

    Some(DeviceInfo {
        card,
        driver,
        path: v4l2_path,
        real_path,
    })
}

/// Get V4L2 driver name using ioctl
fn get_v4l2_driver(device_path: &str) -> Option<String> {
    use std::os::unix::io::AsRawFd;

    // VIDIOC_QUERYCAP ioctl number
    const VIDIOC_QUERYCAP: libc::c_ulong = 0x80685600;

    // V4L2 capability structure (simplified - we only need driver field)
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

    let file = std::fs::File::open(device_path).ok()?;
    let fd = file.as_raw_fd();

    let mut cap = V4l2Capability {
        driver: [0; 16],
        card: [0; 32],
        bus_info: [0; 32],
        version: 0,
        capabilities: 0,
        device_caps: 0,
        reserved: [0; 3],
    };

    let result = unsafe {
        libc::syscall(
            libc::SYS_ioctl,
            fd,
            VIDIOC_QUERYCAP,
            &mut cap as *mut V4l2Capability,
        )
    };

    if result < 0 {
        debug!(device_path, "Failed to query V4L2 capability");
        return None;
    }

    // Convert driver name from null-terminated bytes to String
    let driver_len = cap.driver.iter().position(|&c| c == 0).unwrap_or(16);
    let driver = String::from_utf8_lossy(&cap.driver[..driver_len]).to_string();

    debug!(device_path, driver = %driver, "Got V4L2 driver name");
    Some(driver)
}

/// Try to enumerate cameras using pactl command (PipeWire)
fn try_enumerate_with_pactl() -> Option<Vec<CameraDevice>> {
    debug!("Trying pactl for camera enumeration");

    let output = std::process::Command::new("pactl")
        .args(["list", "sources"])
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
    // Note: pactl doesn't provide rotation info, so we default to None
    for line in stdout.lines() {
        if line.contains("Name:")
            && line.contains("video")
            && let Some(name) = line.split(':').nth(1)
        {
            cameras.push(CameraDevice {
                name: name.trim().to_string(),
                path: name.trim().to_string(),
                metadata_path: None,
                device_info: None,
                rotation: SensorRotation::None,
            });
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
                framerate: Some(Framerate::from_int(fps)),
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
        .args(["enum-params", node_id, "EnumFormat"])
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
    let mut current_framerates: Vec<Framerate> = Vec::new();
    let mut current_subtype: Option<String> = None;
    let mut current_video_format: Option<String> = None;

    for line in stdout.lines() {
        let trimmed = line.trim();

        // Look for mediaSubtype (format type: raw, mjpg, h264, etc.)
        // Format: Id 1   (Spa:Enum:MediaSubtype:raw)
        // Format: Id 131074   (Spa:Enum:MediaSubtype:mjpg)
        if trimmed.contains("Spa:Enum:MediaSubtype:")
            && let Some(subtype_start) = trimmed.rfind(':')
        {
            let subtype = &trimmed[subtype_start + 1..].trim_end_matches(')');
            current_subtype = Some(subtype.to_lowercase());
            debug!(subtype = %subtype, "Found media subtype");
        }

        // Look for VideoFormat (only present for raw formats)
        // Format: Id 4   (Spa:Enum:VideoFormat:YUY2)
        if trimmed.contains("Spa:Enum:VideoFormat:")
            && let Some(format_start) = trimmed.rfind(':')
        {
            let video_format = &trimmed[format_start + 1..].trim_end_matches(')');
            current_video_format = Some(video_format.to_uppercase());
            debug!(video_format = %video_format, "Found video format");
        }

        // Look for resolution
        // Format: Rectangle 1920x1080
        if trimmed.starts_with("Rectangle ")
            && let Some(res_str) = trimmed.strip_prefix("Rectangle ")
            && let Some((w_str, h_str)) = res_str.split_once('x')
        {
            current_width = w_str.parse().ok();
            current_height = h_str.parse().ok();
            debug!(width = ?current_width, height = ?current_height, "Found resolution");
        }

        // Look for framerate
        // Format: Fraction 60/1 or Fraction 60000/1001
        if trimmed.starts_with("Fraction ")
            && let Some(frac_str) = trimmed.strip_prefix("Fraction ")
            && let Some((num_str, denom_str)) = frac_str.split_once('/')
            && let (Ok(num), Ok(denom)) = (num_str.parse::<u32>(), denom_str.parse::<u32>())
            && denom > 0
        {
            let fps = Framerate::new(num, denom);
            // Check for duplicate by integer fps value (e.g., 60000/1001 and 60/1 both ~ 60fps)
            if !current_framerates
                .iter()
                .any(|f| f.as_int() == fps.as_int())
            {
                current_framerates.push(fps);
            }
        }

        // When we hit a new Object, save the previous format
        if trimmed.starts_with("Object:") {
            if let (Some(w), Some(h), Some(subtype)) =
                (current_width, current_height, &current_subtype)
            {
                // Determine the pixel format string:
                // - For raw formats: use VideoFormat (YUY2, NV12, RGBA, etc.)
                // - For compressed formats: use MediaSubtype (MJPG, H264, etc.)
                let pixel_format = if subtype == "raw" {
                    current_video_format
                        .clone()
                        .unwrap_or_else(|| "YUY2".to_string())
                } else {
                    subtype.to_uppercase()
                };

                // Check if this is a libcamera device (no framerates in EnumFormat)
                // libcamera uses FrameDurationLimits for flexible framerate control,
                // which is not exposed via PipeWire's EnumFormat.
                let is_libcamera = current_framerates.is_empty();

                if is_libcamera {
                    // libcamera doesn't expose framerates via PipeWire EnumFormat.
                    // Only offer VFR/Auto mode - let libcamera negotiate the best framerate
                    // per resolution via FrameDurationLimits.
                    formats.push(CameraFormat {
                        width: w,
                        height: h,
                        framerate: None, // VFR/Auto - libcamera manages via FrameDurationLimits
                        hardware_accelerated: false,
                        pixel_format: pixel_format.clone(),
                    });
                } else {
                    // V4L2 device with explicit framerates - use them
                    for fps in &current_framerates {
                        formats.push(CameraFormat {
                            width: w,
                            height: h,
                            framerate: Some(*fps),
                            hardware_accelerated: pixel_format == "MJPG", // MJPEG is hardware accelerated
                            pixel_format: pixel_format.clone(),
                        });
                    }
                }
                debug!(width = w, height = h, pixel_format = %pixel_format, framerates = current_framerates.len(), is_libcamera = is_libcamera, "Completed format group");
            }
            current_width = None;
            current_height = None;
            current_framerates.clear();
            current_subtype = None;
            current_video_format = None;
        }
    }

    // Don't forget the last format
    if let (Some(w), Some(h), Some(subtype)) = (current_width, current_height, &current_subtype) {
        let pixel_format = if subtype == "raw" {
            current_video_format
                .clone()
                .unwrap_or_else(|| "YUY2".to_string())
        } else {
            subtype.to_uppercase()
        };

        // Check if this is a libcamera device (no framerates in EnumFormat)
        let is_libcamera = current_framerates.is_empty();

        if is_libcamera {
            // libcamera doesn't expose framerates via PipeWire EnumFormat.
            // Only offer VFR/Auto mode - let libcamera negotiate the best framerate.
            formats.push(CameraFormat {
                width: w,
                height: h,
                framerate: None, // VFR/Auto - libcamera manages via FrameDurationLimits
                hardware_accelerated: false,
                pixel_format: pixel_format.clone(),
            });
        } else {
            // V4L2 device with explicit framerates - use them
            for fps in &current_framerates {
                formats.push(CameraFormat {
                    width: w,
                    height: h,
                    framerate: Some(*fps),
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
