// SPDX-License-Identifier: GPL-3.0-only
//
// Local tone mapping for night mode
//
// Applies tone mapping for:
// - Shadow lifting (boost dark areas)
// - Highlight compression (protect bright areas)
// - Local contrast enhancement
// - Gamma correction (linear to sRGB)
// - Blue noise dithering (prevent banding when converting to 8-bit)
//
// Based on HDR+ paper Section 6:
// "Dithering to avoid quantization artifacts when reducing from 12 bits per pixel
// to 8 bits for display, implemented by adding blue noise from a precomputed table."

struct TonemapParams {
    width: u32,
    height: u32,
    shadow_boost: f32,       // 0.0 - 1.0, strength of shadow lifting
    local_contrast: f32,     // 0.0 - 1.0, local contrast enhancement
    highlight_compress: f32, // 0.0 - 1.0, highlight compression
    gamma: f32,              // Output gamma (typically 2.2 for sRGB)
    dither_strength: f32,    // Dithering strength (typically 1.0/255.0)
    avg_brightness: f32,     // Scene average brightness for adaptive gamma (HDR+ Section 6)
}

// Input merged frame (RGBA f32, linear)
@group(0) @binding(0)
var<storage, read> input_image: array<f32>;

// Output tone-mapped frame (RGBA f32, will be converted to sRGB)
@group(0) @binding(1)
var<storage, read_write> output_image: array<f32>;

// Local luminance map (downsampled average luminance)
@group(0) @binding(2)
var<storage, read> local_luminance: array<f32>;

@group(0) @binding(3)
var<uniform> params: TonemapParams;

// Local luminance dimensions (downsampled)
@group(0) @binding(4)
var<uniform> lum_width: u32;

@group(0) @binding(5)
var<uniform> lum_height: u32;

@group(0) @binding(6)
var<uniform> block_size: u32;

//=============================================================================
// Utility functions
//=============================================================================

fn get_pixel_idx(x: u32, y: u32) -> u32 {
    return (y * params.width + x) * 4u;
}

fn load_pixel(x: u32, y: u32) -> vec4<f32> {
    let idx = get_pixel_idx(x, y);
    // Input is already in linear space from merge stage - no conversion needed
    return vec4<f32>(input_image[idx], input_image[idx + 1u], input_image[idx + 2u], input_image[idx + 3u]);
}

fn store_pixel(x: u32, y: u32, val: vec4<f32>) {
    let idx = get_pixel_idx(x, y);
    output_image[idx] = val.x;
    output_image[idx + 1u] = val.y;
    output_image[idx + 2u] = val.z;
    output_image[idx + 3u] = val.w;
}

// Get local average luminance (bilinear interpolated)
fn get_local_luminance(x: u32, y: u32) -> f32 {
    let lx_f = f32(x) / f32(block_size);
    let ly_f = f32(y) / f32(block_size);

    let lx0 = u32(floor(lx_f));
    let ly0 = u32(floor(ly_f));
    let lx1 = min(lx0 + 1u, lum_width - 1u);
    let ly1 = min(ly0 + 1u, lum_height - 1u);

    let fx = fract(lx_f);
    let fy = fract(ly_f);

    let l00 = local_luminance[ly0 * lum_width + lx0];
    let l10 = local_luminance[ly0 * lum_width + lx1];
    let l01 = local_luminance[ly1 * lum_width + lx0];
    let l11 = local_luminance[ly1 * lum_width + lx1];

    let top = mix(l00, l10, fx);
    let bottom = mix(l01, l11, fx);
    return mix(top, bottom, fy);
}

// COMMON UTILITY: BT.601 RGB to luminance conversion
// See common.wgsl for reference. Keep in sync across all shaders that use this.
// Formula: Y = 0.299*R + 0.587*G + 0.114*B
fn rgb_to_luminance(rgb: vec3<f32>) -> f32 {
    return 0.299 * rgb.r + 0.587 * rgb.g + 0.114 * rgb.b;
}

