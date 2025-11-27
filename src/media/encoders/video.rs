// SPDX-License-Identifier: MPL-2.0
// Video encoder module - some functions for future encoder selection UI
#![allow(dead_code)]

//! Video encoder selection with hardware acceleration priority
//!
//! This module implements intelligent video encoder selection based on:
//! - Hardware encoder availability (AV1 > HEVC > H.264)
//! - Software fallbacks for maximum compatibility
//! - Configurable quality presets

use gstreamer as gst;
use gstreamer::prelude::*;
use tracing::{debug, info, warn};

/// Video codec types in priority order
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VideoCodec {
    /// AV1 codec (best compression, modern)
    AV1,
    /// HEVC/H.265 codec (good compression)
    HEVC,
    /// H.264 codec (best compatibility)
    H264,
}

impl VideoCodec {
    /// Get the container format for this codec
    pub fn container_format(&self) -> ContainerFormat {
        match self {
            VideoCodec::AV1 => ContainerFormat::WebM,
            VideoCodec::HEVC => ContainerFormat::MP4,
            VideoCodec::H264 => ContainerFormat::MP4,
        }
    }

    /// Get the file extension for this codec's container
    pub fn file_extension(&self) -> &'static str {
        self.container_format().extension()
    }

    /// Get the parser element name (if needed)
    pub fn parser_name(&self) -> Option<&'static str> {
        match self {
            VideoCodec::AV1 => Some("av1parse"),
            VideoCodec::HEVC => Some("h265parse"),
            VideoCodec::H264 => Some("h264parse"),
        }
    }
}

/// Container formats for video
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContainerFormat {
    /// MP4 container (good compatibility)
    MP4,
    /// WebM container (open format)
    WebM,
}

impl ContainerFormat {
    /// Get file extension
    pub fn extension(&self) -> &'static str {
        match self {
            ContainerFormat::MP4 => "mp4",
            ContainerFormat::WebM => "webm",
        }
    }

    /// Get muxer element name
    pub fn muxer_name(&self) -> &'static str {
        match self {
            ContainerFormat::MP4 => "mp4mux",
            ContainerFormat::WebM => "webmmux",
        }
    }
}

/// Video quality presets
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VideoQuality {
    /// Low quality (high compression, smaller files)
    Low,
    /// Medium quality (balanced)
    Medium,
    /// High quality (low compression, larger files)
    High,
    /// Maximum quality (minimal compression)
    Maximum,
}

impl VideoQuality {
    /// Get bitrate in kbps for given quality
    ///
    /// Bitrate scales with resolution using realistic encoding factors:
    /// - 720p: ~2-8 Mbps depending on quality
    /// - 1080p: ~4-15 Mbps depending on quality
    /// - 4K: ~15-40 Mbps depending on quality
    pub fn bitrate_kbps(&self, width: u32, height: u32) -> u32 {
        let pixels = width * height;
        // Factors chosen to give sensible bitrates:
        // 1080p (2M pixels): Low ~4Mbps, Med ~8Mbps, High ~12Mbps, Max ~20Mbps
        let base_bitrate = match self {
            VideoQuality::Low => (pixels as f64 * 0.002) as u32,
            VideoQuality::Medium => (pixels as f64 * 0.004) as u32,
            VideoQuality::High => (pixels as f64 * 0.006) as u32,
            VideoQuality::Maximum => (pixels as f64 * 0.010) as u32,
        };
        // Ensure minimum 500 kbps, maximum 50000 kbps (50 Mbps)
        base_bitrate.clamp(500, 50000)
    }

    /// Get x264/x265 preset name
    pub fn x264_preset(&self) -> &'static str {
        match self {
            VideoQuality::Low => "veryfast",
            VideoQuality::Medium => "fast",
            VideoQuality::High => "medium",
            VideoQuality::Maximum => "slow",
        }
    }
}

