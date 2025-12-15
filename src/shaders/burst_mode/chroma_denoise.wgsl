// SPDX-License-Identifier: GPL-3.0-only
//
// Chroma denoising for night mode
//
// Implements bilateral filtering in YCbCr color space to reduce color noise
// (red/green splotches) in dark areas while preserving edges.
//
// Based on HDR+ paper Section 6, Step 5:
// "Chroma denoising to reduce red and green splotches in dark areas of low-light images.
// For this we use an approximate bilateral filter, implemented using a sparse 3x3 tap
// non-linear kernel applied in two passes in YUV."

struct ChromaDenoiseParams {
    width: u32,
    height: u32,
    strength: f32,        // Denoising strength (0.0 - 1.0)
    edge_threshold: f32,  // Edge preservation threshold
}

// Input image (RGBA f32)
@group(0) @binding(0)
var<storage, read> input_image: array<f32>;

// Output image (RGBA f32)
@group(0) @binding(1)
var<storage, read_write> output_image: array<f32>;

// Temporary buffer for horizontal pass
@group(0) @binding(2)
var<storage, read_write> temp_buffer: array<f32>;

@group(0) @binding(3)
var<uniform> params: ChromaDenoiseParams;

//=============================================================================
// Color space conversion
//=============================================================================

// RGB to YCbCr (BT.601)
fn rgb_to_ycbcr(rgb: vec3<f32>) -> vec3<f32> {
    let y  =  0.299 * rgb.r + 0.587 * rgb.g + 0.114 * rgb.b;
    let cb = -0.169 * rgb.r - 0.331 * rgb.g + 0.500 * rgb.b + 0.5;
    let cr =  0.500 * rgb.r - 0.419 * rgb.g - 0.081 * rgb.b + 0.5;
    return vec3<f32>(y, cb, cr);
}

// YCbCr to RGB (BT.601)
fn ycbcr_to_rgb(ycbcr: vec3<f32>) -> vec3<f32> {
    let y = ycbcr.x;
    let cb = ycbcr.y - 0.5;
    let cr = ycbcr.z - 0.5;

    let r = y + 1.402 * cr;
    let g = y - 0.344 * cb - 0.714 * cr;
    let b = y + 1.772 * cb;

    return vec3<f32>(r, g, b);
}

//=============================================================================
// Utility functions
//=============================================================================

fn get_pixel_idx(x: u32, y: u32) -> u32 {
    return (y * params.width + x) * 4u;
}

fn load_pixel(x: i32, y: i32) -> vec4<f32> {
    let cx = clamp(x, 0, i32(params.width) - 1);
    let cy = clamp(y, 0, i32(params.height) - 1);
    let idx = get_pixel_idx(u32(cx), u32(cy));
    return vec4<f32>(
        input_image[idx],
        input_image[idx + 1u],
        input_image[idx + 2u],
        input_image[idx + 3u]
    );
}

fn load_temp(x: i32, y: i32) -> vec4<f32> {
    let cx = clamp(x, 0, i32(params.width) - 1);
    let cy = clamp(y, 0, i32(params.height) - 1);
    let idx = get_pixel_idx(u32(cx), u32(cy));
    return vec4<f32>(
        temp_buffer[idx],
        temp_buffer[idx + 1u],
        temp_buffer[idx + 2u],
        temp_buffer[idx + 3u]
    );
}

//=============================================================================
// Bilateral filter weight calculation
//=============================================================================

// Spatial weight based on distance (Gaussian, sigma=1.0)
fn spatial_weight(dist_sq: f32) -> f32 {
    return exp(-dist_sq * 0.5);
}

// Range weight based on luminance difference (edge-aware)
fn range_weight(lum_diff: f32, threshold: f32) -> f32 {
    let normalized = lum_diff / max(threshold, 0.001);
    return exp(-normalized * normalized * 0.5);
}

//=============================================================================
// Horizontal bilateral filter pass (chroma only)
// Filters Cb and Cr channels while preserving Y
//=============================================================================

@compute @workgroup_size(16, 16)
fn chroma_denoise_horizontal(@builtin(global_invocation_id) gid: vec3<u32>) {
    let x = gid.x;
    let y = gid.y;

    if (x >= params.width || y >= params.height) {
        return;
    }

    // Load center pixel and convert to YCbCr
    let center_rgb = load_pixel(i32(x), i32(y));
    let center_ycbcr = rgb_to_ycbcr(center_rgb.rgb);
    let center_y = center_ycbcr.x;

    // Bilateral filter on chroma channels
    var sum_cb = 0.0;
    var sum_cr = 0.0;
    var weight_sum = 0.0;

    // 5-tap sparse filter: [-2, -1, 0, 1, 2]
    for (var dx = -2; dx <= 2; dx++) {
        let neighbor_rgb = load_pixel(i32(x) + dx, i32(y));
        let neighbor_ycbcr = rgb_to_ycbcr(neighbor_rgb.rgb);

        // Spatial weight
        let dist_sq = f32(dx * dx);
        let w_spatial = spatial_weight(dist_sq);

        // Range weight based on luminance (preserve edges)
        let lum_diff = abs(neighbor_ycbcr.x - center_y);
        let w_range = range_weight(lum_diff, params.edge_threshold);

        // Combined weight
        let weight = w_spatial * w_range;

        sum_cb += neighbor_ycbcr.y * weight;
        sum_cr += neighbor_ycbcr.z * weight;
        weight_sum += weight;
    }

    // Normalize
    let filtered_cb = sum_cb / max(weight_sum, 0.001);
    let filtered_cr = sum_cr / max(weight_sum, 0.001);

    // Blend based on strength (preserve original chroma partially)
    let final_cb = mix(center_ycbcr.y, filtered_cb, params.strength);
    let final_cr = mix(center_ycbcr.z, filtered_cr, params.strength);

    // Store YCbCr in temp buffer (will be converted back after vertical pass)
    let idx = get_pixel_idx(x, y);
    temp_buffer[idx] = center_y;
    temp_buffer[idx + 1u] = final_cb;
    temp_buffer[idx + 2u] = final_cr;
    temp_buffer[idx + 3u] = center_rgb.a;
}