//=============================================================================
// Blue noise dithering
// Produces visually pleasant noise pattern that avoids banding artifacts
// Based on HDR+ paper Section 6, Step 13
//=============================================================================

// Hash function for pseudo-random blue noise approximation
// Based on "Hash without Sine" by Dave Hoskins
fn hash12(p: vec2<f32>) -> f32 {
    var p3 = fract(vec3<f32>(p.x, p.y, p.x) * 0.1031);
    p3 += dot(p3, p3.yzx + 33.33);
    return fract((p3.x + p3.y) * p3.z);
}

fn hash13(p: vec3<f32>) -> f32 {
    var p3 = fract(p * 0.1031);
    p3 += dot(p3, p3.zyx + 31.32);
    return fract((p3.x + p3.y) * p3.z);
}

// Generate blue noise value at pixel position
// Uses spatial hashing with different seeds per channel for RGB decorrelation
fn blue_noise(x: u32, y: u32, channel: u32) -> f32 {
    // Different seed per channel to decorrelate RGB noise
    let seed = f32(channel) * 0.333;

    // Spatial hash with blue noise characteristics
    // Multiple octaves create more uniform spectral distribution
    let p = vec2<f32>(f32(x), f32(y));
    var noise = hash12(p + seed);

    // Add higher frequency component for better blue noise approximation
    noise = noise * 0.7 + hash12(p * 2.0 + seed + 17.0) * 0.3;

    // Center around 0 (range: -0.5 to +0.5)
    return noise - 0.5;
}

// Apply triangular-PDF dither (better quality than uniform)
fn triangular_dither(x: u32, y: u32, channel: u32) -> f32 {
    let n1 = blue_noise(x, y, channel);
    let n2 = blue_noise(x + 1u, y + 1u, channel + 3u);

    // Sum of two uniform distributions gives triangular PDF
    // This reduces visible noise while maintaining dithering effectiveness
    return (n1 + n2) * 0.5;
}

//=============================================================================
// Tone curve functions
//=============================================================================

// Smooth step function
fn smoothstep(edge0: f32, edge1: f32, x: f32) -> f32 {
    let t = clamp((x - edge0) / (edge1 - edge0), 0.0, 1.0);
    return t * t * (3.0 - 2.0 * t);
}

// Shadow boost curve
// Based on HDR+ paper's approach: lift shadows while preserving midtones
fn shadow_curve(lum: f32, strength: f32) -> f32 {
    // Two-part curve:
    // 1. Aggressive lift for very dark areas (< 0.1)
    // 2. Gentle lift for shadows (0.1 - 0.3)
    // 3. No change for midtones and highlights (> 0.3)

    if (lum < 0.1) {
        // Very dark: strong lift using power curve
        let lifted = pow(lum / 0.1, 1.0 - strength * 0.7) * 0.1;
        return lifted + lum * strength * 0.3;  // Add some original to preserve detail
    } else if (lum < 0.3) {
        // Shadow region: gradual transition
        let t = (lum - 0.1) / 0.2;  // 0 at 0.1, 1 at 0.3
        let shadow_lift = pow(lum, 1.0 - strength * 0.3 * (1.0 - t));
        return shadow_lift;
    } else {
        // Midtones and highlights: no shadow boost
        return lum;
    }
}

// Highlight compression curve
fn highlight_curve(lum: f32, strength: f32) -> f32 {
    // Inverse power curve that compresses highlights
    return 1.0 - pow(1.0 - lum, 1.0 + strength);
}

