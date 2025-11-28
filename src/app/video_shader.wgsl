// SPDX-License-Identifier: MPL-2.0
// GPU shader for direct RGBA texture rendering with object-fit: cover support
// Filter functions are prepended by the Rust code from shaders/filters.wgsl

@group(0) @binding(0)
var texture_rgba: texture_2d<f32>;

@group(0) @binding(1)
var sampler_video: sampler;

struct ViewportUniform {
    viewport_size: vec2<f32>,
    content_fit_mode: u32,  // 0 = Contain, 1 = Cover
    filter_mode: u32,       // Filter index (0-15)
    corner_radius: f32,     // Corner radius in pixels (0 = no rounding)
    mirror_horizontal: u32, // 0 = normal, 1 = mirrored horizontally
    _padding1: f32,         // Padding for 16-byte alignment
    _padding2: f32,
}

@group(0) @binding(2)
var<uniform> viewport: ViewportUniform;

struct VertexOutput {
    @builtin(position) position: vec4<f32>,
    @location(0) tex_coords: vec2<f32>,
}

// Sample luminance at offset for edge detection (RGBA version)
fn sample_luminance_rgba(uv: vec2<f32>) -> f32 {
    let color = textureSample(texture_rgba, sampler_video, uv);
    return luminance(color.rgb);
}

// Sobel edge detection for pencil effect (RGBA version)
fn sobel_edge_rgba(uv: vec2<f32>, texel_size: vec2<f32>) -> f32 {
    let tl = sample_luminance_rgba(uv + vec2<f32>(-texel_size.x, -texel_size.y));
    let tm = sample_luminance_rgba(uv + vec2<f32>(0.0, -texel_size.y));
    let tr = sample_luminance_rgba(uv + vec2<f32>(texel_size.x, -texel_size.y));
    let ml = sample_luminance_rgba(uv + vec2<f32>(-texel_size.x, 0.0));
    let mr = sample_luminance_rgba(uv + vec2<f32>(texel_size.x, 0.0));
    let bl = sample_luminance_rgba(uv + vec2<f32>(-texel_size.x, texel_size.y));
    let bm = sample_luminance_rgba(uv + vec2<f32>(0.0, texel_size.y));
    let br = sample_luminance_rgba(uv + vec2<f32>(texel_size.x, texel_size.y));

    let gx = -tl - 2.0 * ml - bl + tr + 2.0 * mr + br;
    let gy = -tl - 2.0 * tm - tr + bl + 2.0 * bm + br;

    return sqrt(gx * gx + gy * gy);
}

// Distance from point to rounded rectangle
fn rounded_box_sdf(pos: vec2<f32>, size: vec2<f32>, radius: f32) -> f32 {
    let d = abs(pos) - size + vec2<f32>(radius, radius);
    return min(max(d.x, d.y), 0.0) + length(max(d, vec2<f32>(0.0, 0.0))) - radius;
}

// Vertex shader - creates a fullscreen quad
@vertex
fn vs_main(@builtin(vertex_index) vertex_index: u32) -> VertexOutput {
    var out: VertexOutput;

    // Generate fullscreen triangle vertices
    let x = f32((vertex_index & 1u) << 2u) - 1.0;
    let y = f32((vertex_index & 2u) << 1u) - 1.0;

    out.position = vec4<f32>(x, -y, 0.0, 1.0);
    out.tex_coords = vec2<f32>((x + 1.0) * 0.5, (y + 1.0) * 0.5);

    return out;
}

// Fragment shader - RGBA passthrough with Cover mode support
@fragment
fn fs_main(in: VertexOutput) -> @location(0) vec4<f32> {
    var tex_coords = in.tex_coords;

    // Apply horizontal mirror if enabled (selfie mode)
    if (viewport.mirror_horizontal == 1u) {
        tex_coords.x = 1.0 - tex_coords.x;
    }

    // Apply Cover mode adjustment if enabled
    if (viewport.content_fit_mode == 1u) {
        // Get texture dimensions
        let tex_size = vec2<f32>(textureDimensions(texture_rgba));

        // Calculate aspect ratios
        let tex_aspect = tex_size.x / tex_size.y;
        let viewport_aspect = viewport.viewport_size.x / viewport.viewport_size.y;

        // Calculate scale factor for "cover" behavior
        var scale: vec2<f32>;
        if (tex_aspect > viewport_aspect) {
            // Texture is wider than viewport - fit height, crop sides
            scale = vec2<f32>(viewport_aspect / tex_aspect, 1.0);
        } else {
            // Texture is taller than viewport - fit width, crop top/bottom
            scale = vec2<f32>(1.0, tex_aspect / viewport_aspect);
        }

        // Adjust UV coordinates to center and scale the texture
        tex_coords = (tex_coords - vec2<f32>(0.5, 0.5)) * scale + vec2<f32>(0.5, 0.5);
    }

    // Sample RGBA texture
    var pixel = textureSample(texture_rgba, sampler_video, tex_coords);
    var color = pixel.rgb;

    // Apply filter using shared filter function (filters 0-12)
    if (viewport.filter_mode <= 12u) {
        color = apply_filter(color, viewport.filter_mode, tex_coords);
    } else if (viewport.filter_mode == 13u) {
        // Chromatic Aberration: RGB channel split (needs texture re-sampling)
        let offset_uv = 0.004; // 0.4% of width
        let color_r = textureSample(texture_rgba, sampler_video, tex_coords + vec2<f32>(offset_uv, 0.0));
        let color_b = textureSample(texture_rgba, sampler_video, tex_coords - vec2<f32>(offset_uv, 0.0));
        color = vec3<f32>(color_r.r, color.g, color_b.b);
    } else if (viewport.filter_mode == 14u) {
        // Pencil: Pencil sketch drawing effect (needs texture re-sampling for Sobel)
        let tex_size = vec2<f32>(textureDimensions(texture_rgba));
        let texel_size = 1.0 / tex_size;
        let edge = sobel_edge_rgba(tex_coords, texel_size);

        // Invert edge for pencil lines on white background
        let pencil = 1.0 - edge * 2.0;
        // Add subtle paper texture using shared hash function
        let noise = hash(tex_coords * 500.0) * 0.05;
        let paper = 0.95 + noise;
        let final_val = clamp(pencil * paper, 0.0, 1.0);
        color = vec3<f32>(final_val, final_val, final_val);
    }

    // Calculate alpha for rounded corners
    var alpha = pixel.a;
    if (viewport.corner_radius > 0.0) {
        let pixel_pos = (in.tex_coords - vec2<f32>(0.5, 0.5)) * viewport.viewport_size;
        let half_size = viewport.viewport_size * 0.5;
        let dist = rounded_box_sdf(pixel_pos, half_size, viewport.corner_radius);
        let corner_alpha = 1.0 - smoothstep(-1.0, 1.0, dist);
        alpha = pixel.a * corner_alpha;
    }

    return vec4<f32>(color, alpha);
}
