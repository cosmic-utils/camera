// SPDX-License-Identifier: GPL-3.0-only

//! Codec metadata and utilities for video pixel formats

use std::fmt;

/// Supported video codec/pixel format types
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Codec {
    // ===== Compressed formats =====
    /// Motion JPEG - Frame-by-frame JPEG compression
    MJPEG,
    /// H.264/AVC - Interframe compression
    H264,
    /// H.265/HEVC - High efficiency interframe compression
    H265,

    // ===== Packed YUV 4:2:2 formats =====
    /// YUYV 4:2:2 - Packed YUV (Y0 U Y1 V byte order)
    YUYV,
    /// UYVY 4:2:2 - Packed YUV (U Y0 V Y1 byte order)
    UYVY,
    /// YUY2 4:2:2 - Same as YUYV (Microsoft naming)
    YUY2,
    /// YVYU 4:2:2 - Packed YUV (Y0 V Y1 U byte order)
    YVYU,
    /// VYUY 4:2:2 - Packed YUV (V Y0 U Y1 byte order)
    VYUY,

    // ===== Planar/Semi-planar YUV 4:2:0 formats =====
    /// NV12 4:2:0 - Semi-planar YUV (Y plane + interleaved UV)
    NV12,
    /// NV21 4:2:0 - Semi-planar YUV (Y plane + interleaved VU)
    NV21,
    /// YV12 4:2:0 - Planar YUV (Y + V + U planes)
    YV12,
    /// I420 4:2:0 - Planar YUV (Y + U + V planes)
    I420,

    // ===== RGB formats =====
    /// RGB 24-bit - Uncompressed RGB (3 bytes per pixel)
    RGB24,
    /// RGB 32-bit - Uncompressed RGBA (4 bytes per pixel)
    RGB32,
    /// BGR 24-bit - Uncompressed BGR (3 bytes per pixel)
    BGR24,
    /// BGR 32-bit - Uncompressed BGRA (4 bytes per pixel)
    BGR32,

    // ===== Bayer patterns (raw sensor data) =====
    /// Bayer GRBG - Green-Red / Blue-Green pattern (Kinect uses this)
    BayerGRBG,
    /// Bayer RGGB - Red-Green / Green-Blue pattern
    BayerRGGB,
    /// Bayer BGGR - Blue-Green / Green-Red pattern
    BayerBGGR,
    /// Bayer GBRG - Green-Blue / Red-Green pattern
    BayerGBRG,

    // ===== Depth/IR formats =====
    /// Y10B - 10-bit packed grayscale (depth sensor)
    Y10B,
    /// IR10 - 10-bit packed infrared (Kinect IR sensor)
    IR10,
    /// Y16 - 16-bit grayscale (depth in mm)
    Y16,
    /// GREY/Y8 - 8-bit grayscale
    GREY,

    // ===== Special =====
    /// Unknown/unsupported codec
    Unknown,
}

