// SPDX-License-Identifier: GPL-3.0-only

//! Shared decoder definitions for GStreamer pipelines
//!
//! This module provides a single source of truth for decoder preferences,
//! used by both pipeline construction and the Insights diagnostic display.

/// Decoder definition with all metadata needed for pipeline construction and display
#[derive(Debug, Clone, Copy)]
pub struct DecoderDef {
    /// GStreamer element name (e.g., "jpegdec", "vah264dec")
    pub name: &'static str,
    /// Human-readable description for UI display
    pub description: &'static str,
    /// Optional GStreamer properties (e.g., "max-errors=-1")
    pub props: Option<&'static str>,
    /// Whether this is a hardware decoder
    pub is_hardware: bool,
}

impl DecoderDef {
    const fn sw(name: &'static str, description: &'static str) -> Self {
        Self {
            name,
            description,
            props: None,
            is_hardware: false,
        }
    }

    const fn sw_props(name: &'static str, description: &'static str, props: &'static str) -> Self {
        Self {
            name,
            description,
            props: Some(props),
            is_hardware: false,
        }
    }

    const fn hw(name: &'static str, description: &'static str) -> Self {
        Self {
            name,
            description,
            props: None,
            is_hardware: true,
        }
    }

    /// Format as GStreamer element string (e.g., "jpegdec max-errors=-1")
    pub fn as_gst_element(&self) -> String {
        match self.props {
            Some(p) => format!("{} {}", self.name, p),
            None => self.name.to_string(),
        }
    }
}

/// MJPEG decoders in preference order
///
/// **Order rationale:** CPU decoders first for reliability.
/// Hardware MJPEG decoding often has issues with non-standard JPEG streams from webcams.
pub const MJPEG_DECODERS: &[DecoderDef] = &[
    // Software decoders (preferred for reliability with webcam MJPEG)
    DecoderDef::sw_props("jpegdec", "GStreamer JPEG (Software)", "max-errors=-1"),
    DecoderDef::sw("avdec_mjpeg", "FFmpeg MJPEG (Software)"),
    // Hardware decoders (fallback)
    DecoderDef::hw("vaapijpegdec", "VA-API JPEG (Intel/AMD HW)"),
    DecoderDef::hw("nvjpegdec", "NVIDIA JPEG (NVDEC)"),
    DecoderDef::hw("v4l2jpegdec", "V4L2 JPEG (Hardware)"),
];

/// H.264 decoders in preference order
///
/// **Order rationale:** Hardware decoders first for performance.
/// H.264 decoding is computationally expensive; hardware acceleration is preferred.
pub const H264_DECODERS: &[DecoderDef] = &[
    // Hardware decoders (preferred for performance)
    DecoderDef::hw("vah264dec", "VA-API H.264 (Modern HW)"),
    DecoderDef::hw("vaapih264dec", "VA-API H.264 (Legacy HW)"),
    DecoderDef::hw("nvh264dec", "NVIDIA H.264 (NVDEC)"),
    DecoderDef::hw("d3d11h264dec", "Direct3D 11 H.264 (HW)"),
    DecoderDef::hw("v4l2h264dec", "V4L2 H.264 (Hardware)"),
    // Software decoders (fallback)
    DecoderDef::sw_props(
        "avdec_h264",
        "FFmpeg H.264 (SW, multi-threaded)",
        "max-threads=0",
    ),
    DecoderDef::sw("openh264dec", "OpenH264 (SW, single-threaded)"),
];

/// H.265/HEVC decoders in preference order
///
/// **Order rationale:** Hardware decoders first for performance.
/// H.265 decoding is even more computationally expensive than H.264.
pub const H265_DECODERS: &[DecoderDef] = &[
    // Hardware decoders (preferred for performance)
    DecoderDef::hw("vah265dec", "VA-API H.265 (Modern HW)"),
    DecoderDef::hw("vaapih265dec", "VA-API H.265 (Legacy HW)"),
    DecoderDef::hw("nvh265dec", "NVIDIA H.265 (NVDEC)"),
    DecoderDef::hw("d3d11h265dec", "Direct3D 11 H.265 (HW)"),
    DecoderDef::hw("v4l2h265dec", "V4L2 H.265 (Hardware)"),
    // Software decoder (fallback)
    DecoderDef::sw_props(
        "avdec_h265",
        "FFmpeg H.265 (SW, multi-threaded)",
        "max-threads=0",
    ),
];
