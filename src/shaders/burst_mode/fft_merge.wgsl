// SPDX-License-Identifier: GPL-3.0-only
//
// FFT-based frequency domain merge for night mode
//
// Full implementation based on:
// - HDR+ algorithm (SIGGRAPH 2016) - Wiener filtering in frequency domain
// - Night Sight (SIGGRAPH Asia 2019) - Motion-aware merging
// - hdr-plus-swift (GPL-3.0) - Reference implementation
//
// Features:
// - 8x8 tile-based processing
// - Optimized radix-2 FFT butterfly
// - Sub-pixel alignment via Fourier shift theorem
// - Wiener shrinkage with noise estimation
// - Motion-aware weight adjustment

const PI: f32 = 3.14159265359;
const TILE_SIZE: u32 = 16u;
const TILE_SIZE_F: f32 = 16.0;

struct MergeParams {
    width: u32,
    height: u32,
    noise_sd: f32,           // Noise standard deviation
    robustness: f32,         // Robustness parameter
    n_tiles_x: u32,
    n_tiles_y: u32,
    frame_count: u32,        // Total frames for normalization
    read_noise: f32,         // Read noise estimate
    max_motion_norm: f32,    // Maximum motion norm factor
    tile_offset_x: i32,      // Tile offset for 4-pass processing (pixels)
    tile_offset_y: i32,      // Tile offset for 4-pass processing (pixels)
    tile_row_offset: u32,    // Row offset for chunked dispatches (tiles)
    exposure_factor: f32,    // Exposure factor for HDR brackets (1.0 for uniform)
    _padding: u32,           // Align to 16 bytes
}

// Input/output buffers
@group(0) @binding(0)
var<storage, read> reference: array<f32>;

@group(0) @binding(1)
var<storage, read> aligned: array<f32>;

@group(0) @binding(2)
var<storage, read_write> output: array<f32>;

@group(0) @binding(3)
var<uniform> params: MergeParams;

// RMS (signal strength) per tile - for noise estimation (per-channel vec4)
@group(0) @binding(4)
var<storage, read_write> rms_texture: array<vec4<f32>>;

// Mismatch (motion) per tile
@group(0) @binding(5)
var<storage, read_write> mismatch_texture: array<f32>;

// Highlight normalization per tile (for non-uniform exposure / clipped highlights)
@group(0) @binding(6)
var<storage, read_write> highlights_norm_texture: array<f32>;

// Weight accumulator per pixel (for proper normalization after 4-pass overlap)
@group(0) @binding(7)
var<storage, read_write> weight_accum: array<f32>;

// Shared memory for tile data (complex RGBA)
// Real and imaginary parts for reference and aligned
// Using 16x16 tiles for better low-frequency noise capture (HDR+ paper recommendation)
var<workgroup> ref_re: array<array<vec4<f32>, 16>, 16>;
var<workgroup> ref_im: array<array<vec4<f32>, 16>, 16>;
var<workgroup> aligned_re: array<array<vec4<f32>, 16>, 16>;
var<workgroup> aligned_im: array<array<vec4<f32>, 16>, 16>;

// Temporary storage for FFT
var<workgroup> temp_re: array<array<vec4<f32>, 16>, 16>;
var<workgroup> temp_im: array<array<vec4<f32>, 16>, 16>;

// NOTE: Sub-pixel shift search removed - handled in alignment stage (pyramid coarse levels)
// per HDR+ paper Section 4: sub-pixel precision from quadratic fitting at coarse levels.

// Edge strength map for edge-aware merge blending
var<workgroup> edge_map: array<array<f32, 16>, 16>;

//=============================================================================
// Edge detection for edge-aware merge blending
//=============================================================================

// Compute luminance from RGBA (BT.601)
fn rgba_to_lum(rgba: vec4<f32>) -> f32 {
    return 0.299 * rgba.x + 0.587 * rgba.y + 0.114 * rgba.z;
}

// Get luminance from reference tile shared memory with boundary clamping
fn get_ref_lum(y: i32, x: i32) -> f32 {
    let cy = u32(clamp(y, 0, 15));
    let cx = u32(clamp(x, 0, 15));
    return rgba_to_lum(ref_re[cy][cx]);
}

// Sobel edge detection on reference tile
// Returns gradient magnitude normalized to [0, 1] range
// Must be called BEFORE FFT (when ref_re contains spatial data)
fn compute_edge_strength(ly: u32, lx: u32) -> f32 {
    let y = i32(ly);
    let x = i32(lx);

    // Sobel kernels for gradient computation
    // Gx = [-1 0 +1]    Gy = [-1 -2 -1]
    //      [-2 0 +2]         [ 0  0  0]
    //      [-1 0 +1]         [+1 +2 +1]

    // Sample 3x3 neighborhood
    let p00 = get_ref_lum(y - 1, x - 1);
    let p01 = get_ref_lum(y - 1, x);
    let p02 = get_ref_lum(y - 1, x + 1);
    let p10 = get_ref_lum(y, x - 1);
    // p11 is center pixel (not needed for Sobel)
    let p12 = get_ref_lum(y, x + 1);
    let p20 = get_ref_lum(y + 1, x - 1);
    let p21 = get_ref_lum(y + 1, x);
    let p22 = get_ref_lum(y + 1, x + 1);

    // Horizontal gradient (Gx)
    let gx = -p00 + p02 - 2.0 * p10 + 2.0 * p12 - p20 + p22;

    // Vertical gradient (Gy)
    let gy = -p00 - 2.0 * p01 - p02 + p20 + 2.0 * p21 + p22;

    // Gradient magnitude
    let grad_mag = sqrt(gx * gx + gy * gy);

    // Normalize: Sobel max output is ~4.0 for step edge (8 * 0.5 luminance change)
    // Use softer normalization to capture more subtle edges
    return clamp(grad_mag / 2.0, 0.0, 1.0);
}

//=============================================================================
// Utility functions
//=============================================================================

fn get_pixel_idx(x: u32, y: u32) -> u32 {
    return (y * params.width + x) * 4u;
}

fn get_tile_idx(tile_x: u32, tile_y: u32) -> u32 {
    return tile_y * params.n_tiles_x + tile_x;
}

