// SPDX-License-Identifier: GPL-3.0-only
//! Unified pixel format conversion utilities for depth camera backends
//!
//! This module consolidates all pixel format conversion functions used by
//! the various depth camera backends (native freedepth, V4L2 kernel, V4L2 raw).

/// Convert UYVY (YUV 4:2:2) to RGBA
///
/// UYVY format: U0 Y0 V0 Y1 - each 4-byte group encodes 2 pixels.
/// Uses BT.601 coefficients for YUV to RGB conversion.
pub fn uyvy_to_rgba(data: &[u8], width: u32, height: u32) -> Vec<u8> {
    let pixel_count = (width * height) as usize;
    let mut rgba = Vec::with_capacity(pixel_count * 4);

    // UYVY: U0 Y0 V0 Y1 - processes 2 pixels at a time
    for chunk in data.chunks_exact(4) {
        let u = chunk[0] as f32 - 128.0;
        let y0 = chunk[1] as f32;
        let v = chunk[2] as f32 - 128.0;
        let y1 = chunk[3] as f32;

        // Convert YUV to RGB (BT.601)
        for y in [y0, y1] {
            let r = (y + 1.402 * v).clamp(0.0, 255.0) as u8;
            let g = (y - 0.344 * u - 0.714 * v).clamp(0.0, 255.0) as u8;
            let b = (y + 1.772 * u).clamp(0.0, 255.0) as u8;

            rgba.push(r);
            rgba.push(g);
            rgba.push(b);
            rgba.push(255);

            if rgba.len() >= pixel_count * 4 {
                break;
            }
        }
    }

    rgba
}

/// Convert Bayer GRBG to RGBA using simple nearest-neighbor demosaic
///
/// Bayer pattern (GRBG):
/// ```text
/// G R
/// B G
/// ```
/// Each 2x2 block produces 4 pixels with the same RGB values.
pub fn grbg_to_rgba(data: &[u8], width: u32, height: u32) -> Vec<u8> {
    let w = width as usize;
    let h = height as usize;
    let mut rgba = vec![0u8; w * h * 4];

    // Simple demosaic: for each 2x2 Bayer block
    for y in (0..h.saturating_sub(1)).step_by(2) {
        for x in (0..w.saturating_sub(1)).step_by(2) {
            let g0 = data[y * w + x] as u32;
            let r = data[y * w + x + 1] as u32;
            let b = data[(y + 1) * w + x] as u32;
            let g1 = data[(y + 1) * w + x + 1] as u32;
            let g = ((g0 + g1) / 2) as u8;

            // Apply same color to all 4 pixels in block
            for dy in 0..2 {
                for dx in 0..2 {
                    let idx = ((y + dy) * w + (x + dx)) * 4;
                    rgba[idx] = r as u8;
                    rgba[idx + 1] = g;
                    rgba[idx + 2] = b as u8;
                    rgba[idx + 3] = 255;
                }
            }
        }
    }

    rgba
}

/// Unpack Y10B (10-bit packed) depth data to 16-bit values
///
/// Y10B packs 4 10-bit values into 5 bytes:
/// ```text
/// [A9:A2][B9:B2][C9:C2][D9:D2][D1:D0,C1:C0,B1:B0,A1:A0]
/// ```
///
/// Returns raw 10-bit values (0-1023 range).
pub fn unpack_y10b(data: &[u8], width: u32, height: u32) -> Vec<u16> {
    let pixel_count = (width * height) as usize;
    let mut output = Vec::with_capacity(pixel_count);

    for chunk in data.chunks_exact(5) {
        if output.len() >= pixel_count {
            break;
        }

        let a = ((chunk[0] as u16) << 2) | ((chunk[4] as u16) & 0x03);
        let b = ((chunk[1] as u16) << 2) | (((chunk[4] as u16) >> 2) & 0x03);
        let c = ((chunk[2] as u16) << 2) | (((chunk[4] as u16) >> 4) & 0x03);
        let d = ((chunk[3] as u16) << 2) | (((chunk[4] as u16) >> 6) & 0x03);

        output.push(a);
        if output.len() < pixel_count {
            output.push(b);
        }
        if output.len() < pixel_count {
            output.push(c);
        }
        if output.len() < pixel_count {
            output.push(d);
        }
    }

    output
}

