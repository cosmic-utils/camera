// SPDX-License-Identifier: GPL-3.0-only
// GPU compute shader for YUV to RGBA conversion
//
// Supports multiple YUV formats:
// - NV12: Semi-planar 4:2:0 (Y plane + interleaved UV plane)
// - I420: Planar 4:2:0 (Y + U + V separate planes)
// - YUYV: Packed 4:2:2 (Y0 U Y1 V interleaved)
//
// Uses BT.601 color matrix (standard for webcams and JPEG)

struct ConvertParams {
    width: u32,
    height: u32,
    format: u32,      // 0=RGBA (passthrough), 1=NV12, 2=I420, 3=YUYV
    y_stride: u32,    // Y plane stride in texels (for stride-aware sampling)
    uv_stride: u32,   // UV plane stride in texels
    v_stride: u32,    // V plane stride in texels (I420 only)
    _pad0: u32,       // Padding for 16-byte alignment
    _pad1: u32,
}

// Y plane texture (R8 for planar, RG8 for YUYV packed)
@group(0) @binding(0) var tex_y: texture_2d<f32>;

// UV texture: RG8 for NV12 (interleaved UV), R8 for I420 (U plane only)
@group(0) @binding(1) var tex_uv: texture_2d<f32>;

// V texture: R8 for I420 only (V plane)
@group(0) @binding(2) var tex_v: texture_2d<f32>;

// Output RGBA texture (storage texture for compute shader write)
@group(0) @binding(3) var output: texture_storage_2d<rgba8unorm, write>;

// Conversion parameters
@group(0) @binding(4) var<uniform> params: ConvertParams;

// BT.601 YUV to RGB conversion (standard for webcams and JPEG)
// Input: Y in [0,1], U/V in [0,1] (will be shifted to [-0.5, 0.5])
// Output: RGB in [0,1]
fn yuv_to_rgb_bt601(y: f32, u: f32, v: f32) -> vec3<f32> {
    // BT.601 uses limited range Y [16,235] and UV [16,240]
    // Scale Y from [16/255, 235/255] to [0, 1]
    let y_scaled = (y - 16.0 / 255.0) * (255.0 / 219.0);

    // Shift U/V from [0, 1] to [-0.5, 0.5]
    let u_shifted = u - 0.5;
    let v_shifted = v - 0.5;

    // BT.601 conversion matrix
    let r = y_scaled + 1.402 * v_shifted;
    let g = y_scaled - 0.344136 * u_shifted - 0.714136 * v_shifted;
    let b = y_scaled + 1.772 * u_shifted;

    return clamp(vec3(r, g, b), vec3(0.0), vec3(1.0));
}

// Alternative: BT.709 for HD content (uncomment if needed)
// fn yuv_to_rgb_bt709(y: f32, u: f32, v: f32) -> vec3<f32> {
//     let y_scaled = (y - 16.0 / 255.0) * (255.0 / 219.0);
//     let u_shifted = u - 0.5;
//     let v_shifted = v - 0.5;
//     let r = y_scaled + 1.5748 * v_shifted;
//     let g = y_scaled - 0.1873 * u_shifted - 0.4681 * v_shifted;
//     let b = y_scaled + 1.8556 * u_shifted;
//     return clamp(vec3(r, g, b), vec3(0.0), vec3(1.0));
// }

// Convert NV12 pixel at given position
// NV12: Y plane (full res) + UV plane (half res, interleaved U0V0 U1V1...)
fn convert_nv12(pos: vec2<u32>) -> vec3<f32> {
    // Sample Y at full resolution
    let y = textureLoad(tex_y, pos, 0).r;

    // Sample UV at half resolution (2x2 pixels share same UV)
    let uv_pos = pos / 2u;
    let uv = textureLoad(tex_uv, uv_pos, 0);

    return yuv_to_rgb_bt601(y, uv.r, uv.g);
}

// Convert I420 pixel at given position
// I420: Y plane (full res) + U plane (half res) + V plane (half res)
fn convert_i420(pos: vec2<u32>) -> vec3<f32> {
    // Sample Y at full resolution
    let y = textureLoad(tex_y, pos, 0).r;

    // Sample U and V at half resolution
    let uv_pos = pos / 2u;
    let u = textureLoad(tex_uv, uv_pos, 0).r;
    let v = textureLoad(tex_v, uv_pos, 0).r;

    return yuv_to_rgb_bt601(y, u, v);
}

// Convert YUYV (YUY2) pixel at given position
// YUYV: Packed 4:2:2 - each 4 bytes encode 2 pixels: [Y0 U0 Y1 V0]
// Texture is uploaded as RGBA8 where:
//   R = Y0, G = U, B = Y1, A = V
fn convert_yuyv(pos: vec2<u32>) -> vec3<f32> {
    // Each RGBA texel contains 2 pixels worth of data
    // X position determines which Y to use (even=Y0/R, odd=Y1/B)
    let packed_x = pos.x / 2u;
    let packed = textureLoad(tex_y, vec2(packed_x, pos.y), 0);

    // Select Y0 (R channel) for even pixels, Y1 (B channel) for odd pixels
    let is_odd = (pos.x & 1u) == 1u;
    let y = select(packed.r, packed.b, is_odd);

    // U and V are shared between pixel pairs
    let u = packed.g;
    let v = packed.a;

    return yuv_to_rgb_bt601(y, u, v);
}

// Passthrough for RGBA (or already converted) data
fn passthrough_rgba(pos: vec2<u32>) -> vec4<f32> {
    return textureLoad(tex_y, pos, 0);
}

@compute @workgroup_size(16, 16)
fn main(@builtin(global_invocation_id) global_id: vec3<u32>) {
    let x = global_id.x;
    let y = global_id.y;

    // Bounds check
    if (x >= params.width || y >= params.height) {
        return;
    }

    let pos = vec2(x, y);
    var color: vec4<f32>;

    // Select conversion based on format
    switch params.format {
        case 1u: {
            // NV12
            color = vec4(convert_nv12(pos), 1.0);
        }
        case 2u: {
            // I420
            color = vec4(convert_i420(pos), 1.0);
        }
        case 3u: {
            // YUYV
            color = vec4(convert_yuyv(pos), 1.0);
        }
        default: {
            // RGBA passthrough (format 0 or unknown)
            color = passthrough_rgba(pos);
        }
    }

    // Write to output texture
    textureStore(output, pos, color);
}