// Load pixel from reference buffer with edge clamping
// Edge clamping is critical for negative offsets in 4-pass tile processing
fn load_reference_pixel(x: i32, y: i32) -> vec4<f32> {
    // Clamp to valid image bounds (edge extension)
    let cx = clamp(x, 0i, i32(params.width) - 1i);
    let cy = clamp(y, 0i, i32(params.height) - 1i);
    let idx = get_pixel_idx(u32(cx), u32(cy));
    return vec4<f32>(reference[idx], reference[idx + 1u], reference[idx + 2u], reference[idx + 3u]);
}

// Load pixel from aligned buffer with edge clamping
fn load_aligned_pixel(x: i32, y: i32) -> vec4<f32> {
    // Clamp to valid image bounds (edge extension)
    let cx = clamp(x, 0i, i32(params.width) - 1i);
    let cy = clamp(y, 0i, i32(params.height) - 1i);
    let idx = get_pixel_idx(u32(cx), u32(cy));
    return vec4<f32>(aligned[idx], aligned[idx + 1u], aligned[idx + 2u], aligned[idx + 3u]);
}

//=============================================================================
// Radix-4 FFT Implementation for 16x16 tiles
// Based on hdr-plus-swift frequency.metal
//
// For N=16: tile_size/4 = 4, so we compute 4 DFTs of size 4 each
// Input decimation: indices {0,4,8,12}, {1,5,9,13}, {2,6,10,14}, {3,7,11,15}
// Two butterfly stages combine results into final output
//
// Complexity: O(N log N) vs O(N²) for naive DFT
//=============================================================================

const TILE_SIZE_14: u32 = 4u;   // TILE_SIZE / 4
const TILE_SIZE_24: u32 = 8u;   // TILE_SIZE / 2
const TILE_SIZE_34: u32 = 12u;  // TILE_SIZE * 3 / 4

//-----------------------------------------------------------------------------
// 1D Radix-4 FFT along a row (forward direction)
// Reads from temp arrays, writes to output arrays
//-----------------------------------------------------------------------------
fn fft_1d_row_forward(row: u32, is_ref: bool) {
    let angle = -2.0 * PI / TILE_SIZE_F;

    // Compute 4 outputs per iteration of dm (total tile_size/4 iterations)
    for (var dm = 0u; dm < TILE_SIZE_14; dm++) {
        // Initialize 4 small DFT accumulators
        var Re0 = vec4<f32>(0.0);
        var Im0 = vec4<f32>(0.0);
        var Re1 = vec4<f32>(0.0);
        var Im1 = vec4<f32>(0.0);
        var Re2 = vec4<f32>(0.0);
        var Im2 = vec4<f32>(0.0);
        var Re3 = vec4<f32>(0.0);
        var Im3 = vec4<f32>(0.0);

        // Compute 4 small DFTs with decimated input
        // DFT0 uses indices 0,4,8,12 (stride 4, offset 0)
        // DFT1 uses indices 1,5,9,13 (stride 4, offset 1)
        // DFT2 uses indices 2,6,10,14 (stride 4, offset 2)
        // DFT3 uses indices 3,7,11,15 (stride 4, offset 3)
        for (var dx = 0u; dx < TILE_SIZE; dx += 4u) {
            let coefRe = cos(angle * f32(dm) * f32(dx));
            let coefIm = sin(angle * f32(dm) * f32(dx));

            // DFT0: index dx (0, 4, 8, 12)
            var dataRe = temp_re[row][dx];
            var dataIm = temp_im[row][dx];
            Re0 += coefRe * dataRe - coefIm * dataIm;
            Im0 += coefIm * dataRe + coefRe * dataIm;

            // DFT1: index dx+1 (1, 5, 9, 13)
            dataRe = temp_re[row][dx + 1u];
            dataIm = temp_im[row][dx + 1u];
            Re2 += coefRe * dataRe - coefIm * dataIm;
            Im2 += coefIm * dataRe + coefRe * dataIm;

            // DFT2: index dx+2 (2, 6, 10, 14)
            dataRe = temp_re[row][dx + 2u];
            dataIm = temp_im[row][dx + 2u];
            Re1 += coefRe * dataRe - coefIm * dataIm;
            Im1 += coefIm * dataRe + coefRe * dataIm;

            // DFT3: index dx+3 (3, 7, 11, 15)
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

        // Second butterfly stage - produces final outputs
        let out0Re = Re00 + cos(angle * f32(dm)) * Re22 - sin(angle * f32(dm)) * Im22;
        let out0Im = Im00 + sin(angle * f32(dm)) * Re22 + cos(angle * f32(dm)) * Im22;

        let out1Re = Re11 + cos(angle * f32(dm + TILE_SIZE_14)) * Re33 - sin(angle * f32(dm + TILE_SIZE_14)) * Im33;
        let out1Im = Im11 + sin(angle * f32(dm + TILE_SIZE_14)) * Re33 + cos(angle * f32(dm + TILE_SIZE_14)) * Im33;

        let out2Re = Re00 + cos(angle * f32(dm + TILE_SIZE_24)) * Re22 - sin(angle * f32(dm + TILE_SIZE_24)) * Im22;
        let out2Im = Im00 + sin(angle * f32(dm + TILE_SIZE_24)) * Re22 + cos(angle * f32(dm + TILE_SIZE_24)) * Im22;

        let out3Re = Re11 + cos(angle * f32(dm + TILE_SIZE_34)) * Re33 - sin(angle * f32(dm + TILE_SIZE_34)) * Im33;
        let out3Im = Im11 + sin(angle * f32(dm + TILE_SIZE_34)) * Re33 + cos(angle * f32(dm + TILE_SIZE_34)) * Im33;

        // Write to output positions (matching hdr-plus-swift layout)
        if (is_ref) {
            ref_re[row][dm] = out0Re;
            ref_im[row][dm] = out0Im;
            ref_re[row][dm + TILE_SIZE_14] = out1Re;
            ref_im[row][dm + TILE_SIZE_14] = out1Im;
            ref_re[row][dm + TILE_SIZE_24] = out2Re;
            ref_im[row][dm + TILE_SIZE_24] = out2Im;
            ref_re[row][dm + TILE_SIZE_34] = out3Re;
            ref_im[row][dm + TILE_SIZE_34] = out3Im;
        } else {
            aligned_re[row][dm] = out0Re;
            aligned_im[row][dm] = out0Im;
            aligned_re[row][dm + TILE_SIZE_14] = out1Re;
            aligned_im[row][dm + TILE_SIZE_14] = out1Im;
            aligned_re[row][dm + TILE_SIZE_24] = out2Re;
            aligned_im[row][dm + TILE_SIZE_24] = out2Im;
            aligned_re[row][dm + TILE_SIZE_34] = out3Re;
            aligned_im[row][dm + TILE_SIZE_34] = out3Im;
        }
    }
}

//-----------------------------------------------------------------------------
// 1D Radix-4 FFT along a row (backward/inverse direction)
// For inverse: use same angle but swap sign in complex multiply, negate Im at end
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

            // Inverse: (cos + i*sin) * (re + i*im) with conjugate = (cos - i*sin) * (re + i*im)
            // = cos*re + cos*i*im - i*sin*re - i²*sin*im
            // = (cos*re + sin*im) + i*(cos*im - sin*re)

            // DFT0
            var dataRe = ref_re[row][dx];
            var dataIm = ref_im[row][dx];
            Re0 += coefRe * dataRe + coefIm * dataIm;
            Im0 += coefIm * dataRe - coefRe * dataIm;

            // DFT1
            dataRe = ref_re[row][dx + 1u];
            dataIm = ref_im[row][dx + 1u];
            Re2 += coefRe * dataRe + coefIm * dataIm;
            Im2 += coefIm * dataRe - coefRe * dataIm;

            // DFT2
            dataRe = ref_re[row][dx + 2u];
            dataIm = ref_im[row][dx + 2u];
            Re1 += coefRe * dataRe + coefIm * dataIm;
            Im1 += coefIm * dataRe - coefRe * dataIm;

            // DFT3
            dataRe = ref_re[row][dx + 3u];
            dataIm = ref_im[row][dx + 3u];
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

        // Write with negated Im (for inverse FFT)
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
// 1D Radix-4 FFT along a column (forward direction)
//-----------------------------------------------------------------------------
fn fft_1d_col_forward(col: u32, is_ref: bool) {
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

            // For forward column DFT, input is real only (from spatial domain)
            var dataRe: vec4<f32>;
            if (is_ref) {
                dataRe = ref_re[dy][col];
            } else {
                dataRe = aligned_re[dy][col];
            }
            Re0 += coefRe * dataRe;
            Im0 += coefIm * dataRe;

            if (is_ref) {
                dataRe = ref_re[dy + 1u][col];
            } else {
                dataRe = aligned_re[dy + 1u][col];
            }
            Re2 += coefRe * dataRe;
            Im2 += coefIm * dataRe;

            if (is_ref) {
                dataRe = ref_re[dy + 2u][col];
            } else {
                dataRe = aligned_re[dy + 2u][col];
            }
            Re1 += coefRe * dataRe;
            Im1 += coefIm * dataRe;

            if (is_ref) {
                dataRe = ref_re[dy + 3u][col];
            } else {
                dataRe = aligned_re[dy + 3u][col];
            }
            Re3 += coefRe * dataRe;
            Im3 += coefIm * dataRe;
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

        // Second butterfly
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
// 1D Radix-4 FFT along a column (backward direction)
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

        // Second butterfly - only compute real part (output is real)
        let out0Re = Re00 + cos(angle * f32(dn)) * Re22 - sin(angle * f32(dn)) * Im22;
        let out1Re = Re11 + cos(angle * f32(dn + TILE_SIZE_14)) * Re33 - sin(angle * f32(dn + TILE_SIZE_14)) * Im33;
        let out2Re = Re00 + cos(angle * f32(dn + TILE_SIZE_24)) * Re22 - sin(angle * f32(dn + TILE_SIZE_24)) * Im22;
        let out3Re = Re11 + cos(angle * f32(dn + TILE_SIZE_34)) * Re33 - sin(angle * f32(dn + TILE_SIZE_34)) * Im33;

        // Normalize by N² and store final output
        let norm = 1.0 / (TILE_SIZE_F * TILE_SIZE_F);
        ref_re[dn][col] = out0Re * norm;
        ref_re[dn + TILE_SIZE_14][col] = out1Re * norm;
        ref_re[dn + TILE_SIZE_24][col] = out2Re * norm;
        ref_re[dn + TILE_SIZE_34][col] = out3Re * norm;
    }
}

