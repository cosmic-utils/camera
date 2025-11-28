// SPDX-License-Identifier: MPL-2.0

//! CPU-side filter implementations for virtual camera output (fallback when GPU unavailable)
//!
//! These filters process NV12 frames and output filtered NV12 data.
//! They run on CPU and are used as fallback when GPU filtering is unavailable.

use crate::app::FilterType;
use crate::backends::camera::types::{BackendResult, CameraFrame, PixelFormat};

/// Apply a filter to a camera frame and return NV12 output
///
/// Processes NV12 input with the selected filter and outputs NV12.
/// For filters that require RGB calculations, we convert per-pixel,
/// apply the filter, and convert back.
pub fn apply_filter_cpu(frame: &CameraFrame, filter: FilterType) -> BackendResult<Vec<u8>> {
    match frame.format {
        PixelFormat::NV12 => apply_filter_nv12(frame, filter),
        PixelFormat::RGBA => {
            // Convert RGBA to NV12, apply filter
            apply_filter_rgba_to_nv12(frame, filter)
        }
    }
}

/// Apply filter to NV12 frame, output NV12
fn apply_filter_nv12(frame: &CameraFrame, filter: FilterType) -> BackendResult<Vec<u8>> {
    let width = frame.width as usize;
    let height = frame.height as usize;

    // NV12 output size: Y plane + UV plane
    let y_size = width * height;
    let uv_size = width * height / 2;
    let mut output = vec![0u8; y_size + uv_size];

    let y_plane = &frame.data[..frame.offset_uv];
    let uv_plane = &frame.data[frame.offset_uv..];
    let stride_y = frame.stride_y as usize;
    let stride_uv = frame.stride_uv as usize;

    // For Standard filter, just copy the data (with stride handling)
    if filter == FilterType::Standard {
        // Copy Y plane
        for y in 0..height {
            let src_start = y * stride_y;
            let dst_start = y * width;
            output[dst_start..dst_start + width]
                .copy_from_slice(&y_plane[src_start..src_start + width]);
        }

        // Copy UV plane
        let uv_height = height / 2;
        for y in 0..uv_height {
            let src_start = y * stride_uv;
            let dst_start = y_size + y * width;
            output[dst_start..dst_start + width]
                .copy_from_slice(&uv_plane[src_start..src_start + width]);
        }

        return Ok(output);
    }

    // For Pencil filter, use Sobel edge detection (needs neighboring pixels)
    if filter == FilterType::Pencil {
        return apply_pencil_filter_nv12(
            y_plane, uv_plane, stride_y, stride_uv, width, height, y_size,
        );
    }

    // For Chromatic Aberration, we need to sample offset pixels
    if filter == FilterType::ChromaticAberration {
        return apply_chromatic_aberration_nv12(
            y_plane, uv_plane, stride_y, stride_uv, width, height, y_size,
        );
    }

    // For other filters, we need to process each pixel
    // Process Y and UV planes
    for y in 0..height {
        for x in 0..width {
            // Get Y value
            let y_val = y_plane[y * stride_y + x] as f32 / 255.0;

            // Get UV values (subsampled 2x2)
            let uv_x = x / 2;
            let uv_y = y / 2;
            let uv_idx = uv_y * stride_uv + uv_x * 2;
            let u_val = uv_plane.get(uv_idx).copied().unwrap_or(128) as f32 / 255.0;
            let v_val = uv_plane.get(uv_idx + 1).copied().unwrap_or(128) as f32 / 255.0;

            // Convert YUV to RGB for filtering
            let (mut r, mut g, mut b) = yuv_to_rgb(y_val, u_val - 0.5, v_val - 0.5);

            // Apply filter in RGB space
            apply_filter_rgb(&mut r, &mut g, &mut b, filter, x, y, width, height);

            // Convert back to YUV
            let (new_y, new_u, new_v) = rgb_to_yuv(r, g, b);

            // Write Y value
            output[y * width + x] = (new_y.clamp(0.0, 1.0) * 255.0) as u8;

            // Write UV values (only for even coordinates - 2x2 subsampling)
            if x % 2 == 0 && y % 2 == 0 {
                let uv_out_idx = y_size + (y / 2) * width + x;
                output[uv_out_idx] = ((new_u + 0.5).clamp(0.0, 1.0) * 255.0) as u8;
                output[uv_out_idx + 1] = ((new_v + 0.5).clamp(0.0, 1.0) * 255.0) as u8;
            }
        }
    }

    Ok(output)
}