impl Codec {
    /// Parse codec from FourCC string
    pub fn from_fourcc(fourcc: &str) -> Self {
        match fourcc.to_uppercase().as_str() {
            // Compressed
            "MJPG" | "JPEG" => Self::MJPEG,
            "H264" | "AVC1" => Self::H264,
            "H265" | "HEVC" => Self::H265,

            // Packed YUV 4:2:2
            "YUYV" | "YUY2" => Self::YUYV,
            "UYVY" => Self::UYVY,
            "YVYU" => Self::YVYU,
            "VYUY" => Self::VYUY,

            // Planar/Semi-planar YUV 4:2:0
            "NV12" => Self::NV12,
            "NV21" => Self::NV21,
            "YV12" => Self::YV12,
            "I420" | "IYUV" => Self::I420,

            // RGB - various naming conventions (V4L2, GStreamer, etc.)
            "RGB" | "RGB3" | "RGB24" => Self::RGB24,
            "RGBA" | "RGBX" | "RGB4" | "RGB32" => Self::RGB32,
            "BGR" | "BGR3" | "BGR24" => Self::BGR24,
            "BGRA" | "BGRX" | "BGR4" | "BGR32" => Self::BGR32,
            // GStreamer ARGB/XRGB variants (alpha/padding in different positions)
            "ARGB" | "XRGB" | "ARGB32" | "ARGB8888" => Self::RGB32,
            "ABGR" | "XBGR" | "ABGR32" | "ABGR8888" => Self::BGR32,

            // Bayer patterns (V4L2 FourCC codes + libcamera naming)
            "GRBG" | "BA81" | "SGRBG8" => Self::BayerGRBG,
            "RGGB" | "SRGGB8" => Self::BayerRGGB,
            "BGGR" | "BA82" | "SBGGR8" => Self::BayerBGGR,
            "GBRG" | "SGBRG8" => Self::BayerGBRG,
            // Generic "BAYER" defaults to GRBG (most common, Kinect uses this)
            "BAYER" => Self::BayerGRBG,
            // libcamera Bayer format names (e.g., "BayerRGGB10LE" from caps enumeration)
            s if s.starts_with("BAYER") => {
                if s.contains("GRBG") {
                    Self::BayerGRBG
                } else if s.contains("RGGB") {
                    Self::BayerRGGB
                } else if s.contains("BGGR") {
                    Self::BayerBGGR
                } else if s.contains("GBRG") {
                    Self::BayerGBRG
                } else {
                    Self::BayerGRBG
                }
            }

            // Depth/IR/Grayscale
            "Y10B" => Self::Y10B,
            "IR10" => Self::IR10,
            "Y16" | "Y16 " => Self::Y16,
            "GREY" | "GRAY8" | "Y8" | "Y800" => Self::GREY,

            _ => Self::Unknown,
        }
    }