// Combined tone curve
fn apply_tone_curve(lum: f32, local_avg: f32, shadow: f32, contrast: f32) -> f32 {
    // Apply shadow boost (shadow_curve handles the transition internally)
    var mapped = shadow_curve(lum, shadow);

    // Apply highlight compression only to bright areas
    if (lum > 0.7) {
        let highlight_mapped = highlight_curve(lum, 0.5);
        let blend = smoothstep(0.7, 0.9, lum);
        mapped = mix(mapped, highlight_mapped, blend);
    }

    // Local contrast enhancement (subtle, avoid halos)
    // Only apply when we have meaningful local average data
    if (local_avg > 0.01 && contrast > 0.0) {
        let detail = lum - local_avg;
        // Reduce contrast enhancement strength in shadows to avoid noise amplification
        let shadow_factor = smoothstep(0.0, 0.2, lum);
        mapped = mapped + detail * contrast * shadow_factor;
    }

    return clamp(mapped, 0.0, 1.0);
}

// Soft clip to avoid harsh clipping at 1.0
fn soft_clip(x: f32) -> f32 {
    if (x <= 1.0) {
        return x;
    }
    // Soft shoulder above 1.0
    return 1.0 + (1.0 - exp(-2.0 * (x - 1.0))) * 0.5;
}

// Linear to sRGB gamma conversion
fn linear_to_srgb(x: f32) -> f32 {
    if (x <= 0.0031308) {
        return x * 12.92;
    }
    return 1.055 * pow(x, 1.0 / 2.4) - 0.055;
}

// sRGB to linear (for reference)
fn srgb_to_linear(x: f32) -> f32 {
    if (x <= 0.04045) {
        return x / 12.92;
    }
    return pow((x + 0.055) / 1.055, 2.4);
}

//=============================================================================
// Main tone mapping kernel
//=============================================================================

@compute @workgroup_size(16, 16)
fn tonemap(@builtin(global_invocation_id) gid: vec3<u32>) {
    let x = gid.x;
    let y = gid.y;

    if (x >= params.width || y >= params.height) {
        return;
    }

    let pixel = load_pixel(x, y);
    let rgb = vec3<f32>(pixel.x, pixel.y, pixel.z);

    // Compute pixel luminance
    let lum = rgb_to_luminance(rgb);

    // Get local average luminance
    let local_avg = get_local_luminance(x, y);

    // Apply tone curve to luminance
    let mapped_lum = apply_tone_curve(lum, local_avg, params.shadow_boost, params.local_contrast);

    // Scale RGB to preserve color (hue/saturation)
    // Use a higher threshold to avoid color shifts in very dark areas
    var out_r: f32;
    var out_g: f32;
    var out_b: f32;

    if (lum > 0.01) {
        // Normal case: scale RGB proportionally to preserve hue
        let scale = mapped_lum / lum;
        out_r = soft_clip(rgb.r * scale);
        out_g = soft_clip(rgb.g * scale);
        out_b = soft_clip(rgb.b * scale);
    } else {
        // Very dark pixels: apply uniform lift to avoid color noise amplification
        // This prevents the red/green splotches in shadows
        out_r = soft_clip(mapped_lum);
        out_g = soft_clip(mapped_lum);
        out_b = soft_clip(mapped_lum);
    }

    // Apply adaptive gamma correction (HDR+ Section 6)
    // For bright scenes (avg_brightness > 0.4), input is likely already gamma-encoded
    // from PNG/DNG files, so skip or reduce gamma to avoid over-brightening
    var final_r: f32;
    var final_g: f32;
    var final_b: f32;

    if (params.avg_brightness > 0.4) {
        // Bright scene: skip gamma, input appears already gamma-encoded
        final_r = out_r;
        final_g = out_g;
        final_b = out_b;
    } else if (params.avg_brightness > 0.2) {
        // Medium scene: blend between linear and gamma-corrected
        let gamma_blend = (0.4 - params.avg_brightness) / 0.2; // 0 at 0.4, 1 at 0.2
        final_r = mix(out_r, linear_to_srgb(out_r), gamma_blend);
        final_g = mix(out_g, linear_to_srgb(out_g), gamma_blend);
        final_b = mix(out_b, linear_to_srgb(out_b), gamma_blend);
    } else {
        // Dark scene: full gamma correction (input is linear)
        final_r = linear_to_srgb(out_r);
        final_g = linear_to_srgb(out_g);
        final_b = linear_to_srgb(out_b);
    }

    // Apply blue noise dithering to prevent banding
    // Uses triangular-PDF dither with decorrelated RGB channels
    if (params.dither_strength > 0.0) {
        let dither_r = triangular_dither(x, y, 0u) * params.dither_strength;
        let dither_g = triangular_dither(x, y, 1u) * params.dither_strength;
        let dither_b = triangular_dither(x, y, 2u) * params.dither_strength;

        final_r = clamp(final_r + dither_r, 0.0, 1.0);
        final_g = clamp(final_g + dither_g, 0.0, 1.0);
        final_b = clamp(final_b + dither_b, 0.0, 1.0);
    }

    store_pixel(x, y, vec4<f32>(final_r, final_g, final_b, pixel.w));
}