/// Apply Pencil filter using Sobel edge detection
fn apply_pencil_filter_nv12(
    y_plane: &[u8],
    _uv_plane: &[u8],
    stride_y: usize,
    _stride_uv: usize,
    width: usize,
    height: usize,
    y_size: usize,
) -> BackendResult<Vec<u8>> {
    let uv_size = width * height / 2;
    let mut output = vec![0u8; y_size + uv_size];

    // Helper to sample Y plane with bounds checking
    let sample_y = |x: isize, y: isize| -> f32 {
        let x = x.clamp(0, width as isize - 1) as usize;
        let y = y.clamp(0, height as isize - 1) as usize;
        y_plane[y * stride_y + x] as f32 / 255.0
    };

    // Pseudo-random noise for paper texture
    let hash = |x: usize, y: usize| -> f32 {
        let p = (x as f32 * 127.1 + y as f32 * 311.7) * 0.01;
        (p.sin() * 43758.5453).fract()
    };

    for py in 0..height {
        for px in 0..width {
            let x = px as isize;
            let y = py as isize;

            // Sobel edge detection on Y plane (luminance)
            let tl = sample_y(x - 1, y - 1);
            let tm = sample_y(x, y - 1);
            let tr = sample_y(x + 1, y - 1);
            let ml = sample_y(x - 1, y);
            let mr = sample_y(x + 1, y);
            let bl = sample_y(x - 1, y + 1);
            let bm = sample_y(x, y + 1);
            let br = sample_y(x + 1, y + 1);

            let gx = -tl - 2.0 * ml - bl + tr + 2.0 * mr + br;
            let gy = -tl - 2.0 * tm - tr + bl + 2.0 * bm + br;
            let edge = (gx * gx + gy * gy).sqrt();

            // Invert edge for pencil lines on white background
            let pencil = 1.0 - edge * 2.0;

            // Add subtle paper texture
            let noise = hash(px, py) * 0.05;
            let paper = 0.95 + noise;
            let final_val = (pencil * paper).clamp(0.0, 1.0);

            // Write Y value (grayscale pencil effect)
            output[py * width + px] = (final_val * 255.0) as u8;

            // Write UV values (neutral for grayscale)
            if px % 2 == 0 && py % 2 == 0 {
                let uv_out_idx = y_size + (py / 2) * width + px;
                output[uv_out_idx] = 128; // Neutral U
                output[uv_out_idx + 1] = 128; // Neutral V
            }
        }
    }

    Ok(output)
}

/// Apply Chromatic Aberration filter by sampling offset pixels for R and B channels
fn apply_chromatic_aberration_nv12(
    y_plane: &[u8],
    uv_plane: &[u8],
    stride_y: usize,
    stride_uv: usize,
    width: usize,
    height: usize,
    y_size: usize,
) -> BackendResult<Vec<u8>> {
    let uv_size = width * height / 2;
    let mut output = vec![0u8; y_size + uv_size];

    // Offset in pixels (matching GPU shader's 0.004 = 0.4% of width)
    let offset = (width as f32 * 0.004).max(1.0) as isize;

    // Helper to sample pixel with bounds checking
    let sample_pixel = |px: isize, py: isize| -> (f32, f32, f32) {
        let x = px.clamp(0, width as isize - 1) as usize;
        let y = py.clamp(0, height as isize - 1) as usize;

        let y_val = y_plane[y * stride_y + x] as f32 / 255.0;
        let uv_x = x / 2;
        let uv_y = y / 2;
        let uv_idx = uv_y * stride_uv + uv_x * 2;
        let u_val = uv_plane.get(uv_idx).copied().unwrap_or(128) as f32 / 255.0 - 0.5;
        let v_val = uv_plane.get(uv_idx + 1).copied().unwrap_or(128) as f32 / 255.0 - 0.5;

        yuv_to_rgb(y_val, u_val, v_val)
    };

    for py in 0..height {
        for px in 0..width {
            let x = px as isize;
            let y = py as isize;

            // Sample center for green
            let (_, g, _) = sample_pixel(x, y);

            // Sample offset right for red
            let (r, _, _) = sample_pixel(x + offset, y);

            // Sample offset left for blue
            let (_, _, b) = sample_pixel(x - offset, y);

            // Convert back to YUV
            let (new_y, new_u, new_v) = rgb_to_yuv(r, g, b);

            // Write Y value
            output[py * width + px] = (new_y.clamp(0.0, 1.0) * 255.0) as u8;

            // Write UV values (2x2 subsampling)
            if px % 2 == 0 && py % 2 == 0 {
                let uv_out_idx = y_size + (py / 2) * width + px;
                output[uv_out_idx] = ((new_u + 0.5).clamp(0.0, 1.0) * 255.0) as u8;
                output[uv_out_idx + 1] = ((new_v + 0.5).clamp(0.0, 1.0) * 255.0) as u8;
            }
        }
    }

    Ok(output)
}