    /// Get the FourCC code for this codec
    pub fn fourcc(&self) -> &'static str {
        match self {
            Self::MJPEG => "MJPG",
            Self::H264 => "H264",
            Self::H265 => "H265",
            Self::YUYV => "YUYV",
            Self::UYVY => "UYVY",
            Self::YUY2 => "YUY2",
            Self::YVYU => "YVYU",
            Self::VYUY => "VYUY",
            Self::NV12 => "NV12",
            Self::NV21 => "NV21",
            Self::YV12 => "YV12",
            Self::I420 => "I420",
            Self::RGB24 => "RGB3",
            Self::RGB32 => "RGB4",
            Self::BGR24 => "BGR3",
            Self::BGR32 => "BGR4",
            Self::BayerGRBG => "GRBG",
            Self::BayerRGGB => "RGGB",
            Self::BayerBGGR => "BGGR",
            Self::BayerGBRG => "GBRG",
            Self::Y10B => "Y10B",
            Self::IR10 => "IR10",
            Self::Y16 => "Y16",
            Self::GREY => "GREY",
            Self::Unknown => "UNKN",
        }
    }

    /// Short human-readable description for UI dropdowns
    /// Format: "Category" - use with `display_detail()` for full display
    pub fn short_description(&self) -> &'static str {
        match self {
            Self::MJPEG => "Motion JPEG",
            Self::H264 => "H.264/AVC",
            Self::H265 => "H.265/HEVC",
            Self::YUYV | Self::UYVY | Self::YUY2 | Self::YVYU | Self::VYUY => "YUV",
            Self::NV12 | Self::NV21 | Self::YV12 | Self::I420 => "YUV",
            Self::RGB24 => "RGB 24-bit",
            Self::RGB32 => "RGBA 32-bit",
            Self::BGR24 => "BGR 24-bit",
            Self::BGR32 => "BGRA 32-bit",
            Self::BayerGRBG | Self::BayerRGGB | Self::BayerBGGR | Self::BayerGBRG => "Bayer",
            Self::Y10B => "Depth 10-bit",
            Self::IR10 => "Infrared 10-bit",
            Self::Y16 => "Depth 16-bit",
            Self::GREY => "Grayscale 8-bit",
            Self::Unknown => "Unknown",
        }
    }

    /// Detail string for parentheses in UI dropdowns
    /// Returns fourcc with subsampling info where relevant
    pub fn display_detail(&self) -> &'static str {
        match self {
            // Compressed - just fourcc
            Self::MJPEG => "MJPG",
            Self::H264 => "H264",
            Self::H265 => "H265",
            // YUV packed 4:2:2 - fourcc + subsampling
            Self::YUYV => "YUYV 4:2:2",
            Self::UYVY => "UYVY 4:2:2",
            Self::YUY2 => "YUY2 4:2:2",
            Self::YVYU => "YVYU 4:2:2",
            Self::VYUY => "VYUY 4:2:2",
            // YUV planar 4:2:0 - fourcc + subsampling
            Self::NV12 => "NV12 4:2:0",
            Self::NV21 => "NV21 4:2:0",
            Self::YV12 => "YV12 4:2:0",
            Self::I420 => "I420 4:2:0",
            // RGB - just fourcc
            Self::RGB24 => "RGB",
            Self::RGB32 => "RGBA",
            Self::BGR24 => "BGR",
            Self::BGR32 => "BGRA",
            // Bayer - pattern name
            Self::BayerGRBG => "GRBG",
            Self::BayerRGGB => "RGGB",
            Self::BayerBGGR => "BGGR",
            Self::BayerGBRG => "GBRG",
            // Depth/IR - fourcc
            Self::Y10B => "Y10B",
            Self::IR10 => "IR10",
            Self::Y16 => "Y16",
            Self::GREY => "Y8",
            Self::Unknown => "?",
        }
    }

    /// Long detailed description for settings panel
    pub fn long_description(&self) -> &'static str {
        match self {
            Self::MJPEG => "Motion JPEG - Compressed (frame-by-frame JPEG)",
            Self::H264 => "H.264/AVC - Highly compressed (interframe)",
            Self::H265 => "H.265/HEVC - Very efficient (interframe)",
            Self::YUYV => "YUYV 4:2:2 - Packed YUV (Y0 U Y1 V)",
            Self::UYVY => "UYVY 4:2:2 - Packed YUV (U Y0 V Y1)",
            Self::YUY2 => "YUY2 4:2:2 - Packed YUV (same as YUYV)",
            Self::YVYU => "YVYU 4:2:2 - Packed YUV (Y0 V Y1 U)",
            Self::VYUY => "VYUY 4:2:2 - Packed YUV (V Y0 U Y1)",
            Self::NV12 => "NV12 4:2:0 - Semi-planar (Y + UV interleaved)",
            Self::NV21 => "NV21 4:2:0 - Semi-planar (Y + VU interleaved)",
            Self::YV12 => "YV12 4:2:0 - Planar (Y + V + U planes)",
            Self::I420 => "I420 4:2:0 - Planar (Y + U + V planes)",
            Self::RGB24 => "RGB 24-bit - Uncompressed (3 bytes/pixel)",
            Self::RGB32 => "RGBA 32-bit - Uncompressed (4 bytes/pixel)",
            Self::BGR24 => "BGR 24-bit - Uncompressed (3 bytes/pixel)",
            Self::BGR32 => "BGRA 32-bit - Uncompressed (4 bytes/pixel)",
            Self::BayerGRBG => "Bayer GRBG - Raw sensor (Green-Red/Blue-Green)",
            Self::BayerRGGB => "Bayer RGGB - Raw sensor (Red-Green/Green-Blue)",
            Self::BayerBGGR => "Bayer BGGR - Raw sensor (Blue-Green/Green-Red)",
            Self::BayerGBRG => "Bayer GBRG - Raw sensor (Green-Blue/Red-Green)",
            Self::Y10B => "Y10B - Depth sensor (10-bit packed)",
            Self::IR10 => "IR10 - Infrared sensor (10-bit packed)",
            Self::Y16 => "Y16 - Depth sensor (16-bit per pixel)",
            Self::GREY => "GREY - Grayscale (8-bit per pixel)",
            Self::Unknown => "Unknown codec",
        }
    }

    /// Check if this is a raw/uncompressed format
    pub fn is_raw(&self) -> bool {
        matches!(
            self,
            Self::YUYV
                | Self::UYVY
                | Self::YUY2
                | Self::YVYU
                | Self::VYUY
                | Self::NV12
                | Self::NV21
                | Self::YV12
                | Self::I420
                | Self::RGB24
                | Self::RGB32
                | Self::BGR24
                | Self::BGR32
                | Self::BayerGRBG
                | Self::BayerRGGB
                | Self::BayerBGGR
                | Self::BayerGBRG
                | Self::Y10B
                | Self::IR10
                | Self::Y16
                | Self::GREY
        )
    }

    /// Check if this is a Bayer pattern format
    pub fn is_bayer(&self) -> bool {
        matches!(
            self,
            Self::BayerGRBG | Self::BayerRGGB | Self::BayerBGGR | Self::BayerGBRG
        )
    }

    /// Check if this is a YUV format
    pub fn is_yuv(&self) -> bool {
        matches!(
            self,
            Self::YUYV
                | Self::UYVY
                | Self::YUY2
                | Self::YVYU
                | Self::VYUY
                | Self::NV12
                | Self::NV21
                | Self::YV12
                | Self::I420
        )
    }

    /// Check if this is a depth format
    pub fn is_depth(&self) -> bool {
        matches!(self, Self::Y10B | Self::Y16)
    }

    /// Check if this is an IR format
    pub fn is_ir(&self) -> bool {
        matches!(self, Self::IR10 | Self::GREY)
    }

    /// Check if this codec needs a decoder
    pub fn needs_decoder(&self) -> bool {
        matches!(self, Self::MJPEG | Self::H264 | Self::H265)
    }

    /// Get preference rank for codec selection (lower = higher priority)
    /// Used for automatic format selection
    pub fn preference_rank(&self) -> u32 {
        match self {
            // Raw formats - highest priority (no decoding overhead)
            // Bayer formats preferred for higher framerate
            Self::BayerGRBG => 0,
            Self::BayerRGGB => 1,
            Self::BayerBGGR => 2,
            Self::BayerGBRG => 3,
            // Packed YUV 4:2:2 - good quality, moderate bandwidth
            Self::YUYV => 10,
            Self::UYVY => 11,
            Self::YUY2 => 12,
            Self::YVYU => 13,
            Self::VYUY => 14,
            // Planar YUV 4:2:0 - lower bandwidth
            Self::NV12 => 20,
            Self::NV21 => 21,
            Self::YV12 => 22,
            Self::I420 => 23,
            // H.264 - good compression
            Self::H264 => 30,
            // MJPEG - moderate compression
            Self::MJPEG => 40,
            // H.265 - very high compression but more CPU intensive
            Self::H265 => 50,
            // RGB formats - large but simple
            Self::RGB24 => 60,
            Self::RGB32 => 61,
            Self::BGR24 => 62,
            Self::BGR32 => 63,
            // Depth/IR - specialized formats
            Self::GREY => 70,
            Self::Y10B => 71,
            Self::IR10 => 72,
            Self::Y16 => 73,
            // Unknown - lowest priority
            Self::Unknown => 100,
        }
    }

    /// Estimate bits per pixel for bandwidth calculation
    pub fn bits_per_pixel(&self) -> f64 {
        match self {
            // Compressed - variable, estimate
            Self::MJPEG => 4.0, // MJPEG typically 2-8 bpp
            Self::H264 => 0.5,  // H.264 very efficient
            Self::H265 => 0.25, // H.265 even more efficient
            // Packed YUV 4:2:2 - 16 bits per pixel
            Self::YUYV | Self::UYVY | Self::YUY2 | Self::YVYU | Self::VYUY => 16.0,
            // Planar YUV 4:2:0 - 12 bits per pixel
            Self::NV12 | Self::NV21 | Self::YV12 | Self::I420 => 12.0,
            // RGB
            Self::RGB24 | Self::BGR24 => 24.0,
            Self::RGB32 | Self::BGR32 => 32.0,
            // Bayer - 8 bits per pixel (raw sensor)
            Self::BayerGRBG | Self::BayerRGGB | Self::BayerBGGR | Self::BayerGBRG => 8.0,
            // Depth/IR
            Self::GREY => 8.0,
            Self::Y10B => 10.0,
            Self::IR10 => 10.0,
            Self::Y16 => 16.0,
            // Unknown
            Self::Unknown => 8.0,
        }
    }
}

