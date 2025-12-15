// SPDX-License-Identifier: GPL-3.0-only
//
// Chromatic Aberration Estimation Shader
//
// Implements HDR+ paper Section 6 Step 10: Chromatic Aberration Correction
// Auto-estimates lateral CA coefficients from edge pixels in the reference frame.
//
// Algorithm:
// 1. Find edge pixels using Sobel gradient magnitude
// 2. At edges aligned with radial direction, measure R-G and B-G sub-pixel offsets
// 3. Bin measurements by normalized radius from image center
// 4. Fit quadratic radial model: scale = 1 + coeff * radius²
//
// The quadratic model assumes lateral CA scales linearly with distance from center,
// which matches the dominant first-order aberration in most camera lenses.

const PI: f32 = 3.14159265359;
// Scale factor for converting floats to integers for atomic operations
// 1,000,000 preserves 6 decimal places (sub-pixel offsets are typically -0.5 to +0.5)
const ATOMIC_SCALE: f32 = 1000000.0;

struct CAEstimateParams {
    width: u32,
    height: u32,
    center_x: f32,           // Image center X (typically width/2)
    center_y: f32,           // Image center Y (typically height/2)
    edge_threshold: f32,     // Gradient threshold for edge detection (0.05-0.15)
    radial_alignment: f32,   // Min dot product with radial direction (0.5-0.8)
    num_radius_bins: u32,    // Number of radial bins (16-32)
    search_radius: u32,      // Sub-pixel search radius in 1/8 pixel steps (4 = ±0.5px)
}

// Input: Reference frame (RGBA)
@group(0) @binding(0)
var<storage, read> reference: array<f32>;

// Output: Per-bin offset accumulators (atomic integers for thread-safe accumulation)
// Structure: [bin][0] = R offset sum, [bin][1] = B offset sum, [bin][2] = count
// Values are scaled by ATOMIC_SCALE to preserve precision as integers
@group(0) @binding(1)
var<storage, read_write> bin_data: array<atomic<i32>>;

@group(0) @binding(2)
var<uniform> params: CAEstimateParams;

// Output: Final CA coefficients (written by fit pass)
// [0] = ca_r_coeff, [1] = ca_b_coeff
@group(0) @binding(3)
var<storage, read_write> ca_coefficients: array<f32>;

//=============================================================================
// Utility functions
//=============================================================================

fn get_pixel_idx(x: u32, y: u32) -> u32 {
    return (y * params.width + x) * 4u;
}

// Sample pixel with bilinear interpolation for sub-pixel accuracy
fn sample_bilinear(fx: f32, fy: f32, channel: u32) -> f32 {
    let x0 = u32(max(floor(fx), 0.0));
    let y0 = u32(max(floor(fy), 0.0));
    let x1 = min(x0 + 1u, params.width - 1u);
    let y1 = min(y0 + 1u, params.height - 1u);

    let dx = fx - f32(x0);
    let dy = fy - f32(y0);

    let idx00 = get_pixel_idx(x0, y0) + channel;
    let idx10 = get_pixel_idx(x1, y0) + channel;
    let idx01 = get_pixel_idx(x0, y1) + channel;
    let idx11 = get_pixel_idx(x1, y1) + channel;

    let v00 = reference[idx00];
    let v10 = reference[idx10];
    let v01 = reference[idx01];
    let v11 = reference[idx11];

    let v0 = mix(v00, v10, dx);
    let v1 = mix(v01, v11, dx);
    return mix(v0, v1, dy);
}

// Get luminance at integer position
fn get_luminance(x: u32, y: u32) -> f32 {
    let idx = get_pixel_idx(x, y);
    let r = reference[idx];
    let g = reference[idx + 1u];
    let b = reference[idx + 2u];
    return 0.299 * r + 0.587 * g + 0.114 * b;
}

// Get luminance with clamping for boundary pixels
fn get_luminance_clamped(x: i32, y: i32) -> f32 {
    let cx = u32(clamp(x, 0, i32(params.width) - 1));
    let cy = u32(clamp(y, 0, i32(params.height) - 1));
    return get_luminance(cx, cy);
}

