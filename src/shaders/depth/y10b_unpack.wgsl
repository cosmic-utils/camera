// SPDX-License-Identifier: GPL-3.0-only
//
// Y10B unpacking compute shader for Kinect depth sensor
//
// Y10B Format:
//   - 10-bit packed grayscale (depth data from Kinect depth sensor)
//   - 4 pixels are packed into 5 bytes (40 bits for 4 x 10-bit values)
//
// Byte layout:
//   Byte 0: P0[9:2]              (bits 9-2 of pixel 0)
//   Byte 1: P0[1:0] | P1[9:4]    (bits 1-0 of pixel 0, bits 9-4 of pixel 1)
//   Byte 2: P1[3:0] | P2[9:6]    (bits 3-0 of pixel 1, bits 9-6 of pixel 2)
//   Byte 3: P2[5:0] | P3[9:8]    (bits 5-0 of pixel 2, bits 9-8 of pixel 3)
//   Byte 4: P3[7:0]              (bits 7-0 of pixel 3)
//
// Outputs:
//   - RGBA texture for preview display (turbo colormap: blue=near, red=far)
//   - 16-bit depth buffer for lossless storage

struct DepthParams {
    width: u32,
    height: u32,
    min_depth: u32,      // Minimum depth value for visualization (usually 0)
    max_depth: u32,      // Maximum depth value for visualization (usually 1023 or 0x3FF)
    use_colormap: u32,   // 0 = grayscale, 1 = turbo colormap
    depth_only: u32,     // 0 = normal, 1 = depth-only mode (always use colormap)
}

// Input: Raw Y10B packed bytes (read as u32 for efficient access)
@group(0) @binding(0)
var<storage, read> input_bytes: array<u32>;

// Output: RGBA texture for preview display
@group(0) @binding(1)
var output_rgba: texture_storage_2d<rgba8unorm, write>;

// Output: 16-bit depth values (stored as u32 for alignment, only lower 16 bits used)
@group(0) @binding(2)
var<storage, read_write> output_depth: array<u32>;

// Uniform parameters
@group(0) @binding(3)
var<uniform> params: DepthParams;

// Extract a single byte from the packed u32 input array
fn get_byte(byte_index: u32) -> u32 {
    let word_index = byte_index / 4u;
    let byte_offset = byte_index % 4u;
    let word = input_bytes[word_index];
    return (word >> (byte_offset * 8u)) & 0xFFu;
}

// Unpack a single 10-bit depth value from Y10B packed data
fn unpack_pixel(pixel_index: u32) -> u32 {
    // Which group of 4 pixels this belongs to
    let group_index = pixel_index / 4u;
    // Position within the group (0-3)
    let in_group_index = pixel_index % 4u;
    // Starting byte for this group
    let byte_offset = group_index * 5u;

    // Get the 5 bytes for this group
    let b0 = get_byte(byte_offset);
    let b1 = get_byte(byte_offset + 1u);
    let b2 = get_byte(byte_offset + 2u);
    let b3 = get_byte(byte_offset + 3u);
    let b4 = get_byte(byte_offset + 4u);

    // Extract 10-bit value based on position within the group
    var depth_10bit: u32;
    if (in_group_index == 0u) {
        // Pixel 0: bits from b0[7:0] (high 8) and b1[7:6] (low 2)
        depth_10bit = (b0 << 2u) | (b1 >> 6u);
    } else if (in_group_index == 1u) {
        // Pixel 1: bits from b1[5:0] (high 6) and b2[7:4] (low 4)
        depth_10bit = ((b1 & 0x3Fu) << 4u) | (b2 >> 4u);
    } else if (in_group_index == 2u) {
        // Pixel 2: bits from b2[3:0] (high 4) and b3[7:2] (low 6)
        depth_10bit = ((b2 & 0x0Fu) << 6u) | (b3 >> 2u);
    } else {
        // Pixel 3: bits from b3[1:0] (high 2) and b4[7:0] (low 8)
        depth_10bit = ((b3 & 0x03u) << 8u) | b4;
    }

    return depth_10bit;
}

