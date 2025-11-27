// SPDX-License-Identifier: MPL-2.0

use crate::constants::BitratePreset;
use cosmic::cosmic_config::{self, CosmicConfigEntry, cosmic_config_derive::CosmicConfigEntry};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Camera format settings for a specific camera (used for both photo and video modes)
#[derive(Debug, Clone, CosmicConfigEntry, Eq, PartialEq, Default, Serialize, Deserialize)]
pub struct FormatSettings {
    /// Resolution width
    pub width: u32,
    /// Resolution height
    pub height: u32,
    /// Framerate
    pub framerate: Option<u32>,
    /// Pixel format (e.g., "YUYV", "MJPG", "H264")
    pub pixel_format: String,
}

/// Backwards compatibility alias
pub type VideoSettings = FormatSettings;

#[derive(Debug, Clone, CosmicConfigEntry, Eq, PartialEq, Serialize, Deserialize)]
#[version = 4]
pub struct Config {
    /// Last used camera device path
    pub last_camera_path: Option<String>,
    /// Video mode settings per camera (key = camera device path)
    pub video_settings: HashMap<String, FormatSettings>,
    /// Photo mode settings per camera (key = camera device path)
    pub photo_settings: HashMap<String, FormatSettings>,
    /// Camera backend to use (PipeWire or V4L2)
    pub backend: crate::backends::camera::CameraBackendType,
    /// Last selected video encoder index
    pub last_video_encoder_index: Option<usize>,
    /// Bug report submission URL (GitHub issues URL)
    pub bug_report_url: String,
    /// Mirror camera preview horizontally (selfie mode)
    pub mirror_preview: bool,
    /// Video encoder bitrate preset (Low, Medium, High)
    pub bitrate_preset: BitratePreset,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            last_camera_path: None,
            video_settings: HashMap::new(),
            photo_settings: HashMap::new(),
            backend: crate::backends::camera::CameraBackendType::default(),
            last_video_encoder_index: None,
            bug_report_url: "https://github.com/FreddyFunk/cosmic-camera/issues/new".to_string(),
            mirror_preview: true, // Default to mirrored (selfie mode)
            bitrate_preset: BitratePreset::default(), // Default to Medium
        }
    }
}
