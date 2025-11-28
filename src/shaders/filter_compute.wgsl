// SPDX-License-Identifier: MPL-2.0
// GPU compute shader for applying filters to images
// Used by photo capture and virtual camera for GPU-accelerated filtering
// Filter functions are prepended by the Rust code from filters.wgsl

struct FilterParams {
    width: u32,
    height: u32,
    filter_mode: u32,
    _padding: u32,
}

@group(0) @binding(0)
var input_texture: texture_2d<f32>;

@group(0) @binding(1)
var<storage, read_write> output_buffer: array<u32>;

@group(0) @binding(2)
var<uniform> params: FilterParams;

@group(0) @binding(3)
var tex_sampler: sampler;

// Sample luminance at offset for edge detection
fn sample_luminance_at(uv: vec2<f32>) -> f32 {
    let color = textureSampleLevel(input_texture, tex_sampler, uv, 0.0);
    return luminance(color.rgb);
}

// Sobel edge detection for pencil effect
fn sobel_edge(uv: vec2<f32>, texel_size: vec2<f32>) -> f32 {
    let tl = sample_luminance_at(uv + vec2<f32>(-texel_size.x, -texel_size.y));
    let tm = sample_luminance_at(uv + vec2<f32>(0.0, -texel_size.y));
    let tr = sample_luminance_at(uv + vec2<f32>(texel_size.x, -texel_size.y));
    let ml = sample_luminance_at(uv + vec2<f32>(-texel_size.x, 0.0));
    let mr = sample_luminance_at(uv + vec2<f32>(texel_size.x, 0.0));
    let bl = sample_luminance_at(uv + vec2<f32>(-texel_size.x, texel_size.y));
    let bm = sample_luminance_at(uv + vec2<f32>(0.0, texel_size.y));
    let br = sample_luminance_at(uv + vec2<f32>(texel_size.x, texel_size.y));

    let gx = -tl - 2.0 * ml - bl + tr + 2.0 * mr + br;
    let gy = -tl - 2.0 * tm - tr + bl + 2.0 * bm + br;

    return sqrt(gx * gx + gy * gy);
}

@compute @workgroup_size(16, 16)
fn main(@builtin(global_invocation_id) global_id: vec3<u32>) {
    let x = global_id.x;
    let y = global_id.y;

    if (x >= params.width || y >= params.height) {
        return;
    }

    let tex_coords = vec2<f32>(f32(x) + 0.5, f32(y) + 0.5) / vec2<f32>(f32(params.width), f32(params.height));
    let texel_size = 1.0 / vec2<f32>(f32(params.width), f32(params.height));

    // Sample input
    let pixel = textureSampleLevel(input_texture, tex_sampler, tex_coords, 0.0);
    var color = pixel.rgb;

    // Apply filter
    if (params.filter_mode <= 12u) {
        // Use shared filter function for filters 0-12
        color = apply_filter(color, params.filter_mode, tex_coords);
    } else if (params.filter_mode == 13u) {
        // Chromatic Aberration: RGB channel split
        let offset_uv = 0.004;
        let color_r = textureSampleLevel(input_texture, tex_sampler, tex_coords + vec2<f32>(offset_uv, 0.0), 0.0);
        let color_b = textureSampleLevel(input_texture, tex_sampler, tex_coords - vec2<f32>(offset_uv, 0.0), 0.0);
        color = vec3<f32>(color_r.r, color.g, color_b.b);
    } else if (params.filter_mode == 14u) {
        // Pencil: Pencil sketch drawing effect
        let edge = sobel_edge(tex_coords, texel_size);
        let pencil = 1.0 - edge * 2.0;
        let noise = hash(tex_coords * 500.0) * 0.05;
        let paper = 0.95 + noise;
        let final_val = clamp(pencil * paper, 0.0, 1.0);
        color = vec3<f32>(final_val, final_val, final_val);
    }

    // Pack RGBA into u32 (RGBA8 format)
    let r = u32(clamp(color.r, 0.0, 1.0) * 255.0);
    let g = u32(clamp(color.g, 0.0, 1.0) * 255.0);
    let b = u32(clamp(color.b, 0.0, 1.0) * 255.0);
    let a = u32(pixel.a * 255.0);

    let packed = r | (g << 8u) | (b << 16u) | (a << 24u);

    // Write to output buffer
    let idx = y * params.width + x;
    output_buffer[idx] = packed;
}
