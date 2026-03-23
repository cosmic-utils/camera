// SPDX-License-Identifier: GPL-3.0-only
// Lightweight Gaussian blur compute shader for filter pre-processing
//
// Used as a pre-pass before spatial filters (e.g., Pencil edge detection).
// Reads from input texture, writes to output texture. 13-tap kernel (~sigma 1.2).

struct BlurParams {
    width: u32,
    height: u32,
    _pad0: u32,
    _pad1: u32,
}

@group(0) @binding(0)
var input_texture: texture_2d<f32>;

@group(0) @binding(1)
var output_texture: texture_storage_2d<rgba8unorm, write>;

@group(0) @binding(2)
var<uniform> params: BlurParams;

@group(0) @binding(3)
var tex_sampler: sampler;

@compute @workgroup_size(16, 16)
fn main(@builtin(global_invocation_id) global_id: vec3<u32>) {
    let x = global_id.x;
    let y = global_id.y;

    if (x >= params.width || y >= params.height) {
        return;
    }

    let uv = vec2<f32>(f32(x) + 0.5, f32(y) + 0.5) / vec2<f32>(f32(params.width), f32(params.height));
    let px = 1.0 / vec2<f32>(f32(params.width), f32(params.height));

    // 13-tap Gaussian blur (~sigma 1.4)
    // center(dist=0)=4, axis(dist=1)=2, diagonal(dist=√2)=1, far(dist=2)=0.5, total=18
    let c  = textureSampleLevel(input_texture, tex_sampler, uv, 0.0).rgb;

    let n  = textureSampleLevel(input_texture, tex_sampler, uv + vec2<f32>( 0.0, -px.y), 0.0).rgb;
    let s  = textureSampleLevel(input_texture, tex_sampler, uv + vec2<f32>( 0.0,  px.y), 0.0).rgb;
    let e  = textureSampleLevel(input_texture, tex_sampler, uv + vec2<f32>( px.x,  0.0), 0.0).rgb;
    let w  = textureSampleLevel(input_texture, tex_sampler, uv + vec2<f32>(-px.x,  0.0), 0.0).rgb;

    let ne = textureSampleLevel(input_texture, tex_sampler, uv + vec2<f32>( px.x, -px.y), 0.0).rgb;
    let nw = textureSampleLevel(input_texture, tex_sampler, uv + vec2<f32>(-px.x, -px.y), 0.0).rgb;
    let se = textureSampleLevel(input_texture, tex_sampler, uv + vec2<f32>( px.x,  px.y), 0.0).rgb;
    let sw = textureSampleLevel(input_texture, tex_sampler, uv + vec2<f32>(-px.x,  px.y), 0.0).rgb;

    let n2 = textureSampleLevel(input_texture, tex_sampler, uv + vec2<f32>( 0.0, -2.0 * px.y), 0.0).rgb;
    let s2 = textureSampleLevel(input_texture, tex_sampler, uv + vec2<f32>( 0.0,  2.0 * px.y), 0.0).rgb;
    let e2 = textureSampleLevel(input_texture, tex_sampler, uv + vec2<f32>( 2.0 * px.x,  0.0), 0.0).rgb;
    let w2 = textureSampleLevel(input_texture, tex_sampler, uv + vec2<f32>(-2.0 * px.x,  0.0), 0.0).rgb;

    let blurred = (c * 4.0
        + (n + s + e + w) * 2.0
        + (ne + nw + se + sw)
        + (n2 + s2 + e2 + w2) * 0.5
    ) / 18.0;

    textureStore(output_texture, vec2<i32>(i32(x), i32(y)), vec4<f32>(blurred, 1.0));
}
