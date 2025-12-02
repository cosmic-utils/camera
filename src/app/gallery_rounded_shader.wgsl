// SPDX-License-Identifier: GPL-3.0-only
// Shader for rendering images with rounded corners

@group(0) @binding(0)
var texture_img: texture_2d<f32>;

@group(0) @binding(1)
var sampler_img: sampler;

// Uniform struct containing viewport size and corner radius
struct ViewportParams {
    size: vec2<f32>,
    corner_radius: f32,
    _padding: f32, // Padding for 16-byte alignment
}

@group(0) @binding(2)
var<uniform> viewport: ViewportParams;

struct VertexOutput {
    @builtin(position) position: vec4<f32>,
    @location(0) tex_coords: vec2<f32>,
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

// Distance from point to rounded rectangle
fn rounded_box_sdf(pos: vec2<f32>, size: vec2<f32>, radius: f32) -> f32 {
    let d = abs(pos) - size + vec2<f32>(radius, radius);
    return min(max(d.x, d.y), 0.0) + length(max(d, vec2<f32>(0.0, 0.0))) - radius;
}

// Fragment shader - renders image with rounded corners and cover fit
@fragment
fn fs_main(in: VertexOutput) -> @location(0) vec4<f32> {
    // Get texture dimensions
    let tex_size = vec2<f32>(textureDimensions(texture_img));

    // Calculate aspect ratios
    let tex_aspect = tex_size.x / tex_size.y;
    let viewport_aspect = viewport.size.x / viewport.size.y;

    // Calculate scale factor for "cover" behavior (like CSS object-fit: cover)
    // Scale so the image fills the viewport, cropping the overflow
    var scale: vec2<f32>;
    if (tex_aspect > viewport_aspect) {
        // Texture is wider than viewport - fit height, crop sides
        scale = vec2<f32>(viewport_aspect / tex_aspect, 1.0);
    } else {
        // Texture is taller than viewport - fit width, crop top/bottom
        scale = vec2<f32>(1.0, tex_aspect / viewport_aspect);
    }

    // Adjust UV coordinates to center and scale the texture (cover fit)
    let adjusted_uv = (in.tex_coords - vec2<f32>(0.5, 0.5)) * scale + vec2<f32>(0.5, 0.5);

    // Convert to pixel coordinates for rounded corner calculation (centered)
    let pixel_pos = (in.tex_coords - vec2<f32>(0.5, 0.5)) * viewport.size;

    // Calculate distance to rounded rectangle using corner radius from uniform
    let half_size = viewport.size * 0.5;
    let dist = rounded_box_sdf(pixel_pos, half_size, viewport.corner_radius);

    // Sample the texture with adjusted UVs for cover fit
    let color = textureSample(texture_img, sampler_img, adjusted_uv);

    // Apply smooth alpha based on distance (anti-aliasing)
    // Only use the corner alpha, not the image's alpha (photos should be opaque)
    let alpha = 1.0 - smoothstep(-1.0, 1.0, dist);

    return vec4<f32>(color.rgb, alpha);
}
