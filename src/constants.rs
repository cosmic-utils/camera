// SPDX-License-Identifier: GPL-3.0-only

//! Application-wide constants

use serde::{Deserialize, Serialize};
use std::time::Duration;

/// Video encoder bitrate presets
///
/// These presets define the target bitrate for video encoding based on resolution.
/// Users can choose between quality and file size trade-offs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum BitratePreset {
    /// Low bitrate - smaller files, reduced quality
    Low,
    /// Medium bitrate - balanced quality and file size (default)
    #[default]
    Medium,
    /// High bitrate - larger files, better quality
    High,
}

impl BitratePreset {
    /// Get all preset variants for UI iteration
    pub const ALL: [BitratePreset; 3] = [
        BitratePreset::Low,
        BitratePreset::Medium,
        BitratePreset::High,
    ];

    /// Get display name for the preset
    pub fn display_name(&self) -> &'static str {
        match self {
            BitratePreset::Low => "Low",
            BitratePreset::Medium => "Medium",
            BitratePreset::High => "High",
        }
    }

    /// Get bitrate in kbps for a given resolution
    ///
    /// Bitrates are tuned for good quality at each resolution tier:
    /// - SD (640x480): Low=1, Medium=2, High=4 Mbps
    /// - HD (1280x720): Low=2.5, Medium=5, High=10 Mbps
    /// - Full HD (1920x1080): Low=4, Medium=8, High=16 Mbps
    /// - 2K (2560x1440): Low=8, Medium=16, High=32 Mbps
    /// - 4K (3840x2160): Low=15, Medium=30, High=50 Mbps
    pub fn bitrate_kbps(&self, width: u32, _height: u32) -> u32 {
        let resolution_tier = get_resolution_tier(width);

        match (resolution_tier, self) {
            // SD (640x480 and below)
            (ResolutionTier::SD, BitratePreset::Low) => 1_000,
            (ResolutionTier::SD, BitratePreset::Medium) => 2_000,
            (ResolutionTier::SD, BitratePreset::High) => 4_000,
            // HD (1280x720)
            (ResolutionTier::HD, BitratePreset::Low) => 2_500,
            (ResolutionTier::HD, BitratePreset::Medium) => 5_000,
            (ResolutionTier::HD, BitratePreset::High) => 10_000,
            // Full HD (1920x1080)
            (ResolutionTier::FullHD, BitratePreset::Low) => 4_000,
            (ResolutionTier::FullHD, BitratePreset::Medium) => 8_000,
            (ResolutionTier::FullHD, BitratePreset::High) => 16_000,
            // 2K (2560x1440)
            (ResolutionTier::TwoK, BitratePreset::Low) => 8_000,
            (ResolutionTier::TwoK, BitratePreset::Medium) => 16_000,
            (ResolutionTier::TwoK, BitratePreset::High) => 32_000,
            // 4K (3840x2160 and above)
            (ResolutionTier::FourK, BitratePreset::Low) => 15_000,
            (ResolutionTier::FourK, BitratePreset::Medium) => 30_000,
            (ResolutionTier::FourK, BitratePreset::High) => 50_000,
        }
    }

    /// Get the bitrate for a specific resolution tier (for matrix display)
    pub fn bitrate_for_tier(&self, tier: ResolutionTier) -> u32 {
        match (tier, self) {
            (ResolutionTier::SD, BitratePreset::Low) => 1_000,
            (ResolutionTier::SD, BitratePreset::Medium) => 2_000,
            (ResolutionTier::SD, BitratePreset::High) => 4_000,
            (ResolutionTier::HD, BitratePreset::Low) => 2_500,
            (ResolutionTier::HD, BitratePreset::Medium) => 5_000,
            (ResolutionTier::HD, BitratePreset::High) => 10_000,
            (ResolutionTier::FullHD, BitratePreset::Low) => 4_000,
            (ResolutionTier::FullHD, BitratePreset::Medium) => 8_000,
            (ResolutionTier::FullHD, BitratePreset::High) => 16_000,
            (ResolutionTier::TwoK, BitratePreset::Low) => 8_000,
            (ResolutionTier::TwoK, BitratePreset::Medium) => 16_000,
            (ResolutionTier::TwoK, BitratePreset::High) => 32_000,
            (ResolutionTier::FourK, BitratePreset::Low) => 15_000,
            (ResolutionTier::FourK, BitratePreset::Medium) => 30_000,
            (ResolutionTier::FourK, BitratePreset::High) => 50_000,
        }
    }
}

