// SPDX-License-Identifier: MPL-2.0

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
    /// Get all tiers for UI iteration
    pub const ALL: [ResolutionTier; 5] = [
        ResolutionTier::SD,
        ResolutionTier::HD,
        ResolutionTier::FullHD,
        ResolutionTier::TwoK,
        ResolutionTier::FourK,
    ];

    /// Get display name for the tier
    pub fn display_name(&self) -> &'static str {
        match self {
            ResolutionTier::SD => "SD",
            ResolutionTier::HD => "HD",
            ResolutionTier::FullHD => "Full HD",
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

    /// Format picker overlay opacity
    #[allow(dead_code)]
    pub const PICKER_OVERLAY_OPACITY: f32 = 0.7;

    /// Format picker border radius
    pub const PICKER_BORDER_RADIUS: f32 = 8.0;

    /// Placeholder button width when camera switch is hidden
    pub const PLACEHOLDER_BUTTON_WIDTH: f32 = 40.0;

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

/// Camera device constants
pub mod camera {
    /// Maximum number of video devices to scan
    #[allow(dead_code)]
    pub const MAX_VIDEO_DEVICES: usize = 10;

    /// Maximum metadata device offset to search
    #[allow(dead_code)]
    pub const MAX_METADATA_OFFSET: usize = 3;

    /// Default video device path
    #[allow(dead_code)]
    pub const DEFAULT_DEVICE: &str = "/dev/video0";
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
    /// NV12 uses 1.5 bytes/pixel vs RGBA's 4 bytes/pixel (~60% memory savings)
    pub const OUTPUT_FORMAT: &str = "NV12";
}

/// Timing constants
pub mod timing {
    use super::Duration;

    /// Frame counter modulo for periodic logging
    pub const FRAME_LOG_INTERVAL: u64 = 30;

    /// GStreamer state change timeout for validation
    /// Reduced to minimize startup delay - we accept async state changes
    pub const STATE_CHANGE_TIMEOUT_MS: u64 = 50;

    /// Pipeline state change timeout on stop
    #[allow(dead_code)]
    pub const STOP_TIMEOUT_SECS: u64 = 2;

    /// Pipeline playing state timeout on start
    pub const START_TIMEOUT_SECS: u64 = 5;

    /// Camera retry delay after failure
    #[allow(dead_code)]
    pub const CAMERA_RETRY_DELAY: Duration = Duration::from_secs(5);

    /// Hardware release delay after stop
    #[allow(dead_code)]
    pub const HARDWARE_RELEASE_DELAY: Duration = Duration::from_millis(100);

    /// GStreamer probe delay between tests
    #[allow(dead_code)]
    pub const PROBE_DELAY: Duration = Duration::from_millis(200);

    /// GStreamer probe timeout per resolution
    #[allow(dead_code)]
    pub const PROBE_TIMEOUT_SECS: u64 = 3;
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
