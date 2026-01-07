// SPDX-License-Identifier: GPL-3.0-only

//! Depth visualization helpers
//!
//! Provides functions for converting depth data to viewable formats:
//! - Turbo colormap (blue=near, red=far)
//! - Grayscale (bright=near, dark=far)
//! - RGB to RGBA conversion

use super::constants::{DEPTH_COLORMAP_BANDS, DEPTH_MAX_MM, DEPTH_MAX_VALID_MM, DEPTH_MIN_MM};

/// Turbo colormap: perceptually uniform rainbow (blue=near, red=far)
///
/// Based on: https://ai.googleblog.com/2019/08/turbo-improved-rainbow-colormap-for.html
/// Simplified version with polynomial approximation.
#[inline]
fn turbo(t: f32) -> [u8; 4] {
    let r = (0.13572138
        + t * (4.6153926 + t * (-42.66032 + t * (132.13108 + t * (-152.54825 + t * 59.28144)))))
        .clamp(0.0, 1.0);
    let g = (0.09140261
        + t * (2.19418 + t * (4.84296 + t * (-14.18503 + t * (4.27805 + t * 2.53377)))))
        .clamp(0.0, 1.0);
    let b = (0.1066733
        + t * (12.64194 + t * (-60.58204 + t * (109.99648 + t * (-82.52904 + t * 20.43388)))))
        .clamp(0.0, 1.0);
    [(r * 255.0) as u8, (g * 255.0) as u8, (b * 255.0) as u8, 255]
}

/// Convert depth data (in millimeters) to RGBA visualization
///
/// # Arguments
/// * `depth_mm` - Depth values in millimeters (0 = invalid)
/// * `width` - Image width
/// * `height` - Image height
/// * `quantize` - If true, quantize to bands for smoother visualization
/// * `grayscale` - If true, use grayscale instead of colormap
///
/// # Returns
/// RGBA pixel data (4 bytes per pixel)
pub fn depth_mm_to_rgba(
    depth_mm: &[u16],
    width: u32,
    height: u32,
    quantize: bool,
    grayscale: bool,
) -> Vec<u8> {
    let pixel_count = (width * height) as usize;
    let mut rgba = Vec::with_capacity(pixel_count * 4);

    for &depth in depth_mm.iter().take(pixel_count) {
        if depth == 0 || depth > DEPTH_MAX_VALID_MM {
            // Invalid depth - black
            rgba.extend_from_slice(&[0, 0, 0, 255]);
        } else {
            // Normalize to 0.0-1.0 range (near=0.0, far=1.0)
            let mut t = ((depth as f32) - DEPTH_MIN_MM) / (DEPTH_MAX_MM - DEPTH_MIN_MM);
            t = t.clamp(0.0, 1.0);

            // Quantize to bands for smoother visualization
            if quantize {
                t = (t * DEPTH_COLORMAP_BANDS).floor() / DEPTH_COLORMAP_BANDS;
            }

            if grayscale {
                // Grayscale: near=bright, far=dark (invert t)
                let gray = ((1.0 - t) * 255.0) as u8;
                rgba.extend_from_slice(&[gray, gray, gray, 255]);
            } else {
                // Colormap: turbo (blue=near, red=far)
                let color = turbo(t);
                rgba.extend_from_slice(&color);
            }
        }
    }
    rgba
}

/// Convert RGB pixel data to RGBA (add alpha channel)
///
/// # Arguments
/// * `rgb` - RGB pixel data (3 bytes per pixel)
///
/// # Returns
/// RGBA pixel data (4 bytes per pixel, alpha = 255)
pub fn rgb_to_rgba(rgb: &[u8]) -> Vec<u8> {
    let pixel_count = rgb.len() / 3;
    let mut rgba = Vec::with_capacity(pixel_count * 4);
    for chunk in rgb.chunks_exact(3) {
        rgba.push(chunk[0]); // R
        rgba.push(chunk[1]); // G
        rgba.push(chunk[2]); // B
        rgba.push(255); // A
    }
    rgba
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_rgb_to_rgba() {
        let rgb = vec![255, 0, 0, 0, 255, 0, 0, 0, 255];
        let rgba = rgb_to_rgba(&rgb);
        assert_eq!(rgba, vec![255, 0, 0, 255, 0, 255, 0, 255, 0, 0, 255, 255]);
    }

    #[test]
    fn test_depth_invalid() {
        let depth = vec![0u16; 4];
        let rgba = depth_mm_to_rgba(&depth, 2, 2, false, false);
        // All invalid pixels should be black
        for chunk in rgba.chunks(4) {
            assert_eq!(chunk, &[0, 0, 0, 255]);
        }
    }

    #[test]
    fn test_depth_grayscale() {
        // Near depth should be bright, far depth should be dark
        let depth = vec![400u16, 4000u16];
        let rgba = depth_mm_to_rgba(&depth, 2, 1, false, true);
        // Near (400mm) should be very bright
        assert!(rgba[0] > 200);
        // Far (4000mm) should be very dark
        assert!(rgba[4] < 50);
    }
}
