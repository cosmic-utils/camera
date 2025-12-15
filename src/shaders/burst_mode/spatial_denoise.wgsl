// SPDX-License-Identifier: GPL-3.0-only
//
// Spatial denoising with frequency-dependent noise shaping
//
// Based on HDR+ paper Section 5:
// "Because our pairwise temporal filter above does not perform any spatial filtering,
// we apply spatial filtering as a separate post-processing step in the 2D DFT domain."
//
// "We apply a 'noise shaping' function σ̃ = f(ω)σ which adjusts the effective noise
// level as a function of ω, increasing its magnitude for higher frequencies."
//
// This allows more aggressive filtering of high-frequency noise while preserving
// low-frequency structure.

const PI: f32 = 3.14159265359;
const TILE_SIZE: u32 = 16u;
const TILE_SIZE_F: f32 = 16.0;

// Radix-4 FFT constants (TILE_SIZE / 4, / 2, * 3 / 4)
const TILE_SIZE_14: u32 = 4u;
const TILE_SIZE_24: u32 = 8u;
const TILE_SIZE_34: u32 = 12u;

struct SpatialDenoiseParams {
    width: u32,
    height: u32,
    noise_sd: f32,           // Base noise standard deviation
    strength: f32,           // Overall denoising strength (0.0 - 1.0)
    n_tiles_x: u32,
    n_tiles_y: u32,
    high_freq_boost: f32,    // How much more to filter high frequencies (1.0 - 4.0)
    tile_offset_x: i32,      // Tile offset for 4-pass processing
    tile_offset_y: i32,      // Tile offset for 4-pass processing
    frame_count: u32,        // Number of frames merged (for noise variance scaling)
}

// Input image (RGBA f32)
@group(0) @binding(0)
var<storage, read> input_image: array<f32>;

// Output image (RGBA f32) - accumulator for 4-pass processing
@group(0) @binding(1)
var<storage, read_write> output_image: array<f32>;

@group(0) @binding(2)
var<uniform> params: SpatialDenoiseParams;

// Weight accumulator for proper normalization after 4-pass overlap
@group(0) @binding(3)
var<storage, read_write> weight_accum: array<f32>;

// Shared memory for tile data (complex RGBA)
var<workgroup> tile_re: array<array<vec4<f32>, 16>, 16>;
var<workgroup> tile_im: array<array<vec4<f32>, 16>, 16>;
var<workgroup> temp_re: array<array<vec4<f32>, 16>, 16>;
var<workgroup> temp_im: array<array<vec4<f32>, 16>, 16>;

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

//=============================================================================
// Frequency-dependent noise shaping
// HDR+ paper: filter high-frequency content more aggressively
//=============================================================================

fn noise_shape_factor(freq_x: u32, freq_y: u32) -> f32 {
    // Normalized frequency (0 to 0.5 for Nyquist)
    let norm_fx = f32(min(freq_x, TILE_SIZE - freq_x)) / TILE_SIZE_F;
    let norm_fy = f32(min(freq_y, TILE_SIZE - freq_y)) / TILE_SIZE_F;

    // Radial frequency
    let freq_radius = sqrt(norm_fx * norm_fx + norm_fy * norm_fy);

    // Noise shaping: increase effective noise estimate for higher frequencies
    // This allows Wiener filter to be more aggressive on high-freq noise
    // Linear ramp from 1.0 at DC to high_freq_boost at Nyquist
    return 1.0 + (params.high_freq_boost - 1.0) * freq_radius * 2.0;
}

//=============================================================================
// COMMON UTILITY: Raised cosine window
// See common.wgsl for reference. Keep in sync with fft_merge.wgsl.
// Formula: HDR+ paper page 8 - modified raised cosine ½ - ½cos(2π(x + ½)/n)
// With 50% overlap (n/2 step), windows sum to 1.0 at every position
//=============================================================================

fn raised_cosine_window(x: u32, y: u32) -> f32 {
    let angle = 2.0 * PI / TILE_SIZE_F;
    let wx = 0.5 - 0.5 * cos(angle * (f32(x) + 0.5));
    let wy = 0.5 - 0.5 * cos(angle * (f32(y) + 0.5));
    return wx * wy;
}