/// Information about an available encoder
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EncoderInfo {
    /// GStreamer element name
    pub element_name: String,
    /// Display name for UI
    pub display_name: String,
    /// Codec type
    pub codec: VideoCodec,
    /// Whether this is hardware accelerated
    pub is_hardware: bool,
    /// Priority (lower = higher priority)
    pub priority: u32,
}

/// Selected video encoder with configuration
pub struct SelectedVideoEncoder {
    /// The encoder element
    pub encoder: gst::Element,
    /// Optional parser element
    pub parser: Option<gst::Element>,
    /// Muxer element
    pub muxer: gst::Element,
    /// Codec being used
    pub codec: VideoCodec,
    /// Container format
    pub container: ContainerFormat,
    /// File extension
    pub extension: &'static str,
}

/// Enumerate all available video encoders
///
/// Returns a list of available encoders sorted by priority
pub fn enumerate_video_encoders() -> Vec<EncoderInfo> {
    let _ = gst::init();

    let encoder_specs = [
        // Hardware AV1
        ("vaav1enc", "VA-API AV1 (HW)", VideoCodec::AV1, true, 1),
        ("nvav1enc", "NVIDIA AV1 (HW)", VideoCodec::AV1, true, 2),
        ("qsvav1enc", "Intel QSV AV1 (HW)", VideoCodec::AV1, true, 3),
        ("amfav1enc", "AMD AMF AV1 (HW)", VideoCodec::AV1, true, 4),
        // Software AV1
        ("svtav1enc", "SVT-AV1 (SW)", VideoCodec::AV1, false, 10),
        ("av1enc", "AOM AV1 (SW)", VideoCodec::AV1, false, 11),
        // Hardware HEVC/H.265
        (
            "vaapih265enc",
            "VA-API H.265 (HW)",
            VideoCodec::HEVC,
            true,
            20,
        ),
        ("vah265enc", "VA-API H.265 (HW)", VideoCodec::HEVC, true, 21),
        ("nvh265enc", "NVIDIA H.265 (HW)", VideoCodec::HEVC, true, 22),
        (
            "qsvh265enc",
            "Intel QSV H.265 (HW)",
            VideoCodec::HEVC,
            true,
            23,
        ),
        (
            "amfh265enc",
            "AMD AMF H.265 (HW)",
            VideoCodec::HEVC,
            true,
            24,
        ),
        ("v4l2h265enc", "V4L2 H.265 (HW)", VideoCodec::HEVC, true, 25),
        // Software HEVC/H.265
        ("x265enc", "x265 H.265 (SW)", VideoCodec::HEVC, false, 30),
        // Hardware H.264
        (
            "vaapih264enc",
            "VA-API H.264 (HW)",
            VideoCodec::H264,
            true,
            40,
        ),
        ("vah264enc", "VA-API H.264 (HW)", VideoCodec::H264, true, 41),
        ("nvh264enc", "NVIDIA H.264 (HW)", VideoCodec::H264, true, 42),
        (
            "qsvh264enc",
            "Intel QSV H.264 (HW)",
            VideoCodec::H264,
            true,
            43,
        ),
        (
            "amfh264enc",
            "AMD AMF H.264 (HW)",
            VideoCodec::H264,
            true,
            44,
        ),
        ("v4l2h264enc", "V4L2 H.264 (HW)", VideoCodec::H264, true, 45),
        // Software H.264
        ("x264enc", "x264 H.264 (SW)", VideoCodec::H264, false, 50),
        (
            "openh264enc",
            "OpenH264 H.264 (SW)",
            VideoCodec::H264,
            false,
            51,
        ),
    ];

    let mut available_encoders = Vec::new();

    for (element_name, display_name, codec, is_hardware, priority) in &encoder_specs {
        if gst::ElementFactory::make(element_name).build().is_ok() {
            available_encoders.push(EncoderInfo {
                element_name: element_name.to_string(),
                display_name: display_name.to_string(),
                codec: *codec,
                is_hardware: *is_hardware,
                priority: *priority,
            });
        }
    }

    // Sort by priority (lower number = higher priority)
    available_encoders.sort_by_key(|e| e.priority);

    available_encoders
}