// Sobel gradient computation
fn compute_gradient(x: u32, y: u32) -> vec3<f32> {
    let ix = i32(x);
    let iy = i32(y);

    // Sample 3x3 neighborhood
    let p00 = get_luminance_clamped(ix - 1, iy - 1);
    let p01 = get_luminance_clamped(ix, iy - 1);
    let p02 = get_luminance_clamped(ix + 1, iy - 1);
    let p10 = get_luminance_clamped(ix - 1, iy);
    let p12 = get_luminance_clamped(ix + 1, iy);
    let p20 = get_luminance_clamped(ix - 1, iy + 1);
    let p21 = get_luminance_clamped(ix, iy + 1);
    let p22 = get_luminance_clamped(ix + 1, iy + 1);

    // Sobel kernels
    let gx = -p00 + p02 - 2.0 * p10 + 2.0 * p12 - p20 + p22;
    let gy = -p00 - 2.0 * p01 - p02 + p20 + 2.0 * p21 + p22;

    let magnitude = sqrt(gx * gx + gy * gy);
    return vec3<f32>(gx, gy, magnitude);
}

//=============================================================================
// Sub-pixel offset estimation using cross-correlation
//=============================================================================

// Find sub-pixel offset of channel relative to green along search direction
// Uses normalized cross-correlation in a small window
fn find_channel_offset(x: u32, y: u32, channel: u32, search_dir: vec2<f32>) -> f32 {
    let fx = f32(x);
    let fy = f32(y);

    // Green reference value at center
    let g_center = sample_bilinear(fx, fy, 1u);

    // Search for best alignment offset
    var best_offset = 0.0;
    var best_correlation = -1.0;

    let step = 1.0 / 8.0;  // 1/8 pixel steps
    let max_search = f32(params.search_radius) * step;

    for (var offset = -max_search; offset <= max_search; offset += step) {
        // Sample channel at offset position along search direction
        let sample_x = fx + offset * search_dir.x;
        let sample_y = fy + offset * search_dir.y;

        // Check bounds
        if (sample_x < 0.0 || sample_x >= f32(params.width) ||
            sample_y < 0.0 || sample_y >= f32(params.height)) {
            continue;
        }

        let c_value = sample_bilinear(sample_x, sample_y, channel);

        // Simple correlation: how close is channel to green?
        // Using negative absolute difference as correlation measure
        let diff = abs(c_value - g_center);
        let correlation = 1.0 - diff;

        if (correlation > best_correlation) {
            best_correlation = correlation;
            best_offset = offset;
        }
    }

    return best_offset;
}

//=============================================================================
// Pass 1: Edge detection and offset binning
//=============================================================================

@compute @workgroup_size(16, 16)
fn estimate_ca_offsets(@builtin(global_invocation_id) gid: vec3<u32>) {
    let x = gid.x;
    let y = gid.y;

    // Skip boundary pixels (need 3x3 for gradient)
    if (x < 2u || y < 2u || x >= params.width - 2u || y >= params.height - 2u) {
        return;
    }

    // Step 1: Compute gradient
    let grad = compute_gradient(x, y);
    let grad_mag = grad.z;

    // Skip non-edge pixels
    if (grad_mag < params.edge_threshold) {
        return;
    }

    // Step 2: Compute edge direction (perpendicular to gradient)
    let grad_dir = normalize(vec2<f32>(grad.x, grad.y));
    let edge_dir = vec2<f32>(-grad_dir.y, grad_dir.x);

    // Step 3: Compute radial direction from image center
    let dx = f32(x) - params.center_x;
    let dy = f32(y) - params.center_y;
    let radius = sqrt(dx * dx + dy * dy);

    // Skip center pixels (radius too small for reliable CA measurement)
    if (radius < 50.0) {
        return;
    }

    let radial_dir = vec2<f32>(dx, dy) / radius;

    // Step 4: Only use edges roughly aligned with radial direction
    // CA is most visible at radially-oriented edges
    let alignment = abs(dot(edge_dir, radial_dir));
    if (alignment < params.radial_alignment) {
        return;
    }

    // Step 5: Measure R-G and B-G offsets along radial direction
    let r_offset = find_channel_offset(x, y, 0u, radial_dir);  // Red vs Green
    let b_offset = find_channel_offset(x, y, 2u, radial_dir);  // Blue vs Green

    // Convert offset to radial scale factor
    // Positive offset means channel needs to shift outward (scale > 1)
    // offset_in_pixels = scale_diff * radius
    // scale_diff = offset / radius
    let r_scale_diff = r_offset / radius;
    let b_scale_diff = b_offset / radius;

    // Step 6: Bin by normalized radius
    let max_radius = sqrt(params.center_x * params.center_x + params.center_y * params.center_y);
    let norm_radius = radius / max_radius;
    let bin_idx = min(u32(norm_radius * f32(params.num_radius_bins)), params.num_radius_bins - 1u);

    // Step 7: Atomic accumulate into bins
    // bin_data layout: [bin * 3 + 0] = r_scale_sum, [bin * 3 + 1] = b_scale_sum, [bin * 3 + 2] = count
    let base_idx = bin_idx * 3u;

    // Scale floats to integers for atomic operations (WGSL doesn't have atomic floats)
    // Scale by ATOMIC_SCALE to preserve precision, convert back in fit pass
    let r_scaled = i32(r_scale_diff * ATOMIC_SCALE);
    let b_scaled = i32(b_scale_diff * ATOMIC_SCALE);

    atomicAdd(&bin_data[base_idx], r_scaled);
    atomicAdd(&bin_data[base_idx + 1u], b_scaled);
    atomicAdd(&bin_data[base_idx + 2u], 1i);  // Count as integer (no scaling needed)
}