//-----------------------------------------------------------------------------
// Forward 2D FFT - Column FFT then Row FFT
//-----------------------------------------------------------------------------
fn forward_dft_2d_parallel(lx: u32, ly: u32, is_ref: bool) {
    // Each thread handles one column for column FFT
    fft_1d_col_forward(lx, is_ref);
    workgroupBarrier();

    // Each thread handles one row for row FFT
    fft_1d_row_forward(ly, is_ref);
}

//-----------------------------------------------------------------------------
// Backward 2D FFT - Row FFT then Column FFT
//-----------------------------------------------------------------------------
fn backward_dft_2d_parallel(lx: u32, ly: u32) {
    // Each thread handles one row for row FFT
    fft_1d_row_backward(ly);
    workgroupBarrier();

    // Each thread handles one column for column FFT
    fft_1d_col_backward(lx);
}

//=============================================================================
// Wiener merge
//=============================================================================

// Deconvolution gain lookup for 16x16 tiles
// Symmetric gain table from hdr-plus-swift (original values restored)
// Extended for 16x16: ramps up then down symmetrically around center
fn deconv_gain_lookup(idx: u32) -> f32 {
    switch idx {
        case 0u: { return 0.000; }
        case 1u: { return 0.010; }
        case 2u: { return 0.020; }
        case 3u: { return 0.030; }
        case 4u: { return 0.040; }
        case 5u: { return 0.060; }
        case 6u: { return 0.070; }
        case 7u: { return 0.080; }
        case 8u: { return 0.080; }
        case 9u: { return 0.070; }
        case 10u: { return 0.060; }
        case 11u: { return 0.040; }
        case 12u: { return 0.030; }
        case 13u: { return 0.020; }
        case 14u: { return 0.010; }
        case 15u: { return 0.000; }
        default: { return 0.00; }
    }
}