//=============================================================================
// Radix-4 FFT Implementation for 16x16 tiles
// Based on hdr-plus-swift frequency.metal
//
// Complexity: O(N log N) vs O(N²) for naive DFT
// Each thread handles one row or column, computing 4 outputs per iteration
//=============================================================================

//-----------------------------------------------------------------------------
// 1D Radix-4 FFT along a column (forward direction)
// Input is real-only (tile_im is zero), output to temp_re/temp_im
//-----------------------------------------------------------------------------
fn fft_1d_col_forward(col: u32) {
    let angle = -2.0 * PI / TILE_SIZE_F;

    for (var dn = 0u; dn < TILE_SIZE_14; dn++) {
        var Re0 = vec4<f32>(0.0);
        var Im0 = vec4<f32>(0.0);
        var Re1 = vec4<f32>(0.0);
        var Im1 = vec4<f32>(0.0);
        var Re2 = vec4<f32>(0.0);
        var Im2 = vec4<f32>(0.0);
        var Re3 = vec4<f32>(0.0);
        var Im3 = vec4<f32>(0.0);

        // Compute 4 small DFTs with decimated input
        for (var dy = 0u; dy < TILE_SIZE; dy += 4u) {
            let coefRe = cos(angle * f32(dn) * f32(dy));
            let coefIm = sin(angle * f32(dn) * f32(dy));

            // DFT0: index dy (0, 4, 8, 12)
            var dataRe = tile_re[dy][col];
            Re0 += coefRe * dataRe;
            Im0 += coefIm * dataRe;

            // DFT1: index dy+1 (1, 5, 9, 13)
            dataRe = tile_re[dy + 1u][col];
            Re2 += coefRe * dataRe;
            Im2 += coefIm * dataRe;

            // DFT2: index dy+2 (2, 6, 10, 14)
            dataRe = tile_re[dy + 2u][col];
            Re1 += coefRe * dataRe;
            Im1 += coefIm * dataRe;

            // DFT3: index dy+3 (3, 7, 11, 15)
            dataRe = tile_re[dy + 3u][col];
            Re3 += coefRe * dataRe;
            Im3 += coefIm * dataRe;
        }

        // First butterfly stage
        var coefRe = cos(angle * 2.0 * f32(dn));
        var coefIm = sin(angle * 2.0 * f32(dn));
        let Re00 = Re0 + coefRe * Re1 - coefIm * Im1;
        let Im00 = Im0 + coefIm * Re1 + coefRe * Im1;
        let Re22 = Re2 + coefRe * Re3 - coefIm * Im3;
        let Im22 = Im2 + coefIm * Re3 + coefRe * Im3;

        coefRe = cos(angle * 2.0 * f32(dn + TILE_SIZE_14));
        coefIm = sin(angle * 2.0 * f32(dn + TILE_SIZE_14));
        let Re11 = Re0 + coefRe * Re1 - coefIm * Im1;
        let Im11 = Im0 + coefIm * Re1 + coefRe * Im1;
        let Re33 = Re2 + coefRe * Re3 - coefIm * Im3;
        let Im33 = Im2 + coefIm * Re3 + coefRe * Im3;

        // Second butterfly stage - produces final outputs
        let out0Re = Re00 + cos(angle * f32(dn)) * Re22 - sin(angle * f32(dn)) * Im22;
        let out0Im = Im00 + sin(angle * f32(dn)) * Re22 + cos(angle * f32(dn)) * Im22;

        let out1Re = Re11 + cos(angle * f32(dn + TILE_SIZE_14)) * Re33 - sin(angle * f32(dn + TILE_SIZE_14)) * Im33;
        let out1Im = Im11 + sin(angle * f32(dn + TILE_SIZE_14)) * Re33 + cos(angle * f32(dn + TILE_SIZE_14)) * Im33;

        let out2Re = Re00 + cos(angle * f32(dn + TILE_SIZE_24)) * Re22 - sin(angle * f32(dn + TILE_SIZE_24)) * Im22;
        let out2Im = Im00 + sin(angle * f32(dn + TILE_SIZE_24)) * Re22 + cos(angle * f32(dn + TILE_SIZE_24)) * Im22;

        let out3Re = Re11 + cos(angle * f32(dn + TILE_SIZE_34)) * Re33 - sin(angle * f32(dn + TILE_SIZE_34)) * Im33;
        let out3Im = Im11 + sin(angle * f32(dn + TILE_SIZE_34)) * Re33 + cos(angle * f32(dn + TILE_SIZE_34)) * Im33;

        // Store to temp arrays for row FFT pass
        temp_re[dn][col] = out0Re;
        temp_im[dn][col] = out0Im;
        temp_re[dn + TILE_SIZE_14][col] = out1Re;
        temp_im[dn + TILE_SIZE_14][col] = out1Im;
        temp_re[dn + TILE_SIZE_24][col] = out2Re;
        temp_im[dn + TILE_SIZE_24][col] = out2Im;
        temp_re[dn + TILE_SIZE_34][col] = out3Re;
        temp_im[dn + TILE_SIZE_34][col] = out3Im;
    }
}