//=============================================================================
// Pass 2: Fit quadratic CA model from binned data
//=============================================================================

// Fits the model: offset = coeff * radius²
// Using weighted least squares where weight = count per bin
@compute @workgroup_size(1, 1)
fn fit_ca_model(@builtin(global_invocation_id) gid: vec3<u32>) {
    // Only run on a single thread
    if (gid.x != 0u || gid.y != 0u) {
        return;
    }

    // Compute max radius for normalization
    let max_radius = sqrt(params.center_x * params.center_x + params.center_y * params.center_y);

    // Weighted least squares: sum(w * r^4) * coeff = sum(w * r^2 * offset)
    // For quadratic model: scale = 1 + coeff * (r/r_max)^2
    // scale_diff = coeff * (r/r_max)^2 = coeff * norm_r^2

    var sum_r4 = 0.0;           // sum of weight * norm_r^4
    var sum_r2_offset_r = 0.0;  // sum of weight * norm_r^2 * r_offset
    var sum_r2_offset_b = 0.0;  // sum of weight * norm_r^2 * b_offset
    var total_weight = 0.0;

    for (var bin = 0u; bin < params.num_radius_bins; bin++) {
        let base_idx = bin * 3u;
        // Read atomic integers and convert back to floats
        let r_sum_scaled = atomicLoad(&bin_data[base_idx]);
        let b_sum_scaled = atomicLoad(&bin_data[base_idx + 1u]);
        let count_int = atomicLoad(&bin_data[base_idx + 2u]);

        if (count_int < 10i) {
            continue;  // Skip bins with too few samples
        }

        // Convert from scaled integers back to floats
        let r_sum = f32(r_sum_scaled) / ATOMIC_SCALE;
        let b_sum = f32(b_sum_scaled) / ATOMIC_SCALE;
        let count = f32(count_int);

        // Average offset for this bin
        let r_avg = r_sum / count;
        let b_avg = b_sum / count;

        // Normalized radius for this bin (bin center)
        let norm_r = (f32(bin) + 0.5) / f32(params.num_radius_bins);
        let norm_r2 = norm_r * norm_r;
        let norm_r4 = norm_r2 * norm_r2;

        // Weight by count (more samples = more reliable)
        let weight = count;

        sum_r4 += weight * norm_r4;
        sum_r2_offset_r += weight * norm_r2 * r_avg;
        sum_r2_offset_b += weight * norm_r2 * b_avg;
        total_weight += weight;
    }

    // Solve for coefficients
    var ca_r_coeff = 0.0;
    var ca_b_coeff = 0.0;

    if (sum_r4 > 0.001 && total_weight > 100.0) {
        // coeff = sum(w * r^2 * offset) / sum(w * r^4)
        ca_r_coeff = sum_r2_offset_r / sum_r4;
        ca_b_coeff = sum_r2_offset_b / sum_r4;

        // Clamp to reasonable range (typical CA is 0.001 to 0.01)
        ca_r_coeff = clamp(ca_r_coeff, -0.02, 0.02);
        ca_b_coeff = clamp(ca_b_coeff, -0.02, 0.02);
    }

    // Write output coefficients
    ca_coefficients[0] = ca_r_coeff;
    ca_coefficients[1] = ca_b_coeff;
}

//=============================================================================
// Initialization: Clear bin data
//=============================================================================

@compute @workgroup_size(64, 1)
fn init_bins(@builtin(global_invocation_id) gid: vec3<u32>) {
    let idx = gid.x;
    let total_bins = params.num_radius_bins * 3u;

    if (idx < total_bins) {
        atomicStore(&bin_data[idx], 0i);
    }

    // Also clear output coefficients
    if (idx == 0u) {
        ca_coefficients[0] = 0.0;
        ca_coefficients[1] = 0.0;
    }
}
