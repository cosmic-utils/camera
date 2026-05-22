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

// Workgroup shared state for parallel reduction.
//
// All 256 threads participate in:
//   1. Tree reduction (log2(N) = 8 steps) for total count, weighted sum,
//      shadow count, and highlight count.
//   2. Hillis-Steele inclusive prefix-scan (8 steps) on per-bin counts.
//   3. Parallel percentile detection via workgroup-scoped `atomicMin`.
//
// The previous implementation barrier-loaded the histogram into shared memory
// then had a single thread walk all 256 bins serially. The parallel path does
// the same work in O(log N) steps across all threads.
var<workgroup> sum_buf: array<u32, 256>;
var<workgroup> weighted_buf: array<f32, 256>;
var<workgroup> shadow_buf: array<u32, 256>;
var<workgroup> highlight_buf: array<u32, 256>;
var<workgroup> cum_buf: array<u32, 256>;

// Workgroup atomics for percentile detection. Initialised to 255 (max bin) so
// the very rare case where no bin satisfies the threshold still yields a
// defined value.
var<workgroup> p5_bin: atomic<u32>;
var<workgroup> p50_bin: atomic<u32>;
var<workgroup> p95_bin: atomic<u32>;

// Pass 2: Reduce histogram to metrics.
// Single workgroup with 256 threads (one per bin).
@compute @workgroup_size(256, 1, 1)
fn reduce_pass(@builtin(local_invocation_id) lid: vec3<u32>) {
    let tid = lid.x;
    let total_pixels = params.width * params.height;

    // Initialise the workgroup-scope atomic targets exactly once.
    if tid == 0u {
        atomicStore(&p5_bin, 255u);
        atomicStore(&p50_bin, 255u);
        atomicStore(&p95_bin, 255u);
    }

    // Load this thread's histogram bin and seed the shared buffers.
    let count = atomicLoad(&histogram[tid]);
    let luminance = f32(tid) / 255.0;

    sum_buf[tid] = count;
    weighted_buf[tid] = f32(count) * luminance;
    cum_buf[tid] = count;
    // Bins 0..25 contribute to shadow_count, bins 230..255 to highlight_count.
    shadow_buf[tid] = select(0u, count, tid < 26u);
    highlight_buf[tid] = select(0u, count, tid >= 230u);

    workgroupBarrier();

    // ----- Tree reduction (8 steps for N=256) -----
    // After this loop, *_buf[0] holds the total over all 256 bins.
    var offset: u32 = 128u;
    loop {
        if tid < offset {
            sum_buf[tid] = sum_buf[tid] + sum_buf[tid + offset];
            weighted_buf[tid] = weighted_buf[tid] + weighted_buf[tid + offset];
            shadow_buf[tid] = shadow_buf[tid] + shadow_buf[tid + offset];
            highlight_buf[tid] = highlight_buf[tid] + highlight_buf[tid + offset];
        }
        workgroupBarrier();
        offset = offset / 2u;
        if offset == 0u {
            break;
        }
    }

    // ----- Hillis-Steele inclusive prefix scan on cum_buf (8 steps) -----
    // After this loop, cum_buf[i] = sum of counts for bins 0..=i.
    var stride: u32 = 1u;
    loop {
        // Read the neighbour into a local before any thread writes, so the
        // pre-write value is captured for all threads atomically.
        let prev = select(0u, cum_buf[tid - stride], tid >= stride);
        workgroupBarrier();
        if tid >= stride {
            cum_buf[tid] = cum_buf[tid] + prev;
        }
        workgroupBarrier();
        stride = stride * 2u;
        if stride >= 256u {
            break;
        }
    }

    // ----- Parallel percentile detection -----
    // Each thread checks whether its bin is the (unique) crossing point for
    // each threshold and records it via atomicMin. Several threads may meet
    // the "≥ threshold" condition but only the smallest-index crossing wins.
    let p5_threshold = total_pixels / 20u;          // 5%
    let p50_threshold = total_pixels / 2u;          // 50% (median)
    let p95_threshold = (total_pixels * 19u) / 20u; // 95%

    let cum_self = cum_buf[tid];
    let cum_prev = select(0u, cum_buf[tid - 1u], tid > 0u);

    if cum_self >= p5_threshold && cum_prev < p5_threshold {
        atomicMin(&p5_bin, tid);
    }
    if cum_self >= p50_threshold && cum_prev < p50_threshold {
        atomicMin(&p50_bin, tid);
    }
    if cum_self >= p95_threshold && cum_prev < p95_threshold {
        atomicMin(&p95_bin, tid);
    }

    workgroupBarrier();

    // ----- Single thread writes the final metrics -----
    if tid == 0u {
        let sum_count = sum_buf[0];
        let sum_weighted = weighted_buf[0];
        let shadow_count = shadow_buf[0];
        let highlight_count = highlight_buf[0];
        let p5 = atomicLoad(&p5_bin);
        let p50 = atomicLoad(&p50_bin);
        let p95 = atomicLoad(&p95_bin);

        let mean_lum = select(0.0, sum_weighted / f32(sum_count), sum_count > 0u);
        let median_lum = f32(p50) / 255.0;
        let p5_lum = f32(p5) / 255.0;
        let p95_lum = f32(p95) / 255.0;

        // Dynamic range in stops (avoid log of zero)
        let p5_safe = max(p5_lum, 0.001);
        let p95_safe = max(p95_lum, 0.001);
        let dynamic_range = log2(p95_safe / p5_safe);

        // Guard against zero-pixel divides if width or height was 0.
        let total_safe = max(total_pixels, 1u);

        metrics.mean_luminance = mean_lum;
        metrics.median_luminance = median_lum;
        metrics.percentile_5 = p5_lum;
        metrics.percentile_95 = p95_lum;
        metrics.dynamic_range_stops = dynamic_range;
        metrics.shadow_fraction = f32(shadow_count) / f32(total_safe);
        metrics.highlight_fraction = f32(highlight_count) / f32(total_safe);
        metrics.total_pixels = total_pixels;
    }
}
