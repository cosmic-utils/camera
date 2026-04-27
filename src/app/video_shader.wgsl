// SPDX-License-Identifier: GPL-3.0-only
// GPU shader for direct RGBA texture rendering with object-fit: cover support
// Filter functions are prepended by the Rust code from shaders/filters.wgsl

@group(0) @binding(0)
var texture_rgba: texture_2d<f32>;

@group(0) @binding(1)
var sampler_video: sampler;

struct ViewportUniform {
    viewport_size: vec2<f32>,   // Full widget size
    content_fit_mode: f32,      // 0.0 = Contain, 1.0 = Cover (interpolated during animation)
    filter_mode: u32,           // Filter index (0-15)
    corner_radius: f32,         // Corner radius in pixels (0 = no rounding)
    mirror_horizontal: u32,     // 0 = normal, 1 = mirrored horizontally
    uv_offset: vec2<f32>,       // UV offset for scroll clipping (0-1)
    uv_scale: vec2<f32>,        // UV scale for scroll clipping (0-1)
    crop_uv_min: vec2<f32>,     // Crop UV min (u_min, v_min) - normalized 0-1
    crop_uv_max: vec2<f32>,     // Crop UV max (u_max, v_max) - normalized 0-1
    zoom_level: f32,            // Zoom level (1.0 = no zoom, 2.0 = 2x zoom)
    rotation: u32,              // Sensor rotation: 0=None, 1=90CW, 2=180, 3=270CW
    bar_top_height: f32,        // Top bar height in pixels (for contain centering)
    bar_bottom_height: f32,     // Bottom bar height in pixels
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

    // Apply Cover/Contain blend (0.0 = Contain, 1.0 = Cover, intermediate = animating).
    //
    // The effective crop region is itself blended: at 1.0 it degenerates to the
    // full texture (no crop), at 0.0 it is the full aspect-ratio crop.  This keeps
    // the animated transition continuous — no discrete snap at the endpoints even
    // when Cover and Contain would nominally show different regions.
    let blend = viewport.content_fit_mode;
    let effective_crop_min = mix(viewport.crop_uv_min, vec2<f32>(0.0, 0.0), blend);
    let effective_crop_max = mix(viewport.crop_uv_max, vec2<f32>(1.0, 1.0), blend);
    {
        // Get texture dimensions, accounting for rotation
        let raw_tex_size = vec2<f32>(textureDimensions(texture_rgba));
        var tex_size = raw_tex_size;
        if (viewport.rotation == 1u || viewport.rotation == 3u) {
            tex_size = vec2<f32>(raw_tex_size.y, raw_tex_size.x);
        }
        // Effective dimensions after the (blended) crop
        let crop_range = effective_crop_max - effective_crop_min;
        let effective_tex = tex_size * crop_range;

        // Content area between UI bars (for contain centering)
        let content_height = viewport.viewport_size.y - viewport.bar_top_height - viewport.bar_bottom_height;
        let content_center_y = (viewport.bar_top_height + content_height * 0.5) / viewport.viewport_size.y;

        // Zoom levels using the blended effective texture dimensions
        let contain_zoom = min(viewport.viewport_size.x / effective_tex.x, content_height / effective_tex.y);
        let cover_zoom = max(viewport.viewport_size.x / effective_tex.x, viewport.viewport_size.y / effective_tex.y);

        let zoom = mix(contain_zoom, cover_zoom, blend);
        let center_y = mix(content_center_y, 0.5, blend);

        var scale = vec2<f32>(
            viewport.viewport_size.x / (effective_tex.x * zoom),
            viewport.viewport_size.y / (effective_tex.y * zoom),
        );
        // For 90/270 rotations, swap scale factors since we're in rotated UV space
        if (viewport.rotation == 1u || viewport.rotation == 3u) {
            scale = vec2<f32>(scale.y, scale.x);
        }

        tex_coords = (tex_coords - vec2<f32>(0.5, center_y)) * scale + vec2<f32>(0.5, 0.5);
    }

    // Discard letterbox *before* the crop remap.  The intermediate range [0,1]
    // is the rendered image region; outside that range the crop remap can still
    // produce an in-range texture UV (when `crop_min > 0`), so a post-remap check
    // would silently stretch the crop across the letterbox.
    if (tex_coords.x < 0.0 || tex_coords.x > 1.0 || tex_coords.y < 0.0 || tex_coords.y > 1.0) {
        return vec4<f32>(0.0, 0.0, 0.0, 0.0);
    }

    // Apply the blended crop remap.  Uses the same `effective_crop_*` as the zoom
    // above so the [0,1] intermediate maps onto the same region the zoom sized for.
    tex_coords = mix(effective_crop_min, effective_crop_max, tex_coords);

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
        // When used with multi-pass pre-blur, input is already smoothed for clean edges.
        let tex_size = vec2<f32>(textureDimensions(texture_rgba));
        let texel_size = 1.0 / tex_size;
        let edge = sobel_edge_rgba(tex_coords, texel_size);

        // Use smooth edge response for natural pencil pressure variation
        let edge_strength = smoothstep(0.02, 0.25, edge);

        // Invert: dark strokes on light paper
        let pencil = 1.0 - edge_strength;

        // Two-layer paper texture: coarse grain + symmetric fine noise
        let coarse = hash(floor(tex_coords * tex_size * 0.5) * 0.7) * 0.04;
        let fine = (hash(tex_coords * tex_size) - 0.5) * 0.06;
        let paper = 0.96 + coarse + fine;

        let final_val = clamp(pencil * paper, 0.0, 1.0);
        // Slight warm tint for natural paper look
        color = vec3<f32>(final_val, final_val * 0.98, final_val * 0.95);
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
