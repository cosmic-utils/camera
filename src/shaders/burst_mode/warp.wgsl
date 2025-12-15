// SPDX-License-Identifier: GPL-3.0-only
//
// Frame warping for night mode alignment
//
// Warps the comparison frame according to per-tile or per-pixel displacement.
// Uses inverse mapping with bilinear interpolation for smooth results.
//
// Based on HDR+ alignment warping.

struct WarpParams {
    width: u32,
    height: u32,
    n_tiles_x: u32,
    n_tiles_y: u32,
    tile_size: u32,
    tile_step: u32,
    use_bilinear: u32,  // 1 for bilinear interpolation, 0 for nearest
    _padding0: u32,
    // CA correction parameters
    center_x: f32,
    center_y: f32,
    ca_r_coeff: f32,    // Red channel radial CA coefficient (typically positive)
    ca_b_coeff: f32,    // Blue channel radial CA coefficient (typically negative)
    enable_ca_correction: u32,
    _padding: u32,
    _padding2: u32,
    _padding3: u32,
}

// Input frame (RGBA f32)
@group(0) @binding(0)
var<storage, read> input_frame: array<f32>;

// Output warped frame (RGBA f32)
@group(0) @binding(1)
var<storage, read_write> output_frame: array<f32>;

// Per-tile alignment vectors (sub-pixel precision)
@group(0) @binding(2)
var<storage, read> alignment: array<vec2<f32>>;

@group(0) @binding(3)
var<uniform> params: WarpParams;

//=============================================================================
// Utility functions
//=============================================================================

fn get_pixel_idx(x: u32, y: u32) -> u32 {
    return (y * params.width + x) * 4u;
}

fn get_tile_idx(tx: u32, ty: u32) -> u32 {
    return ty * params.n_tiles_x + tx;
}

// Sample input frame with bilinear interpolation
fn sample_bilinear(x: f32, y: f32) -> vec4<f32> {
    let x0 = i32(floor(x));
    let y0 = i32(floor(y));
    let x1 = x0 + 1;
    let y1 = y0 + 1;

    let fx = x - f32(x0);
    let fy = y - f32(y0);

    // Clamp coordinates
    let cx0 = clamp(x0, 0, i32(params.width) - 1);
    let cy0 = clamp(y0, 0, i32(params.height) - 1);
    let cx1 = clamp(x1, 0, i32(params.width) - 1);
    let cy1 = clamp(y1, 0, i32(params.height) - 1);

    // Sample four corners
    let idx00 = get_pixel_idx(u32(cx0), u32(cy0));
    let idx10 = get_pixel_idx(u32(cx1), u32(cy0));
    let idx01 = get_pixel_idx(u32(cx0), u32(cy1));
    let idx11 = get_pixel_idx(u32(cx1), u32(cy1));

    let p00 = vec4<f32>(input_frame[idx00], input_frame[idx00 + 1u], input_frame[idx00 + 2u], input_frame[idx00 + 3u]);
    let p10 = vec4<f32>(input_frame[idx10], input_frame[idx10 + 1u], input_frame[idx10 + 2u], input_frame[idx10 + 3u]);
    let p01 = vec4<f32>(input_frame[idx01], input_frame[idx01 + 1u], input_frame[idx01 + 2u], input_frame[idx01 + 3u]);
    let p11 = vec4<f32>(input_frame[idx11], input_frame[idx11 + 1u], input_frame[idx11 + 2u], input_frame[idx11 + 3u]);

    // Bilinear interpolation
    let top = mix(p00, p10, fx);
    let bottom = mix(p01, p11, fx);
    return mix(top, bottom, fy);
}

// Sample input frame with nearest neighbor
fn sample_nearest(x: i32, y: i32) -> vec4<f32> {
    let cx = clamp(x, 0, i32(params.width) - 1);
    let cy = clamp(y, 0, i32(params.height) - 1);
    let idx = get_pixel_idx(u32(cx), u32(cy));
    return vec4<f32>(input_frame[idx], input_frame[idx + 1u], input_frame[idx + 2u], input_frame[idx + 3u]);
}