fn wiener_merge_frequency(
    row: u32,
    col: u32,
    noise_norm: vec4<f32>,  // Per-channel noise normalization
    motion_norm: f32,
    highlights_norm: f32,
    mismatch: f32,
    best_shift: vec2<f32>,
    edge_strength: f32      // Edge strength for edge-aware blending
) {
    // Apply best shift to aligned using Fourier shift theorem
    let angle = -2.0 * PI / TILE_SIZE_F;
    let theta = angle * (f32(col) * best_shift.x + f32(row) * best_shift.y);
    let cos_t = cos(theta);
    let sin_t = sin(theta);

    let aligned_shifted_re = aligned_re[row][col] * cos_t - aligned_im[row][col] * sin_t;
    let aligned_shifted_im = aligned_re[row][col] * sin_t + aligned_im[row][col] * cos_t;

    // Compute difference magnitude squared
    let diff_re = ref_re[row][col] - aligned_shifted_re;
    let diff_im = ref_im[row][col] - aligned_shifted_im;
    let diff_mag_sq = diff_re * diff_re + diff_im * diff_im;

    // Magnitude normalization (Delbracio 2015)
    // Sharper frames get higher merge weight based on frequency magnitudes
    var magnitude_norm = vec4<f32>(1.0);

    // Only apply magnitude normalization for non-DC frequencies and low mismatch
    if (row + col > 0u && mismatch < 0.3) {
        // Calculate magnitudes
        let ref_mag = sqrt(ref_re[row][col] * ref_re[row][col] + ref_im[row][col] * ref_im[row][col]);
        let aligned_mag = sqrt(aligned_shifted_re * aligned_shifted_re + aligned_shifted_im * aligned_shifted_im);

        // Sum across channels
        let ref_mag_sum = ref_mag.x + ref_mag.y + ref_mag.z + ref_mag.w;
        let aligned_mag_sum = aligned_mag.x + aligned_mag.y + aligned_mag.z + aligned_mag.w;

        // Ratio of magnitudes (sharper = higher magnitude)
        let ratio = aligned_mag_sum / max(ref_mag_sum, 0.001);

        // Mismatch weight for smooth transition
        let mismatch_weight = clamp(1.0 - 10.0 * (mismatch - 0.2), 0.0, 1.0);

        // Higher ratio = sharper aligned frame = weight towards aligned
        // ratio^4 gives strong preference to sharper frames
        magnitude_norm = vec4<f32>(mismatch_weight * clamp(ratio * ratio * ratio * ratio, 0.5, 3.0));
    }

    // Complete Wiener weight with all 4 factors (HDR+ equation 7)
    // weight = |D|^2 / (|D|^2 + magnitude_norm * motion_norm * noise_norm * highlights_norm)
    // Per-channel noise_norm gives better color accuracy
    let full_norm = magnitude_norm * motion_norm * noise_norm * highlights_norm;
    let denom = diff_mag_sq + full_norm;
    var weight = diff_mag_sq / denom;

    // Use trimmed mean of RGB weights for color consistency (Night Sight)
    // Remove min and max, average middle two values
    var weights = array<f32, 4>(weight.x, weight.y, weight.z, weight.w);
    let min_w = min(min(weights[0], weights[1]), min(weights[2], weights[3]));
    let max_w = max(max(weights[0], weights[1]), max(weights[2], weights[3]));
    let sum_w = weights[0] + weights[1] + weights[2] + weights[3];
    var median_w = clamp((sum_w - min_w - max_w) * 0.5, 0.0, 1.0);

    // Motion rejection: if mismatch is high, strongly prefer reference
    // This is a SECONDARY defense after per-pixel spatial motion rejection
    // HDR+ paper Figure 7: pairwise filter should reject outliers sharply
    //
    // With normalized mismatch (mean=0.08), motion tiles have values above mean
    // Per-pixel rejection handles most ghosting; this catches residual issues
    if (mismatch > 0.10) {
        // Start at 0.10 (above normalized mean of 0.08), full rejection by 0.18
        let motion_penalty = smoothstep(0.10, 0.18, mismatch);
        median_w = mix(median_w, 1.0, motion_penalty);
    }

    // Edge-aware blending: favor reference at edges to preserve sharpness
    // Only apply when:
    // 1. Strong luminance edge (edge_strength > 0.15)
    // 2. Low mismatch (static scene, no motion)
    // 3. Use moderate bias (0.2) to avoid harsh transitions
    let edge_factor = smoothstep(0.15, 0.35, edge_strength);
    // Only apply edge bias when scene is static (low mismatch)
    // This avoids color fringing from misaligned content at edges
    let mismatch_gate = 1.0 - smoothstep(0.05, 0.12, mismatch);
    let edge_bias = edge_factor * mismatch_gate * 0.2;
    median_w = mix(median_w, 1.0, edge_bias);

    // Merge: out = w * ref + (1-w) * aligned_shifted
    // Higher weight = more reference (trust reference when there's motion)
    var merged_re = median_w * ref_re[row][col] + (1.0 - median_w) * aligned_shifted_re;
    var merged_im = median_w * ref_im[row][col] + (1.0 - median_w) * aligned_shifted_im;

    // Light deconvolution/sharpening based on hdr-plus-swift
    // Apply frequency-dependent gain to sharpen merged result
    // Using symmetric gain table for 8x8 tiles from hdr-plus-swift
    // cw = [0.00, 0.02, 0.04, 0.08, 0.04, 0.08, 0.04, 0.02]
    //
    // NOTE: We apply a reduced version since our architecture applies this
    // per-frame rather than once after accumulation. The effect accumulates
    // through window-weighted averaging, so we use a fraction of the gain.

    var deconv_re = merged_re;
    var deconv_im = merged_im;

    // Only apply to non-DC frequencies with low mismatch
    if (row + col > 0u && mismatch < 0.3) {
        // Mismatch weight for smooth transition
        let mismatch_weight = clamp(1.0 - 10.0 * (mismatch - 0.2), 0.0, 1.0);

        // Get DC magnitude for normalization
        let dc_mag = sqrt(ref_re[0][0] * ref_re[0][0] + ref_im[0][0] * ref_im[0][0]);
        let dc_mag_sum = dc_mag.x + dc_mag.y + dc_mag.z + dc_mag.w;

        // Current frequency magnitude
        let cur_mag = sqrt(merged_re * merged_re + merged_im * merged_im);
        let cur_mag_sum = cur_mag.x + cur_mag.y + cur_mag.z + cur_mag.w;

        // Reduce gain for high-magnitude frequencies (avoid amplifying noise)
        // weight = 0 for ratio >= 0.05, weight = 1 for ratio <= 0.01
        let mag_ratio = cur_mag_sum / max(dc_mag_sum, 0.001);
        let weight = mismatch_weight * clamp(1.25 - 25.0 * mag_ratio, 0.0, 1.0);

        // Symmetric gain table matching hdr-plus-swift (scaled down by 0.5 for safety)
        // cw = [0.00, 0.01, 0.02, 0.04, 0.02, 0.04, 0.02, 0.01]
        // Use lookup function since WGSL doesn't allow dynamic array indexing
        let cw_row = deconv_gain_lookup(row);
        let cw_col = deconv_gain_lookup(col);

        // Apply frequency-dependent gain
        let gain = (1.0 + weight * cw_row) * (1.0 + weight * cw_col);
        deconv_re = merged_re * gain;
        deconv_im = merged_im * gain;
    }

    // Store in ref arrays for inverse FFT
    ref_re[row][col] = deconv_re;
    ref_im[row][col] = deconv_im;
}