/// Unpack Y10B and scale to 16-bit range
///
/// Same as `unpack_y10b` but shifts values left by 6 bits to use full 16-bit range.
/// This is useful when the 10-bit values need to be treated as 16-bit depth.
pub fn unpack_y10b_to_u16(data: &[u8], width: u32, height: u32) -> Vec<u16> {
    unpack_y10b(data, width, height)
        .into_iter()
        .map(|v| v << 6)
        .collect()
}

/// Depth visualization options
#[derive(Debug, Clone, Copy, Default)]
pub struct DepthVisualizationOptions {
    /// Use grayscale instead of colormap (near=bright, far=dark)
    pub grayscale: bool,
    /// Quantize depth into bands for smoother visualization
    pub quantize: bool,
    /// Number of quantization bands (default: 32)
    pub quantize_bands: u32,
    /// Minimum depth in mm (values below are clamped)
    pub min_depth_mm: u16,
    /// Maximum depth in mm (values above are clamped)
    pub max_depth_mm: u16,
    /// Value that indicates invalid depth (rendered as black)
    pub invalid_value: u16,
}

impl DepthVisualizationOptions {
    /// Create options for Kinect sensor (freedepth usable range)
    pub fn kinect() -> Self {
        Self {
            grayscale: false,
            quantize: false,
            quantize_bands: 32,
            min_depth_mm: 500,   // DEPTH_MIN_USABLE_MM
            max_depth_mm: 4000,  // DEPTH_MAX_USABLE_MM
            invalid_value: 8191, // DEPTH_INVALID_THRESHOLD_MM
        }
    }

    /// Create options for generic depth sensor with auto-ranging
    pub fn auto_range() -> Self {
        Self {
            grayscale: true,
            quantize: false,
            quantize_bands: 32,
            min_depth_mm: 0,
            max_depth_mm: 0, // 0 = auto-detect from data
            invalid_value: 0,
        }
    }
}

/// Turbo colormap: perceptually uniform rainbow (blue=near, red=far)
///
/// Based on the Google Turbo colormap.
fn turbo(t: f32) -> [u8; 3] {
    let r = (0.13572138
        + t * (4.6153926 + t * (-42.66032 + t * (132.13108 + t * (-152.54825 + t * 59.28144)))))
        .clamp(0.0, 1.0);
    let g = (0.09140261
        + t * (2.19418 + t * (4.84296 + t * (-14.18503 + t * (4.27805 + t * 2.53377)))))
        .clamp(0.0, 1.0);
    let b = (0.1066733
        + t * (12.64194 + t * (-60.58204 + t * (109.99648 + t * (-82.52904 + t * 20.43388)))))
        .clamp(0.0, 1.0);
    [(r * 255.0) as u8, (g * 255.0) as u8, (b * 255.0) as u8]
}

/// Convert 16-bit depth values to RGB visualization
///
/// This is the unified depth visualization function that supports:
/// - Grayscale mode (near=bright, far=dark)
/// - Turbo colormap (blue=near, red=far)
/// - Optional band quantization
/// - Auto-ranging or fixed range
pub fn depth_to_rgb(
    depth: &[u16],
    width: u32,
    height: u32,
    options: &DepthVisualizationOptions,
) -> Vec<u8> {
    let pixel_count = (width * height) as usize;
    let mut rgb = Vec::with_capacity(pixel_count * 3);

    // Determine range (auto-detect if max_depth_mm is 0)
    let (min_depth, max_depth) = if options.max_depth_mm == 0 {
        // Auto-range: find min/max from data (excluding invalid)
        let mut min_d = u16::MAX;
        let mut max_d = 0u16;
        for &d in depth.iter().take(pixel_count) {
            if d != 0 && d != options.invalid_value && d < 10000 {
                min_d = min_d.min(d);
                max_d = max_d.max(d);
            }
        }
        if max_d <= min_d {
            (0u16, 4000u16) // Default fallback
        } else {
            (min_d, max_d)
        }
    } else {
        (options.min_depth_mm, options.max_depth_mm)
    };

    let range = (max_depth - min_depth) as f32;

    for &d in depth.iter().take(pixel_count) {
        if d == 0 || d == options.invalid_value || d >= 10000 {
            // Invalid depth - black
            rgb.extend_from_slice(&[0, 0, 0]);
        } else {
            // Normalize to 0.0-1.0 range
            let mut t = (d.saturating_sub(min_depth) as f32) / range;
            t = t.clamp(0.0, 1.0);

            // Quantize to bands for smoother visualization
            if options.quantize && options.quantize_bands > 0 {
                let bands = options.quantize_bands as f32;
                t = (t * bands).floor() / bands;
            }

            if options.grayscale {
                // Grayscale: near=bright, far=dark (invert t)
                let gray = ((1.0 - t) * 255.0) as u8;
                rgb.extend_from_slice(&[gray, gray, gray]);
            } else {
                // Colormap: turbo (blue=near, red=far)
                let color = turbo(t);
                rgb.extend_from_slice(&color);
            }
        }
    }

    rgb
}

