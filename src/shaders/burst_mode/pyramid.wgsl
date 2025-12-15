// SPDX-License-Identifier: GPL-3.0-only
//
// Gaussian pyramid builder for night mode alignment
//
// Builds a 4-level Gaussian pyramid for hierarchical alignment.
// Each level is 2x downsampled with Gaussian blur to avoid aliasing.
//
// Based on HDR+ alignment pyramid.

struct PyramidParams {
    src_width: u32,      // Source level width
    src_height: u32,     // Source level height
    dst_width: u32,      // Destination level width (src/2)
    dst_height: u32,     // Destination level height (src/2)
}

// Source level (grayscale luminance or RGBA)
@group(0) @binding(0)
var<storage, read> src_level: array<f32>;

// Destination level (2x downsampled)
@group(0) @binding(1)
var<storage, read_write> dst_level: array<f32>;

@group(0) @binding(2)
var<uniform> params: PyramidParams;

//=============================================================================
// Gaussian kernel (5x5, sigma ~= 1.4)
// Used for anti-aliasing before 2x downsampling
//=============================================================================

// Separable Gaussian weights as individual constants
// (WGSL doesn't allow runtime indexing of const arrays)
const G0: f32 = 0.0625;
const G1: f32 = 0.25;
const G2: f32 = 0.375;
const G3: f32 = 0.25;
const G4: f32 = 0.0625;

// Helper function to get Gaussian weight by index
fn gauss_weight(i: i32) -> f32 {
    // Map -2..2 to weights
    switch (i) {
        case -2: { return G0; }
        case -1: { return G1; }
        case 0:  { return G2; }
        case 1:  { return G3; }
        case 2:  { return G4; }
        default: { return 0.0; }
    }
}

//=============================================================================
// Utility functions
//=============================================================================

fn get_src_pixel(x: i32, y: i32) -> f32 {
    let cx = clamp(x, 0, i32(params.src_width) - 1);
    let cy = clamp(y, 0, i32(params.src_height) - 1);
    let idx = u32(cy) * params.src_width + u32(cx);
    return src_level[idx];
}

fn get_src_rgba(x: i32, y: i32) -> vec4<f32> {
    let cx = clamp(x, 0, i32(params.src_width) - 1);
    let cy = clamp(y, 0, i32(params.src_height) - 1);
    let idx = (u32(cy) * params.src_width + u32(cx)) * 4u;
    return vec4<f32>(src_level[idx], src_level[idx + 1u], src_level[idx + 2u], src_level[idx + 3u]);
}

//=============================================================================
// Downsample grayscale with 5x5 Gaussian blur
//=============================================================================

@compute @workgroup_size(16, 16)
fn downsample_gray(@builtin(global_invocation_id) gid: vec3<u32>) {
    let dst_x = gid.x;
    let dst_y = gid.y;

    if (dst_x >= params.dst_width || dst_y >= params.dst_height) {
        return;
    }

    // Corresponding source position (center of 2x2 block)
    let src_x = i32(dst_x * 2u);
    let src_y = i32(dst_y * 2u);

    // Apply 5x5 Gaussian (using helper function for weight lookup)
    var sum = 0.0;

    for (var dy = -2; dy <= 2; dy++) {
        for (var dx = -2; dx <= 2; dx++) {
            let weight = gauss_weight(dx) * gauss_weight(dy);
            sum += get_src_pixel(src_x + dx, src_y + dy) * weight;
        }
    }

    let dst_idx = dst_y * params.dst_width + dst_x;
    dst_level[dst_idx] = sum;
}

//=============================================================================
// Downsample RGBA with 5x5 Gaussian blur
//=============================================================================

@compute @workgroup_size(16, 16)
fn downsample_rgba(@builtin(global_invocation_id) gid: vec3<u32>) {
    let dst_x = gid.x;
    let dst_y = gid.y;

    if (dst_x >= params.dst_width || dst_y >= params.dst_height) {
        return;
    }

    let src_x = i32(dst_x * 2u);
    let src_y = i32(dst_y * 2u);

    var sum = vec4<f32>(0.0);

    for (var dy = -2; dy <= 2; dy++) {
        for (var dx = -2; dx <= 2; dx++) {
            let weight = gauss_weight(dx) * gauss_weight(dy);
            sum += get_src_rgba(src_x + dx, src_y + dy) * weight;
        }
    }

    let dst_idx = (dst_y * params.dst_width + dst_x) * 4u;
    dst_level[dst_idx] = sum.x;
    dst_level[dst_idx + 1u] = sum.y;
    dst_level[dst_idx + 2u] = sum.z;
    dst_level[dst_idx + 3u] = sum.w;
}

