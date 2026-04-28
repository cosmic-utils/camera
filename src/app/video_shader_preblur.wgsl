// SPDX-License-Identifier: GPL-3.0-only
// Lightweight Gaussian blur for filter pre-processing (multi-pass filters)
//
// This is a simple 13-tap blur used as a pre-pass before filters that need
// smooth input (e.g., Pencil edge detection). Much lighter than the transition
// blur shader — just enough to suppress sensor noise for clean spatial operations.
//
// IMPORTANT: The UV transform chain (mirror → rotation → crop → cover → zoom)
// is intentionally duplicated from video_shader.wgsl. This pass bakes all
// transforms into the intermediate texture so the second pass (filter application)
// can use identity transforms. If the transform logic in video_shader.wgsl
// changes, it must be updated here as well.

@group(0) @binding(0)
var texture_source: texture_2d<f32>;

@group(0) @binding(1)
var sampler_source: sampler;

struct ViewportUniform {
    viewport_size: vec2<f32>,
    content_fit_mode: f32,      // 0.0 = Contain, 1.0 = Cover
    filter_mode: u32,
    corner_radius: f32,
    mirror_horizontal: u32,
    uv_offset: vec2<f32>,
    uv_scale: vec2<f32>,
    crop_uv_min: vec2<f32>,
    crop_uv_max: vec2<f32>,
    zoom_level: f32,
    rotation: u32,
    bar_top_height: f32,
    bar_bottom_height: f32,
}

@group(0) @binding(2)
var<uniform> viewport: ViewportUniform;

struct VertexOutput {
    @builtin(position) position: vec4<f32>,
    @location(0) tex_coords: vec2<f32>,
}

@vertex
fn vs_main(@builtin(vertex_index) vertex_index: u32) -> VertexOutput {
    var out: VertexOutput;
    let x = f32((vertex_index & 1u) << 2u) - 1.0;
    let y = f32((vertex_index & 2u) << 1u) - 1.0;
    out.position = vec4<f32>(x, -y, 0.0, 1.0);
    out.tex_coords = vec2<f32>((x + 1.0) * 0.5, (y + 1.0) * 0.5);
    return out;
}

@fragment
fn fs_main(in: VertexOutput) -> @location(0) vec4<f32> {
    // Pass through UV transforms (mirror, rotation, crop) so the intermediate
    // texture contains the correctly oriented, cropped image.
    var uv = viewport.uv_offset + in.tex_coords * viewport.uv_scale;

    if (viewport.mirror_horizontal == 1u) {
        uv.x = 1.0 - uv.x;
    }

    if (viewport.rotation == 1u) {
        uv = vec2<f32>(1.0 - uv.y, uv.x);
    } else if (viewport.rotation == 2u) {
        uv = vec2<f32>(1.0 - uv.x, 1.0 - uv.y);
    } else if (viewport.rotation == 3u) {
        uv = vec2<f32>(uv.y, 1.0 - uv.x);
    }

    // Cover/Contain blend with bar-aware centering and a blended crop region.
    // See video_shader.wgsl for the rationale.
    let blend = viewport.content_fit_mode;
    let effective_crop_min = mix(viewport.crop_uv_min, vec2<f32>(0.0, 0.0), blend);
    let effective_crop_max = mix(viewport.crop_uv_max, vec2<f32>(1.0, 1.0), blend);
    {
        let raw_tex_size = vec2<f32>(textureDimensions(texture_source));
        var tex_size = raw_tex_size;
        if (viewport.rotation == 1u || viewport.rotation == 3u) {
            tex_size = vec2<f32>(raw_tex_size.y, raw_tex_size.x);
        }
        let crop_range = effective_crop_max - effective_crop_min;
        let effective_tex = tex_size * crop_range;
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
        uv = (uv - vec2<f32>(0.5, center_y)) * scale + vec2<f32>(0.5, 0.5);
    }

    // Discard letterbox before the crop remap — see video_shader.wgsl for the
    // rationale (post-remap check can falsely keep fragments inside the crop).
    if (uv.x < 0.0 || uv.x > 1.0 || uv.y < 0.0 || uv.y > 1.0) {
        return vec4<f32>(0.0, 0.0, 0.0, 0.0);
    }

    // Apply digital zoom
    if (viewport.zoom_level > 1.0) {
        let inv_zoom = 1.0 / viewport.zoom_level;
        uv = (uv - vec2<f32>(0.5, 0.5)) * inv_zoom + vec2<f32>(0.5, 0.5);
    }

    // Apply the blended crop remap
    uv = mix(effective_crop_min, effective_crop_max, uv);

    // 13-tap Gaussian blur: center + 4 axis + 4 diagonal + 4 far axis
    // Weights approximate a Gaussian with sigma ~1.4, enough to smooth sensor noise.
    // center(dist=0)=4, axis(dist=1)=2, diagonal(dist=√2)=1, far(dist=2)=0.5
    // Total = 4 + 4*2 + 4*1 + 4*0.5 = 18
    let tex_dims = vec2<f32>(textureDimensions(texture_source));
    let px = 1.0 / tex_dims;

    let c  = textureSample(texture_source, sampler_source, uv).rgb;

    // 4 axis neighbors at distance 1 (weight 2)
    let n  = textureSample(texture_source, sampler_source, uv + vec2<f32>( 0.0, -px.y)).rgb;
    let s  = textureSample(texture_source, sampler_source, uv + vec2<f32>( 0.0,  px.y)).rgb;
    let e  = textureSample(texture_source, sampler_source, uv + vec2<f32>( px.x,  0.0)).rgb;
    let w  = textureSample(texture_source, sampler_source, uv + vec2<f32>(-px.x,  0.0)).rgb;

    // 4 diagonal neighbors at distance √2 (weight 1)
    let ne = textureSample(texture_source, sampler_source, uv + vec2<f32>( px.x, -px.y)).rgb;
    let nw = textureSample(texture_source, sampler_source, uv + vec2<f32>(-px.x, -px.y)).rgb;
    let se = textureSample(texture_source, sampler_source, uv + vec2<f32>( px.x,  px.y)).rgb;
    let sw = textureSample(texture_source, sampler_source, uv + vec2<f32>(-px.x,  px.y)).rgb;

    // 4 far axis at distance 2 (weight 0.5)
    let n2 = textureSample(texture_source, sampler_source, uv + vec2<f32>( 0.0, -2.0 * px.y)).rgb;
    let s2 = textureSample(texture_source, sampler_source, uv + vec2<f32>( 0.0,  2.0 * px.y)).rgb;
    let e2 = textureSample(texture_source, sampler_source, uv + vec2<f32>( 2.0 * px.x,  0.0)).rgb;
    let w2 = textureSample(texture_source, sampler_source, uv + vec2<f32>(-2.0 * px.x,  0.0)).rgb;

    let blurred = (c * 4.0
        + (n + s + e + w) * 2.0
        + (ne + nw + se + sw)
        + (n2 + s2 + e2 + w2) * 0.5
    ) / 18.0;

    return vec4<f32>(blurred, 1.0);
}