// Get alignment vector for a tile (now sub-pixel precision)
fn get_tile_alignment(tx: i32, ty: i32) -> vec2<f32> {
    let cx = clamp(tx, 0, i32(params.n_tiles_x) - 1);
    let cy = clamp(ty, 0, i32(params.n_tiles_y) - 1);
    let idx = get_tile_idx(u32(cx), u32(cy));
    return alignment[idx];
}

// Interpolate alignment between tiles
// Grid coordinate calculation matches hdr-plus-swift:
// (x + 0.5) / tile_step - 1.0 for proper pixel center alignment
fn get_interpolated_alignment(x: u32, y: u32) -> vec2<f32> {
    let tile_x_f = (f32(x) + 0.5) / f32(params.tile_step) - 1.0;
    let tile_y_f = (f32(y) + 0.5) / f32(params.tile_step) - 1.0;

    let tx0 = i32(floor(tile_x_f));
    let ty0 = i32(floor(tile_y_f));

    let fx = fract(tile_x_f);
    let fy = fract(tile_y_f);

    // Get four surrounding tile alignments (already vec2<f32> with sub-pixel precision)
    let a00 = get_tile_alignment(tx0, ty0);
    let a10 = get_tile_alignment(tx0 + 1, ty0);
    let a01 = get_tile_alignment(tx0, ty0 + 1);
    let a11 = get_tile_alignment(tx0 + 1, ty0 + 1);

    // Bilinear interpolation of alignment
    let top = mix(a00, a10, fx);
    let bottom = mix(a01, a11, fx);
    return mix(top, bottom, fy);
}

//=============================================================================
// Chromatic Aberration Correction (HDR+ Section 6 Step 10)
//=============================================================================

// Apply radial CA correction to source coordinates
// CA model: scale = 1 + coeff * normalized_radius²
// Red typically shifts outward (positive coeff)
// Blue typically shifts inward (negative coeff)
fn apply_ca_correction(src_x: f32, src_y: f32, channel: u32) -> vec2<f32> {
    if (params.enable_ca_correction == 0u) {
        return vec2<f32>(src_x, src_y);
    }

    let dx = src_x - params.center_x;
    let dy = src_y - params.center_y;
    let radius_sq = dx * dx + dy * dy;
    let max_radius_sq = params.center_x * params.center_x + params.center_y * params.center_y;
    let norm_r_sq = radius_sq / max(max_radius_sq, 1.0);

    var scale = 1.0;
    if (channel == 0u) {      // Red
        scale = 1.0 + params.ca_r_coeff * norm_r_sq;
    } else if (channel == 2u) { // Blue
        scale = 1.0 + params.ca_b_coeff * norm_r_sq;
    }
    // Green (channel 1) uses scale = 1.0 (reference)

    return vec2<f32>(
        params.center_x + dx * scale,
        params.center_y + dy * scale
    );
}

//=============================================================================
// Single-channel bilinear sampling
//=============================================================================

// Sample a single channel with bilinear interpolation
fn sample_bilinear_channel(x: f32, y: f32, channel: u32) -> f32 {
    let x0 = i32(floor(x));
    let y0 = i32(floor(y));
    let x1 = x0 + 1;
    let y1 = y0 + 1;

    let fx = x - f32(x0);
    let fy = y - f32(y0);

    // Clamp coordinates
    let cx0 = clamp(x0, 0, i32(params.width) - 1);
    let cy0 = clamp(y0, 0, i32(params.height) - 1);
    let cx1 = clamp(x1, 0, i32(params.width) - 1);
    let cy1 = clamp(y1, 0, i32(params.height) - 1);

    // Sample four corners for this channel
    let idx00 = get_pixel_idx(u32(cx0), u32(cy0)) + channel;
    let idx10 = get_pixel_idx(u32(cx1), u32(cy0)) + channel;
    let idx01 = get_pixel_idx(u32(cx0), u32(cy1)) + channel;
    let idx11 = get_pixel_idx(u32(cx1), u32(cy1)) + channel;

    let p00 = input_frame[idx00];
    let p10 = input_frame[idx10];
    let p01 = input_frame[idx01];
    let p11 = input_frame[idx11];

    // Bilinear interpolation
    let top = mix(p00, p10, fx);
    let bottom = mix(p01, p11, fx);
    return mix(top, bottom, fy);
}

