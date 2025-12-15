// SPDX-License-Identifier: GPL-3.0-only
//
// Guided Filter for Edge-Preserving Weight Smoothing
//
// Implements the guided filter algorithm from:
// "Guided Image Filtering" (He et al., ECCV 2010)
//
// Used to smooth merge weights while preserving edges, preventing:
// - Weight discontinuities at tile boundaries
// - Abrupt weight changes that cause visible seams
// - Weight noise in smooth regions
//
// The filter uses the reference frame luminance as guide image to ensure
// weight smoothing respects image structure.
//
// Algorithm (3-pass implementation):
// Pass 1: Compute box-filtered means (mean_I, mean_p, mean_Ip, mean_II)
// Pass 2: Compute coefficients a = cov_Ip / (var_I + eps), b = mean_p - a * mean_I
// Pass 3: Compute output = mean_a * I + mean_b
//
// For efficiency, we use separable box filtering (2 passes per mean).

struct GuidedFilterParams {
    width: u32,
    height: u32,
    radius: u32,           // Filter radius (4-8 pixels typical)
    epsilon: f32,          // Regularization (0.01-0.1, higher = more smoothing)
}

// Guide image: reference frame luminance
@group(0) @binding(0)
var<storage, read> guide: array<f32>;

// Input: per-pixel merge weights (or any signal to be filtered)
@group(0) @binding(1)
var<storage, read> input_weights: array<f32>;

// Output: edge-preserving smoothed weights
@group(0) @binding(2)
var<storage, read_write> output_weights: array<f32>;

// Intermediate buffers for multi-pass algorithm
@group(0) @binding(3)
var<storage, read_write> mean_I: array<f32>;

@group(0) @binding(4)
var<storage, read_write> mean_p: array<f32>;

@group(0) @binding(5)
var<storage, read_write> mean_Ip: array<f32>;

@group(0) @binding(6)
var<storage, read_write> mean_II: array<f32>;

@group(0) @binding(7)
var<storage, read_write> coeff_a: array<f32>;

@group(0) @binding(8)
var<storage, read_write> coeff_b: array<f32>;

@group(0) @binding(9)
var<uniform> params: GuidedFilterParams;

//=============================================================================
// Utility functions
//=============================================================================

fn get_idx(x: u32, y: u32) -> u32 {
    return y * params.width + x;
}

fn get_guide_clamped(x: i32, y: i32) -> f32 {
    let cx = u32(clamp(x, 0, i32(params.width) - 1));
    let cy = u32(clamp(y, 0, i32(params.height) - 1));
    return guide[get_idx(cx, cy)];
}

fn get_weights_clamped(x: i32, y: i32) -> f32 {
    let cx = u32(clamp(x, 0, i32(params.width) - 1));
    let cy = u32(clamp(y, 0, i32(params.height) - 1));
    return input_weights[get_idx(cx, cy)];
}

//=============================================================================
// Pass 1: Compute local means using box filter
// For a radius r, the box filter averages over a (2r+1)x(2r+1) window
//=============================================================================

@compute @workgroup_size(16, 16)
fn compute_means(@builtin(global_invocation_id) gid: vec3<u32>) {
    let x = gid.x;
    let y = gid.y;

    if (x >= params.width || y >= params.height) {
        return;
    }

    let ix = i32(x);
    let iy = i32(y);
    let r = i32(params.radius);

    var sum_I = 0.0;
    var sum_p = 0.0;
    var sum_Ip = 0.0;
    var sum_II = 0.0;
    var count = 0.0;

    // Box filter over (2r+1)x(2r+1) window
    for (var dy = -r; dy <= r; dy++) {
        for (var dx = -r; dx <= r; dx++) {
            let nx = ix + dx;
            let ny = iy + dy;

            // Skip out-of-bounds (edge handling: ignore)
            if (nx < 0 || nx >= i32(params.width) || ny < 0 || ny >= i32(params.height)) {
                continue;
            }

            let I_val = get_guide_clamped(nx, ny);
            let p_val = get_weights_clamped(nx, ny);

            sum_I += I_val;
            sum_p += p_val;
            sum_Ip += I_val * p_val;
            sum_II += I_val * I_val;
            count += 1.0;
        }
    }

    let idx = get_idx(x, y);
    let inv_count = 1.0 / max(count, 1.0);

    mean_I[idx] = sum_I * inv_count;
    mean_p[idx] = sum_p * inv_count;
    mean_Ip[idx] = sum_Ip * inv_count;
    mean_II[idx] = sum_II * inv_count;
}

//=============================================================================
// Pass 2: Compute filter coefficients a and b
// var_I = mean_II - mean_I * mean_I
// cov_Ip = mean_Ip - mean_I * mean_p
// a = cov_Ip / (var_I + epsilon)
// b = mean_p - a * mean_I
//=============================================================================

@compute @workgroup_size(16, 16)
fn compute_coefficients(@builtin(global_invocation_id) gid: vec3<u32>) {
    let x = gid.x;
    let y = gid.y;

    if (x >= params.width || y >= params.height) {
        return;
    }

    let idx = get_idx(x, y);

    let m_I = mean_I[idx];
    let m_p = mean_p[idx];
    let m_Ip = mean_Ip[idx];
    let m_II = mean_II[idx];

    // Local variance and covariance
    let var_I = m_II - m_I * m_I;
    let cov_Ip = m_Ip - m_I * m_p;

    // Filter coefficients with regularization
    let a = cov_Ip / (var_I + params.epsilon);
    let b = m_p - a * m_I;

    coeff_a[idx] = a;
    coeff_b[idx] = b;
}