//=============================================================================
// Vertical bilateral filter pass (chroma only)
// Second pass of the separable bilateral filter
//=============================================================================

@compute @workgroup_size(16, 16)
fn chroma_denoise_vertical(@builtin(global_invocation_id) gid: vec3<u32>) {
    let x = gid.x;
    let y = gid.y;

    if (x >= params.width || y >= params.height) {
        return;
    }

    // Load center pixel from temp (YCbCr)
    let center = load_temp(i32(x), i32(y));
    let center_y = center.x;

    // Bilateral filter on chroma channels
    var sum_cb = 0.0;
    var sum_cr = 0.0;
    var weight_sum = 0.0;

    // 5-tap sparse filter: [-2, -1, 0, 1, 2]
    for (var dy = -2; dy <= 2; dy++) {
        let neighbor = load_temp(i32(x), i32(y) + dy);

        // Spatial weight
        let dist_sq = f32(dy * dy);
        let w_spatial = spatial_weight(dist_sq);

        // Range weight based on luminance (preserve edges)
        let lum_diff = abs(neighbor.x - center_y);
        let w_range = range_weight(lum_diff, params.edge_threshold);

        // Combined weight
        let weight = w_spatial * w_range;

        sum_cb += neighbor.y * weight;
        sum_cr += neighbor.z * weight;
        weight_sum += weight;
    }

    // Normalize
    let filtered_cb = sum_cb / max(weight_sum, 0.001);
    let filtered_cr = sum_cr / max(weight_sum, 0.001);

    // Convert back to RGB
    let final_ycbcr = vec3<f32>(center_y, filtered_cb, filtered_cr);
    let final_rgb = ycbcr_to_rgb(final_ycbcr);

    // Output clamped RGB
    let idx = get_pixel_idx(x, y);
    output_image[idx] = clamp(final_rgb.r, 0.0, 1.0);
    output_image[idx + 1u] = clamp(final_rgb.g, 0.0, 1.0);
    output_image[idx + 2u] = clamp(final_rgb.b, 0.0, 1.0);
    output_image[idx + 3u] = center.w;  // Preserve alpha
}

//=============================================================================
// Single-pass chroma denoising (simpler, faster alternative)
// Uses 3x3 bilateral kernel
//=============================================================================

@compute @workgroup_size(16, 16)
fn chroma_denoise_single(@builtin(global_invocation_id) gid: vec3<u32>) {
    let x = gid.x;
    let y = gid.y;

    if (x >= params.width || y >= params.height) {
        return;
    }

    // Load center pixel and convert to YCbCr
    let center_rgb = load_pixel(i32(x), i32(y));
    let center_ycbcr = rgb_to_ycbcr(center_rgb.rgb);
    let center_y = center_ycbcr.x;

    // Bilateral filter on chroma channels (3x3 kernel)
    var sum_cb = 0.0;
    var sum_cr = 0.0;
    var weight_sum = 0.0;

    for (var dy = -1; dy <= 1; dy++) {
        for (var dx = -1; dx <= 1; dx++) {
            let neighbor_rgb = load_pixel(i32(x) + dx, i32(y) + dy);
            let neighbor_ycbcr = rgb_to_ycbcr(neighbor_rgb.rgb);

            // Spatial weight
            let dist_sq = f32(dx * dx + dy * dy);
            let w_spatial = spatial_weight(dist_sq);

            // Range weight based on luminance (preserve edges)
            let lum_diff = abs(neighbor_ycbcr.x - center_y);
            let w_range = range_weight(lum_diff, params.edge_threshold);

            // Combined weight
            let weight = w_spatial * w_range;

            sum_cb += neighbor_ycbcr.y * weight;
            sum_cr += neighbor_ycbcr.z * weight;
            weight_sum += weight;
        }
    }

    // Normalize
    let filtered_cb = sum_cb / max(weight_sum, 0.001);
    let filtered_cr = sum_cr / max(weight_sum, 0.001);

    // Blend based on strength
    let final_cb = mix(center_ycbcr.y, filtered_cb, params.strength);
    let final_cr = mix(center_ycbcr.z, filtered_cr, params.strength);

    // Convert back to RGB
    let final_ycbcr = vec3<f32>(center_y, final_cb, final_cr);
    let final_rgb = ycbcr_to_rgb(final_ycbcr);

    // Output clamped RGB
    let idx = get_pixel_idx(x, y);
    output_image[idx] = clamp(final_rgb.r, 0.0, 1.0);
    output_image[idx + 1u] = clamp(final_rgb.g, 0.0, 1.0);
    output_image[idx + 2u] = clamp(final_rgb.b, 0.0, 1.0);
    output_image[idx + 3u] = center_rgb.a;
}