/// Apply filter to RGBA frame, output NV12
fn apply_filter_rgba_to_nv12(frame: &CameraFrame, filter: FilterType) -> BackendResult<Vec<u8>> {
    let width = frame.width as usize;
    let height = frame.height as usize;

    // NV12 output size
    let y_size = width * height;
    let uv_size = width * height / 2;
    let mut output = vec![0u8; y_size + uv_size];

    for y in 0..height {
        for x in 0..width {
            let in_idx = (y * width + x) * 4;
            let mut r = frame.data[in_idx] as f32 / 255.0;
            let mut g = frame.data[in_idx + 1] as f32 / 255.0;
            let mut b = frame.data[in_idx + 2] as f32 / 255.0;

            // Apply filter
            apply_filter_rgb(&mut r, &mut g, &mut b, filter, x, y, width, height);

            // Convert to YUV
            let (y_val, u_val, v_val) = rgb_to_yuv(r, g, b);

            // Write Y
            output[y * width + x] = (y_val.clamp(0.0, 1.0) * 255.0) as u8;

            // Write UV (2x2 subsampling)
            if x % 2 == 0 && y % 2 == 0 {
                let uv_out_idx = y_size + (y / 2) * width + x;
                output[uv_out_idx] = ((u_val + 0.5).clamp(0.0, 1.0) * 255.0) as u8;
                output[uv_out_idx + 1] = ((v_val + 0.5).clamp(0.0, 1.0) * 255.0) as u8;
            }
        }
    }

    Ok(output)
}

/// YUV to RGB conversion (BT.601)
#[inline]
fn yuv_to_rgb(y: f32, u: f32, v: f32) -> (f32, f32, f32) {
    let r = (y + 1.402 * v).clamp(0.0, 1.0);
    let g = (y - 0.344 * u - 0.714 * v).clamp(0.0, 1.0);
    let b = (y + 1.772 * u).clamp(0.0, 1.0);
    (r, g, b)
}

/// RGB to YUV conversion (BT.601)
#[inline]
fn rgb_to_yuv(r: f32, g: f32, b: f32) -> (f32, f32, f32) {
    let y = 0.299 * r + 0.587 * g + 0.114 * b;
    let u = -0.169 * r - 0.331 * g + 0.500 * b;
    let v = 0.500 * r - 0.419 * g - 0.081 * b;
    (y, u, v)
}