//-----------------------------------------------------------------------------
// 1D Radix-4 FFT along a row (forward direction)
// Input from temp_re/temp_im (column FFT output), output to tile_re/tile_im
//-----------------------------------------------------------------------------
fn fft_1d_row_forward(row: u32) {
    let angle = -2.0 * PI / TILE_SIZE_F;

    for (var dm = 0u; dm < TILE_SIZE_14; dm++) {
        var Re0 = vec4<f32>(0.0);
        var Im0 = vec4<f32>(0.0);
        var Re1 = vec4<f32>(0.0);
        var Im1 = vec4<f32>(0.0);
        var Re2 = vec4<f32>(0.0);
        var Im2 = vec4<f32>(0.0);
        var Re3 = vec4<f32>(0.0);
        var Im3 = vec4<f32>(0.0);

        for (var dx = 0u; dx < TILE_SIZE; dx += 4u) {
            let coefRe = cos(angle * f32(dm) * f32(dx));
            let coefIm = sin(angle * f32(dm) * f32(dx));

            // DFT0
            var dataRe = temp_re[row][dx];
            var dataIm = temp_im[row][dx];
            Re0 += coefRe * dataRe - coefIm * dataIm;
            Im0 += coefIm * dataRe + coefRe * dataIm;

            // DFT1
            dataRe = temp_re[row][dx + 1u];
            dataIm = temp_im[row][dx + 1u];
            Re2 += coefRe * dataRe - coefIm * dataIm;
            Im2 += coefIm * dataRe + coefRe * dataIm;

            // DFT2
            dataRe = temp_re[row][dx + 2u];
            dataIm = temp_im[row][dx + 2u];
            Re1 += coefRe * dataRe - coefIm * dataIm;
            Im1 += coefIm * dataRe + coefRe * dataIm;

            // DFT3
            dataRe = temp_re[row][dx + 3u];
            dataIm = temp_im[row][dx + 3u];
            Re3 += coefRe * dataRe - coefIm * dataIm;
            Im3 += coefIm * dataRe + coefRe * dataIm;
        }

        // First butterfly stage
        var coefRe = cos(angle * 2.0 * f32(dm));
        var coefIm = sin(angle * 2.0 * f32(dm));
        let Re00 = Re0 + coefRe * Re1 - coefIm * Im1;
        let Im00 = Im0 + coefIm * Re1 + coefRe * Im1;
        let Re22 = Re2 + coefRe * Re3 - coefIm * Im3;
        let Im22 = Im2 + coefIm * Re3 + coefRe * Im3;

        coefRe = cos(angle * 2.0 * f32(dm + TILE_SIZE_14));
        coefIm = sin(angle * 2.0 * f32(dm + TILE_SIZE_14));
        let Re11 = Re0 + coefRe * Re1 - coefIm * Im1;
        let Im11 = Im0 + coefIm * Re1 + coefRe * Im1;
        let Re33 = Re2 + coefRe * Re3 - coefIm * Im3;
        let Im33 = Im2 + coefIm * Re3 + coefRe * Im3;

        // Second butterfly stage
        let out0Re = Re00 + cos(angle * f32(dm)) * Re22 - sin(angle * f32(dm)) * Im22;
        let out0Im = Im00 + sin(angle * f32(dm)) * Re22 + cos(angle * f32(dm)) * Im22;

        let out1Re = Re11 + cos(angle * f32(dm + TILE_SIZE_14)) * Re33 - sin(angle * f32(dm + TILE_SIZE_14)) * Im33;
        let out1Im = Im11 + sin(angle * f32(dm + TILE_SIZE_14)) * Re33 + cos(angle * f32(dm + TILE_SIZE_14)) * Im33;

        let out2Re = Re00 + cos(angle * f32(dm + TILE_SIZE_24)) * Re22 - sin(angle * f32(dm + TILE_SIZE_24)) * Im22;
        let out2Im = Im00 + sin(angle * f32(dm + TILE_SIZE_24)) * Re22 + cos(angle * f32(dm + TILE_SIZE_24)) * Im22;

        let out3Re = Re11 + cos(angle * f32(dm + TILE_SIZE_34)) * Re33 - sin(angle * f32(dm + TILE_SIZE_34)) * Im33;
        let out3Im = Im11 + sin(angle * f32(dm + TILE_SIZE_34)) * Re33 + cos(angle * f32(dm + TILE_SIZE_34)) * Im33;

        // Store to tile arrays (frequency domain)
        tile_re[row][dm] = out0Re;
        tile_im[row][dm] = out0Im;
        tile_re[row][dm + TILE_SIZE_14] = out1Re;
        tile_im[row][dm + TILE_SIZE_14] = out1Im;
        tile_re[row][dm + TILE_SIZE_24] = out2Re;
        tile_im[row][dm + TILE_SIZE_24] = out2Im;
        tile_re[row][dm + TILE_SIZE_34] = out3Re;
        tile_im[row][dm + TILE_SIZE_34] = out3Im;
    }
}

