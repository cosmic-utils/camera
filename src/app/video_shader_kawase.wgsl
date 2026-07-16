// SPDX-License-Identifier: GPL-3.0-only
// Portions Copyright (C) System76, Inc. — derived from cosmic-comp (GPL-3.0-only)
//
// Dual-Kawase blur — a direct port of cosmic-comp's own kernels.
//
// Source of truth:
//   cosmic-comp/src/backend/render/shaders/blur_downsample.frag
//   cosmic-comp/src/backend/render/shaders/blur_upsample.frag
//   cosmic-comp/src/backend/render/wayland/blur_effect.rs::render_blur
//
// This file is a TRANSCRIPTION, not an adaptation. The tap positions, the
// weights and the `sum / sum.a` normalization are upstream's verbatim; the only
// changes are WGSL syntax and how `v_coords` is produced (see below). Please
// keep it that way — the entire point of this rewrite is that our frosted glass
// runs THEIR algorithm with THEIR parameters, so parity is exact by
// construction rather than by a sigma model that has to be re-solved every time
// either kernel is touched.
//
// # How `v_coords` is reproduced
//
// Upstream ping-pongs between TWO full-size textures using progressively
// smaller SUB-RECTS: at down pass `i` the source is the region
// `[0, W>>i] x [0, H>>i]` of the source texture, and the destination is the
// region `[0, W>>(i+1)] x [0, H>>(i+1)]` of the target. It gets there via
// smithay's `render_texture_from_to(src_rect, dst_rect)`, which normalizes
// `v_coords` over the FULL texture.
//
// We have no such helper, so `VideoPipeline::render` sets the render pass
// viewport to the destination sub-rect (making `tex_coords` span 0..1 over it)
// and hands us `uv_scale = src_sub_size / tex_size`. Multiplying gives exactly
// upstream's `v_coords`: normalized over the full texture, covering only the
// live sub-rect. `half_pixel` is likewise `0.5 / src_sub_size` — computed from
// `viewport_size`, which these passes carry the source SUB-RECT size in, not a
// screen size.
//
// # Why the alpha division is load-bearing
//
// Upstream clears each target to TRANSPARENT black and normalizes by `sum.a`
// rather than by the constant tap weight. Since the content is opaque (a = 1)
// and everything outside the live sub-rect is a = 0, this makes taps that fall
// off the region contribute nothing *and* not darken the result — it is the
// edge handling, and it is why the region boundary does not bleed black inward.
// It works because the colour is premultiplied: our pass-0 transform writes
// opaque RGB with a = 1 (letterbox included), where premultiplied and straight
// coincide, and every pass after that preserves the invariant.
//
// Our sampler is ClampToEdge, matching the `TEXTURE_WRAP_S/T = CLAMP_TO_EDGE`
// upstream sets around each of these draws.

@group(0) @binding(0)
var texture_blur: texture_2d<f32>;

@group(0) @binding(1)
var sampler_blur: sampler;

// Mirror of `ViewportUniform`. These passes read only three of its fields:
//
// * `viewport_size` — the SOURCE SUB-RECT size in texels (`W>>i`, `H>>i`), from
//   which `half_pixel = 0.5 / viewport_size` follows exactly as upstream's
//   `0.5 / adjusted_tex_size`.
// * `uv_scale` — `src_sub_size / tex_size`, the full-texture normalization.
// * `kawase_offset` — upstream's `offset / 2^i` (down) or `offset / 2^(passes-i)`
//   (up), already divided down by the Rust side.
struct ViewportUniform {
    viewport_size: vec2<f32>,
    content_fit_mode: f32,
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
    kawase_offset: f32,
    dim_factor: f32,
    letterbox_color: vec4<f32>,
    panel_rect: vec4<f32>,
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

// blur_downsample.frag: a centre tap of weight 4 plus four diagonal taps of
// weight 1 at (±u, ±u). Total weight 8.
@fragment
fn fs_down(in: VertexOutput) -> @location(0) vec4<f32> {
    let v_coords = in.tex_coords * viewport.uv_scale;
    let half_pixel = vec2<f32>(0.5, 0.5) / viewport.viewport_size;
    let offset = viewport.kawase_offset;

    var sum = textureSample(texture_blur, sampler_blur, v_coords) * 4.0;
    sum += textureSample(texture_blur, sampler_blur, v_coords - half_pixel * offset);
    sum += textureSample(texture_blur, sampler_blur, v_coords + half_pixel * offset);
    sum += textureSample(
        texture_blur,
        sampler_blur,
        v_coords + vec2<f32>(half_pixel.x, -half_pixel.y) * offset,
    );
    sum += textureSample(
        texture_blur,
        sampler_blur,
        v_coords - vec2<f32>(half_pixel.x, -half_pixel.y) * offset,
    );

    if (sum.a == 0.0) {
        return vec4<f32>(0.0, 0.0, 0.0, 0.0);
    }
    return sum / sum.a;
}

// blur_upsample.frag: 8 taps — weight 1 at (±2h, 0) and (0, ±2h), weight 2 at
// (±h, ±h). Total weight 12.
@fragment
fn fs_up(in: VertexOutput) -> @location(0) vec4<f32> {
    let v_coords = in.tex_coords * viewport.uv_scale;
    let half_pixel = vec2<f32>(0.5, 0.5) / viewport.viewport_size;
    let offset = viewport.kawase_offset;

    var sum = textureSample(
        texture_blur,
        sampler_blur,
        v_coords + vec2<f32>(-half_pixel.x * 2.0, 0.0) * offset,
    );
    sum += textureSample(
        texture_blur,
        sampler_blur,
        v_coords + vec2<f32>(-half_pixel.x, half_pixel.y) * offset,
    ) * 2.0;
    sum += textureSample(
        texture_blur,
        sampler_blur,
        v_coords + vec2<f32>(0.0, half_pixel.y * 2.0) * offset,
    );
    sum += textureSample(
        texture_blur,
        sampler_blur,
        v_coords + vec2<f32>(half_pixel.x, half_pixel.y) * offset,
    ) * 2.0;
    sum += textureSample(
        texture_blur,
        sampler_blur,
        v_coords + vec2<f32>(half_pixel.x * 2.0, 0.0) * offset,
    );
    sum += textureSample(
        texture_blur,
        sampler_blur,
        v_coords + vec2<f32>(half_pixel.x, -half_pixel.y) * offset,
    ) * 2.0;
    sum += textureSample(
        texture_blur,
        sampler_blur,
        v_coords + vec2<f32>(0.0, -half_pixel.y * 2.0) * offset,
    );
    sum += textureSample(
        texture_blur,
        sampler_blur,
        v_coords + vec2<f32>(-half_pixel.x, -half_pixel.y) * offset,
    ) * 2.0;

    if (sum.a == 0.0) {
        return vec4<f32>(0.0, 0.0, 0.0, 0.0);
    }
    return sum / sum.a;
}