/// Create encoder from encoder info
pub fn create_encoder_from_info(
    info: &EncoderInfo,
    quality: VideoQuality,
    width: u32,
    height: u32,
) -> Result<SelectedVideoEncoder, String> {
    create_encoder_from_info_with_bitrate(info, quality, width, height, None)
}

/// Create encoder from encoder info with optional bitrate override
pub fn create_encoder_from_info_with_bitrate(
    info: &EncoderInfo,
    quality: VideoQuality,
    width: u32,
    height: u32,
    bitrate_override_kbps: Option<u32>,
) -> Result<SelectedVideoEncoder, String> {
    let encoder = gst::ElementFactory::make(&info.element_name)
        .build()
        .map_err(|e| format!("Failed to create encoder {}: {}", info.element_name, e))?;

    // Configure encoder
    configure_video_encoder(
        &encoder,
        &info.element_name,
        quality,
        width,
        height,
        bitrate_override_kbps,
    );

    // Create parser if needed
    let parser = if let Some(parser_name) = info.codec.parser_name() {
        match gst::ElementFactory::make(parser_name).build() {
            Ok(p) => {
                debug!("Created parser: {}", parser_name);
                Some(p)
            }
            Err(e) => {
                warn!("Failed to create parser {}: {}", parser_name, e);
                None
            }
        }
    } else {
        None
    };

    // Create muxer
    let container = info.codec.container_format();
    let muxer = gst::ElementFactory::make(container.muxer_name())
        .build()
        .map_err(|e| format!("Failed to create muxer {}: {}", container.muxer_name(), e))?;

    Ok(SelectedVideoEncoder {
        encoder,
        parser,
        muxer,
        codec: info.codec,
        container,
        extension: info.codec.file_extension(),
    })
}

/// Select the best available video encoder
///
/// Priority order:
/// 1. Hardware AV1 (vaapiavcenc, nvav1enc)
/// 2. Hardware HEVC/H.265 (vaapih265enc, nvh265enc)
/// 3. Hardware H.264 (vaapih264enc, nvh264enc)
/// 4. Software HEVC/H.265 (x265enc)
/// 5. Software H.264 (x264enc)
///
/// # Arguments
/// * `quality` - Quality preset for encoding
/// * `width` - Video width (for bitrate calculation)
/// * `height` - Video height (for bitrate calculation)
///
/// # Returns
/// * `Ok(SelectedVideoEncoder)` - Selected encoder with configuration
/// * `Err(String)` - Error message if no encoder available
pub fn select_video_encoder(
    quality: VideoQuality,
    width: u32,
    height: u32,
) -> Result<SelectedVideoEncoder, String> {
    select_video_encoder_with_bitrate(quality, width, height, None)
}

