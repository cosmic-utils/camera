// SPDX-License-Identifier: GPL-3.0-only
// GPU compute shader for planar YUV to RGBA conversion
//
// Supports any chroma subsampling (4:2:0, 4:2:2, 4:4:4, etc.) by
// deriving UV coordinates from the actual texture dimensions.
// Uses BT.601 color matrix (standard for webcams and JPEG)

struct ConvertParams {
    width: u32,
    height: u32,
    _pad0: u32,
    _pad1: u32,
}

@group(0) @binding(0) var tex_y: texture_2d<f32>;
@group(0) @binding(1) var tex_u: texture_2d<f32>;
@group(0) @binding(2) var tex_v: texture_2d<f32>;
@group(0) @binding(3) var output: texture_storage_2d<rgba8unorm, write>;
@group(0) @binding(4) var<uniform> params: ConvertParams;

fn yuv_to_rgb_bt601(y: f32, u: f32, v: f32) -> vec3<f32> {
    let y_scaled = (y - 16.0 / 255.0) * (255.0 / 219.0);
    let u_shifted = u - 0.5;
    let v_shifted = v - 0.5;
    let r = y_scaled + 1.402 * v_shifted;
    let g = y_scaled - 0.344136 * u_shifted - 0.714136 * v_shifted;
    let b = y_scaled + 1.772 * u_shifted;
    return clamp(vec3(r, g, b), vec3(0.0), vec3(1.0));
}

@compute @workgroup_size(16, 16)
fn main(@builtin(global_invocation_id) global_id: vec3<u32>) {
    let x = global_id.x;
    let y = global_id.y;

    if (x >= params.width || y >= params.height) {
        return;
    }

    let pos = vec2(x, y);

    // Sample Y at full resolution
    let luma = textureLoad(tex_y, pos, 0).r;

    // Scale UV coordinates based on actual texture dimensions.
    // This handles all subsampling types automatically:
    //   4:2:0  UV is half-width, half-height  → pos * uv_dim / y_dim
    //   4:2:2  UV is half-width, full-height  → scales x only
    //   4:4:4  UV is full-width, full-height  → no scaling
    let y_dim = textureDimensions(tex_y);
    let uv_dim = textureDimensions(tex_u);
    let uv_pos = vec2(x * uv_dim.x / y_dim.x, y * uv_dim.y / y_dim.y);
    let u = textureLoad(tex_u, uv_pos, 0).r;
    let v = textureLoad(tex_v, uv_pos, 0).r;

    let rgb = yuv_to_rgb_bt601(luma, u, v);
    textureStore(output, pos, vec4(rgb, 1.0));
}
