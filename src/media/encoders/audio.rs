// SPDX-License-Identifier: MPL-2.0
// Audio encoder module - some features for future multi-quality support
#![allow(dead_code)]

//! Audio encoder selection with quality configuration
//!
//! This module implements audio encoder selection with priority:
//! 1. Opus (best quality, all channel configs)
//! 2. AAC (good fallback)

use gstreamer as gst;
use gstreamer::prelude::*;
use tracing::{debug, info};

/// Audio codec types in priority order
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AudioCodec {
    /// Opus codec (preferred - best quality, all channels)
    Opus,
    /// AAC codec (fallback - good compatibility)
    AAC,
}

impl AudioCodec {
    /// Get audio caps string for this codec
    pub fn caps_string(&self) -> &'static str {
        match self {
            AudioCodec::Opus => "audio/x-opus",
            AudioCodec::AAC => "audio/mpeg,mpegversion=4",
        }
    }
}

/// Audio channel configuration
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AudioChannels {
    /// Mono (1 channel)
    Mono,
    /// Stereo (2 channels)
    Stereo,
    /// Multi-channel (more than 2)
    MultiChannel(u32),
}

impl AudioChannels {
    /// Get number of channels
    pub fn count(&self) -> u32 {
        match self {
            AudioChannels::Mono => 1,
            AudioChannels::Stereo => 2,
            AudioChannels::MultiChannel(n) => *n,
        }
    }

    /// Create from channel count
    pub fn from_count(count: u32) -> Self {
        match count {
            1 => AudioChannels::Mono,
            2 => AudioChannels::Stereo,
            n => AudioChannels::MultiChannel(n),
        }
    }
}

/// Audio quality presets
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AudioQuality {
    /// Low quality (64 kbps)
    Low,
    /// Medium quality (96 kbps)
    Medium,
    /// High quality (128 kbps)
    High,
    /// Maximum quality (192 kbps)
    Maximum,
}

impl AudioQuality {
    /// Get bitrate in bits per second
    pub fn bitrate_bps(&self) -> i32 {
        match self {
            AudioQuality::Low => 64_000,
            AudioQuality::Medium => 96_000,
            AudioQuality::High => 128_000,
            AudioQuality::Maximum => 192_000,
        }
    }
}

/// Selected audio encoder with configuration
pub struct SelectedAudioEncoder {
    /// The encoder element
    pub encoder: gst::Element,
    /// Codec being used
    pub codec: AudioCodec,
}

/// Select the best available audio encoder
///
/// Priority order:
/// 1. Opus (opusenc) - best quality, all channel configs
/// 2. AAC (avenc_aac, faac, voaacenc) - good fallback
///
/// # Arguments
/// * `quality` - Quality preset for encoding
/// * `channels` - Audio channel configuration
///
/// # Returns
/// * `Ok(SelectedAudioEncoder)` - Selected encoder with configuration
/// * `Err(String)` - Error message if no encoder available
pub fn select_audio_encoder(
    quality: AudioQuality,
    channels: AudioChannels,
) -> Result<SelectedAudioEncoder, String> {
    gst::init().map_err(|e| format!("Failed to initialize GStreamer: {}", e))?;

    // Try Opus first (preferred)
    if let Ok(encoder) = gst::ElementFactory::make("opusenc").build() {
        info!(
            codec = "Opus",
            channels = channels.count(),
            "Selected audio encoder"
        );

        configure_opus_encoder(&encoder, quality, channels);

        return Ok(SelectedAudioEncoder {
            encoder,
            codec: AudioCodec::Opus,
        });
    }

    // Try AAC encoders as fallback
    let aac_encoders = ["avenc_aac", "faac", "voaacenc"];

    for encoder_name in &aac_encoders {
        if let Ok(encoder) = gst::ElementFactory::make(encoder_name).build() {
            info!(
                codec = "AAC",
                encoder = %encoder_name,
                channels = channels.count(),
                "Selected audio encoder"
            );

            configure_aac_encoder(&encoder, encoder_name, quality, channels);

            return Ok(SelectedAudioEncoder {
                encoder,
                codec: AudioCodec::AAC,
            });
        }
    }

    Err("No audio encoder available. Please install gstreamer1-plugins-base (opusenc) or gstreamer1-plugins-bad (avenc_aac)".to_string())
}

/// Configure Opus encoder
fn configure_opus_encoder(encoder: &gst::Element, quality: AudioQuality, channels: AudioChannels) {
    let bitrate = quality.bitrate_bps();

    // Opus bitrate is in bits per second
    let _ = encoder.set_property("bitrate", bitrate);

    // Audio type: voice for mono, music for stereo/multi-channel
    let audio_type = match channels {
        AudioChannels::Mono => "voice",
        AudioChannels::Stereo | AudioChannels::MultiChannel(_) => "generic",
    };
    let _ = encoder.set_property_from_str("audio-type", audio_type);

    // Bandwidth: wide for voice, fullband for music
    let bandwidth = match channels {
        AudioChannels::Mono => "wideband",
        AudioChannels::Stereo | AudioChannels::MultiChannel(_) => "fullband",
    };
    let _ = encoder.set_property_from_str("bandwidth", bandwidth);

    debug!(
        "Configured opusenc: bitrate={} bps, audio-type={}, bandwidth={}",
        bitrate, audio_type, bandwidth
    );
}

/// Configure AAC encoder
fn configure_aac_encoder(
    encoder: &gst::Element,
    encoder_name: &str,
    quality: AudioQuality,
    _channels: AudioChannels,
) {
    let bitrate = quality.bitrate_bps();

    match encoder_name {
        "avenc_aac" => {
            // avenc_aac uses bitrate in bits per second
            let _ = encoder.set_property("bitrate", bitrate);
            debug!("Configured avenc_aac: bitrate={} bps", bitrate);
        }

        "faac" => {
            // faac uses bitrate in kbps
            let _ = encoder.set_property("bitrate", bitrate / 1000);
            debug!("Configured faac: bitrate={} kbps", bitrate / 1000);
        }

        "voaacenc" => {
            // voaacenc uses bitrate in bits per second
            let _ = encoder.set_property("bitrate", bitrate);
            debug!("Configured voaacenc: bitrate={} bps", bitrate);
        }

        _ => {
            debug!("Unknown AAC encoder type, using default configuration");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_audio_channels() {
        assert_eq!(AudioChannels::Mono.count(), 1);
        assert_eq!(AudioChannels::Stereo.count(), 2);
        assert_eq!(AudioChannels::MultiChannel(6).count(), 6);

        assert_eq!(AudioChannels::from_count(1), AudioChannels::Mono);
        assert_eq!(AudioChannels::from_count(2), AudioChannels::Stereo);
        assert_eq!(AudioChannels::from_count(6), AudioChannels::MultiChannel(6));
    }

    #[test]
    fn test_audio_quality_bitrates() {
        assert!(AudioQuality::Low.bitrate_bps() < AudioQuality::High.bitrate_bps());
        assert_eq!(AudioQuality::Low.bitrate_bps(), 64_000);
        assert_eq!(AudioQuality::Maximum.bitrate_bps(), 192_000);
    }

    #[test]
    fn test_codec_caps() {
        assert_eq!(AudioCodec::Opus.caps_string(), "audio/x-opus");
        assert!(AudioCodec::AAC.caps_string().contains("audio/mpeg"));
    }
}