/// Select the best available video encoder with optional bitrate override
pub fn select_video_encoder_with_bitrate(
    quality: VideoQuality,
    width: u32,
    height: u32,
    bitrate_override_kbps: Option<u32>,
) -> Result<SelectedVideoEncoder, String> {
    gst::init().map_err(|e| format!("Failed to initialize GStreamer: {}", e))?;

    // Try encoders in priority order
    let encoders = [
        // Hardware AV1
        ("vaapiavcenc", VideoCodec::AV1, true),
        ("nvav1enc", VideoCodec::AV1, true),
        // Hardware HEVC
        ("vaapih265enc", VideoCodec::HEVC, true),
        ("nvh265enc", VideoCodec::HEVC, true),
        ("v4l2h265enc", VideoCodec::HEVC, true),
        // Hardware H.264
        ("vaapih264enc", VideoCodec::H264, true),
        ("nvh264enc", VideoCodec::H264, true),
        ("v4l2h264enc", VideoCodec::H264, true),
        // Software HEVC
        ("x265enc", VideoCodec::HEVC, false),
        // Software H.264
        ("x264enc", VideoCodec::H264, false),
        ("openh264enc", VideoCodec::H264, false),
    ];

    for (encoder_name, codec, is_hardware) in &encoders {
        if let Ok(encoder) = gst::ElementFactory::make(encoder_name).build() {
            info!(
                encoder = %encoder_name,
                codec = ?codec,
                hardware = is_hardware,
                "Selected video encoder"
            );

            // Configure encoder
            configure_video_encoder(
                &encoder,
                encoder_name,
                quality,
                width,
                height,
                bitrate_override_kbps,
            );

            // Create parser if needed
            let parser = if let Some(parser_name) = codec.parser_name() {
                match gst::ElementFactory::make(parser_name).build() {
                    Ok(p) => {
                        debug!("Created parser: {}", parser_name);
                        Some(p)
                    }
                    Err(e) => {
                        warn!("Failed to create parser {}: {}", parser_name, e);
                        None
                    }
                }
            } else {
                None
            };

            // Create muxer
            let container = codec.container_format();
            let muxer = gst::ElementFactory::make(container.muxer_name())
                .build()
                .map_err(|e| format!("Failed to create muxer {}: {}", container.muxer_name(), e))?;

            return Ok(SelectedVideoEncoder {
                encoder,
                parser,
                muxer,
                codec: *codec,
                container,
                extension: codec.file_extension(),
            });
        }
    }

    Err("No video encoder available. Please install gstreamer1-plugins-ugly (x264enc) or gstreamer1-plugin-openh264".to_string())
}

