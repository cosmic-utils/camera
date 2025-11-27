// SPDX-License-Identifier: MPL-2.0
// Format conversion utilities for future GStreamer integration
#![allow(dead_code)]

//! Format conversion utilities
//!
//! This module provides utilities for converting between different pixel formats.

use super::Codec;

/// Get GStreamer caps string for a codec
pub fn codec_to_gst_caps(codec: &Codec) -> &'static str {
    match codec {
        Codec::MJPEG => "image/jpeg",
        Codec::H264 => "video/x-h264",
        Codec::H265 => "video/x-h265",
        Codec::YUYV => "video/x-raw,format=YUYV",
        Codec::UYVY => "video/x-raw,format=UYVY",
        Codec::YUY2 => "video/x-raw,format=YUY2",
        Codec::NV12 => "video/x-raw,format=NV12",
        Codec::YV12 => "video/x-raw,format=YV12",
        Codec::I420 => "video/x-raw,format=I420",
        Codec::RGB24 => "video/x-raw,format=RGB",
        Codec::RGB32 => "video/x-raw,format=RGBA",
        Codec::Unknown => "video/x-raw",
    }
}

/// Get appropriate GStreamer decoder element for a codec
pub fn codec_to_gst_decoder(codec: &Codec) -> Option<&'static str> {
    match codec {
        Codec::MJPEG => Some("jpegdec"),
        Codec::H264 => Some("decodebin"),
        Codec::H265 => Some("decodebin"),
        _ => None, // Raw formats don't need decoders
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_codec_to_caps() {
        assert_eq!(codec_to_gst_caps(&Codec::MJPEG), "image/jpeg");
        assert_eq!(codec_to_gst_caps(&Codec::H264), "video/x-h264");
        assert_eq!(codec_to_gst_caps(&Codec::YUYV), "video/x-raw,format=YUYV");
    }

    #[test]
    fn test_codec_to_decoder() {
        assert_eq!(codec_to_gst_decoder(&Codec::MJPEG), Some("jpegdec"));
        assert_eq!(codec_to_gst_decoder(&Codec::H264), Some("decodebin"));
        assert_eq!(codec_to_gst_decoder(&Codec::YUYV), None);
        assert_eq!(codec_to_gst_decoder(&Codec::NV12), None);
    }
}