impl fmt::Display for Codec {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.short_description())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_codec_parsing() {
        assert_eq!(Codec::from_fourcc("MJPG"), Codec::MJPEG);
        assert_eq!(Codec::from_fourcc("H264"), Codec::H264);
        assert_eq!(Codec::from_fourcc("YUYV"), Codec::YUYV);
        assert_eq!(Codec::from_fourcc("UYVY"), Codec::UYVY);
        assert_eq!(Codec::from_fourcc("GRBG"), Codec::BayerGRBG);
        assert_eq!(Codec::from_fourcc("BAYER"), Codec::BayerGRBG);
        assert_eq!(Codec::from_fourcc("UNKN"), Codec::Unknown);
    }

    #[test]
    fn test_gstreamer_format_names() {
        // GStreamer-style RGB variants
        assert_eq!(Codec::from_fourcc("RGB"), Codec::RGB24);
        assert_eq!(Codec::from_fourcc("RGBA"), Codec::RGB32);
        assert_eq!(Codec::from_fourcc("RGBX"), Codec::RGB32);
        assert_eq!(Codec::from_fourcc("BGR"), Codec::BGR24);
        assert_eq!(Codec::from_fourcc("BGRA"), Codec::BGR32);
        assert_eq!(Codec::from_fourcc("BGRX"), Codec::BGR32);
        assert_eq!(Codec::from_fourcc("ARGB"), Codec::RGB32);
        assert_eq!(Codec::from_fourcc("XRGB"), Codec::RGB32);
        assert_eq!(Codec::from_fourcc("ABGR"), Codec::BGR32);
        assert_eq!(Codec::from_fourcc("XBGR"), Codec::BGR32);
        // GStreamer grayscale
        assert_eq!(Codec::from_fourcc("GRAY8"), Codec::GREY);
    }

    #[test]
    fn test_bayer_detection() {
        assert!(Codec::BayerGRBG.is_bayer());
        assert!(Codec::BayerRGGB.is_bayer());
        assert!(!Codec::YUYV.is_bayer());
        assert!(!Codec::MJPEG.is_bayer());
    }

    #[test]
    fn test_yuv_detection() {
        assert!(Codec::YUYV.is_yuv());
        assert!(Codec::UYVY.is_yuv());
        assert!(Codec::NV12.is_yuv());
        assert!(!Codec::BayerGRBG.is_yuv());
        assert!(!Codec::MJPEG.is_yuv());
    }

    #[test]
    fn test_raw_detection() {
        assert!(Codec::YUYV.is_raw());
        assert!(Codec::NV12.is_raw());
        assert!(Codec::BayerGRBG.is_raw());
        assert!(!Codec::MJPEG.is_raw());
        assert!(!Codec::H264.is_raw());
    }

    #[test]
    fn test_decoder_requirement() {
        assert!(Codec::MJPEG.needs_decoder());
        assert!(Codec::H264.needs_decoder());
        assert!(!Codec::YUYV.needs_decoder());
        assert!(!Codec::NV12.needs_decoder());
    }

    #[test]
    fn test_preference_ranking() {
        assert!(Codec::BayerGRBG.preference_rank() < Codec::YUYV.preference_rank());
        assert!(Codec::YUYV.preference_rank() < Codec::MJPEG.preference_rank());
        assert!(Codec::H264.preference_rank() < Codec::H265.preference_rank());
        assert!(Codec::Unknown.preference_rank() > Codec::MJPEG.preference_rank());
    }
}