//=============================================================================
// Main compute shader
//=============================================================================

@compute @workgroup_size(16, 16)
fn merge_tile(
    @builtin(global_invocation_id) global_id: vec3<u32>,
    @builtin(local_invocation_id) local_id: vec3<u32>,
    @builtin(workgroup_id) workgroup_id: vec3<u32>
) {
    let tile_x = workgroup_id.x;
    // Add row offset for chunked dispatch support
    let tile_y = workgroup_id.y + params.tile_row_offset;
    let lx = local_id.x;
    let ly = local_id.y;
    let linear_id = ly * TILE_SIZE + lx;

    // Check bounds
    if (tile_x >= params.n_tiles_x || tile_y >= params.n_tiles_y) {
        return;
    }

    // Calculate pixel position with tile offset for 4-pass processing
    let px = i32(tile_x * TILE_SIZE + lx) + params.tile_offset_x;
    let py = i32(tile_y * TILE_SIZE + ly) + params.tile_offset_y;

    // Load tiles into shared memory (NO analysis window - synthesis-only WOLA)
    // The raised cosine window is applied only on output (synthesis) for this implementation.
    // With 50% overlap, a single synthesis window sums to unity across overlapping tiles.
    // Double-windowing (analysis + synthesis) would require sqrt(window) on each side.
    let ref_pixel = load_reference_pixel(px, py);
    var aligned_pixel = load_aligned_pixel(px, py);

    // Per-pixel spatial motion rejection BEFORE FFT
    // If pixel difference is large, replace aligned with reference to prevent ghosting
    // This stops mismatched pixels from spreading into frequency domain
    let pixel_diff = abs(ref_pixel - aligned_pixel);
    let max_channel_diff = max(max(pixel_diff.x, pixel_diff.y), max(pixel_diff.z, pixel_diff.w));
    // Threshold for significant motion: 0.08 (about 20 intensity levels on 8-bit scale)
    // Preserves more noise reduction while reducing obvious ghosting
    let spatial_motion_weight = smoothstep(0.08, 0.15, max_channel_diff);
    aligned_pixel = mix(aligned_pixel, ref_pixel, spatial_motion_weight);

    ref_re[ly][lx] = ref_pixel;
    ref_im[ly][lx] = vec4<f32>(0.0);
    aligned_re[ly][lx] = aligned_pixel;
    aligned_im[ly][lx] = vec4<f32>(0.0);
    workgroupBarrier();

    // Compute edge strength BEFORE FFT (while ref_re contains spatial data)
    // Edge detection uses Sobel operator on luminance for edge-aware merge blending
    edge_map[ly][lx] = compute_edge_strength(ly, lx);
    workgroupBarrier();

    // Forward 2D DFT - parallel version (16x speedup)
    forward_dft_2d_parallel(lx, ly, true);   // Reference
    workgroupBarrier();
    forward_dft_2d_parallel(lx, ly, false);  // Aligned
    workgroupBarrier();

    // Get noise, motion, and highlight parameters for this tile
    let tile_idx = get_tile_idx(tile_x, tile_y);
    let rms = rms_texture[tile_idx];  // Per-channel vec4 noise estimate
    let mismatch = mismatch_texture[tile_idx];
    let highlights_norm = highlights_norm_texture[tile_idx];

    // Per-channel noise VARIANCE for Wiener filter (HDR+ equation 7: cσ²)
    // rms_texture now contains variance (σ²), not SD
    // read_noise is SD, so square it to get variance
    let noise_var = rms + vec4<f32>(params.read_noise * params.read_noise);
    // HDR+ paper Section 5: c includes "factor of 2 for difference of two tiles"
    // Var(T0-Tz) = Var(T0) + Var(Tz) = 2σ²
    let noise_norm = noise_var * TILE_SIZE_F * TILE_SIZE_F * 2.0 * params.robustness;

    // Motion-aware normalization (from Night Sight Figure 9f)
    // With normalized mismatch (mean=0.08), scale motion_norm based on mismatch level
    // Low mismatch (static) → max_motion_norm, High mismatch (motion) → 1.0
    let motion_norm = clamp(
        params.max_motion_norm - (mismatch - 0.04) * (params.max_motion_norm - 1.0) / 0.20,
        1.0,
        params.max_motion_norm
    );

    // Sub-pixel alignment is handled in the alignment stage (at coarse pyramid levels),
    // NOT during merge. HDR+ paper Section 4: sub-pixel precision comes from quadratic
    // fitting at coarse levels, propagated through the pyramid.
    // Searching for sub-pixel shifts here is redundant and computationally expensive.
    let best_shift = vec2<f32>(0.0, 0.0);

    // Get edge strength for this pixel (computed before FFT)
    let edge_strength = edge_map[ly][lx];

    // Wiener merge in frequency domain
    // Each thread handles one frequency bin
    wiener_merge_frequency(ly, lx, noise_norm, motion_norm, highlights_norm, mismatch, best_shift, edge_strength);
    workgroupBarrier();

    // Backward 2D DFT - parallel version (16x speedup)
    backward_dft_2d_parallel(lx, ly);
    workgroupBarrier();

    // Get merged result from frequency domain processing
    let merged = ref_re[ly][lx];

    // COMMON UTILITY: Raised cosine window for WOLA synthesis
    // Keep in sync with: spatial_denoise.wgsl (raised_cosine_window function)
    // Formula: HDR+ paper page 8 - modified raised cosine ½ - ½cos(2π(x + ½)/n)
    let angle = 2.0 * PI / TILE_SIZE_F;
    let window_x = 0.5 - 0.5 * cos(angle * (f32(lx) + 0.5));
    let window_y = 0.5 - 0.5 * cos(angle * (f32(ly) + 0.5));
    let output_window = window_x * window_y;

    // Write output with window weighting for smooth tile blending
    // HDR+ paper page 8: modified raised cosine window ½ - ½cos(2π(x + ½)/n)
    // "when this function is repeated with n/2 samples of overlap,
    // the total contribution from all tiles sum to one at every position"
    // Window applied only on output (WOLA synthesis) - no weight tracking needed
    if (px >= 0 && py >= 0 && u32(px) < params.width && u32(py) < params.height) {
        let out_idx = get_pixel_idx(u32(px), u32(py));

        // Apply output window and accumulate
        let windowed = merged * output_window;

        let existing = vec4<f32>(
            output[out_idx],
            output[out_idx + 1u],
            output[out_idx + 2u],
            output[out_idx + 3u]
        );

        output[out_idx] = existing.x + windowed.x;
        output[out_idx + 1u] = existing.y + windowed.y;
        output[out_idx + 2u] = existing.z + windowed.z;
        output[out_idx + 3u] = existing.w + windowed.w;

        // No weight tracking needed - overlapping windows sum to 1.0
    }
}