//-----------------------------------------------------------------------------
// 1D Radix-4 FFT along a row (backward/inverse direction)
// Input from tile_re/tile_im (Wiener filtered), output to temp_re/temp_im
//-----------------------------------------------------------------------------
fn fft_1d_row_backward(row: u32) {
    let angle = -2.0 * PI / TILE_SIZE_F;

    for (var dm = 0u; dm < TILE_SIZE_14; dm++) {
        var Re0 = vec4<f32>(0.0);
        var Im0 = vec4<f32>(0.0);
        var Re1 = vec4<f32>(0.0);
        var Im1 = vec4<f32>(0.0);
        var Re2 = vec4<f32>(0.0);
        var Im2 = vec4<f32>(0.0);
        var Re3 = vec4<f32>(0.0);
        var Im3 = vec4<f32>(0.0);

        for (var dx = 0u; dx < TILE_SIZE; dx += 4u) {
            let coefRe = cos(angle * f32(dm) * f32(dx));
            let coefIm = sin(angle * f32(dm) * f32(dx));

            // Inverse: conjugate multiply (swap sign of imaginary coefficient)
            // DFT0
            var dataRe = tile_re[row][dx];
            var dataIm = tile_im[row][dx];
            Re0 += coefRe * dataRe + coefIm * dataIm;
            Im0 += coefIm * dataRe - coefRe * dataIm;

            // DFT1
            dataRe = tile_re[row][dx + 1u];
            dataIm = tile_im[row][dx + 1u];
            Re2 += coefRe * dataRe + coefIm * dataIm;
            Im2 += coefIm * dataRe - coefRe * dataIm;

            // DFT2
            dataRe = tile_re[row][dx + 2u];
            dataIm = tile_im[row][dx + 2u];
            Re1 += coefRe * dataRe + coefIm * dataIm;
            Im1 += coefIm * dataRe - coefRe * dataIm;

            // DFT3
            dataRe = tile_re[row][dx + 3u];
            dataIm = tile_im[row][dx + 3u];
            Re3 += coefRe * dataRe + coefIm * dataIm;
            Im3 += coefIm * dataRe - coefRe * dataIm;
        }

        // First butterfly
        var coefRe = cos(angle * 2.0 * f32(dm));
        var coefIm = sin(angle * 2.0 * f32(dm));
        let Re00 = Re0 + coefRe * Re1 - coefIm * Im1;
        let Im00 = Im0 + coefIm * Re1 + coefRe * Im1;
        let Re22 = Re2 + coefRe * Re3 - coefIm * Im3;
        let Im22 = Im2 + coefIm * Re3 + coefRe * Im3;

        coefRe = cos(angle * 2.0 * f32(dm + TILE_SIZE_14));
        coefIm = sin(angle * 2.0 * f32(dm + TILE_SIZE_14));
        let Re11 = Re0 + coefRe * Re1 - coefIm * Im1;
        let Im11 = Im0 + coefIm * Re1 + coefRe * Im1;
        let Re33 = Re2 + coefRe * Re3 - coefIm * Im3;
        let Im33 = Im2 + coefIm * Re3 + coefRe * Im3;

        // Second butterfly
        let out0Re = Re00 + cos(angle * f32(dm)) * Re22 - sin(angle * f32(dm)) * Im22;
        let out0Im = Im00 + sin(angle * f32(dm)) * Re22 + cos(angle * f32(dm)) * Im22;

        let out1Re = Re11 + cos(angle * f32(dm + TILE_SIZE_14)) * Re33 - sin(angle * f32(dm + TILE_SIZE_14)) * Im33;
        let out1Im = Im11 + sin(angle * f32(dm + TILE_SIZE_14)) * Re33 + cos(angle * f32(dm + TILE_SIZE_14)) * Im33;

        let out2Re = Re00 + cos(angle * f32(dm + TILE_SIZE_24)) * Re22 - sin(angle * f32(dm + TILE_SIZE_24)) * Im22;
        let out2Im = Im00 + sin(angle * f32(dm + TILE_SIZE_24)) * Re22 + cos(angle * f32(dm + TILE_SIZE_24)) * Im22;

        let out3Re = Re11 + cos(angle * f32(dm + TILE_SIZE_34)) * Re33 - sin(angle * f32(dm + TILE_SIZE_34)) * Im33;
        let out3Im = Im11 + sin(angle * f32(dm + TILE_SIZE_34)) * Re33 + cos(angle * f32(dm + TILE_SIZE_34)) * Im33;

        // Store with negated Im for column pass (inverse FFT convention)
        temp_re[row][dm] = out0Re;
        temp_im[row][dm] = -out0Im;
        temp_re[row][dm + TILE_SIZE_14] = out1Re;
        temp_im[row][dm + TILE_SIZE_14] = -out1Im;
        temp_re[row][dm + TILE_SIZE_24] = out2Re;
        temp_im[row][dm + TILE_SIZE_24] = -out2Im;
        temp_re[row][dm + TILE_SIZE_34] = out3Re;
        temp_im[row][dm + TILE_SIZE_34] = -out3Im;
    }
}

