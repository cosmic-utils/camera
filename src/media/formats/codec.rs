// SPDX-License-Identifier: MPL-2.0
// Codec utilities - some methods for future format selection UI
#![allow(dead_code)]

//! Codec metadata and utilities for video pixel formats

use std::fmt;

/// Supported video codec/pixel format types
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Codec {
    /// Motion JPEG - Frame-by-frame JPEG compression
    MJPEG,
    /// H.264/AVC - Interframe compression
    H264,
    /// H.265/HEVC - High efficiency interframe compression
    H265,
    /// YUYV 4:2:2 - Uncompressed YUV
    YUYV,
    /// UYVY 4:2:2 - Uncompressed YUV (alternate byte order)
    UYVY,
    /// YUY2 4:2:2 - Uncompressed YUV (alternate format)
    YUY2,
    /// NV12 4:2:0 - Semi-planar YUV
    NV12,
    /// YV12 4:2:0 - Planar YUV
    YV12,
    /// I420 4:2:0 - Planar YUV
    I420,
    /// RGB 24-bit - Uncompressed RGB
    RGB24,
    /// RGB 32-bit - Uncompressed RGB with alpha
    RGB32,
    /// Unknown/unsupported codec
    Unknown,
}

impl Codec {
    /// Parse codec from FourCC string
    pub fn from_fourcc(fourcc: &str) -> Self {
        match fourcc {
            "MJPG" | "JPEG" => Self::MJPEG,
            "H264" => Self::H264,
            "H265" | "HEVC" => Self::H265,
            "YUYV" => Self::YUYV,
            "UYVY" => Self::UYVY,
            "YUY2" => Self::YUY2,
            "NV12" => Self::NV12,
            "YV12" => Self::YV12,
            "I420" => Self::I420,
            "RGB3" | "BGR3" => Self::RGB24,
            "RGB4" | "BGR4" => Self::RGB32,
            _ => Self::Unknown,
        }
    }

    /// Get the FourCC code for this codec
    #[allow(dead_code)]
    pub fn fourcc(&self) -> &'static str {
        match self {
            Self::MJPEG => "MJPG",
            Self::H264 => "H264",
            Self::H265 => "H265",
            Self::YUYV => "YUYV",
            Self::UYVY => "UYVY",
            Self::YUY2 => "YUY2",
            Self::NV12 => "NV12",
            Self::YV12 => "YV12",
            Self::I420 => "I420",
            Self::RGB24 => "RGB3",
            Self::RGB32 => "RGB4",
            Self::Unknown => "UNKN",
        }
    }

    /// Short human-readable description for UI dropdowns
    pub fn short_description(&self) -> &'static str {
        match self {
            Self::MJPEG => "Motion JPEG",
            Self::H264 => "H.264/AVC",
            Self::H265 => "H.265/HEVC",
            Self::YUYV => "YUYV 4:2:2",
            Self::UYVY => "UYVY 4:2:2",
            Self::YUY2 => "YUY2 4:2:2",
            Self::NV12 => "NV12 4:2:0",
            Self::YV12 => "YV12 4:2:0",
            Self::I420 => "I420 4:2:0",
            Self::RGB24 => "RGB 24-bit",
            Self::RGB32 => "RGB 32-bit",
            Self::Unknown => "Unknown",
        }
    }

    /// Long detailed description for settings panel
    pub fn long_description(&self) -> &'static str {
        match self {
            Self::MJPEG => "Motion JPEG - Compressed (frame-by-frame JPEG)",
            Self::H264 => "H.264/AVC - Highly compressed (interframe)",
            Self::H265 => "H.265/HEVC - Very efficient (interframe)",
            Self::YUYV => "YUYV 4:2:2 - Uncompressed YUV",
            Self::UYVY => "UYVY 4:2:2 - Uncompressed YUV",
            Self::YUY2 => "YUY2 4:2:2 - Uncompressed YUV",
            Self::NV12 => "NV12 4:2:0 - Semi-planar YUV",
            Self::YV12 => "YV12 4:2:0 - Planar YUV",
            Self::I420 => "I420 4:2:0 - Planar YUV",
            Self::RGB24 => "RGB 24-bit - Uncompressed",
            Self::RGB32 => "RGB 32-bit - Uncompressed",
            Self::Unknown => "Unknown codec",
        }
    }

    /// Check if this is a raw/uncompressed format
    pub fn is_raw(&self) -> bool {
        matches!(
            self,
            Self::YUYV | Self::UYVY | Self::YUY2 | Self::NV12 | Self::YV12 | Self::I420
        )
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
            Self::YUYV => 0,
            Self::UYVY => 1,
            Self::YUY2 => 2,
            Self::NV12 => 3,
            Self::YV12 => 4,
            Self::I420 => 5,
            // H.264 - good compression
            Self::H264 => 10,
            // MJPEG - moderate compression
            Self::MJPEG => 20,
            // H.265 - very high compression but more CPU intensive
            Self::H265 => 30,
            // RGB formats - large but simple
            Self::RGB24 => 40,
            Self::RGB32 => 41,
            // Unknown - lowest priority
            Self::Unknown => 100,
        }
    }

    /// Estimate bits per pixel for bandwidth calculation
    pub fn bits_per_pixel(&self) -> f64 {
        match self {
            Self::MJPEG => 0.5,   // MJPEG typically 0.3-0.8 bpp
            Self::H264 => 0.1,    // H.264 very efficient
            Self::H265 => 0.05,   // H.265 even more efficient
            Self::YUYV => 16.0,   // 16 bits per pixel
            Self::UYVY => 16.0,   // 16 bits per pixel
            Self::YUY2 => 16.0,   // 16 bits per pixel
            Self::NV12 => 12.0,   // 12 bits per pixel (4:2:0)
            Self::YV12 => 12.0,   // 12 bits per pixel (4:2:0)
            Self::I420 => 12.0,   // 12 bits per pixel (4:2:0)
            Self::RGB24 => 24.0,  // 24 bits per pixel
            Self::RGB32 => 32.0,  // 32 bits per pixel
            Self::Unknown => 1.0, // Unknown, use minimal estimate
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
        assert_eq!(Codec::from_fourcc("UNKN"), Codec::Unknown);
    }

    #[test]
    fn test_raw_detection() {
        assert!(Codec::YUYV.is_raw());
        assert!(Codec::NV12.is_raw());
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
        assert!(Codec::YUYV.preference_rank() < Codec::MJPEG.preference_rank());
        assert!(Codec::H264.preference_rank() < Codec::H265.preference_rank());
        assert!(Codec::Unknown.preference_rank() > Codec::MJPEG.preference_rank());
    }
}