/// Resolution tiers for bitrate calculation
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResolutionTier {
    /// SD: 640x480 and below
    SD,
    /// HD: 1280x720
    HD,
    /// Full HD: 1920x1080
    FullHD,
    /// 2K: 2560x1440
    TwoK,
    /// 4K: 3840x2160 and above
    FourK,
}

impl ResolutionTier {
    /// Get all tiers for UI iteration (HD = 1080p, excludes 720p)
    pub const ALL: [ResolutionTier; 4] = [
        ResolutionTier::SD,
        ResolutionTier::FullHD,
        ResolutionTier::TwoK,
        ResolutionTier::FourK,
    ];

    /// Get display name for the tier
    pub fn display_name(&self) -> &'static str {
        match self {
            ResolutionTier::SD => "SD",
            ResolutionTier::HD => "720p",
            ResolutionTier::FullHD => "HD",
            ResolutionTier::TwoK => "2K",
            ResolutionTier::FourK => "4K",
        }
    }

    /// Get typical resolution for this tier
    pub fn typical_resolution(&self) -> &'static str {
        match self {
            ResolutionTier::SD => "640×480",
            ResolutionTier::HD => "1280×720",
            ResolutionTier::FullHD => "1920×1080",
            ResolutionTier::TwoK => "2560×1440",
            ResolutionTier::FourK => "3840×2160",
        }
    }
}

/// Get the resolution tier for a given width
pub fn get_resolution_tier(width: u32) -> ResolutionTier {
    match width {
        w if w >= 3840 => ResolutionTier::FourK,
        w if w >= 2560 => ResolutionTier::TwoK,
        w if w >= 1920 => ResolutionTier::FullHD,
        w if w >= 1280 => ResolutionTier::HD,
        _ => ResolutionTier::SD,
    }
}

/// Format bitrate for display (e.g., "8 Mbps" or "2.5 Mbps")
pub fn format_bitrate(kbps: u32) -> String {
    let mbps = kbps as f64 / 1000.0;
    if mbps == mbps.floor() {
        format!("{} Mbps", mbps as u32)
    } else {
        format!("{:.1} Mbps", mbps)
    }
}

/// UI Constants
pub mod ui {
    /// Button width for format picker
    pub const PICKER_BUTTON_WIDTH: f32 = 50.0;

    /// Capture button size (outer)
    pub const CAPTURE_BUTTON_OUTER: f32 = 60.0;

    /// Capture button size (inner)
    pub const CAPTURE_BUTTON_INNER: f32 = 50.0;

    /// Capture button border radius
    pub const CAPTURE_BUTTON_RADIUS: f32 = 25.0;

    /// Overlay button/container background transparency (0.0 = transparent, 1.0 = opaque)
    ///
    /// Used for semi-transparent backgrounds on buttons and panels overlaid on the camera preview.
    pub const OVERLAY_BACKGROUND_ALPHA: f32 = 0.6;

    /// Format picker border radius
    pub const PICKER_BORDER_RADIUS: f32 = 8.0;

    /// Placeholder button width when camera switch is hidden
    pub const PLACEHOLDER_BUTTON_WIDTH: f32 = 40.0;

    /// Standard icon button width (for layout balancing)
    pub const ICON_BUTTON_WIDTH: f32 = 44.0;

    /// Picker label text size
    pub const PICKER_LABEL_TEXT_SIZE: u16 = 12;

    /// Picker label width
    pub const PICKER_LABEL_WIDTH: f32 = 80.0;

    /// Resolution label text size in top bar
    pub const RES_LABEL_TEXT_SIZE: u16 = 14;

    /// Superscript text size in top bar
    pub const SUPERSCRIPT_TEXT_SIZE: u16 = 8;

    /// Superscript padding (bottom padding to push text up)
    pub const SUPERSCRIPT_PADDING: [u16; 4] = [0, 0, 4, 0];

    /// Resolution label spacing
    pub const RES_LABEL_SPACING: u16 = 1;

    /// Default framerate display string
    pub const DEFAULT_FPS_DISPLAY: &str = "30";

    /// Default resolution label
    pub const DEFAULT_RES_LABEL: &str = "HD";
}

/// Resolution thresholds for label detection
pub mod resolution_thresholds {
    /// 4K threshold (3840x2160)
    pub const THRESHOLD_4K: u32 = 3840;

    /// Full HD threshold (1920x1080)
    pub const THRESHOLD_HD: u32 = 1920;

    /// HD 720p threshold (1280x720)
    pub const THRESHOLD_720P: u32 = 1280;
}

/// Video format constants
pub mod formats {
    /// Common frame rates to try when exact enumeration fails
    pub const COMMON_FRAMERATES: &[u32] = &[30, 60, 15, 24];

