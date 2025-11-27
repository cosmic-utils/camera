// SPDX-License-Identifier: MPL-2.0

//! Video recording pipeline with intelligent encoder selection
//!
//! This module provides an async video recording pipeline that:
//! - Automatically selects the best available encoder (hardware preferred)
//! - Continues preview during recording
//! - Supports audio recording
//! - Provides quality presets

pub mod encoder_selection;
pub mod muxer;
pub mod recorder;

// Re-export commonly used types
pub use encoder_selection::EncoderConfig;
pub use recorder::{VideoRecorder, check_available_encoders};

// Re-export encoder types for convenience
pub use crate::media::encoders::{AudioChannels, AudioQuality, VideoQuality};
