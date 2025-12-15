// SPDX-License-Identifier: GPL-3.0-only
//
// Common Shader Utilities Reference
// =================================
//
// This file documents shared algorithms used across night mode shaders.
// WGSL doesn't support #include, so these utilities are replicated in
// each shader that needs them. When modifying these algorithms, update
// ALL shaders that use them.
//
// IMPORTANT: When changing any of these algorithms, grep for the function
// name across all .wgsl files and update consistently.

//=============================================================================
// BT.601 RGB to Luminance
// Used by: tonemap.wgsl, sharpness.wgsl, pyramid.wgsl, noise_estimate.wgsl, align_tile.wgsl
//=============================================================================
//
// fn rgb_to_luminance(rgb: vec3<f32>) -> f32 {
//     return 0.299 * rgb.r + 0.587 * rgb.g + 0.114 * rgb.b;
// }
//
// This is the standard BT.601 luma coefficient formula used for grayscale
// conversion in video processing. The coefficients account for human
// perception where green is most sensitive, red is moderate, blue is least.

//=============================================================================
// Raised Cosine Window (WOLA)
// Used by: spatial_denoise.wgsl, fft_merge.wgsl
//=============================================================================
//
// fn raised_cosine_window(x: u32, y: u32, tile_size: f32) -> f32 {
//     let angle = 2.0 * PI / tile_size;
//     let wx = 0.5 - 0.5 * cos(angle * (f32(x) + 0.5));
//     let wy = 0.5 - 0.5 * cos(angle * (f32(y) + 0.5));
//     return wx * wy;
// }
//
// With 50% tile overlap (tile_step = tile_size/2), these windows sum to 1.0
// at every pixel position, enabling artifact-free Weighted Overlap-Add synthesis.
// Based on HDR+ paper Section 5.

//=============================================================================
// Laplacian Edge Detection
// Used by: sharpness.wgsl, noise_estimate.wgsl
//=============================================================================
//
// fn compute_laplacian(x: i32, y: i32) -> f32 {
//     let center = get_luminance(x, y);
//     let left = get_luminance(x - 1, y);
//     let right = get_luminance(x + 1, y);
//     let top = get_luminance(x, y - 1);
//     let bottom = get_luminance(x, y + 1);
//     return abs(4.0 * center - left - right - top - bottom);
// }
//
// Standard discrete Laplacian operator for edge detection. Used for both
// sharpness estimation (reference frame selection) and noise estimation
// (MAD of Laplacian gives noise level).

//=============================================================================
// Pixel Access with Edge Clamping
// Used by: warp.wgsl, tonemap.wgsl, spatial_denoise.wgsl, and others
//=============================================================================
//
// fn load_pixel_clamped(x: i32, y: i32) -> vec4<f32> {
//     let cx = clamp(x, 0, i32(params.width) - 1);
//     let cy = clamp(y, 0, i32(params.height) - 1);
//     let idx = (u32(cy) * params.width + u32(cx)) * 4u;
//     return vec4<f32>(buffer[idx], buffer[idx+1], buffer[idx+2], buffer[idx+3]);
// }
//
// Edge clamping (replicate border) prevents artifacts at image boundaries.
// Each shader has its own variant accessing its specific buffers.