    /// Default resolution for picker selection
    pub const DEFAULT_PICKER_RESOLUTION: u32 = 1920;
}

/// GStreamer pipeline constants
pub mod pipeline {
    /// Maximum buffer queue size (keep small for low latency)
    pub const MAX_BUFFERS: u32 = 2;

    /// Get number of threads for videoconvert based on available CPU threads
    pub fn videoconvert_threads() -> u32 {
        std::thread::available_parallelism()
            .map(|n| n.get() as u32)
            .unwrap_or(4) // Fallback to 4 if detection fails
    }

    /// Output pixel format for appsink
    /// RGBA uses 4 bytes/pixel - native RGB for simplified GPU processing
    pub const OUTPUT_FORMAT: &str = "RGBA";
}

/// Timing constants
pub mod timing {
    /// Frame counter modulo for periodic logging
    pub const FRAME_LOG_INTERVAL: u64 = 30;

    /// GStreamer state change timeout for validation
    /// Reduced to minimize startup delay - we accept async state changes
    pub const STATE_CHANGE_TIMEOUT_MS: u64 = 50;

    /// Pipeline state change timeout on stop
    pub const STOP_TIMEOUT_SECS: u64 = 2;

    /// Pipeline playing state timeout on start
    pub const START_TIMEOUT_SECS: u64 = 5;
}

/// Resolution labels for format picker
pub fn get_resolution_label(width: u32) -> Option<&'static str> {
    match width {
        w if w >= 7680 => Some("8K"), // 7680x4320
        w if w >= 6144 => Some("6K"), // 6144x3456
        w if w >= 5120 => Some("5K"), // 5120x2880
        w if w >= 3840 => Some("4K"), // 3840x2160
        w if w >= 2560 => Some("2K"), // 2560x1440
        w if w >= 1920 => Some("HD"), // 1920x1080
        w if w >= 640 => Some("SD"),  // 640x480
        _ => None,
    }
}

/// Supported file formats for virtual camera file source
pub mod file_formats {
    /// Supported image file extensions
    pub const IMAGE_EXTENSIONS: &[&str] = &["png", "jpg", "jpeg", "gif", "bmp", "webp"];

    /// Supported video file extensions
    pub const VIDEO_EXTENSIONS: &[&str] = &["mp4", "mkv", "webm", "avi", "mov"];

    /// Check if a file extension is a supported image format
    pub fn is_image_extension(ext: &str) -> bool {
        IMAGE_EXTENSIONS.contains(&ext.to_lowercase().as_str())
    }

    /// Check if a file extension is a supported video format
    pub fn is_video_extension(ext: &str) -> bool {
        VIDEO_EXTENSIONS.contains(&ext.to_lowercase().as_str())
    }
}

/// Virtual camera timing constants
pub mod virtual_camera {
    use super::Duration;

    /// Progress update interval for video playback
    pub const PROGRESS_UPDATE_INTERVAL: Duration = Duration::from_millis(250);

    /// Frame rate for image streaming (~30fps)
    pub const IMAGE_STREAM_FRAME_DURATION: Duration = Duration::from_millis(33);

    /// Pause check interval when video is paused
    pub const PAUSE_CHECK_INTERVAL: Duration = Duration::from_millis(50);

    /// Audio pipeline startup wait time
    pub const AUDIO_PIPELINE_STARTUP_DELAY: Duration = Duration::from_millis(500);

    /// GStreamer pipeline timeout for video frame extraction
    pub const VIDEO_FRAME_TIMEOUT_SECS: u64 = 5;

    /// GStreamer pipeline timeout for duration query
    pub const DURATION_QUERY_TIMEOUT_SECS: u64 = 5;
}

/// Virtual camera output device type
///
/// Determines which sink to use for virtual camera output:
/// - PipeWire: Modern Linux multimedia framework (default)
/// - V4L2Loopback: Traditional V4L2 loopback device (better app compatibility)
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum VirtualCameraOutput {
    /// PipeWire virtual camera (pipewiresink)
    /// Works with PipeWire-aware applications
    #[default]
    PipeWire,
    /// V4L2 loopback device (v4l2sink)
    /// Works with applications that expect /dev/video* devices (e.g., Discord, Chrome)
    V4L2Loopback,
}

impl VirtualCameraOutput {
    /// Get all output variants for UI iteration
    pub const ALL: [VirtualCameraOutput; 2] = [
        VirtualCameraOutput::PipeWire,
        VirtualCameraOutput::V4L2Loopback,
    ];