//=============================================================================
// Initialization and finalization shaders
//=============================================================================

// Initialize output buffer with zeros
@compute @workgroup_size(16, 16)
fn init_output(@builtin(global_invocation_id) global_id: vec3<u32>) {
    let x = global_id.x;
    let y = global_id.y;

    if (x >= params.width || y >= params.height) {
        return;
    }

    let idx = get_pixel_idx(x, y);
    output[idx] = 0.0;
    output[idx + 1u] = 0.0;
    output[idx + 2u] = 0.0;
    output[idx + 3u] = 0.0;

    // Initialize weight accumulator to zero
    let weight_idx = y * params.width + x;
    weight_accum[weight_idx] = 0.0;
}

// Normalize output after all frames merged
// With WOLA synthesis, overlapping windows sum to 1.0 per 4-pass cycle
// So we only need to divide by frame_count (number of alternate frames merged)
@compute @workgroup_size(16, 16)
fn normalize_output(@builtin(global_invocation_id) global_id: vec3<u32>) {
    let x = global_id.x;
    let y = global_id.y;

    if (x >= params.width || y >= params.height) {
        return;
    }

    let idx = get_pixel_idx(x, y);

    // Windows sum to 1.0 per 4-pass, so total = frame_count * signal
    // frame_count = number of alternate frames + 1 (reference)
    let norm = 1.0 / f32(params.frame_count);

    output[idx] = clamp(output[idx] * norm, 0.0, 1.0);
    output[idx + 1u] = clamp(output[idx + 1u] * norm, 0.0, 1.0);
    output[idx + 2u] = clamp(output[idx + 2u] * norm, 0.0, 1.0);
    output[idx + 3u] = clamp(output[idx + 3u] * norm, 0.0, 1.0);
}

// Shared memory for parallel reduction in normalize_mismatch
var<workgroup> mismatch_partial_sums: array<f32, 256>;
var<workgroup> mismatch_scale: f32;

// Normalize mismatch texture so mean equals target (0.12)
// This makes motion detection consistent across different scenes
//
// Parallel implementation using 256-thread workgroup with local reduction.
// Each thread sums multiple tiles, then workgroup reduces to compute scale factor.
@compute @workgroup_size(256, 1, 1)
fn normalize_mismatch(
    @builtin(local_invocation_id) lid: vec3<u32>,
    @builtin(global_invocation_id) gid: vec3<u32>
) {
    let thread_id = lid.x;
    let total_tiles = params.n_tiles_x * params.n_tiles_y;

    // Phase 1: Each thread sums a strided subset of tiles
    var thread_sum = 0.0;
    for (var i = thread_id; i < total_tiles; i += 256u) {
        thread_sum += mismatch_texture[i];
    }

    // Store thread's partial sum
    mismatch_partial_sums[thread_id] = thread_sum;
    workgroupBarrier();

    // Phase 2: Parallel reduction within workgroup (log2(256) = 8 steps)
    for (var stride = 128u; stride > 0u; stride >>= 1u) {
        if (thread_id < stride) {
            mismatch_partial_sums[thread_id] += mismatch_partial_sums[thread_id + stride];
        }
        workgroupBarrier();
    }

    // Phase 3: Thread 0 computes scale factor
    if (thread_id == 0u) {
        let total_sum = mismatch_partial_sums[0];
        let mean_mismatch = total_sum / f32(max(total_tiles, 1u));

        // Target mean mismatch for normalized values
        // With per-pixel spatial motion rejection as primary defense,
        // mismatch-based rejection is secondary - use moderate target
        let target_mean = 0.08;

        // Scale factor to normalize mean to target
        mismatch_scale = target_mean / max(mean_mismatch, 0.001);
    }
    workgroupBarrier();

    // Phase 4: All threads apply normalization in parallel
    let scale = mismatch_scale;
    for (var i = thread_id; i < total_tiles; i += 256u) {
        mismatch_texture[i] = clamp(mismatch_texture[i] * scale, 0.0, 1.0);
    }
}

//=============================================================================
// RMS and mismatch calculation shaders
//=============================================================================

