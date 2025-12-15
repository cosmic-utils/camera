// SPDX-License-Identifier: GPL-3.0-only
//
// Sharpness computation for night mode reference frame selection
//
// Computes sharpness metric for each frame using Laplacian gradient magnitude.
// The sharpest frame is selected as the reference for alignment.
// This follows the "lucky imaging" approach from the HDR+ paper.
//
// Based on hdr-plus-swift reference selection.

struct SharpnessParams {
    width: u32,
    height: u32,
    tile_size: u32,      // Block size for partial reduction (e.g., 64)
    n_tiles_x: u32,
    n_tiles_y: u32,
    _padding0: u32,
    _padding1: u32,
    _padding2: u32,
}

// Input frame (RGBA f32, normalized 0-1)
@group(0) @binding(0)
var<storage, read> input_frame: array<f32>;

// Partial sums per tile (for parallel reduction)
@group(0) @binding(1)
var<storage, read_write> partial_sums: array<f32>;

// Final sharpness score (single value output)
@group(0) @binding(2)
var<storage, read_write> sharpness_output: array<f32>;

@group(0) @binding(3)
var<uniform> params: SharpnessParams;

//=============================================================================
// Utility functions
//=============================================================================

fn get_pixel_idx(x: u32, y: u32) -> u32 {
    return (y * params.width + x) * 4u;
}

// Get green channel (good luminance approximation)
fn get_luminance(x: i32, y: i32) -> f32 {
    let cx = clamp(x, 0, i32(params.width) - 1);
    let cy = clamp(y, 0, i32(params.height) - 1);
    let idx = get_pixel_idx(u32(cx), u32(cy));
    // Use green channel as luminance approximation
    return input_frame[idx + 1u];
}

// COMMON UTILITY: BT.601 RGB to luminance conversion
// See common.wgsl for reference. Keep in sync across all shaders that use this.
// Formula: Y = 0.299*R + 0.587*G + 0.114*B
fn get_luminance_rgb(x: i32, y: i32) -> f32 {
    let cx = clamp(x, 0, i32(params.width) - 1);
    let cy = clamp(y, 0, i32(params.height) - 1);
    let idx = get_pixel_idx(u32(cx), u32(cy));
    let r = input_frame[idx];
    let g = input_frame[idx + 1u];
    let b = input_frame[idx + 2u];
    return 0.299 * r + 0.587 * g + 0.114 * b;
}

//=============================================================================
// COMMON UTILITY: Laplacian operator for edge/sharpness detection
// Keep in sync with: noise_estimate.wgsl (compute_laplacian)
// Formula: |4*center - left - right - top - bottom|
//=============================================================================

fn compute_laplacian(x: i32, y: i32) -> f32 {
    let center = get_luminance(x, y);
    let left = get_luminance(x - 1, y);
    let right = get_luminance(x + 1, y);
    let top = get_luminance(x, y - 1);
    let bottom = get_luminance(x, y + 1);

    // Laplacian magnitude
    return abs(4.0 * center - left - right - top - bottom);
}

//=============================================================================
// Shared memory for workgroup reduction
//=============================================================================

var<workgroup> shared_sum: array<f32, 256>;
var<workgroup> shared_count: array<f32, 256>;

//=============================================================================
// Stage 1: Compute partial sums per tile
//=============================================================================

@compute @workgroup_size(16, 16)
fn compute_sharpness_tiles(
    @builtin(global_invocation_id) gid: vec3<u32>,
    @builtin(local_invocation_id) lid: vec3<u32>,
    @builtin(workgroup_id) wid: vec3<u32>
) {
    let x = gid.x;
    let y = gid.y;
    let local_idx = lid.y * 16u + lid.x;

    // Skip edge pixels (Laplacian needs neighbors)
    var laplacian = 0.0;
    var valid = 0.0;

    if (x >= 1u && y >= 1u && x < params.width - 1u && y < params.height - 1u) {
        laplacian = compute_laplacian(i32(x), i32(y));
        valid = 1.0;
    }

    // Store in shared memory
    shared_sum[local_idx] = laplacian;
    shared_count[local_idx] = valid;
    workgroupBarrier();

    // Parallel reduction within workgroup
    for (var stride = 128u; stride > 0u; stride >>= 1u) {
        if (local_idx < stride) {
            shared_sum[local_idx] += shared_sum[local_idx + stride];
            shared_count[local_idx] += shared_count[local_idx + stride];
        }
        workgroupBarrier();
    }

    // Thread 0 writes result for this workgroup
    if (local_idx == 0u) {
        let tile_idx = wid.y * params.n_tiles_x + wid.x;
        if (tile_idx < params.n_tiles_x * params.n_tiles_y) {
            // Store sum in partial_sums[2*idx] and count in partial_sums[2*idx+1]
            partial_sums[tile_idx * 2u] = shared_sum[0];
            partial_sums[tile_idx * 2u + 1u] = shared_count[0];
        }
    }
}