//=============================================================================
// Main warp kernel
//=============================================================================

@compute @workgroup_size(16, 16)
fn warp_frame(@builtin(global_invocation_id) gid: vec3<u32>) {
    let x = gid.x;
    let y = gid.y;

    if (x >= params.width || y >= params.height) {
        return;
    }

    // Get displacement for this pixel (inverse mapping)
    let displacement = get_interpolated_alignment(x, y);

    // Source coordinates (inverse: subtract displacement)
    let src_x = f32(x) - displacement.x;
    let src_y = f32(y) - displacement.y;

    // Apply CA correction per-channel if enabled
    var r: f32;
    var g: f32;
    var b: f32;
    var a: f32;

    if (params.enable_ca_correction == 1u) {
        // Apply CA correction: scale source position radially for R and B channels
        let ca_src_r = apply_ca_correction(src_x, src_y, 0u);  // Red
        let ca_src_g = vec2<f32>(src_x, src_y);                 // Green (reference)
        let ca_src_b = apply_ca_correction(src_x, src_y, 2u);  // Blue

        // Sample each channel from its CA-corrected position
        r = sample_bilinear_channel(ca_src_r.x, ca_src_r.y, 0u);
        g = sample_bilinear_channel(ca_src_g.x, ca_src_g.y, 1u);
        b = sample_bilinear_channel(ca_src_b.x, ca_src_b.y, 2u);
        a = sample_bilinear_channel(ca_src_g.x, ca_src_g.y, 3u);
    } else {
        // No CA correction - sample all channels from same position
        var pixel: vec4<f32>;
        if (params.use_bilinear == 1u) {
            pixel = sample_bilinear(src_x, src_y);
        } else {
            pixel = sample_nearest(i32(round(src_x)), i32(round(src_y)));
        }
        r = pixel.x;
        g = pixel.y;
        b = pixel.z;
        a = pixel.w;
    }

    // Write output
    let out_idx = get_pixel_idx(x, y);
    output_frame[out_idx] = r;
    output_frame[out_idx + 1u] = g;
    output_frame[out_idx + 2u] = b;
    output_frame[out_idx + 3u] = a;
}

//=============================================================================
// Compute alignment quality per tile (for merge weighting)
//=============================================================================

struct QualityParams {
    n_tiles_x: u32,
    n_tiles_y: u32,
    max_displacement: f32,
    _padding: u32,
}

@group(0) @binding(0)
var<storage, read> quality_alignment: array<vec2<f32>>;

@group(0) @binding(1)
var<storage, read_write> quality_output: array<f32>;

@group(0) @binding(2)
var<uniform> qual_params: QualityParams;

@compute @workgroup_size(16, 16)
fn compute_alignment_quality(@builtin(global_invocation_id) gid: vec3<u32>) {
    let tx = gid.x;
    let ty = gid.y;

    if (tx >= qual_params.n_tiles_x || ty >= qual_params.n_tiles_y) {
        return;
    }

    let tile_idx = ty * qual_params.n_tiles_x + tx;
    let align_vec = quality_alignment[tile_idx];

    // Quality is inverse of displacement magnitude (now sub-pixel precision)
    let magnitude = sqrt(align_vec.x * align_vec.x + align_vec.y * align_vec.y);
    let quality = 1.0 / (1.0 + magnitude / qual_params.max_displacement);

    quality_output[tile_idx] = quality;
}
