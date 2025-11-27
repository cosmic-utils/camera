// SPDX-License-Identifier: MPL-2.0
// Compute shader for efficient NV12 to RGB conversion on GPU
//
// This shader converts NV12 format (semi-planar YUV 4:2:0) to RGB8
// using the BT.601 color space standard.

@group(0) @binding(0)
var texture_y: texture_2d<f32>;

@group(0) @binding(1)
var texture_uv: texture_2d<f32>;

@group(0) @binding(2)
var output: texture_storage_2d<rgba8unorm, write>;

// Workgroup size: 8x8 threads per workgroup
// Each thread processes one pixel
@compute @workgroup_size(8, 8, 1)
fn main(@builtin(global_invocation_id) global_id: vec3<u32>) {
    let dimensions = textureDimensions(texture_y);
    let pixel_coord = vec2<i32>(global_id.xy);

    // Bounds check
    if (pixel_coord.x >= i32(dimensions.x) || pixel_coord.y >= i32(dimensions.y)) {
        return;
    }

    // Sample Y value (full resolution)
    let y = textureLoad(texture_y, pixel_coord, 0).r;

    // Sample UV values (half resolution, so divide coords by 2)
    let uv_coord = vec2<i32>(pixel_coord.x / 2, pixel_coord.y / 2);
    let uv = textureLoad(texture_uv, uv_coord, 0).rg;

    // Convert from normalized [0,1] to YUV value ranges
    // Y: 16-235, U/V: 16-240 (but we work in normalized space)
    let y_val = y;
    let u_val = uv.r - 0.5;
    let v_val = uv.g - 0.5;

    // BT.601 YUV to RGB conversion matrix
    // R = Y + 1.402 * V
    // G = Y - 0.344 * U - 0.714 * V
    // B = Y + 1.772 * U
    let r = y_val + 1.402 * v_val;
    let g = y_val - 0.344 * u_val - 0.714 * v_val;
    let b = y_val + 1.772 * u_val;

    // Clamp and write output (alpha = 1.0)
    let rgb = vec4<f32>(
        clamp(r, 0.0, 1.0),
        clamp(g, 0.0, 1.0),
        clamp(b, 0.0, 1.0),
        1.0
    );

    textureStore(output, pixel_coord, rgb);
}