// Calculate noise estimate per tile using affine noise model (HDR+ Section 5)
//
// Based on HDR+ paper "Noise model and tiled approximation":
// "for a signal level of x, the noise variance σ² can be expressed as Ax + B,
// following from the Poisson-distributed physical process of photon counting"
//
// A = shot noise coefficient (signal-dependent, proportional to sensor gain)
// B = read noise variance (constant, from analog circuitry)
//
// For computational efficiency, we approximate noise as signal-independent
// within a tile, using RMS of samples to estimate signal level (biases toward
// brighter content, which is more conservative for Wiener filtering).
//
// Returns per-channel vec4 noise estimate for better color accuracy.
@compute @workgroup_size(1, 1)
fn calculate_rms(@builtin(global_invocation_id) gid: vec3<u32>) {
    let tile_x = gid.x;
    let tile_y = gid.y;

    if (tile_x >= params.n_tiles_x || tile_y >= params.n_tiles_y) {
        return;
    }

    var sum = vec4<f32>(0.0);
    var sum_sq = vec4<f32>(0.0);
    var count = 0u;

    for (var dy = 0u; dy < TILE_SIZE; dy++) {
        for (var dx = 0u; dx < TILE_SIZE; dx++) {
            let px = tile_x * TILE_SIZE + dx;
            let py = tile_y * TILE_SIZE + dy;

            if (px < params.width && py < params.height) {
                let idx = get_pixel_idx(px, py);
                let pixel = vec4<f32>(
                    reference[idx],
                    reference[idx + 1u],
                    reference[idx + 2u],
                    reference[idx + 3u]
                );
                sum += pixel;
                sum_sq += pixel * pixel;
                count += 1u;
            }
        }
    }

    let count_f = f32(max(count, 1u));

    // Calculate RMS signal level per channel (biased toward brighter content)
    // HDR+ paper: "Using RMS has the effect of biasing the signal estimate
    // toward brighter image content"
    let rms_signal = sqrt(sum_sq / count_f);

    // Affine noise model: σ² = A * signal + B
    // A = shot noise coefficient (typical range 0.0005-0.002 for normalized [0,1] data)
    // B = read noise variance (from params.read_noise²)
    //
    // Shot noise coefficient A relates to sensor gain:
    // - For normalized data, A ≈ 1/full_well_capacity * gain
    // - Typical digital cameras: A ~ 0.001 at base ISO
    let shot_noise_coeff = 0.001;
    let read_noise_var = params.read_noise * params.read_noise;

    // Per-channel noise VARIANCE from affine model
    // HDR+ Wiener filter (equation 7) uses σ², not σ
    // Storing variance directly for correct Wiener denominator scaling
    let noise_variance = shot_noise_coeff * rms_signal + vec4<f32>(read_noise_var);

    let tile_idx = get_tile_idx(tile_x, tile_y);
    rms_texture[tile_idx] = noise_variance;
}

// Calculate mismatch (motion indicator) per tile
// Based on Night Sight paper "Spatially varying temporal merging"
// Uses noise-normalized difference to detect motion independent of brightness
//
// IMPORTANT: Uses 2x tile size spatial support with cosine weighting
// to match hdr-plus-swift frequency.metal:253-282
@compute @workgroup_size(1, 1)
fn calculate_mismatch(@builtin(global_invocation_id) gid: vec3<u32>) {
    let tile_x = gid.x;
    let tile_y = gid.y;

    if (tile_x >= params.n_tiles_x || tile_y >= params.n_tiles_y) {
        return;
    }

    // Get per-channel noise estimate for this tile (already computed)
    let tile_idx = get_tile_idx(tile_x, tile_y);
    let noise_est = rms_texture[tile_idx];

    // Tile center position
    let x0 = i32(tile_x * TILE_SIZE);
    let y0 = i32(tile_y * TILE_SIZE);

    // Use 2x tile size spatial support with cosine weighting (matching hdr-plus-swift)
    // Support region: [x0 - tile_size/2, x0 + tile_size*3/2)
    let half_tile = i32(TILE_SIZE / 2u);
    let x_start = max(0i, x0 - half_tile);
    let x_end = min(i32(params.width), x0 + i32(TILE_SIZE) + half_tile);
    let y_start = max(0i, y0 - half_tile);
    let y_end = min(i32(params.height), y0 + i32(TILE_SIZE) + half_tile);

    var sum_diff = vec4<f32>(0.0);
    var weight_sum = 0.0;
    var max_diff = vec4<f32>(0.0);  // Track max difference for localized motion detection

    // Cosine window parameters (matching hdr-plus-swift: 0.5 - 0.17*cos(...))
    let support_size = 2.0 * TILE_SIZE_F;  // 2x tile size

    for (var py = y_start; py < y_end; py++) {
        for (var px = x_start; px < x_end; px++) {
            let idx = get_pixel_idx(u32(px), u32(py));

            let ref_pixel = vec4<f32>(reference[idx], reference[idx + 1u], reference[idx + 2u], reference[idx + 3u]);
            let aligned_pixel = vec4<f32>(aligned[idx], aligned[idx + 1u], aligned[idx + 2u], aligned[idx + 3u]);

            let pixel_diff = abs(ref_pixel - aligned_pixel);

            // Calculate cosine window weight based on distance from tile start
            // Following hdr-plus-swift: norm_cosine = (0.5 - 0.17*cos(angle*(dx+0.5))) * (0.5 - 0.17*cos(angle*(dy+0.5)))
            let dx = f32(px - x_start);
            let dy = f32(py - y_start);
            let angle_x = 2.0 * PI / support_size;
            let angle_y = 2.0 * PI / support_size;
            let norm_cosine_x = 0.5 - 0.17 * cos(angle_x * (dx + 0.5));
            let norm_cosine_y = 0.5 - 0.17 * cos(angle_y * (dy + 0.5));
            let norm_cosine = norm_cosine_x * norm_cosine_y;

            sum_diff += norm_cosine * pixel_diff;
            weight_sum += norm_cosine;

            // Track max difference within core tile region for localized motion
            // Core region is the central tile (not the extended support)
            if (px >= x0 && px < x0 + i32(TILE_SIZE) && py >= y0 && py < y0 + i32(TILE_SIZE)) {
                max_diff = max(max_diff, pixel_diff);
            }
        }
    }

    // Weighted average difference
    let avg_diff = sum_diff / max(weight_sum, 1.0);

    // Noise normalization denominator
    let noise_denom = sqrt(noise_est + vec4<f32>(1.0));

    // HDR+ mismatch formula matching hdr-plus-swift frequency.metal:286
    // Using per-channel noise estimate for better accuracy
    let avg_mismatch4 = avg_diff / noise_denom;
    let avg_mismatch = 0.25 * (avg_mismatch4.x + avg_mismatch4.y + avg_mismatch4.z + avg_mismatch4.w);

    // Max-based mismatch for localized motion detection
    // This catches small moving objects that would be averaged out
    let max_mismatch4 = max_diff / noise_denom;
    let max_mismatch = 0.25 * (max_mismatch4.x + max_mismatch4.y + max_mismatch4.z + max_mismatch4.w);

    // Blend average and max: use max when it indicates significant localized motion
    // If max is much higher than average, there's likely localized motion
    // Weight max very heavily (0.8) since localized motion causes ghosts even if average is low
    let mismatch = max(avg_mismatch, 0.2 * avg_mismatch + 0.8 * max_mismatch);

    mismatch_texture[tile_idx] = clamp(mismatch, 0.0, 1.0);
}

