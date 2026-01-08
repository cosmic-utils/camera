// SPDX-License-Identifier: GPL-3.0-only
//
// GPU histogram computation with reduction to brightness metrics
//
// This shader computes a luminance histogram and reduces it to key metrics
// without transferring the full histogram to CPU.
//
// Two passes:
// 1. Histogram: Compute 256-bin histogram using atomics
// 2. Reduce: Calculate brightness metrics from histogram bins

// Input texture containing RGBA image
@group(0) @binding(0)
var input_texture: texture_2d<f32>;

// Histogram bins (256 atomic counters)
@group(0) @binding(1)
var<storage, read_write> histogram: array<atomic<u32>, 256>;

// Output metrics (small buffer sent to CPU)
@group(0) @binding(2)
var<storage, read_write> metrics: BrightnessMetrics;

// Parameters
@group(0) @binding(3)
var<uniform> params: Params;

struct Params {
    width: u32,
    height: u32,
    stage: u32,      // 0 = histogram, 1 = reduce
    _padding: u32,
}

struct BrightnessMetrics {
    mean_luminance: f32,       // Average luminance [0,1]
    median_luminance: f32,     // Approximate median [0,1]
    percentile_5: f32,         // 5th percentile (shadow level) [0,1]
    percentile_95: f32,        // 95th percentile (highlight level) [0,1]
    dynamic_range_stops: f32,  // log2(p95/p5) - dynamic range in stops
    shadow_fraction: f32,      // Fraction of pixels in shadows (<0.1)
    highlight_fraction: f32,   // Fraction of pixels in highlights (>0.9)
    total_pixels: u32,         // Total pixel count
}

// BT.601 luminance coefficients
const LUMA_R: f32 = 0.299;
const LUMA_G: f32 = 0.587;
const LUMA_B: f32 = 0.114;

fn rgb_to_luminance(rgb: vec3<f32>) -> f32 {
    return LUMA_R * rgb.r + LUMA_G * rgb.g + LUMA_B * rgb.b;
}

// Pass 1: Build histogram
// Each thread processes one pixel
@compute @workgroup_size(16, 16, 1)
fn histogram_pass(@builtin(global_invocation_id) gid: vec3<u32>) {
    let x = gid.x;
    let y = gid.y;

    if x >= params.width || y >= params.height {
        return;
    }

    let pixel = textureLoad(input_texture, vec2<i32>(i32(x), i32(y)), 0);
    let luminance = rgb_to_luminance(pixel.rgb);

    // Clamp and quantize to 0-255
    let bin = u32(clamp(luminance * 255.0, 0.0, 255.0));

    // Atomically increment histogram bin
    atomicAdd(&histogram[bin], 1u);
}

// Workgroup shared memory for parallel reduction
var<workgroup> partial_sum: array<u32, 256>;
var<workgroup> partial_weighted_sum: array<f32, 256>;

// Pass 2: Reduce histogram to metrics
// Single workgroup with 256 threads (one per bin)
@compute @workgroup_size(256, 1, 1)
fn reduce_pass(@builtin(local_invocation_id) lid: vec3<u32>) {
    let bin_idx = lid.x;
    let total_pixels = params.width * params.height;

    // Load histogram count for this bin
    let count = atomicLoad(&histogram[bin_idx]);
    let luminance = f32(bin_idx) / 255.0;

    // Store in shared memory
    partial_sum[bin_idx] = count;
    partial_weighted_sum[bin_idx] = f32(count) * luminance;

    workgroupBarrier();

    // Only thread 0 computes final metrics
    if bin_idx == 0u {
        var sum_count: u32 = 0u;
        var sum_weighted: f32 = 0.0;
        var cumulative: u32 = 0u;

        // Calculate percentile thresholds
        let p5_threshold = total_pixels / 20u;      // 5%
        let p50_threshold = total_pixels / 2u;       // 50% (median)
        let p95_threshold = (total_pixels * 19u) / 20u;  // 95%

        var p5_bin: u32 = 0u;
        var p50_bin: u32 = 128u;
        var p95_bin: u32 = 255u;
        var p5_found: bool = false;
        var p50_found: bool = false;
        var p95_found: bool = false;

        // Shadow/highlight counters (bins 0-25 and 230-255)
        var shadow_count: u32 = 0u;
        var highlight_count: u32 = 0u;

        // Single pass through histogram
        for (var i: u32 = 0u; i < 256u; i = i + 1u) {
            let bin_count = partial_sum[i];
            sum_count = sum_count + bin_count;
            sum_weighted = sum_weighted + partial_weighted_sum[i];
            cumulative = cumulative + bin_count;

            // Track percentiles
            if !p5_found && cumulative >= p5_threshold {
                p5_bin = i;
                p5_found = true;
            }
            if !p50_found && cumulative >= p50_threshold {
                p50_bin = i;
                p50_found = true;
            }
            if !p95_found && cumulative >= p95_threshold {
                p95_bin = i;
                p95_found = true;
            }

            // Count shadows (luminance < 0.1, bins 0-25)
            if i < 26u {
                shadow_count = shadow_count + bin_count;
            }
            // Count highlights (luminance > 0.9, bins 230-255)
            if i >= 230u {
                highlight_count = highlight_count + bin_count;
            }
        }

        // Calculate final metrics
        let mean_lum = select(0.0, sum_weighted / f32(sum_count), sum_count > 0u);
        let median_lum = f32(p50_bin) / 255.0;
        let p5_lum = f32(p5_bin) / 255.0;
        let p95_lum = f32(p95_bin) / 255.0;

        // Dynamic range in stops (avoid log of zero)
        let p5_safe = max(p5_lum, 0.001);
        let p95_safe = max(p95_lum, 0.001);
        let dynamic_range = log2(p95_safe / p5_safe);

        // Store results
        metrics.mean_luminance = mean_lum;
        metrics.median_luminance = median_lum;
        metrics.percentile_5 = p5_lum;
        metrics.percentile_95 = p95_lum;
        metrics.dynamic_range_stops = dynamic_range;
        metrics.shadow_fraction = f32(shadow_count) / f32(total_pixels);
        metrics.highlight_fraction = f32(highlight_count) / f32(total_pixels);
        metrics.total_pixels = total_pixels;
    }
}