    /// Get display name for the output type
    pub fn display_name(&self) -> &'static str {
        match self {
            VirtualCameraOutput::PipeWire => "PipeWire",
            VirtualCameraOutput::V4L2Loopback => "V4L2 Loopback",
        }
    }

    /// Check if this output type is available on the system
    pub fn is_available(&self) -> bool {
        match self {
            VirtualCameraOutput::PipeWire => is_pipewire_available(),
            VirtualCameraOutput::V4L2Loopback => is_v4l2loopback_available(),
        }
    }

    /// Get a description of why this output might not be available
    pub fn unavailable_reason(&self) -> Option<&'static str> {
        if self.is_available() {
            return None;
        }
        match self {
            VirtualCameraOutput::PipeWire => Some("PipeWire not running or pipewiresink plugin not found"),
            VirtualCameraOutput::V4L2Loopback => Some("v4l2loopback module not loaded"),
        }
    }

    /// Get the v4l2loopback device path if available
    pub fn v4l2loopback_device() -> Option<String> {
        find_v4l2loopback_device()
    }
}

/// Check if PipeWire is available (daemon running and GStreamer plugin available)
fn is_pipewire_available() -> bool {
    // Check if pipewiresink element is available in GStreamer
    if gstreamer::init().is_err() {
        return false;
    }
    gstreamer::ElementFactory::find("pipewiresink").is_some()
}

/// Check if v4l2loopback is available (module loaded and device exists)
fn is_v4l2loopback_available() -> bool {
    find_v4l2loopback_device().is_some()
}

/// Find a v4l2loopback device
///
/// Scans /dev/video* devices and checks if any are v4l2loopback devices
/// by checking the driver name via sysfs. Handles Flatpak sandboxing gracefully.
fn find_v4l2loopback_device() -> Option<String> {
    use std::fs;
    use std::path::Path;

    // Check if v4l2loopback module is loaded (skip if /proc/modules is not accessible)
    // In Flatpak, this path might be sandboxed
    let modules_path = Path::new("/proc/modules");
    let module_check_passed = if modules_path.exists() {
        match fs::read_to_string(modules_path) {
            Ok(content) => content.contains("v4l2loopback"),
            Err(_) => true, // Can't read, assume it might be available
        }
    } else {
        true // No /proc/modules (might be Flatpak), continue checking devices
    };

    if !module_check_passed {
        return None;
    }

    // Scan for video devices
    let dev_path = Path::new("/dev");
    if !dev_path.exists() {
        return None;
    }

    // Collect and sort video devices to get consistent results
    let mut video_devices: Vec<_> = fs::read_dir(dev_path)
        .ok()?
        .filter_map(|e| e.ok())
        .filter(|e| e.file_name().to_string_lossy().starts_with("video"))
        .collect();
    video_devices.sort_by_key(|e| e.file_name());

    // Check each video device
    for entry in video_devices {
        let name = entry.file_name();
        let name_str = name.to_string_lossy();
        let device_path = entry.path();

        // Try to read the device name from sysfs
        // v4l2loopback devices have "Dummy video device" or custom names
        let Some(device_num) = name_str.strip_prefix("video") else {
            continue;
        };
        let sysfs_name = format!("/sys/class/video4linux/video{}/name", device_num);

        if let Ok(device_name) = fs::read_to_string(&sysfs_name) {
            let device_name = device_name.trim();
            // v4l2loopback default names or check for "loopback" in name
            // Also check for "OBS" which is commonly used for OBS Virtual Camera
            if device_name.contains("Dummy video device")
                || device_name.to_lowercase().contains("loopback")
                || device_name.to_lowercase().contains("virtual")
                || device_name.contains("OBS")
            {
                return Some(device_path.to_string_lossy().to_string());
            }
        }
    }

    None
}

/// Application information utilities
pub mod app_info {
    use std::path::Path;

    /// Get the application version from build-time environment
    pub fn version() -> &'static str {
        env!("GIT_VERSION")
    }

    /// Check if the application is running inside a Flatpak sandbox
    pub fn is_flatpak() -> bool {
        Path::new("/.flatpak-info").exists()
    }

    /// Get the runtime environment string (e.g., "Flatpak" or "Native")
    pub fn runtime_environment() -> &'static str {
        if is_flatpak() { "Flatpak" } else { "Native" }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_resolution_labels() {
        assert_eq!(get_resolution_label(3840), Some("4K"));
        assert_eq!(get_resolution_label(1920), Some("HD"));
        assert_eq!(get_resolution_label(640), Some("SD"));
        assert_eq!(get_resolution_label(320), None);
    }
}