/// Apply filter effect to RGB values in-place
#[inline]
fn apply_filter_rgb(
    r: &mut f32,
    g: &mut f32,
    b: &mut f32,
    filter: FilterType,
    x: usize,
    y: usize,
    width: usize,
    height: usize,
) {
    match filter {
        FilterType::Standard => {}

        FilterType::Mono => {
            let gray = 0.299 * *r + 0.587 * *g + 0.114 * *b;
            *r = gray;
            *g = gray;
            *b = gray;
        }

        FilterType::Sepia => {
            let luminance = 0.299 * *r + 0.587 * *g + 0.114 * *b;
            *r = (luminance * 1.2 + 0.1).clamp(0.0, 1.0);
            *g = (luminance * 0.9 + 0.05).clamp(0.0, 1.0);
            *b = (luminance * 0.7).clamp(0.0, 1.0);
        }

        FilterType::Noir => {
            let luminance = 0.299 * *r + 0.587 * *g + 0.114 * *b;
            let contrast = 2.0;
            let adjusted = ((luminance - 0.5) * contrast + 0.5).clamp(0.0, 1.0);
            *r = adjusted;
            *g = adjusted;
            *b = adjusted;
        }

        FilterType::Vivid => {
            let luminance = 0.299 * *r + 0.587 * *g + 0.114 * *b;
            *r = (luminance + (*r - luminance) * 1.4).clamp(0.0, 1.0);
            *g = (luminance + (*g - luminance) * 1.4).clamp(0.0, 1.0);
            *b = (luminance + (*b - luminance) * 1.4).clamp(0.0, 1.0);
            *r = ((*r - 0.5) * 1.15 + 0.5).clamp(0.0, 1.0);
            *g = ((*g - 0.5) * 1.15 + 0.5).clamp(0.0, 1.0);
            *b = ((*b - 0.5) * 1.15 + 0.5).clamp(0.0, 1.0);
        }

        FilterType::Cool => {
            *r = (*r * 0.9).clamp(0.0, 1.0);
            *g = (*g * 0.95).clamp(0.0, 1.0);
            *b = (*b * 1.1).clamp(0.0, 1.0);
        }

        FilterType::Warm => {
            *r = (*r * 1.1).clamp(0.0, 1.0);
            *b = (*b * 0.85).clamp(0.0, 1.0);
        }

        FilterType::Fade => {
            *r = (*r * 0.85 + 0.1).clamp(0.0, 1.0);
            *g = (*g * 0.85 + 0.1).clamp(0.0, 1.0);
            *b = (*b * 0.85 + 0.1).clamp(0.0, 1.0);
            let luminance = 0.299 * *r + 0.587 * *g + 0.114 * *b;
            *r = (luminance + (*r - luminance) * 0.7).clamp(0.0, 1.0);
            *g = (luminance + (*g - luminance) * 0.7).clamp(0.0, 1.0);
            *b = (luminance + (*b - luminance) * 0.7).clamp(0.0, 1.0);
        }

        FilterType::Duotone => {
            let luminance = 0.299 * *r + 0.587 * *g + 0.114 * *b;
            let dark = (0.1, 0.1, 0.4);
            let light = (1.0, 0.9, 0.5);
            *r = dark.0 + luminance * (light.0 - dark.0);
            *g = dark.1 + luminance * (light.1 - dark.1);
            *b = dark.2 + luminance * (light.2 - dark.2);
        }

        FilterType::Vignette => {
            // Use normalized 0-1 coordinates like the GPU shader
            let tex_x = x as f32 / width as f32;
            let tex_y = y as f32 / height as f32;
            // Distance from center (0.5, 0.5)
            let dx = tex_x - 0.5;
            let dy = tex_y - 0.5;
            let dist = (dx * dx + dy * dy).sqrt();
            let vignette = 1.0 - smoothstep(0.3, 0.9, dist);
            *r *= vignette;
            *g *= vignette;
            *b *= vignette;
        }

        FilterType::Negative => {
            *r = 1.0 - *r;
            *g = 1.0 - *g;
            *b = 1.0 - *b;
        }

        FilterType::Posterize => {
            let levels = 4.0;
            *r = (*r * levels).floor() / levels;
            *g = (*g * levels).floor() / levels;
            *b = (*b * levels).floor() / levels;
        }

        FilterType::Solarize => {
            let threshold = 0.5;
            if *r > threshold {
                *r = 1.0 - *r;
            }
            if *g > threshold {
                *g = 1.0 - *g;
            }
            if *b > threshold {
                *b = 1.0 - *b;
            }
        }

        FilterType::ChromaticAberration => {
            // Simplified version - just color shift
            *r = (*r * 1.03).clamp(0.0, 1.0);
            *b = (*b * 0.97).clamp(0.0, 1.0);
        }

        FilterType::Pencil => {
            // Simplified version - high contrast grayscale
            let gray = 0.299 * *r + 0.587 * *g + 0.114 * *b;
            let threshold = 0.4;
            let pencil = if gray > threshold {
                1.0
            } else {
                (gray * 2.0).min(0.8)
            };
            *r = pencil;
            *g = pencil;
            *b = pencil;
        }
    }
}

/// Smoothstep function for vignette
#[inline]
fn smoothstep(edge0: f32, edge1: f32, x: f32) -> f32 {
    let t = ((x - edge0) / (edge1 - edge0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::time::Instant;

    fn create_test_frame() -> CameraFrame {
        let width = 4u32;
        let height = 4u32;
        let y_size = (width * height) as usize;
        let uv_size = (width * height / 2) as usize;

        let mut data = vec![0u8; y_size + uv_size];

        // Fill Y plane with mid-gray
        for i in 0..y_size {
            data[i] = 128;
        }

        // Fill UV plane with neutral values
        for i in 0..uv_size {
            data[y_size + i] = 128;
        }

        CameraFrame {
            width,
            height,
            data: Arc::from(data),
            format: PixelFormat::NV12,
            stride_y: width,
            stride_uv: width,
            offset_uv: y_size,
            captured_at: Instant::now(),
        }
    }

    #[test]
    fn test_standard_filter() {
        let frame = create_test_frame();
        let result = apply_filter_cpu(&frame, FilterType::Standard).unwrap();

        // Should be NV12 output
        let expected_size = 4 * 4 + 4 * 4 / 2; // Y + UV
        assert_eq!(result.len(), expected_size);
    }

    #[test]
    fn test_mono_filter() {
        let frame = create_test_frame();
        let result = apply_filter_cpu(&frame, FilterType::Mono).unwrap();

        // Should produce grayscale output
        assert_eq!(result.len(), 4 * 4 + 4 * 4 / 2);
    }
}
