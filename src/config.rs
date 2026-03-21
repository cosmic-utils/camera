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

/// Audio encoder preference
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub enum AudioEncoder {
    /// Opus codec (preferred - best quality)
    #[default]
    Opus,
    /// AAC codec (fallback - good compatibility)
    AAC,
}

impl AudioEncoder {
    /// Get display name for this encoder
    pub fn display_name(&self) -> &'static str {
        match self {
            AudioEncoder::Opus => "Opus",
            AudioEncoder::AAC => "AAC",
        }
    }

    /// Get all available encoders
    pub const ALL: [AudioEncoder; 2] = [AudioEncoder::Opus, AudioEncoder::AAC];
}

/// Timelapse interval setting
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub enum TimelapseInterval {
    /// 2 frames per second (500ms between captures, default)
    #[default]
    Fps2,
    /// 1 frame per second
    Sec1,
    /// 2 seconds between captures
    Sec2,
    /// 5 seconds between captures
    Sec5,
    /// 10 seconds between captures
    Sec10,
    /// 30 seconds between captures
    Sec30,
    /// 1 minute between captures
    Min1,
    /// 5 minutes between captures
    Min5,
}

impl TimelapseInterval {
    /// Get interval duration in milliseconds
    pub fn millis(&self) -> u64 {
        match self {
            TimelapseInterval::Fps2 => 500,
            TimelapseInterval::Sec1 => 1_000,
            TimelapseInterval::Sec2 => 2_000,
            TimelapseInterval::Sec5 => 5_000,
            TimelapseInterval::Sec10 => 10_000,
            TimelapseInterval::Sec30 => 30_000,
            TimelapseInterval::Min1 => 60_000,
            TimelapseInterval::Min5 => 300_000,
        }
    }

    /// Get display name for this interval
    pub fn display_name(&self) -> &'static str {
        match self {
            TimelapseInterval::Fps2 => "2 fps",
            TimelapseInterval::Sec1 => "1 second",
            TimelapseInterval::Sec2 => "2 seconds",
            TimelapseInterval::Sec5 => "5 seconds",
            TimelapseInterval::Sec10 => "10 seconds",
            TimelapseInterval::Sec30 => "30 seconds",
            TimelapseInterval::Min1 => "1 minute",
            TimelapseInterval::Min5 => "5 minutes",
        }
    }

    /// Get all available intervals
    pub const ALL: [TimelapseInterval; 8] = [
        TimelapseInterval::Fps2,
        TimelapseInterval::Sec1,
        TimelapseInterval::Sec2,
        TimelapseInterval::Sec5,
        TimelapseInterval::Sec10,
        TimelapseInterval::Sec30,
        TimelapseInterval::Min1,
        TimelapseInterval::Min5,
    ];
}

/// Composition guide overlay for camera preview
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub enum CompositionGuide {
    /// No guide overlay
    #[default]
    None,
    /// Rule of Thirds (2H + 2V lines at 1/3 and 2/3)
    RuleOfThirds,
    /// Phi Grid (2H + 2V lines at 0.382 and 0.618)
    PhiGrid,
    /// Fibonacci Spiral — focus top-left
    SpiralTopLeft,
    /// Fibonacci Spiral — focus top-right
    SpiralTopRight,
    /// Fibonacci Spiral — focus bottom-left
    SpiralBottomLeft,
    /// Fibonacci Spiral — focus bottom-right
    SpiralBottomRight,
    /// Diagonal lines from corners
    Diagonals,
    /// Crosshair (1H + 1V line through center)
    Crosshair,
}

impl CompositionGuide {
    /// Get all available guides
    pub const ALL: [CompositionGuide; 9] = [
        CompositionGuide::None,
        CompositionGuide::RuleOfThirds,
        CompositionGuide::PhiGrid,
        CompositionGuide::SpiralTopLeft,
        CompositionGuide::SpiralTopRight,
        CompositionGuide::SpiralBottomLeft,
        CompositionGuide::SpiralBottomRight,
        CompositionGuide::Diagonals,
        CompositionGuide::Crosshair,
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
    /// Get the COSMIC theme for this app theme preference.
    ///
    /// On non-COSMIC desktops, `system_dark()`/`system_light()`/`system_preference()`
    /// read broken defaults from cosmic_config, so we use built-in themes instead.
    /// For `System` mode, the initial theme defaults to dark; the portal subscription
    /// in `mod.rs` sends the correct value asynchronously once connected.
    pub fn theme(&self) -> Theme {
        if is_cosmic_desktop() {
            match self {
                Self::Dark => {
                    let mut t = theme::system_dark();
                    t.theme_type.prefer_dark(Some(true));
                    t
                }
                Self::Light => {
                    let mut t = theme::system_light();
                    t.theme_type.prefer_dark(Some(false));
                    t
                }
                Self::System => theme::system_preference(),
            }
        } else {
            match self {
                Self::Dark => Theme::dark(),
                Self::Light | Self::System => Theme::light(),
            }
        }
    }
}

/// Whether we're running on the COSMIC desktop (cached for process lifetime).
pub fn is_cosmic_desktop() -> bool {
    static IS_COSMIC: std::sync::LazyLock<bool> = std::sync::LazyLock::new(|| {
        std::env::var("XDG_CURRENT_DESKTOP")
            .map(|d| d.to_ascii_uppercase().contains("COSMIC"))
            .unwrap_or(false)
    });
    *IS_COSMIC
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
#[version = 15]
pub struct Config {
    /// Application theme preference (System, Dark, Light)
    pub app_theme: AppTheme,
    /// Folder name for saving captures (photos go to XDG Pictures, videos go to XDG Videos)
    pub save_folder_name: String,
    /// Last used camera device path
    pub last_camera_path: Option<String>,
    /// Video mode settings per camera (key = camera device path)
    pub video_settings: HashMap<String, FormatSettings>,
    /// Photo mode settings per camera (key = camera device path)
    pub photo_settings: HashMap<String, FormatSettings>,
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
    /// Record audio with video
    pub record_audio: bool,
    /// Audio encoder preference (Opus or AAC)
    pub audio_encoder: AudioEncoder,
    /// Composition guide overlay for camera preview
    pub composition_guide: CompositionGuide,
    /// Timelapse capture interval
    pub timelapse_interval: TimelapseInterval,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            app_theme: AppTheme::default(), // Default to System theme
            save_folder_name: crate::constants::DEFAULT_SAVE_FOLDER.to_string(),
            last_camera_path: None,
            video_settings: HashMap::new(),
            photo_settings: HashMap::new(),
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
            record_audio: true,   // Enable audio recording by default
            audio_encoder: AudioEncoder::default(), // Default to Opus
            composition_guide: CompositionGuide::default(), // Default to None
            timelapse_interval: TimelapseInterval::default(), // Default to 2 fps
        }
    }
}