//=============================================================================
// Compute local luminance (preprocessing step)
//=============================================================================

struct LocalLumParams {
    width: u32,
    height: u32,
    block_size: u32,
    lum_width: u32,
    lum_height: u32,
    _padding0: u32,
    _padding1: u32,
    _padding2: u32,
}

@group(0) @binding(0)
var<storage, read> lum_input: array<f32>;

@group(0) @binding(1)
var<storage, read_write> lum_output: array<f32>;

@group(0) @binding(2)
var<uniform> lum_params: LocalLumParams;

// Global brightness accumulator (for atomic integer operations)
// Uses fixed-point: multiply by 65536, accumulate, then divide
@group(0) @binding(3)
var<storage, read_write> global_brightness_accum: array<atomic<u32>>;  // [0]=sum, [1]=count

@compute @workgroup_size(16, 16)
fn compute_local_luminance(@builtin(global_invocation_id) gid: vec3<u32>) {
    let lx = gid.x;
    let ly = gid.y;

    if (lx >= lum_params.lum_width || ly >= lum_params.lum_height) {
        return;
    }

    var sum = 0.0;
    var count = 0.0;

    let start_x = lx * lum_params.block_size;
    let start_y = ly * lum_params.block_size;

    for (var dy = 0u; dy < lum_params.block_size; dy++) {
        for (var dx = 0u; dx < lum_params.block_size; dx++) {
            let px = start_x + dx;
            let py = start_y + dy;

            if (px < lum_params.width && py < lum_params.height) {
                let idx = (py * lum_params.width + px) * 4u;
                let r = lum_input[idx];
                let g = lum_input[idx + 1u];
                let b = lum_input[idx + 2u];

                // BT.601 luminance
                let lum = 0.299 * r + 0.587 * g + 0.114 * b;
                sum += lum;
                count += 1.0;
            }
        }
    }

    let avg = select(0.5, sum / count, count > 0.0);
    lum_output[ly * lum_params.lum_width + lx] = avg;

    // Accumulate for global brightness calculation (HDR+ adaptive tone mapping)
    // Use fixed-point: multiply by 65536 to preserve precision in u32
    let fixed_avg = u32(avg * 65536.0);
    atomicAdd(&global_brightness_accum[0], fixed_avg);
    atomicAdd(&global_brightness_accum[1], 1u);
}

//=============================================================================
// Simple gamma-only correction (fast path)
//=============================================================================

@compute @workgroup_size(16, 16)
fn gamma_correct(@builtin(global_invocation_id) gid: vec3<u32>) {
    let x = gid.x;
    let y = gid.y;

    if (x >= params.width || y >= params.height) {
        return;
    }

    let pixel = load_pixel(x, y);

    // Apply sRGB gamma
    let out = vec4<f32>(
        linear_to_srgb(clamp(pixel.x, 0.0, 1.0)),
        linear_to_srgb(clamp(pixel.y, 0.0, 1.0)),
        linear_to_srgb(clamp(pixel.z, 0.0, 1.0)),
        pixel.w
    );

    store_pixel(x, y, out);
}
