// SPDX-License-Identifier: GPL-3.0-only
//
// YUV 4:2:2 to RGBA compute shader
//
// Converts YUYV or UYVY packed YUV data to RGBA on the GPU.
// The output texture stays on GPU for further processing or display.
//
// YUV 4:2:2 formats pack 2 pixels into 4 bytes:
// - YUYV: Y0, U, Y1, V (used by Kinect)
// - UYVY: U, Y0, V, Y1

// Conversion parameters
struct Params {
    // Image dimensions
    width: u32,
    height: u32,
    // Format: 0 = YUYV, 1 = UYVY
    format: u32,
    // Padding for alignment
    _pad: u32,
}

@group(0) @binding(0) var<uniform> params: Params;
// Input: packed YUV data (2 bytes per pixel = width * height * 2 bytes total)
// Stored as u32 array (4 bytes = 2 pixels worth of YUV data)
@group(0) @binding(1) var<storage, read> input_yuv: array<u32>;
// Output: RGBA texture
@group(0) @binding(2) var output_rgba: texture_storage_2d<rgba8unorm, write>;

// ITU-R BT.601 YUV to RGB conversion
// R = 1.164*(Y-16) + 1.596*(V-128)
// G = 1.164*(Y-16) - 0.813*(V-128) - 0.391*(U-128)
// B = 1.164*(Y-16) + 2.018*(U-128)
fn yuv_to_rgb(y: i32, u: i32, v: i32) -> vec3<f32> {
    // Scale factors (multiplied by 256 for integer math, then divided)
    let c = (y - 16) * 298;
    let d = u - 128;
    let e = v - 128;

    let r = (c + 409 * e + 128) >> 8;
    let g = (c - 100 * d - 208 * e + 128) >> 8;
    let b = (c + 516 * d + 128) >> 8;

    return vec3<f32>(
        clamp(f32(r) / 255.0, 0.0, 1.0),
        clamp(f32(g) / 255.0, 0.0, 1.0),
        clamp(f32(b) / 255.0, 0.0, 1.0)
    );
}

@compute @workgroup_size(16, 16, 1)
fn main(@builtin(global_invocation_id) global_id: vec3<u32>) {
    let x = global_id.x;
    let y = global_id.y;

    // Each thread processes 2 horizontal pixels (one YUYV/UYVY macro-pixel)
    let pixel_x = x * 2u;

    // Bounds check
    if (pixel_x >= params.width || y >= params.height) {
        return;
    }

    // Calculate input index
    // Each u32 contains 4 bytes = one macro-pixel (2 pixels worth)
    let input_idx = y * (params.width / 2u) + x;

    if (input_idx >= arrayLength(&input_yuv)) {
        return;
    }

    // Read 4 bytes as u32
    let packed = input_yuv[input_idx];

    // Extract bytes based on format
    let byte0 = i32((packed >> 0u) & 0xFFu);
    let byte1 = i32((packed >> 8u) & 0xFFu);
    let byte2 = i32((packed >> 16u) & 0xFFu);
    let byte3 = i32((packed >> 24u) & 0xFFu);

    var y0: i32;
    var u: i32;
    var y1: i32;
    var v: i32;

    if (params.format == 0u) {
        // YUYV format: Y0, U, Y1, V
        y0 = byte0;
        u = byte1;
        y1 = byte2;
        v = byte3;
    } else {
        // UYVY format: U, Y0, V, Y1
        u = byte0;
        y0 = byte1;
        v = byte2;
        y1 = byte3;
    }

    // Convert both pixels
    let rgb0 = yuv_to_rgb(y0, u, v);
    let rgb1 = yuv_to_rgb(y1, u, v);

    // Write to output texture
    textureStore(output_rgba, vec2<i32>(i32(pixel_x), i32(y)), vec4<f32>(rgb0, 1.0));

    // Only write second pixel if within bounds
    if (pixel_x + 1u < params.width) {
        textureStore(output_rgba, vec2<i32>(i32(pixel_x) + 1, i32(y)), vec4<f32>(rgb1, 1.0));
    }
}
