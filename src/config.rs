// SPDX-License-Identifier: GPL-3.0-only

use crate::constants::BitratePreset;
use cosmic::cosmic_config::{self, CosmicConfigEntry, cosmic_config_derive::CosmicConfigEntry};
use cosmic::{Theme, theme};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Photo output format preference
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub enum PhotoOutputFormat {
    /// JPEG format (lossy, smaller files)
    #[default]
    Jpeg,
    /// PNG format (lossless, larger files)
    Png,
    /// DNG format (raw image data)
    Dng,
}

impl PhotoOutputFormat {
    /// Get file extension for this format
    pub fn extension(&self) -> &'static str {
        match self {
            PhotoOutputFormat::Jpeg => "jpg",
            PhotoOutputFormat::Png => "png",
            PhotoOutputFormat::Dng => "dng",
        }
    }

    /// Get display name for this format
    pub fn display_name(&self) -> &'static str {
        match self {
            PhotoOutputFormat::Jpeg => "JPEG",
            PhotoOutputFormat::Png => "PNG",
            PhotoOutputFormat::Dng => "DNG (Raw)",
        }
    }

    /// Get all available formats
    pub const ALL: [PhotoOutputFormat; 3] = [
        PhotoOutputFormat::Jpeg,
        PhotoOutputFormat::Png,
        PhotoOutputFormat::Dng,
    ];
}

/// Burst mode setting
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub enum BurstModeSetting {
    /// Burst mode disabled (default - experimental feature)
    #[default]
    Off,
    /// Auto-detect frame count based on scene brightness
    Auto,
    /// Fixed 4 frames
    Frames4,
    /// Fixed 6 frames
    Frames6,
    /// Fixed 8 frames
    Frames8,
    /// Fixed 50 frames
    Frames50,
}

impl BurstModeSetting {
    /// Check if burst mode is enabled (not Off)
    pub fn is_enabled(&self) -> bool {
        !matches!(self, BurstModeSetting::Off)
    }

    /// Get the fixed frame count, if any
    pub fn frame_count(&self) -> Option<usize> {
        match self {
            BurstModeSetting::Off => None,
            BurstModeSetting::Auto => None,
            BurstModeSetting::Frames4 => Some(4),
            BurstModeSetting::Frames6 => Some(6),
            BurstModeSetting::Frames8 => Some(8),
            BurstModeSetting::Frames50 => Some(50),
        }
    }

    /// Get all available settings
    pub const ALL: [BurstModeSetting; 6] = [
        BurstModeSetting::Off,
        BurstModeSetting::Auto,
        BurstModeSetting::Frames4,
        BurstModeSetting::Frames6,
        BurstModeSetting::Frames8,
        BurstModeSetting::Frames50,
    ];
}

/// Application theme preference
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub enum AppTheme {
    /// Follow system theme (dark or light based on system setting)
    #[default]
    System,
    /// Always use dark theme
    Dark,
    /// Always use light theme
    Light,
}

impl AppTheme {
    /// Get the COSMIC theme for this app theme preference
    pub fn theme(&self) -> Theme {
        match self {
            Self::Dark => {
                let mut theme = theme::system_dark();
                theme.theme_type.prefer_dark(Some(true));
                theme
            }
            Self::Light => {
                let mut theme = theme::system_light();
                theme.theme_type.prefer_dark(Some(false));
                theme
            }
            Self::System => theme::system_preference(),
        }
    }
}

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
#[version = 11]
pub struct Config {
    /// Application theme preference (System, Dark, Light)
    pub app_theme: AppTheme,
    /// Folder name for saving photos and videos (inside Pictures/Videos directories)
    pub save_folder_name: String,
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
    /// Virtual camera feature enabled (disabled by default)
    pub virtual_camera_enabled: bool,
    /// Photo output format (JPEG, PNG, or DNG)
    pub photo_output_format: PhotoOutputFormat,
    /// Save raw burst frames as DNG files (for debugging burst mode pipeline)
    pub save_burst_raw: bool,
    /// Burst mode setting (Off, Auto, or fixed frame count)
    pub burst_mode_setting: BurstModeSetting,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            app_theme: AppTheme::default(), // Default to System theme
            save_folder_name: "Camera".to_string(),
            last_camera_path: None,
            video_settings: HashMap::new(),
            photo_settings: HashMap::new(),
            backend: crate::backends::camera::CameraBackendType::default(),
            last_video_encoder_index: None,
            bug_report_url:
                "https://github.com/cosmic-utils/camera/issues/new?template=bug_report_from_app.yml"
                    .to_string(),
            mirror_preview: true, // Default to mirrored (selfie mode)
            bitrate_preset: BitratePreset::default(), // Default to Medium
            virtual_camera_enabled: false, // Disabled by default
            photo_output_format: PhotoOutputFormat::default(), // Default to JPEG
            save_burst_raw: false, // Disabled by default (debugging feature)
            burst_mode_setting: BurstModeSetting::default(), // Default to Auto
        }
    }
}
