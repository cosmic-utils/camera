// SPDX-License-Identifier: MPL-2.0

//! Media encoder selection and configuration
//!
//! This module provides centralized encoder selection for video and audio with:
//! - Hardware encoder priority (AV1 > HEVC > H.264)
//! - Software fallbacks for maximum compatibility
//! - Quality presets for easy configuration
//! - Automatic encoder detection

pub mod audio;
pub mod detection;
pub mod video;

// Re-export commonly used types
pub use video::VideoQuality;

pub use audio::{AudioChannels, AudioQuality};

pub use detection::log_available_encoders;