/// Convert 16-bit depth values to RGBA visualization
///
/// Same as `depth_to_rgb` but outputs RGBA with alpha=255.
pub fn depth_to_rgba(
    depth: &[u16],
    width: u32,
    height: u32,
    options: &DepthVisualizationOptions,
) -> Vec<u8> {
    let rgb = depth_to_rgb(depth, width, height, options);
    let mut rgba = Vec::with_capacity(rgb.len() / 3 * 4);

    for chunk in rgb.chunks_exact(3) {
        rgba.push(chunk[0]);
        rgba.push(chunk[1]);
        rgba.push(chunk[2]);
        rgba.push(255);
    }

    rgba
}

/// Simple depth to grayscale conversion (upper 8 bits)
///
/// Fast conversion for preview when full visualization isn't needed.
pub fn depth_to_grayscale(depth: &[u16]) -> Vec<u8> {
    depth.iter().map(|&d| (d >> 8) as u8).collect()
}

/// Convert RGB to RGBA by adding alpha=255
pub fn rgb_to_rgba(rgb: &[u8]) -> Vec<u8> {
    let mut rgba = Vec::with_capacity(rgb.len() / 3 * 4);
    for chunk in rgb.chunks_exact(3) {
        rgba.push(chunk[0]);
        rgba.push(chunk[1]);
        rgba.push(chunk[2]);
        rgba.push(255);
    }
    rgba
}

/// Convert IR 8-bit grayscale data to RGB
///
/// Simply expands each grayscale byte to RGB triplet.
pub fn ir_8bit_to_rgb(data: &[u8], width: u32, height: u32) -> Vec<u8> {
    let pixel_count = (width * height) as usize;
    let mut rgb = Vec::with_capacity(pixel_count * 3);

    for &gray in data.iter().take(pixel_count) {
        rgb.extend_from_slice(&[gray, gray, gray]);
    }

    // Handle missing pixels (if any)
    while rgb.len() < pixel_count * 3 {
        rgb.extend_from_slice(&[0, 0, 0]);
    }

    rgb
}

/// Convert IR 10-bit unpacked data (u16 little-endian bytes) to RGB grayscale
///
/// The data is stored as little-endian u16 values (10-bit in lower bits).
pub fn ir_10bit_to_rgb(data: &[u8], width: u32, height: u32) -> Vec<u8> {
    let pixel_count = (width * height) as usize;
    let mut rgb = Vec::with_capacity(pixel_count * 3);

    // Interpret data as u16 (little-endian)
    for chunk in data.chunks_exact(2).take(pixel_count) {
        let val = u16::from_le_bytes([chunk[0], chunk[1]]);
        let gray = (val >> 2) as u8; // 10-bit to 8-bit
        rgb.extend_from_slice(&[gray, gray, gray]);
    }

    // Handle missing pixels (if any)
    while rgb.len() < pixel_count * 3 {
        rgb.extend_from_slice(&[0, 0, 0]);
    }

    rgb
}

