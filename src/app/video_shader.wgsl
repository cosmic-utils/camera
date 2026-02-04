// SPDX-License-Identifier: GPL-3.0-only
// GPU shader for direct RGBA texture rendering with object-fit: cover support
// Filter functions are prepended by the Rust code from shaders/filters.wgsl

@group(0) @binding(0)
var texture_rgba: texture_2d<f32>;

@group(0) @binding(1)
var sampler_video: sampler;

struct ViewportUniform {
    viewport_size: vec2<f32>,   // Full widget size
    content_fit_mode: u32,      // 0 = Contain, 1 = Cover
    filter_mode: u32,           // Filter index (0-15)
    corner_radius: f32,         // Corner radius in pixels (0 = no rounding)
    mirror_horizontal: u32,     // 0 = normal, 1 = mirrored horizontally
    uv_offset: vec2<f32>,       // UV offset for scroll clipping (0-1)
    uv_scale: vec2<f32>,        // UV scale for scroll clipping (0-1)
    crop_uv_min: vec2<f32>,     // Crop UV min (u_min, v_min) - normalized 0-1
    crop_uv_max: vec2<f32>,     // Crop UV max (u_max, v_max) - normalized 0-1
    zoom_level: f32,            // Zoom level (1.0 = no zoom, 2.0 = 2x zoom)
    rotation: u32,              // Sensor rotation: 0=None, 1=90CW, 2=180, 3=270CW
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
    // Apply scroll clipping UV transformation
    // This maps the visible portion's UV (0-1) to the correct portion of the full widget
    var tex_coords = viewport.uv_offset + in.tex_coords * viewport.uv_scale;

    // Apply horizontal mirror if enabled (selfie mode)
    // This happens BEFORE rotation so the mirror is in screen space
    if (viewport.mirror_horizontal == 1u) {
        tex_coords.x = 1.0 - tex_coords.x;
    }

    // Apply rotation correction for sensor orientation
    // Transforms UV coordinates to correct for physical sensor rotation
    // For a sensor mounted N degrees CW, we rotate the UV coords N degrees CW
    // to sample from the correct position in the rotated texture
    if (viewport.rotation == 1u) {
        // 90 CW sensor -> sample rotated 90 CW: (u,v) -> (1-v, u)
        tex_coords = vec2<f32>(1.0 - tex_coords.y, tex_coords.x);
    } else if (viewport.rotation == 2u) {
        // 180 sensor -> rotate 180: (u,v) -> (1-u, 1-v)
        tex_coords = vec2<f32>(1.0 - tex_coords.x, 1.0 - tex_coords.y);
    } else if (viewport.rotation == 3u) {
        // 270 CW sensor -> sample rotated 270 CW: (u,v) -> (v, 1-u)
        tex_coords = vec2<f32>(tex_coords.y, 1.0 - tex_coords.x);
    }

    // Apply crop UV mapping (aspect ratio cropping)
    // Remap tex_coords from 0-1 range to crop_uv_min to crop_uv_max range
    tex_coords = mix(viewport.crop_uv_min, viewport.crop_uv_max, tex_coords);

    // Apply Cover mode adjustment if enabled
    if (viewport.content_fit_mode == 1u) {
        // Get texture dimensions, accounting for rotation
        // For 90/270 degree rotations, swap width and height since UV is already rotated
        let raw_tex_size = vec2<f32>(textureDimensions(texture_rgba));
        var tex_size = raw_tex_size;
        if (viewport.rotation == 1u || viewport.rotation == 3u) {
            tex_size = vec2<f32>(raw_tex_size.y, raw_tex_size.x);
        }

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

        // For 90/270 rotations, we're in rotated UV space where x and y are swapped
        // So swap the scale factors to apply them to the correct axes
        if (viewport.rotation == 1u || viewport.rotation == 3u) {
            scale = vec2<f32>(scale.y, scale.x);
        }

        // Adjust UV coordinates to center and scale the texture
        tex_coords = (tex_coords - vec2<f32>(0.5, 0.5)) * scale + vec2<f32>(0.5, 0.5);
    }

    // Apply digital zoom (center crop)
    // At zoom_level 2.0, show only center 50% of the image
    if (viewport.zoom_level > 1.0) {
        let inv_zoom = 1.0 / viewport.zoom_level;
        tex_coords = (tex_coords - vec2<f32>(0.5, 0.5)) * inv_zoom + vec2<f32>(0.5, 0.5);
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
