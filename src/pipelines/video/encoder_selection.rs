// SPDX-License-Identifier: MPL-2.0

//! Encoder selection for video recording pipeline
//!
//! This module provides a simple interface to select video and audio encoders
//! for the recording pipeline.

use crate::media::encoders::{
    audio::{AudioChannels, AudioQuality, SelectedAudioEncoder, select_audio_encoder},
    video::{
        EncoderInfo, SelectedVideoEncoder, VideoQuality, create_encoder_from_info_with_bitrate,
        select_video_encoder_with_bitrate,
    },
};

/// Configuration for encoder selection
#[derive(Debug, Clone)]
pub struct EncoderConfig {
    /// Video quality preset
    pub video_quality: VideoQuality,
    /// Audio quality preset
    pub audio_quality: AudioQuality,
    /// Audio channel configuration
    pub audio_channels: AudioChannels,
    /// Video width (for bitrate calculation)
    pub width: u32,
    /// Video height (for bitrate calculation)
    pub height: u32,
    /// Optional bitrate override in kbps (takes precedence over quality preset)
    pub bitrate_override_kbps: Option<u32>,
}

impl Default for EncoderConfig {
    fn default() -> Self {
        Self {
            video_quality: VideoQuality::High,
            audio_quality: AudioQuality::High,
            audio_channels: AudioChannels::Stereo,
            width: 1920,
            height: 1080,
            bitrate_override_kbps: None,
        }
    }
}

/// Selected encoders for recording
pub struct SelectedEncoders {
    /// Video encoder configuration
    pub video: SelectedVideoEncoder,
    /// Audio encoder configuration (optional if no audio)
    pub audio: Option<SelectedAudioEncoder>,
}

/// Select best available encoders based on configuration
///
/// This will select the best video and audio encoders based on hardware
/// availability and the provided configuration.
///
/// # Arguments
/// * `config` - Encoder configuration
/// * `enable_audio` - Whether to select an audio encoder
///
/// # Returns
/// * `Ok(SelectedEncoders)` - Selected encoders
/// * `Err(String)` - Error message if encoder selection fails
pub fn select_encoders(
    config: &EncoderConfig,
    enable_audio: bool,
) -> Result<SelectedEncoders, String> {
    // Select video encoder
    let video = select_video_encoder_with_bitrate(
        config.video_quality,
        config.width,
        config.height,
        config.bitrate_override_kbps,
    )?;

    // Select audio encoder if enabled
    let audio = if enable_audio {
        match select_audio_encoder(config.audio_quality, config.audio_channels) {
            Ok(encoder) => Some(encoder),
            Err(e) => {
                tracing::warn!(
                    "Failed to select audio encoder: {}. Recording without audio.",
                    e
                );
                None
            }
        }
    } else {
        None
    };

    Ok(SelectedEncoders { video, audio })
}

/// Select encoders with specific video encoder
///
/// # Arguments
/// * `config` - Encoder configuration
/// * `encoder_info` - Specific video encoder to use
/// * `enable_audio` - Whether to select an audio encoder
///
/// # Returns
/// * `Ok(SelectedEncoders)` - Selected encoders
/// * `Err(String)` - Error message if encoder selection fails
pub fn select_encoders_with_video(
    config: &EncoderConfig,
    encoder_info: &EncoderInfo,
    enable_audio: bool,
) -> Result<SelectedEncoders, String> {
    // Create specific video encoder
    let video = create_encoder_from_info_with_bitrate(
        encoder_info,
        config.video_quality,
        config.width,
        config.height,
        config.bitrate_override_kbps,
    )?;

    // Select audio encoder if enabled
    let audio = if enable_audio {
        match select_audio_encoder(config.audio_quality, config.audio_channels) {
            Ok(encoder) => Some(encoder),
            Err(e) => {
                tracing::warn!(
                    "Failed to select audio encoder: {}. Recording without audio.",
                    e
                );
                None
            }
        }
    } else {
        None
    };

    Ok(SelectedEncoders { video, audio })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_config() {
        let config = EncoderConfig::default();
        assert_eq!(config.width, 1920);
        assert_eq!(config.height, 1080);
        assert_eq!(config.audio_channels, AudioChannels::Stereo);
    }
}