//=============================================================================
// Fast 2x downsample (box filter for RGBA - used for preview pyramid)
//=============================================================================

@compute @workgroup_size(16, 16)
fn downsample_fast_rgba(@builtin(global_invocation_id) gid: vec3<u32>) {
    let dst_x = gid.x;
    let dst_y = gid.y;

    if (dst_x >= params.dst_width || dst_y >= params.dst_height) {
        return;
    }

    let src_x = dst_x * 2u;
    let src_y = dst_y * 2u;

    // Sample 2x2 block
    let idx00 = (src_y * params.src_width + src_x) * 4u;
    let idx10 = (src_y * params.src_width + min(src_x + 1u, params.src_width - 1u)) * 4u;
    let idx01 = (min(src_y + 1u, params.src_height - 1u) * params.src_width + src_x) * 4u;
    let idx11 = (min(src_y + 1u, params.src_height - 1u) * params.src_width + min(src_x + 1u, params.src_width - 1u)) * 4u;

    let p00 = vec4<f32>(src_level[idx00], src_level[idx00 + 1u], src_level[idx00 + 2u], src_level[idx00 + 3u]);
    let p10 = vec4<f32>(src_level[idx10], src_level[idx10 + 1u], src_level[idx10 + 2u], src_level[idx10 + 3u]);
    let p01 = vec4<f32>(src_level[idx01], src_level[idx01 + 1u], src_level[idx01 + 2u], src_level[idx01 + 3u]);
    let p11 = vec4<f32>(src_level[idx11], src_level[idx11 + 1u], src_level[idx11 + 2u], src_level[idx11 + 3u]);

    let avg = (p00 + p10 + p01 + p11) * 0.25;

    let dst_idx = (dst_y * params.dst_width + dst_x) * 4u;
    dst_level[dst_idx] = avg.x;
    dst_level[dst_idx + 1u] = avg.y;
    dst_level[dst_idx + 2u] = avg.z;
    dst_level[dst_idx + 3u] = avg.w;
}

//=============================================================================
// Convert RGBA to grayscale luminance
//=============================================================================

struct ConvertParams {
    width: u32,
    height: u32,
    channel: u32,    // 0=R, 1=G, 2=B, 3=luminance (for per-channel alignment)
    _padding1: u32,
}

@group(0) @binding(0)
var<storage, read> rgba_input: array<f32>;

@group(0) @binding(1)
var<storage, read_write> gray_output: array<f32>;

@group(0) @binding(2)
var<uniform> conv_params: ConvertParams;

// COMMON UTILITY: BT.601 RGB to grayscale conversion
// See common.wgsl for reference. Keep in sync across all shaders that use this.
// Formula: Y = 0.299*R + 0.587*G + 0.114*B
@compute @workgroup_size(16, 16)
fn rgba_to_gray(@builtin(global_invocation_id) gid: vec3<u32>) {
    let x = gid.x;
    let y = gid.y;

    if (x >= conv_params.width || y >= conv_params.height) {
        return;
    }

    let rgba_idx = (y * conv_params.width + x) * 4u;
    let r = rgba_input[rgba_idx];
    let g = rgba_input[rgba_idx + 1u];
    let b = rgba_input[rgba_idx + 2u];

    let lum = 0.299 * r + 0.587 * g + 0.114 * b;

    let gray_idx = y * conv_params.width + x;
    gray_output[gray_idx] = lum;
}

//=============================================================================
// Extract single channel from RGBA for per-channel alignment
// channel: 0=R, 1=G, 2=B
//=============================================================================

@compute @workgroup_size(16, 16)
fn rgba_to_channel(@builtin(global_invocation_id) gid: vec3<u32>) {
    let x = gid.x;
    let y = gid.y;

    if (x >= conv_params.width || y >= conv_params.height) {
        return;
    }

    let rgba_idx = (y * conv_params.width + x) * 4u;

    // Extract the specified channel
    var value: f32;
    switch (conv_params.channel) {
        case 0u: { value = rgba_input[rgba_idx]; }         // Red
        case 1u: { value = rgba_input[rgba_idx + 1u]; }    // Green
        case 2u: { value = rgba_input[rgba_idx + 2u]; }    // Blue
        default: {
            // Fallback to luminance for backward compatibility
            let r = rgba_input[rgba_idx];
            let g = rgba_input[rgba_idx + 1u];
            let b = rgba_input[rgba_idx + 2u];
            value = 0.299 * r + 0.587 * g + 0.114 * b;
        }
    }

    let out_idx = y * conv_params.width + x;
    gray_output[out_idx] = value;
}