/// Convert already-unpacked IR 10-bit values (u16) to RGB grayscale
///
/// Takes unpacked 10-bit values (0-1023) and converts to 8-bit grayscale RGB.
pub fn ir_10bit_unpacked_to_rgb(unpacked: &[u16], width: u32, height: u32) -> Vec<u8> {
    let pixel_count = (width * height) as usize;
    let mut rgb = Vec::with_capacity(pixel_count * 3);

    for &val in unpacked.iter().take(pixel_count) {
        let gray = (val >> 2) as u8; // 10-bit to 8-bit
        rgb.extend_from_slice(&[gray, gray, gray]);
    }

    // Handle any missing pixels
    while rgb.len() < pixel_count * 3 {
        rgb.extend_from_slice(&[0, 0, 0]);
    }

    rgb
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_y10b_unpacking() {
        // Kernel Y10B format: [A9:A2][B9:B2][C9:C2][D9:D2][D1:D0,C1:C0,B1:B0,A1:A0]
        // Test data: 4 pixels A=1023, B=512, C=256, D=0
        // Byte 0 = A[9:2] = 1023 >> 2 = 255
        // Byte 1 = B[9:2] = 512 >> 2 = 128
        // Byte 2 = C[9:2] = 256 >> 2 = 64
        // Byte 3 = D[9:2] = 0
        // Byte 4 = (D[1:0]<<6) | (C[1:0]<<4) | (B[1:0]<<2) | A[1:0]
        //        = (0<<6) | (0<<4) | (0<<2) | 3 = 3
        let raw_data = vec![255u8, 128, 64, 0, 3];
        let depth = unpack_y10b(&raw_data, 2, 2);

        assert_eq!(depth[0], 1023);
        assert_eq!(depth[1], 512);
        assert_eq!(depth[2], 256);
        assert_eq!(depth[3], 0);
    }

    #[test]
    fn test_y10b_to_u16() {
        // Same kernel Y10B format data as above
        let raw_data = vec![255u8, 128, 64, 0, 3];
        let depth = unpack_y10b_to_u16(&raw_data, 2, 2);

        // Values are shifted left by 6 to use full 16-bit range
        assert_eq!(depth[0], 1023 << 6);
        assert_eq!(depth[1], 512 << 6);
        assert_eq!(depth[2], 256 << 6);
        assert_eq!(depth[3], 0);
    }

    #[test]
    fn test_depth_to_rgba_grayscale() {
        let depth = vec![0xFFFF, 0x8000, 0x0000];
        let options = DepthVisualizationOptions {
            grayscale: true,
            max_depth_mm: 0, // Auto-range
            ..Default::default()
        };
        let rgba = depth_to_rgba(&depth, 3, 1, &options);

        assert_eq!(rgba.len(), 12);
        // Alpha should always be 255
        assert_eq!(rgba[3], 255);
        assert_eq!(rgba[7], 255);
        assert_eq!(rgba[11], 255);
    }

    #[test]
    fn test_uyvy_to_rgba() {
        // Pure white in YUV (Y=255, U=128, V=128)
        let uyvy = vec![128u8, 255, 128, 255];
        let rgba = uyvy_to_rgba(&uyvy, 2, 1);

        assert_eq!(rgba.len(), 8);
        // Both pixels should be near white
        assert!(rgba[0] > 250); // R
        assert!(rgba[1] > 250); // G
        assert!(rgba[2] > 250); // B
        assert_eq!(rgba[3], 255); // A
    }

    #[test]
    fn test_rgb_to_rgba() {
        let rgb = vec![255, 128, 64, 0, 0, 0];
        let rgba = rgb_to_rgba(&rgb);

        assert_eq!(rgba.len(), 8);
        assert_eq!(rgba[0..4], [255, 128, 64, 255]);
        assert_eq!(rgba[4..8], [0, 0, 0, 255]);
    }

    #[test]
    fn test_turbo_colormap() {
        // Test that colormap changes across the range (t=0 to t=1)
        let start = turbo(0.0);
        let mid = turbo(0.5);
        let end = turbo(1.0);

        // Colors should be different at different t values
        assert_ne!(start, mid);
        assert_ne!(mid, end);
        assert_ne!(start, end);

        // End (t=1) should have higher red than start (t=0)
        assert!(end[0] > start[0]);
    }
}
