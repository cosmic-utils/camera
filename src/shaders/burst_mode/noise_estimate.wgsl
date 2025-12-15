// SPDX-License-Identifier: GPL-3.0-only
//
// GPU-accelerated noise estimation for night mode
//
// Estimates noise standard deviation using the Median Absolute Deviation (MAD) method
// on Laplacian-filtered image data. This is more robust than variance-based methods
// for natural images with edges and textures.
//
// The algorithm:
// 1. Compute Laplacian (high-pass filter) to isolate noise from signal
// 2. Build histogram of Laplacian values to find median
// 3. Compute MAD (mean absolute deviation from median)
// 4. Convert to noise σ: noise_sd = MAD * 1.4826 / 4.47
//
// Uses histogram-based median approximation which is efficient on GPU.

struct NoiseParams {
    width: u32,
    height: u32,
    // Histogram parameters
    num_bins: u32,          // Number of histogram bins (e.g., 256)
    bin_scale: f32,         // Scale factor: bin = laplacian * bin_scale
    // Output from previous passes
    median_value: f32,      // Median from histogram (set after pass 1)
    _padding0: u32,
    _padding1: u32,
    _padding2: u32,
}

// Input frame (RGBA u8 packed as u32, or RGBA f32)
@group(0) @binding(0)
var<storage, read> input_frame: array<u32>;

// Histogram bins (atomic counters)
@group(0) @binding(1)
var<storage, read_write> histogram: array<atomic<u32>>;

// Partial sums for MAD computation (sum of |laplacian - median|)
@group(0) @binding(2)
var<storage, read_write> partial_sums: array<f32>;

// Final output: [noise_sd, median, mad, pixel_count]
@group(0) @binding(3)
var<storage, read_write> output: array<f32>;

@group(0) @binding(4)
var<uniform> params: NoiseParams;

//=============================================================================
// Utility functions
//=============================================================================

// Get green channel from packed RGBA u8 (most info, least Bayer noise)
fn get_green(x: u32, y: u32) -> f32 {
    let idx = y * params.width + x;
    let packed = input_frame[idx];
    // RGBA packed as u32: R in low byte, then G, B, A
    let g = (packed >> 8u) & 0xFFu;
    return f32(g);
}

// COMMON UTILITY: Laplacian operator for edge/noise detection
// Keep in sync with: sharpness.wgsl (compute_laplacian)
// Formula: |4*center - left - right - top - bottom|
// Note: This version uses packed u32 data, sharpness.wgsl uses f32 RGBA
fn compute_laplacian(x: i32, y: i32) -> f32 {
    let cx = clamp(x, 1, i32(params.width) - 2);
    let cy = clamp(y, 1, i32(params.height) - 2);

    let center = get_green(u32(cx), u32(cy));
    let left = get_green(u32(cx - 1), u32(cy));
    let right = get_green(u32(cx + 1), u32(cy));
    let top = get_green(u32(cx), u32(cy - 1));
    let bottom = get_green(u32(cx), u32(cy + 1));

    // Laplacian: |4*center - neighbors|
    return abs(4.0 * center - left - right - top - bottom);
}

//=============================================================================
// Shared memory for workgroup reduction
//=============================================================================

var<workgroup> shared_sum: array<f32, 256>;
var<workgroup> shared_count: array<f32, 256>;

//=============================================================================
// Pass 1: Build histogram of Laplacian values
//=============================================================================

@compute @workgroup_size(16, 16)
fn build_histogram(@builtin(global_invocation_id) gid: vec3<u32>) {
    let x = gid.x;
    let y = gid.y;

    // Skip edge pixels
    if (x < 1u || y < 1u || x >= params.width - 1u || y >= params.height - 1u) {
        return;
    }

    let laplacian = compute_laplacian(i32(x), i32(y));

    // Map to histogram bin
    // Laplacian range is roughly 0-1020 (4*255), but most values are small
    // Use bin_scale to map to [0, num_bins-1]
    let bin = min(u32(laplacian * params.bin_scale), params.num_bins - 1u);

    // Atomic increment
    atomicAdd(&histogram[bin], 1u);
}

//=============================================================================
// Pass 2: Find median from histogram (single workgroup)
// Runs after histogram is complete
//=============================================================================

@compute @workgroup_size(1)
fn find_median_from_histogram() {
    let total_pixels = (params.width - 2u) * (params.height - 2u);
    let median_target = total_pixels / 2u;

    // Scan histogram to find median bin
    var cumsum = 0u;
    var median_bin = 0u;

    for (var i = 0u; i < params.num_bins; i++) {
        cumsum += atomicLoad(&histogram[i]);
        if (cumsum >= median_target) {
            median_bin = i;
            break;
        }
    }

    // Convert bin back to Laplacian value (bin center)
    let median_laplacian = (f32(median_bin) + 0.5) / params.bin_scale;

    // Store median for next pass
    output[1] = median_laplacian;
    output[3] = f32(total_pixels);
}

//=============================================================================
// Pass 3: Compute MAD (Mean Absolute Deviation from median)
// Uses parallel reduction
//=============================================================================

@compute @workgroup_size(16, 16)
fn compute_mad_tiles(
    @builtin(global_invocation_id) gid: vec3<u32>,
    @builtin(local_invocation_id) lid: vec3<u32>,
    @builtin(workgroup_id) wid: vec3<u32>
) {
    let x = gid.x;
    let y = gid.y;
    let local_idx = lid.y * 16u + lid.x;

    let median = params.median_value;

    var abs_dev = 0.0;
    var valid = 0.0;

    // Skip edge pixels
    if (x >= 1u && y >= 1u && x < params.width - 1u && y < params.height - 1u) {
        let laplacian = compute_laplacian(i32(x), i32(y));
        abs_dev = abs(laplacian - median);
        valid = 1.0;
    }

    // Store in shared memory
    shared_sum[local_idx] = abs_dev;
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
        let n_tiles_x = (params.width + 15u) / 16u;
        let tile_idx = wid.y * n_tiles_x + wid.x;
        // Store sum and count
        partial_sums[tile_idx * 2u] = shared_sum[0];
        partial_sums[tile_idx * 2u + 1u] = shared_count[0];
    }
}

//=============================================================================
// Pass 4: Final reduction to compute noise SD
//=============================================================================

@compute @workgroup_size(256)
fn finalize_noise_estimate(@builtin(local_invocation_id) lid: vec3<u32>) {
    let local_idx = lid.x;

    let n_tiles_x = (params.width + 15u) / 16u;
    let n_tiles_y = (params.height + 15u) / 16u;
    let n_tiles = n_tiles_x * n_tiles_y;

    // Each thread loads multiple elements
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

    // Thread 0 computes final noise estimate
    if (local_idx == 0u) {
        let total_sum = shared_sum[0];
        let total_count = shared_count[0];

        // MAD = mean absolute deviation
        let mad = select(0.0, total_sum / total_count, total_count > 0.0);

        // Convert MAD to noise standard deviation
        // For Gaussian noise: sigma = MAD * 1.4826
        // Laplacian amplifies noise by sqrt(20) ≈ 4.47 for 3x3 kernel
        // Minimum of 0.5 (in 0-255 scale) to avoid division issues
        let noise_sd = max(mad * 1.4826 / 4.47, 0.5);

        // Output: [noise_sd, median, mad, pixel_count]
        output[0] = noise_sd;
        // output[1] already set by find_median
        output[2] = mad;
        // output[3] already set by find_median
    }
}
