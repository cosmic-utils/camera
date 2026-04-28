// SPDX-License-Identifier: GPL-3.0-only
// GPU shader for Gaussian blur (for multi-pass blur transitions)

@group(0) @binding(0)
var texture_blur: texture_2d<f32>;

@group(0) @binding(1)
var sampler_blur: sampler;

struct ViewportUniform {
    viewport_size: vec2<f32>,   // Full widget size
    content_fit_mode: f32,      // 0.0 = Contain, 1.0 = Cover
    filter_mode: u32,           // Filter index (applied in Pass 1, 0 = none in later passes)
    corner_radius: f32,         // Unused in blur
    mirror_horizontal: u32,     // 0 = normal, 1 = mirrored horizontally
    uv_offset: vec2<f32>,       // UV offset for scroll clipping (0-1)
    uv_scale: vec2<f32>,        // UV scale for scroll clipping (0-1)
    crop_uv_min: vec2<f32>,     // Crop UV min (u_min, v_min) - normalized 0-1
    crop_uv_max: vec2<f32>,     // Crop UV max (u_max, v_max) - normalized 0-1
    zoom_level: f32,            // Unused in blur, but kept for struct compatibility
    rotation: u32,              // Sensor rotation: 0=None, 1=90CW, 2=180, 3=270CW
    bar_top_height: f32,        // Top bar height in pixels
    bar_bottom_height: f32,     // Bottom bar height in pixels
}

@group(0) @binding(2)
var<uniform> viewport: ViewportUniform;

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

// Fragment shader - Gaussian blur on RGB texture
@fragment
fn fs_main(in: VertexOutput) -> @location(0) vec4<f32> {
    // Apply scroll clipping UV transformation
    var tex_coords = viewport.uv_offset + in.tex_coords * viewport.uv_scale;

    // Apply horizontal mirror if enabled (selfie mode)
    // This happens BEFORE rotation so the mirror is in screen space
    if (viewport.mirror_horizontal == 1u) {
        tex_coords.x = 1.0 - tex_coords.x;
    }

    // Apply rotation correction for sensor orientation
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

    // Cover/Contain blend with bar-aware centering and a blended crop region.
    // See video_shader.wgsl for the rationale — same model is used here so the
    // blur texture tracks the main preview during the fit/fill animation.
    let blend = viewport.content_fit_mode;
    let effective_crop_min = mix(viewport.crop_uv_min, vec2<f32>(0.0, 0.0), blend);
    let effective_crop_max = mix(viewport.crop_uv_max, vec2<f32>(1.0, 1.0), blend);
    {
        let raw_tex_size = vec2<f32>(textureDimensions(texture_blur));
        var tex_size_dim = raw_tex_size;
        if (viewport.rotation == 1u || viewport.rotation == 3u) {
            tex_size_dim = vec2<f32>(raw_tex_size.y, raw_tex_size.x);
        }
        let crop_range = effective_crop_max - effective_crop_min;
        let effective_tex = tex_size_dim * crop_range;

        let content_height = viewport.viewport_size.y - viewport.bar_top_height - viewport.bar_bottom_height;
        let content_center_y = (viewport.bar_top_height + content_height * 0.5) / viewport.viewport_size.y;
        let contain_zoom = min(viewport.viewport_size.x / effective_tex.x, content_height / effective_tex.y);
        let cover_zoom = max(viewport.viewport_size.x / effective_tex.x, viewport.viewport_size.y / effective_tex.y);
        let zoom = mix(contain_zoom, cover_zoom, blend);
        let center_y = mix(content_center_y, 0.5, blend);
        var scale = vec2<f32>(
            viewport.viewport_size.x / (effective_tex.x * zoom),
            viewport.viewport_size.y / (effective_tex.y * zoom),
        );

        if (viewport.rotation == 1u || viewport.rotation == 3u) {
            scale = vec2<f32>(scale.y, scale.x);
        }

        tex_coords = (tex_coords - vec2<f32>(0.5, center_y)) * scale + vec2<f32>(0.5, 0.5);
    }

    // Discard letterbox before the crop remap — see video_shader.wgsl for the
    // rationale (post-remap check can falsely keep fragments inside the crop).
    if (tex_coords.x < 0.0 || tex_coords.x > 1.0 || tex_coords.y < 0.0 || tex_coords.y > 1.0) {
        return vec4<f32>(0.0, 0.0, 0.0, 0.0);
    }

    // Apply the blended crop remap
    tex_coords = mix(effective_crop_min, effective_crop_max, tex_coords);

    // Get texture dimensions
    let tex_size = textureDimensions(texture_blur);

    // Blur settings — runs on 1/4 resolution textures so a moderate radius
    // gives a strong visual blur. 3 rings × 12 samples + center = 37 samples.

    let blur_radius = 16.0;
    let samples = 12;

    let pixel_step = vec2<f32>(1.0 / f32(tex_size.x), 1.0 / f32(tex_size.y));

    var rgb_sum = vec3<f32>(0.0, 0.0, 0.0);
    var weight_sum = 0.0;

    let sigma = blur_radius / 2.5;
    let sigma_squared_2 = 2.0 * sigma * sigma;

    let angle_step = 6.28318530718 / f32(samples);
    let golden_angle = 2.399963229728653;

    for (var ring = 1; ring <= 3; ring++) {
        let ring_factor = f32(ring) / 3.0;
        let radius = blur_radius * ring_factor;

        let ring_offset = f32(ring - 1) * golden_angle;

        for (var i = 0; i < samples; i++) {
            let angle = f32(i) * angle_step + ring_offset;
            let offset_tex = vec2<f32>(
                cos(angle) * radius * pixel_step.x,
                sin(angle) * radius * pixel_step.y,
            );
            let rgb = textureSample(texture_blur, sampler_blur, tex_coords + offset_tex).rgb;

            let dist_squared = radius * radius;
            let weight = exp(-dist_squared / sigma_squared_2);

            rgb_sum += rgb * weight;
            weight_sum += weight;
        }
    }

    // Center sample
    let center_rgb = textureSample(texture_blur, sampler_blur, tex_coords).rgb;
    rgb_sum += center_rgb;
    weight_sum += 1.0;

    // Normalize by total weight
    var rgb_val = rgb_sum / weight_sum;

    // Apply filter if enabled (Pass 1 only — later passes have filter_mode=0)
    if (viewport.filter_mode > 0u && viewport.filter_mode <= 12u) {
        rgb_val = apply_filter(rgb_val, viewport.filter_mode, tex_coords);
    }

    // Apply slight darkening for subtle transition indication
    return vec4<f32>(
        clamp(rgb_val.r * 0.85, 0.0, 1.0),
        clamp(rgb_val.g * 0.85, 0.0, 1.0),
        clamp(rgb_val.b * 0.85, 0.0, 1.0),
        1.0
    );
}