//-----------------------------------------------------------------------------
// 1D Radix-4 FFT along a column (backward/inverse direction)
// Input from temp_re/temp_im (row inverse output), output to tile_re (real only)
//-----------------------------------------------------------------------------
fn fft_1d_col_backward(col: u32) {
    let angle = -2.0 * PI / TILE_SIZE_F;

    for (var dn = 0u; dn < TILE_SIZE_14; dn++) {
        var Re0 = vec4<f32>(0.0);
        var Im0 = vec4<f32>(0.0);
        var Re1 = vec4<f32>(0.0);
        var Im1 = vec4<f32>(0.0);
        var Re2 = vec4<f32>(0.0);
        var Im2 = vec4<f32>(0.0);
        var Re3 = vec4<f32>(0.0);
        var Im3 = vec4<f32>(0.0);

        for (var dy = 0u; dy < TILE_SIZE; dy += 4u) {
            let coefRe = cos(angle * f32(dn) * f32(dy));
            let coefIm = sin(angle * f32(dn) * f32(dy));

            // DFT0
            var dataRe = temp_re[dy][col];
            var dataIm = temp_im[dy][col];
            Re0 += coefRe * dataRe + coefIm * dataIm;
            Im0 += coefIm * dataRe - coefRe * dataIm;

            // DFT1
            dataRe = temp_re[dy + 1u][col];
            dataIm = temp_im[dy + 1u][col];
            Re2 += coefRe * dataRe + coefIm * dataIm;
            Im2 += coefIm * dataRe - coefRe * dataIm;

            // DFT2
            dataRe = temp_re[dy + 2u][col];
            dataIm = temp_im[dy + 2u][col];
            Re1 += coefRe * dataRe + coefIm * dataIm;
            Im1 += coefIm * dataRe - coefRe * dataIm;

            // DFT3
            dataRe = temp_re[dy + 3u][col];
            dataIm = temp_im[dy + 3u][col];
            Re3 += coefRe * dataRe + coefIm * dataIm;
            Im3 += coefIm * dataRe - coefRe * dataIm;
        }

        // First butterfly
        var coefRe = cos(angle * 2.0 * f32(dn));
        var coefIm = sin(angle * 2.0 * f32(dn));
        let Re00 = Re0 + coefRe * Re1 - coefIm * Im1;
        let Im00 = Im0 + coefIm * Re1 + coefRe * Im1;
        let Re22 = Re2 + coefRe * Re3 - coefIm * Im3;
        let Im22 = Im2 + coefIm * Re3 + coefRe * Im3;

        coefRe = cos(angle * 2.0 * f32(dn + TILE_SIZE_14));
        coefIm = sin(angle * 2.0 * f32(dn + TILE_SIZE_14));
        let Re11 = Re0 + coefRe * Re1 - coefIm * Im1;
        let Im11 = Im0 + coefIm * Re1 + coefRe * Im1;
        let Re33 = Re2 + coefRe * Re3 - coefIm * Im3;
        let Im33 = Im2 + coefIm * Re3 + coefRe * Im3;

        // Second butterfly - only compute real part (output is real after IFFT)
        let out0Re = Re00 + cos(angle * f32(dn)) * Re22 - sin(angle * f32(dn)) * Im22;
        let out1Re = Re11 + cos(angle * f32(dn + TILE_SIZE_14)) * Re33 - sin(angle * f32(dn + TILE_SIZE_14)) * Im33;
        let out2Re = Re00 + cos(angle * f32(dn + TILE_SIZE_24)) * Re22 - sin(angle * f32(dn + TILE_SIZE_24)) * Im22;
        let out3Re = Re11 + cos(angle * f32(dn + TILE_SIZE_34)) * Re33 - sin(angle * f32(dn + TILE_SIZE_34)) * Im33;

        // Normalize by N² and store final spatial-domain output
        let norm = 1.0 / (TILE_SIZE_F * TILE_SIZE_F);
        tile_re[dn][col] = out0Re * norm;
        tile_re[dn + TILE_SIZE_14][col] = out1Re * norm;
        tile_re[dn + TILE_SIZE_24][col] = out2Re * norm;
        tile_re[dn + TILE_SIZE_34][col] = out3Re * norm;
    }
}