/// Configure encoder based on type and quality
fn configure_video_encoder(
    encoder: &gst::Element,
    encoder_name: &str,
    quality: VideoQuality,
    width: u32,
    height: u32,
    bitrate_override_kbps: Option<u32>,
) {
    // Use bitrate override if provided, otherwise calculate from quality preset
    let bitrate = bitrate_override_kbps.unwrap_or_else(|| quality.bitrate_kbps(width, height));

    match encoder_name {
        // x264 software encoder
        "x264enc" => {
            let _ = encoder.set_property_from_str("speed-preset", quality.x264_preset());
            let _ = encoder.set_property_from_str("tune", "zerolatency");
            let _ = encoder.set_property("bitrate", bitrate);
            debug!(
                "Configured x264enc: preset={}, bitrate={} kbps",
                quality.x264_preset(),
                bitrate
            );
        }

        // x265 software encoder
        "x265enc" => {
            let _ = encoder.set_property_from_str("speed-preset", quality.x264_preset());
            let _ = encoder.set_property("bitrate", bitrate);
            debug!(
                "Configured x265enc: preset={}, bitrate={} kbps",
                quality.x264_preset(),
                bitrate
            );
        }

        // VA-API encoders (old plugin style - uses integer)
        "vaapih264enc" | "vaapih265enc" => {
            let _ = encoder.set_property("rate-control", 2); // CBR
            let _ = encoder.set_property("bitrate", bitrate);
            debug!("Configured VA-API encoder: bitrate={} kbps", bitrate);
        }

        // NVIDIA encoders
        "nvh264enc" | "nvh265enc" | "nvav1enc" => {
            let _ = encoder.set_property("bitrate", bitrate);
            let _ = encoder.set_property_from_str("rc-mode", "vbr"); // Variable bitrate
            let preset = match quality {
                VideoQuality::Low | VideoQuality::Medium => "fast",
                VideoQuality::High | VideoQuality::Maximum => "hq",
            };
            let _ = encoder.set_property_from_str("preset", preset);
            debug!(
                "Configured NVIDIA encoder: preset={}, bitrate={} kbps",
                preset, bitrate
            );
        }

        // V4L2 hardware encoders
        "v4l2h264enc" | "v4l2h265enc" => {
            // V4L2 encoders typically have limited configuration
            debug!("Using V4L2 encoder with default configuration");
        }

        // OpenH264 (software H.264 encoder)
        "openh264enc" => {
            // Set rate control mode to bitrate mode using string enum
            let _ = encoder.set_property_from_str("rate-control", "bitrate");
            let _ = encoder.set_property("bitrate", bitrate * 1000); // Bits per second
            // Set usage type to camera for real-time encoding
            let _ = encoder.set_property_from_str("usage-type", "camera");
            debug!(
                "Configured openh264enc: rate-control=bitrate, bitrate={} bps",
                bitrate * 1000
            );
        }

        // SVT-AV1 encoder
        "svtav1enc" => {
            let _ = encoder.set_property("target-bitrate", bitrate);
            let preset = match quality {
                VideoQuality::Low => 8,
                VideoQuality::Medium => 6,
                VideoQuality::High => 4,
                VideoQuality::Maximum => 2,
            };
            let _ = encoder.set_property("speed", preset);
            debug!(
                "Configured svtav1enc: preset={}, bitrate={} kbps",
                preset, bitrate
            );
        }

        // AOM AV1 encoder
        "av1enc" => {
            let _ = encoder.set_property("target-bitrate", bitrate * 1000); // bits per second
            let cpu_used = match quality {
                VideoQuality::Low => 8,
                VideoQuality::Medium => 5,
                VideoQuality::High => 3,
                VideoQuality::Maximum => 1,
            };
            let _ = encoder.set_property("cpu-used", cpu_used);
            debug!(
                "Configured av1enc: cpu-used={}, bitrate={} bps",
                cpu_used,
                bitrate * 1000
            );
        }

        // VA-API AV1 encoder
        "vaav1enc" | "vaapiavcenc" => {
            let _ = encoder.set_property_from_str("rate-control", "cbr");
            let _ = encoder.set_property("bitrate", bitrate);
            debug!("Configured VA-API AV1 encoder: bitrate={} kbps", bitrate);
        }

        // VA-API H.264/H.265 encoders (new plugin style - uses string)
        "vah264enc" | "vah265enc" => {
            let _ = encoder.set_property_from_str("rate-control", "cbr");
            let _ = encoder.set_property("bitrate", bitrate);
            debug!("Configured VA-API encoder: bitrate={} kbps", bitrate);
        }

        // AMD AMF encoders
        "amfh264enc" | "amfh265enc" | "amfav1enc" => {
            let _ = encoder.set_property("bitrate", bitrate);
            let _ = encoder.set_property_from_str("rate-control", "cbr");
            debug!("Configured AMD AMF encoder: bitrate={} kbps", bitrate);
        }

        // Intel QSV encoders
        "qsvh264enc" | "qsvh265enc" | "qsvav1enc" => {
            let _ = encoder.set_property("bitrate", bitrate);
            debug!("Configured Intel QSV encoder: bitrate={} kbps", bitrate);
        }

        _ => {
            debug!("Unknown encoder type, using default configuration");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_codec_extensions() {
        assert_eq!(VideoCodec::H264.file_extension(), "mp4");
        assert_eq!(VideoCodec::HEVC.file_extension(), "mp4");
        assert_eq!(VideoCodec::AV1.file_extension(), "webm");
    }

    #[test]
    fn test_quality_bitrates() {
        // 1920x1080 (Full HD)
        let low = VideoQuality::Low.bitrate_kbps(1920, 1080);
        let high = VideoQuality::High.bitrate_kbps(1920, 1080);
        assert!(low < high);
        assert!(low >= 500); // Minimum
        assert!(high <= 50000); // Maximum
    }

    #[test]
    fn test_container_formats() {
        assert_eq!(ContainerFormat::MP4.extension(), "mp4");
        assert_eq!(ContainerFormat::WebM.extension(), "webm");
        assert_eq!(ContainerFormat::MP4.muxer_name(), "mp4mux");
        assert_eq!(ContainerFormat::WebM.muxer_name(), "webmmux");
    }
}