// Convert 10-bit depth to normalized value for visualization
fn normalize_depth(depth_10bit: u32) -> f32 {
    // Clamp to visualization range
    let min_d = f32(params.min_depth);
    let max_d = f32(params.max_depth);
    let range = max_d - min_d;

    if (range <= 0.0) {
        return 0.0;
    }

    let depth_f = f32(depth_10bit);
    let normalized = clamp((depth_f - min_d) / range, 0.0, 1.0);

    return normalized;
}

// Turbo-inspired colormap for depth visualization
// Maps 0.0 (near/blue) to 1.0 (far/red) with perceptually uniform colors
// Based on Google's Turbo colormap for scientific visualization
fn turbo_colormap(t: f32) -> vec3<f32> {
    // Clamp input
    let x = clamp(t, 0.0, 1.0);

    // Polynomial approximation of turbo colormap
    // Red channel: starts low, peaks in middle-high
    let r = clamp(
        0.13572138 + x * (4.61539260 + x * (-42.66032258 + x * (132.13108234 + x * (-152.94239396 + x * 59.28637943)))),
        0.0, 1.0
    );

    // Green channel: peaks in middle
    let g = clamp(
        0.09140261 + x * (2.19418839 + x * (4.84296658 + x * (-14.18503333 + x * (4.27729857 + x * 2.82956604)))),
        0.0, 1.0
    );

    // Blue channel: starts high, decreases
    let b = clamp(
        0.10667330 + x * (12.64194608 + x * (-60.58204836 + x * (110.36276771 + x * (-89.90310912 + x * 27.34824973)))),
        0.0, 1.0
    );

    return vec3<f32>(r, g, b);
}

// Special value indicating no depth data (saturated/invalid)
// This matches freedepth::DEPTH_10BIT_NO_VALUE (1023 = 0x3FF for 10-bit data)
const DEPTH_NO_VALUE: u32 = 1023u;

@compute @workgroup_size(16, 16)
fn main(@builtin(global_invocation_id) global_id: vec3<u32>) {
    let x = global_id.x;
    let y = global_id.y;

    // Bounds check
    if (x >= params.width || y >= params.height) {
        return;
    }

    // Calculate pixel index
    let pixel_index = y * params.width + x;

    // Unpack the 10-bit depth value
    let depth_10bit = unpack_pixel(pixel_index);

    // Store 16-bit depth value (shift left by 6 to use full 16-bit range)
    // This preserves full precision: 10-bit -> 16-bit
    output_depth[pixel_index] = depth_10bit << 6u;

    // Determine if colormap should be used
    // depth_only mode always uses colormap, otherwise check use_colormap flag
    let should_use_colormap = (params.depth_only == 1u) || (params.use_colormap == 1u);

    // Handle invalid depth (no data)
    if (depth_10bit >= DEPTH_NO_VALUE) {
        if (should_use_colormap) {
            // Black for invalid pixels when using colormap
            textureStore(output_rgba, vec2<i32>(i32(x), i32(y)), vec4<f32>(0.0, 0.0, 0.0, 1.0));
        } else {
            // Dark gray for invalid pixels in grayscale mode
            textureStore(output_rgba, vec2<i32>(i32(x), i32(y)), vec4<f32>(0.1, 0.1, 0.1, 1.0));
        }
        return;
    }

    // Convert to normalized value
    var normalized = normalize_depth(depth_10bit);

    // Apply colormap or grayscale based on mode
    var color: vec3<f32>;
    if (should_use_colormap) {
        // In depth_only mode, quantize to discrete bands for pure depth visualization
        // This removes the fine texture from the IR pattern
        if (params.depth_only == 1u) {
            // Quantize to 32 depth bands for smooth color regions
            let bands = 32.0;
            normalized = floor(normalized * bands) / bands;
        }
        // Apply turbo colormap (near=blue, far=red)
        color = turbo_colormap(normalized);
    } else {
        // Grayscale output (near=bright, far=dark)
        // In Kinect raw data: high values = near, low values = far
        color = vec3<f32>(normalized, normalized, normalized);
    }

    // Write RGBA output
    textureStore(output_rgba, vec2<i32>(i32(x), i32(y)), vec4<f32>(color, 1.0));
}