//=============================================================================
// Wiener filter with frequency-dependent noise shaping
//=============================================================================

fn wiener_shrinkage(row: u32, col: u32, base_noise: f32) {
    // Get frequency-dependent noise estimate
    let shape_factor = noise_shape_factor(col, row);

    // HDR+ paper Section 5: "we update our estimate of the noise variance to be σ²/N"
    // assuming N frames were averaged. This prevents over-aggressive spatial denoising.
    let frame_scale = 1.0 / f32(max(params.frame_count, 1u));
    let noise_var = base_noise * base_noise * frame_scale * shape_factor * shape_factor * params.strength;

    // Signal magnitude squared
    let mag_sq = tile_re[row][col] * tile_re[row][col] + tile_im[row][col] * tile_im[row][col];

    // Wiener filter: signal / (signal + noise)
    // For vec4, apply element-wise
    let denom = mag_sq + vec4<f32>(noise_var);
    let weight = mag_sq / denom;

    // Apply shrinkage
    tile_re[row][col] = tile_re[row][col] * weight;
    tile_im[row][col] = tile_im[row][col] * weight;
}

//=============================================================================
// Main spatial denoising kernel with 4-pass overlap support
// Processes one 16x16 tile per workgroup with tile offsets for proper overlap
//=============================================================================