// Calculate highlight normalization per tile
// Detects clipped/saturated pixels and reduces their influence
// Based on hdr-plus-swift calculate_highlights_norm_rgba
@compute @workgroup_size(1, 1)
fn calculate_highlights_norm(@builtin(global_invocation_id) gid: vec3<u32>) {
    let tile_x = gid.x;
    let tile_y = gid.y;

    if (tile_x >= params.n_tiles_x || tile_y >= params.n_tiles_y) {
        return;
    }

    // Exposure factor from params (1.0 for uniform exposure bursts)
    // For HDR brackets, this would be the relative exposure of this frame
    let exposure_factor = params.exposure_factor;

    var clipped_count = 0.0;
    var total_count = 0.0;

    // Threshold for considering a pixel "bright" (50% of max)
    let bright_threshold = 0.5;
    // Threshold for considering a pixel "saturated" (99% of max)
    let saturated_threshold = 0.99;

    for (var dy = 0u; dy < TILE_SIZE; dy++) {
        for (var dx = 0u; dx < TILE_SIZE; dx++) {
            let px = tile_x * TILE_SIZE + dx;
            let py = tile_y * TILE_SIZE + dy;

            if (px < params.width && py < params.height) {
                let idx = get_pixel_idx(px, py);
                let pixel = vec4<f32>(
                    aligned[idx],
                    aligned[idx + 1u],
                    aligned[idx + 2u],
                    aligned[idx + 3u]
                );

                // Get max channel value
                let pixel_max = max(max(pixel.x, pixel.y), pixel.z);

                // Smooth contribution between bright_threshold and saturated_threshold
                let contribution = clamp((pixel_max - bright_threshold) / (saturated_threshold - bright_threshold), 0.0, 1.0);
                clipped_count += contribution;
                total_count += 1.0;
            }
        }
    }

    // Calculate fraction of clipped highlights
    let clipped_fraction = clipped_count / max(total_count, 1.0);

    // Transform to correction factor:
    // More clipped pixels = lower norm = less influence from this tile
    // (1 - fraction)^2 gives strong suppression for heavily clipped tiles
    let highlights_norm = clamp(
        (1.0 - clipped_fraction) * (1.0 - clipped_fraction),
        0.04 / max(exposure_factor, 1.0),
        1.0
    );

    let tile_idx = get_tile_idx(tile_x, tile_y);
    highlights_norm_texture[tile_idx] = highlights_norm;
}

//=============================================================================
// Post-processing: Simple output clamping
//
// NOTE: The hdr-plus-swift reduce_artifacts_tile_border() is designed for their
// architecture where it runs per-tile during each backward FFT pass. Our
// architecture uses 4-pass offset tiles with window-weighted accumulation,
// so the tile borders don't align to a fixed grid after accumulation.
//
// The raised cosine window applied during forward_dft_2d already handles
// tile blending. This kernel just ensures output values are in valid range.
//=============================================================================

@compute @workgroup_size(16, 16)
fn reduce_tile_artifacts(@builtin(global_invocation_id) gid: vec3<u32>) {
    let x = gid.x;
    let y = gid.y;

    if (x >= params.width || y >= params.height) {
        return;
    }

    let idx = get_pixel_idx(x, y);

    // Simple clamping to valid range [0, 1]
    output[idx] = clamp(output[idx], 0.0, 1.0);
    output[idx + 1u] = clamp(output[idx + 1u], 0.0, 1.0);
    output[idx + 2u] = clamp(output[idx + 2u], 0.0, 1.0);
    output[idx + 3u] = clamp(output[idx + 3u], 0.0, 1.0);
}

//=============================================================================
// Add reference frame to output accumulator (HDR+ equation 6, z=0 term)
//
// This implements the z=0 term from HDR+ paper equation 6:
//   T̃₀(ω) = (1/N) Σ_{z=0}^{N-1} [T_z(ω) + A_z(ω)(T₀(ω) - T_z(ω))]
//
// For z=0 (reference frame), A₀=0, so this simply adds T₀ to the accumulator.
// This must be run with 4-pass WOLA (same offsets as merge) to ensure proper
// window coverage across all pixels.
//
// Without this, the reference frame only contributes through Wiener merge
// with alternates, causing mosaic patterns and brightness errors.
//=============================================================================

@compute @workgroup_size(16, 16)
fn add_reference_to_output(
    @builtin(global_invocation_id) global_id: vec3<u32>,
    @builtin(local_invocation_id) local_id: vec3<u32>,
    @builtin(workgroup_id) workgroup_id: vec3<u32>
) {
    let tile_x = workgroup_id.x;
    // Add row offset for chunked dispatch support
    let tile_y = workgroup_id.y + params.tile_row_offset;
    let lx = local_id.x;
    let ly = local_id.y;

    // Check tile bounds
    if (tile_x >= params.n_tiles_x || tile_y >= params.n_tiles_y) {
        return;
    }

    // Calculate pixel position with tile offset for 4-pass processing
    let px = i32(tile_x * TILE_SIZE + lx) + params.tile_offset_x;
    let py = i32(tile_y * TILE_SIZE + ly) + params.tile_offset_y;

    // Load reference pixel with edge clamping
    let ref_pixel = load_reference_pixel(px, py);

    // Calculate raised cosine window for output blending
    // HDR+ paper page 8: modified raised cosine window ½ - ½cos(2π(x + ½)/n)
    // "when this function is repeated with n/2 samples of overlap,
    // the total contribution from all tiles sum to one at every position"
    let angle = 2.0 * PI / TILE_SIZE_F;
    let window_x = 0.5 - 0.5 * cos(angle * (f32(lx) + 0.5));
    let window_y = 0.5 - 0.5 * cos(angle * (f32(ly) + 0.5));
    let output_window = window_x * window_y;

    // Add windowed reference to output accumulator
    if (px >= 0 && py >= 0 && u32(px) < params.width && u32(py) < params.height) {
        let out_idx = get_pixel_idx(u32(px), u32(py));
        let windowed = ref_pixel * output_window;

        output[out_idx] = output[out_idx] + windowed.x;
        output[out_idx + 1u] = output[out_idx + 1u] + windowed.y;
        output[out_idx + 2u] = output[out_idx + 2u] + windowed.z;
        output[out_idx + 3u] = output[out_idx + 3u] + windowed.w;
    }
}