//=============================================================================
// Pass 3: Compute output by averaging coefficients over window
// mean_a = box_filter(a)
// mean_b = box_filter(b)
// output = mean_a * guide + mean_b
//=============================================================================

@compute @workgroup_size(16, 16)
fn compute_output(@builtin(global_invocation_id) gid: vec3<u32>) {
    let x = gid.x;
    let y = gid.y;

    if (x >= params.width || y >= params.height) {
        return;
    }

    let ix = i32(x);
    let iy = i32(y);
    let r = i32(params.radius);

    var sum_a = 0.0;
    var sum_b = 0.0;
    var count = 0.0;

    // Average coefficients over window
    for (var dy = -r; dy <= r; dy++) {
        for (var dx = -r; dx <= r; dx++) {
            let nx = ix + dx;
            let ny = iy + dy;

            // Skip out-of-bounds
            if (nx < 0 || nx >= i32(params.width) || ny < 0 || ny >= i32(params.height)) {
                continue;
            }

            let nidx = get_idx(u32(nx), u32(ny));
            sum_a += coeff_a[nidx];
            sum_b += coeff_b[nidx];
            count += 1.0;
        }
    }

    let inv_count = 1.0 / max(count, 1.0);
    let mean_a = sum_a * inv_count;
    let mean_b = sum_b * inv_count;

    // Final output: linear transform of guide at this pixel
    let idx = get_idx(x, y);
    let I_val = guide[idx];
    let filtered = mean_a * I_val + mean_b;

    // Clamp to valid weight range [0, 1]
    output_weights[idx] = clamp(filtered, 0.0, 1.0);
}

//=============================================================================
// Single-pass guided filter for per-channel weights (RGBA)
// More efficient when filtering 4-channel data
//=============================================================================

// For RGBA weights (vec4)
@group(0) @binding(10)
var<storage, read> input_weights_rgba: array<f32>;

@group(0) @binding(11)
var<storage, read_write> output_weights_rgba: array<f32>;

fn get_rgba_idx(x: u32, y: u32) -> u32 {
    return (y * params.width + x) * 4u;
}

// Simplified single-pass guided filter for vec4 weights
// Less accurate but much faster (single pass)
@compute @workgroup_size(16, 16)
fn guided_filter_rgba_simple(@builtin(global_invocation_id) gid: vec3<u32>) {
    let x = gid.x;
    let y = gid.y;

    if (x >= params.width || y >= params.height) {
        return;
    }

    let ix = i32(x);
    let iy = i32(y);
    let r = i32(params.radius);

    var sum_I = 0.0;
    var sum_p = vec4<f32>(0.0);
    var sum_Ip = vec4<f32>(0.0);
    var sum_II = 0.0;
    var count = 0.0;

    // First pass: compute local statistics
    for (var dy = -r; dy <= r; dy++) {
        for (var dx = -r; dx <= r; dx++) {
            let nx = ix + dx;
            let ny = iy + dy;

            if (nx < 0 || nx >= i32(params.width) || ny < 0 || ny >= i32(params.height)) {
                continue;
            }

            let I_val = get_guide_clamped(nx, ny);
            let rgba_idx = get_rgba_idx(u32(nx), u32(ny));
            let p_val = vec4<f32>(
                input_weights_rgba[rgba_idx],
                input_weights_rgba[rgba_idx + 1u],
                input_weights_rgba[rgba_idx + 2u],
                input_weights_rgba[rgba_idx + 3u]
            );

            sum_I += I_val;
            sum_p += p_val;
            sum_Ip += I_val * p_val;
            sum_II += I_val * I_val;
            count += 1.0;
        }
    }

    let inv_count = 1.0 / max(count, 1.0);
    let m_I = sum_I * inv_count;
    let m_p = sum_p * inv_count;
    let m_Ip = sum_Ip * inv_count;
    let m_II = sum_II * inv_count;

    // Compute coefficients
    let var_I = m_II - m_I * m_I;
    let cov_Ip = m_Ip - m_I * m_p;
    let a = cov_Ip / (var_I + params.epsilon);
    let b = m_p - a * m_I;

    // Apply filter using local guide value
    let idx = get_idx(x, y);
    let I_val = guide[idx];
    let filtered = a * I_val + b;

    // Write output
    let out_idx = get_rgba_idx(x, y);
    output_weights_rgba[out_idx] = clamp(filtered.x, 0.0, 1.0);
    output_weights_rgba[out_idx + 1u] = clamp(filtered.y, 0.0, 1.0);
    output_weights_rgba[out_idx + 2u] = clamp(filtered.z, 0.0, 1.0);
    output_weights_rgba[out_idx + 3u] = clamp(filtered.w, 0.0, 1.0);
}

//=============================================================================
// Convert RGBA to luminance for guide image
//=============================================================================

@group(0) @binding(12)
var<storage, read> rgba_input: array<f32>;

@group(0) @binding(13)
var<storage, read_write> lum_output: array<f32>;

@compute @workgroup_size(16, 16)
fn rgba_to_luminance(@builtin(global_invocation_id) gid: vec3<u32>) {
    let x = gid.x;
    let y = gid.y;

    if (x >= params.width || y >= params.height) {
        return;
    }

    let rgba_idx = get_rgba_idx(x, y);
    let r = rgba_input[rgba_idx];
    let g = rgba_input[rgba_idx + 1u];
    let b = rgba_input[rgba_idx + 2u];

    // BT.601 luminance
    let lum = 0.299 * r + 0.587 * g + 0.114 * b;

    let out_idx = get_idx(x, y);
    lum_output[out_idx] = lum;
}