//=============================================================================
// Stage 2: Final reduction of partial sums
//=============================================================================

@compute @workgroup_size(256)
fn reduce_sharpness(
    @builtin(local_invocation_id) lid: vec3<u32>
) {
    let local_idx = lid.x;
    let n_tiles = params.n_tiles_x * params.n_tiles_y;

    // Each thread loads multiple elements if needed
    var sum = 0.0;
    var count = 0.0;

    var idx = local_idx;
    while (idx < n_tiles) {
        sum += partial_sums[idx * 2u];
        count += partial_sums[idx * 2u + 1u];
        idx += 256u;
    }

    shared_sum[local_idx] = sum;
    shared_count[local_idx] = count;
    workgroupBarrier();

    // Parallel reduction
    for (var stride = 128u; stride > 0u; stride >>= 1u) {
        if (local_idx < stride) {
            shared_sum[local_idx] += shared_sum[local_idx + stride];
            shared_count[local_idx] += shared_count[local_idx + stride];
        }
        workgroupBarrier();
    }

    // Thread 0 computes final average
    if (local_idx == 0u) {
        let total_sum = shared_sum[0];
        let total_count = shared_count[0];
        let avg_sharpness = select(0.0, total_sum / total_count, total_count > 0.0);
        sharpness_output[0] = avg_sharpness;
    }
}

//=============================================================================
// Alternative: Simple per-pixel sharpness (no reduction, for debugging)
//=============================================================================

@group(0) @binding(0)
var<storage, read> simple_input: array<f32>;

@group(0) @binding(1)
var<storage, read_write> simple_output: array<f32>;

struct SimpleParams {
    width: u32,
    height: u32,
    _padding0: u32,
    _padding1: u32,
}

@group(0) @binding(2)
var<uniform> simple_params: SimpleParams;

fn get_simple_lum(x: i32, y: i32) -> f32 {
    let cx = clamp(x, 0, i32(simple_params.width) - 1);
    let cy = clamp(y, 0, i32(simple_params.height) - 1);
    let idx = (u32(cy) * simple_params.width + u32(cx)) * 4u;
    return simple_input[idx + 1u];  // Green channel
}

@compute @workgroup_size(16, 16)
fn compute_sharpness_map(@builtin(global_invocation_id) gid: vec3<u32>) {
    let x = gid.x;
    let y = gid.y;

    if (x >= simple_params.width || y >= simple_params.height) {
        return;
    }

    var sharpness = 0.0;

    if (x >= 1u && y >= 1u && x < simple_params.width - 1u && y < simple_params.height - 1u) {
        let center = get_simple_lum(i32(x), i32(y));
        let left = get_simple_lum(i32(x) - 1, i32(y));
        let right = get_simple_lum(i32(x) + 1, i32(y));
        let top = get_simple_lum(i32(x), i32(y) - 1);
        let bottom = get_simple_lum(i32(x), i32(y) + 1);

        sharpness = abs(4.0 * center - left - right - top - bottom);
    }

    // Output as grayscale (store in all RGB channels)
    let out_idx = (y * simple_params.width + x) * 4u;
    simple_output[out_idx] = sharpness;
    simple_output[out_idx + 1u] = sharpness;
    simple_output[out_idx + 2u] = sharpness;
    simple_output[out_idx + 3u] = 1.0;
}
