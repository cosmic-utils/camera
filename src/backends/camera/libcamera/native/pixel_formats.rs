// SPDX-License-Identifier: GPL-3.0-only

//! Pixel format mapping between libcamera DRM fourcc and internal PixelFormat types.

use crate::backends::camera::types::PixelFormat;
use drm_fourcc::DrmFourcc;
use tracing::debug;

/// Map libcamera PixelFormat (DRM fourcc) to our PixelFormat enum
///
/// DRM fourcc names describe MSB-to-LSB bit order in a 32-bit word.
/// On little-endian (ARM, x86), the memory byte order is reversed:
///   DRM ABGR8888 → memory R,G,B,A → PixelFormat::RGBA
///   DRM ARGB8888 → memory B,G,R,A → PixelFormat::BGRA
///   DRM RGBA8888 → memory A,B,G,R → PixelFormat::ABGR
pub(crate) fn map_pixel_format(pf: libcamera::pixel_format::PixelFormat) -> Option<PixelFormat> {
    match DrmFourcc::try_from(pf.fourcc()) {
        Ok(DrmFourcc::Abgr8888) | Ok(DrmFourcc::Xbgr8888) => Some(PixelFormat::RGBA),
        Ok(DrmFourcc::Argb8888) | Ok(DrmFourcc::Xrgb8888) => Some(PixelFormat::BGRA),
        Ok(DrmFourcc::Rgba8888) => Some(PixelFormat::ABGR),
        Ok(DrmFourcc::Nv12) => Some(PixelFormat::NV12),
        Ok(DrmFourcc::Nv21) => Some(PixelFormat::NV21),
        Ok(DrmFourcc::Yuv420) => Some(PixelFormat::I420),
        Ok(DrmFourcc::Yuyv) => Some(PixelFormat::YUYV),
        Ok(DrmFourcc::Uyvy) => Some(PixelFormat::UYVY),
        Ok(DrmFourcc::Yvyu) => Some(PixelFormat::YVYU),
        Ok(DrmFourcc::Vyuy) => Some(PixelFormat::VYUY),
        Ok(DrmFourcc::Rgb888) | Ok(DrmFourcc::Bgr888) => Some(PixelFormat::RGB24),
        _ => map_bayer_format(pf),
    }
}

/// Map Bayer pixel formats from fourcc bytes or format info
fn map_bayer_format(pf: libcamera::pixel_format::PixelFormat) -> Option<PixelFormat> {
    // Use format info name as the most reliable approach
    if let Some(info) = pf.info() {
        let name = info.name.to_lowercase();
        if name.contains("rggb") {
            return Some(PixelFormat::BayerRGGB);
        } else if name.contains("bggr") {
            return Some(PixelFormat::BayerBGGR);
        } else if name.contains("grbg") {
            return Some(PixelFormat::BayerGRBG);
        } else if name.contains("gbrg") {
            return Some(PixelFormat::BayerGBRG);
        }
    }

    // Fallback: check raw fourcc bytes for common patterns
    let bytes = pf.fourcc().to_le_bytes();
    match &bytes {
        [b'R', b'G', ..] => Some(PixelFormat::BayerRGGB),
        [b'B', b'G', ..] => Some(PixelFormat::BayerBGGR),
        [b'G', b'R', ..] => Some(PixelFormat::BayerGRBG),
        [b'G', b'B', ..] => Some(PixelFormat::BayerGBRG),
        // CSI-2 packed formats: pRAA = SRGGB10_CSI2P, etc.
        [b'p', b'R', ..] => Some(PixelFormat::BayerRGGB),
        [b'p', b'B', ..] => Some(PixelFormat::BayerBGGR),
        [b'p', b'g', ..] => Some(PixelFormat::BayerGRBG),
        [b'p', b'G', ..] => Some(PixelFormat::BayerGBRG),
        _ => {
            debug!(
                fourcc = format!("0x{:08x}", pf.fourcc()),
                bytes = ?bytes,
                "Unknown pixel format"
            );
            None
        }
    }
}

/// Get a human-readable name for a libcamera pixel format
pub(crate) fn pixel_format_name(pf: libcamera::pixel_format::PixelFormat) -> String {
    if let Some(info) = pf.info() {
        return info.name.clone();
    }
    if let Ok(drm) = DrmFourcc::try_from(pf.fourcc()) {
        return format!("{:?}", drm);
    }
    // Decode fourcc bytes as ASCII, map Bayer fourccs to canonical names
    let bytes = pf.fourcc().to_le_bytes();
    if bytes.iter().all(|b| b.is_ascii_graphic()) {
        let fourcc_str: String = bytes.iter().map(|&b| b as char).collect();
        return v4l2_bayer_fourcc_name(&fourcc_str).unwrap_or(fourcc_str);
    }
    format!("0x{:08x}", pf.fourcc())
}

/// Check if a libcamera pixel format is a Bayer (raw) format
pub(crate) fn is_bayer_format(pf: libcamera::pixel_format::PixelFormat) -> bool {
    if let Some(info) = pf.info() {
        matches!(
            info.colour_encoding,
            libcamera::pixel_format::ColourEncoding::Raw
        )
    } else {
        // Fallback: check fourcc bytes for common Bayer patterns
        let bytes = pf.fourcc().to_le_bytes();
        matches!(
            &bytes,
            [b'R', b'G', ..]
                | [b'B', b'G', ..]
                | [b'G', b'R', ..]
                | [b'G', b'B', ..]
                | [b'p', b'R', ..]
                | [b'p', b'B', ..]
                | [b'p', b'g', ..]
                | [b'p', b'G', ..]
        )
    }
}

/// Map V4L2 Bayer fourcc strings to libcamera-style canonical names
fn v4l2_bayer_fourcc_name(fourcc: &str) -> Option<String> {
    let name = match fourcc {
        // 10-bit unpacked
        "RG10" => "SRGGB10",
        "BG10" => "SBGGR10",
        "GR10" => "SGRBG10",
        "GB10" => "SGBRG10",
        // 10-bit CSI-2 packed
        "pRAA" => "SRGGB10_CSI2P",
        "pBAA" => "SBGGR10_CSI2P",
        "pgAA" => "SGRBG10_CSI2P",
        "pGAA" => "SGBRG10_CSI2P",
        // 12-bit unpacked
        "RG12" => "SRGGB12",
        "BG12" => "SBGGR12",
        "GR12" => "SGRBG12",
        "GB12" => "SGBRG12",
        // 12-bit CSI-2 packed
        "pRCC" => "SRGGB12_CSI2P",
        "pBCC" => "SBGGR12_CSI2P",
        "pgCC" => "SGRBG12_CSI2P",
        "pGCC" => "SGBRG12_CSI2P",
        // 8-bit
        "RGGB" => "SRGGB8",
        "BGGR" => "SBGGR8",
        "GRBG" => "SGRBG8",
        "GBRG" => "SGBRG8",
        // 16-bit
        "RG16" => "SRGGB16",
        "BG16" => "SBGGR16",
        "GR16" => "SGRBG16",
        "GB16" => "SGBRG16",
        _ => return None,
    };
    Some(name.to_string())
}