@compute @workgroup_size(16, 16)
fn spatial_denoise(
    @builtin(global_invocation_id) global_id: vec3<u32>,
    @builtin(local_invocation_id) local_id: vec3<u32>,
    @builtin(workgroup_id) workgroup_id: vec3<u32>
) {
    let tile_x = workgroup_id.x;
    let tile_y = workgroup_id.y;
    let lx = local_id.x;
    let ly = local_id.y;
    let linear_id = ly * TILE_SIZE + lx;

    if (tile_x >= params.n_tiles_x || tile_y >= params.n_tiles_y) {
        return;
    }

    // Calculate pixel position with tile offset for 4-pass processing
    let px = i32(tile_x * TILE_SIZE + lx) + params.tile_offset_x;
    let py = i32(tile_y * TILE_SIZE + ly) + params.tile_offset_y;

    // Load tile into shared memory (with edge clamping for negative offsets)
    // NO analysis window - synthesis-only WOLA for proper reconstruction
    let pixel = load_pixel(px, py);
    tile_re[ly][lx] = pixel;
    tile_im[ly][lx] = vec4<f32>(0.0);
    workgroupBarrier();

    // Forward FFT using radix-4 (O(N log N) complexity)
    // Column pass: each thread handles one column (lx)
    fft_1d_col_forward(lx);
    workgroupBarrier();

    // Row pass: each thread handles one row (ly)
    fft_1d_row_forward(ly);
    workgroupBarrier();

    // Apply Wiener shrinkage with noise shaping (all 256 threads)
    wiener_shrinkage(ly, lx, params.noise_sd);
    workgroupBarrier();

    // Backward FFT using radix-4 (inverse transform)
    // Row pass (inverse): each thread handles one row (ly)
    fft_1d_row_backward(ly);
    workgroupBarrier();

    // Column pass (inverse): each thread handles one column (lx)
    fft_1d_col_backward(lx);
    workgroupBarrier();

    // Accumulate output with window weighting for smooth tile blending
    // HDR+ paper page 8: modified raised cosine window ½ - ½cos(2π(x + ½)/n)
    // "when this function is repeated with n/2 samples of overlap,
    // the total contribution from all tiles sum to one at every position"
    // Window applied only on output (WOLA synthesis) - no weight tracking needed
    if (px >= 0 && py >= 0 && u32(px) < params.width && u32(py) < params.height) {
        let window = raised_cosine_window(lx, ly);
        let out_idx = get_pixel_idx(u32(px), u32(py));
        let denoised = tile_re[ly][lx];

        // Apply window and accumulate
        output_image[out_idx] = output_image[out_idx] + denoised.x * window;
        output_image[out_idx + 1u] = output_image[out_idx + 1u] + denoised.y * window;
        output_image[out_idx + 2u] = output_image[out_idx + 2u] + denoised.z * window;
        output_image[out_idx + 3u] = output_image[out_idx + 3u] + pixel.w * window;

        // No weight tracking needed - overlapping windows sum to 1.0
    }
}

//=============================================================================
// Initialize output and weight buffers for 4-pass processing
//=============================================================================

@compute @workgroup_size(16, 16)
fn spatial_denoise_init(@builtin(global_invocation_id) global_id: vec3<u32>) {
    let x = global_id.x;
    let y = global_id.y;

    if (x >= params.width || y >= params.height) {
        return;
    }

    let idx = get_pixel_idx(x, y);
    output_image[idx] = 0.0;
    output_image[idx + 1u] = 0.0;
    output_image[idx + 2u] = 0.0;
    output_image[idx + 3u] = 0.0;

    let weight_idx = y * params.width + x;
    weight_accum[weight_idx] = 0.0;
}

//=============================================================================
// Normalize output after all 4 passes
//=============================================================================

@compute @workgroup_size(16, 16)
fn spatial_denoise_normalize(@builtin(global_invocation_id) global_id: vec3<u32>) {
    let x = global_id.x;
    let y = global_id.y;

    if (x >= params.width || y >= params.height) {
        return;
    }

    let idx = get_pixel_idx(x, y);

    // With WOLA synthesis, overlapping windows sum to 1.0
    // No normalization needed - just clamp to valid range
    output_image[idx] = clamp(output_image[idx], 0.0, 1.0);
    output_image[idx + 1u] = clamp(output_image[idx + 1u], 0.0, 1.0);
    output_image[idx + 2u] = clamp(output_image[idx + 2u], 0.0, 1.0);
    output_image[idx + 3u] = clamp(output_image[idx + 3u], 0.0, 1.0);
}
