// SPDX-License-Identifier: GPL-3.0-only
//
// Point cloud rendering compute shader - Main entry points
//
// Requires geometry.wgsl to be prepended for shared functions:
// rotation_matrix, unproject, project_to_screen, unpack_rgba, DEPTH_INVALID_16BIT

// Unified 3D rendering parameters (shared layout with mesh shader)
struct Render3DParams {
    // === Dimensions ===
    input_width: u32,      // Depth input width
    input_height: u32,     // Depth input height
    output_width: u32,     // Output image width
    output_height: u32,    // Output image height
    rgb_width: u32,        // RGB input width (may differ from depth)
    rgb_height: u32,       // RGB input height

    // === Camera intrinsics ===
    fx: f32,  // Focal length X (594.21 for Kinect)
    fy: f32,  // Focal length Y (591.04 for Kinect)
    cx: f32,  // Principal point X (339.5 for Kinect 640x480)
    cy: f32,  // Principal point Y (242.7 for Kinect 640x480)

    // === Depth format and conversion ===
    depth_format: u32,     // 0 = millimeters, 1 = disparity (10-bit shifted to 16-bit)
    depth_coeff_a: f32,    // Disparity coefficient A (-0.0030711 for Kinect)
    depth_coeff_b: f32,    // Disparity coefficient B (3.3309495 for Kinect)
    min_depth: f32,        // Minimum valid depth in meters
    max_depth: f32,        // Maximum valid depth in meters

    // === View transform ===
    pitch: f32,            // Rotation around X axis (radians)
    yaw: f32,              // Rotation around Y axis (radians)
    fov: f32,              // Field of view for perspective projection
    view_distance: f32,    // Camera distance from origin

    // === Registration parameters ===
    use_registration_tables: u32,  // 1 = use lookup tables, 0 = use simple shift
    target_offset: u32,            // Y offset from pad_info
    reg_x_val_scale: i32,          // Fixed-point scale factor (256)
    mirror: u32,                   // 1 = mirror horizontally, 0 = normal
    reg_scale_x: f32,              // X scale factor (1.0 for 640, 2.0 for 1280)
    reg_scale_y: f32,              // Y scale factor
    reg_y_offset: i32,             // Y offset (0 for top-aligned crop)

    // === Mode-specific parameters ===
    point_size: f32,                    // Point size in pixels (point cloud only)
    depth_discontinuity_threshold: f32, // Mesh discontinuity threshold (mesh only, 0 for point cloud)
    filter_mode: u32,                   // Color filter mode (0 = none, 1-14 = various filters)
}

// Input: RGB data (RGBA format)
@group(0) @binding(0)
var<storage, read> input_rgb: array<u32>;

// Input: Depth data (16-bit values stored in u32)
@group(0) @binding(1)
var<storage, read> input_depth: array<u32>;

// Output: Rendered point cloud image
@group(0) @binding(2)
var output_texture: texture_storage_2d<rgba8unorm, write>;

// Depth buffer for z-ordering (atomic min for nearest point)
@group(0) @binding(3)
var<storage, read_write> depth_buffer: array<atomic<u32>>;

// Parameters
@group(0) @binding(4)
var<uniform> params: Render3DParams;

// Registration table: 640*480 [x_scaled, y] pairs for depth-RGB alignment
// x_scaled is multiplied by REG_X_VAL_SCALE (256), y is integer
@group(0) @binding(5)
var<storage, read> registration_table: array<vec2<i32>>;

// Depth-to-RGB shift table: 10001 i32 values indexed by depth in mm (0-10000)
// Values are scaled by REG_X_VAL_SCALE and represent horizontal pixel shift
@group(0) @binding(6)
var<storage, read> depth_to_rgb_shift: array<i32>;

@compute @workgroup_size(16, 16)
fn main(@builtin(global_invocation_id) global_id: vec3<u32>) {
    let x = global_id.x;
    let y = global_id.y;

    // Bounds check for input
    if (x >= params.input_width || y >= params.input_height) {
        return;
    }

    let pixel_idx = y * params.input_width + x;

    // Get depth value (u16 stored in u32)
    let depth_u16 = input_depth[pixel_idx] & 0xFFFFu;

    var depth_m: f32;

    if (params.depth_format == 0u) {
        // Format 0: Depth in millimeters (from native Kinect backend)
        // Skip invalid depth (0 = invalid)
        if (depth_u16 == 0u || depth_u16 >= 10000u) {
            return;
        }
        // Convert millimeters to meters
        depth_m = f32(depth_u16) / 1000.0;
    } else {
        // Format 1: 10-bit disparity shifted to 16-bit (from V4L2 Y10B)
        // Skip invalid depth (1023 << 6 = 65472)
        if (depth_u16 >= DEPTH_INVALID_16BIT) {
            return;
        }
        // Convert back to 10-bit raw value
        let depth_raw = f32(depth_u16 >> 6u);
        // Convert raw disparity to meters using Kinect formula:
        // depth_m = 1.0 / (raw * coeff_a + coeff_b)
        let denom = depth_raw * params.depth_coeff_a + params.depth_coeff_b;
        // Avoid division by zero or negative values
        if (denom <= 0.01) {
            return;
        }
        depth_m = 1.0 / denom;
    }

    // Skip depth outside valid range
    if (depth_m < params.min_depth || depth_m > params.max_depth) {
        return;
    }

    // Get RGB color - apply stereo registration for depth-RGB alignment
    var rgb_x: u32;
    var rgb_y: u32;
    var color: vec4<f32>;

    if (params.use_registration_tables == 1u) {
        // Use proper registration lookup tables from device calibration
        let reg_idx = y * params.input_width + x;
        let reg = registration_table[reg_idx];

        // Convert depth to mm for shift table lookup
        let depth_mm = u32(depth_m * 1000.0);
        let clamped_depth_mm = clamp(depth_mm, 0u, 10000u);
        let shift = depth_to_rgb_shift[clamped_depth_mm];

        // Calculate RGB coordinates using registration formula from libfreenect:
        // rgb_x = (registration_table[idx][0] + depth_to_rgb_shift[depth_mm]) / REG_X_VAL_SCALE
        // rgb_y = registration_table[idx][1] - target_offset
        // These coordinates are in 640x480 space (base registration resolution)
        let rgb_x_scaled = reg.x + shift;
        let rgb_x_base = rgb_x_scaled / params.reg_x_val_scale;
        let rgb_y_base = reg.y - i32(params.target_offset);

        // Scale to actual RGB resolution if different from base 640x480
        // For 1280x1024: scale by 2x and add Y offset for aspect ratio difference
        // The 640x480 RGB is from 1280x1024 cropped to 1280x960 then scaled by 0.5
        // So to go back: scale by 2, then add offset for the cropped rows
        let rgb_x_i = i32(f32(rgb_x_base) * params.reg_scale_x);
        let rgb_y_i = i32(f32(rgb_y_base) * params.reg_scale_y) + params.reg_y_offset;

        // Skip pixels that fall outside the RGB image bounds
        // (registration can push bottom/edge rows beyond RGB dimensions)
        if (rgb_x_i < 0 || rgb_x_i >= i32(params.rgb_width) ||
            rgb_y_i < 0 || rgb_y_i >= i32(params.rgb_height)) {
            return;
        }

        rgb_x = u32(rgb_x_i);
        rgb_y = u32(rgb_y_i);

        let rgb_idx = rgb_y * params.rgb_width + rgb_x;
        color = unpack_rgba(input_rgb[rgb_idx]);
    } else {
        // Fallback: Use simple identity mapping (no registration)
        // This is used when registration data is not available
        var rgb_x_f = f32(x);
        var rgb_y_f = f32(y);

        // Scale to RGB resolution if different from depth
        if (params.rgb_width != params.input_width || params.rgb_height != params.input_height) {
            rgb_x_f = rgb_x_f * f32(params.rgb_width) / f32(params.input_width);
            rgb_y_f = rgb_y_f * f32(params.rgb_height) / f32(params.input_height);
        }

        // Clamp to valid RGB coordinates
        rgb_x = u32(clamp(rgb_x_f, 0.0, f32(params.rgb_width - 1u)));
        rgb_y = u32(clamp(rgb_y_f, 0.0, f32(params.rgb_height - 1u)));

        let rgb_idx = rgb_y * params.rgb_width + rgb_x;
        color = unpack_rgba(input_rgb[rgb_idx]);
    }

    // Unproject to 3D
    var point_3d = unproject(f32(x), f32(y), depth_m);

    // Apply horizontal mirror if enabled
    if (params.mirror == 1u) {
        point_3d.x = -point_3d.x;
    }

    // Apply rotation around scene center (typical viewing distance ~1.5m for Kinect)
    // This allows rotating the view while keeping the scene centered
    let rotation_center = 1.5;  // Rotate around 1.5m depth
    point_3d.z -= rotation_center;
    let rot = rotation_matrix(params.pitch, params.yaw);
    point_3d = rot * point_3d;
    point_3d.z += rotation_center;

    // Project to screen
    let screen = project_to_screen(point_3d);

    // Check if point is visible
    if (screen.x < 0.0 || screen.x >= f32(params.output_width) ||
        screen.y < 0.0 || screen.y >= f32(params.output_height) ||
        screen.z < 0.0) {
        return;
    }

    let screen_x = u32(screen.x);
    let screen_y = u32(screen.y);
    let out_idx = screen_y * params.output_width + screen_x;

    // Convert depth to integer for atomic comparison (closer = smaller z = larger integer)
    let depth_int = u32((1.0 - screen.z / (params.max_depth * 2.0)) * 4294967295.0);

    // Atomic depth test - only render if this point is closer
    let old_depth = atomicMax(&depth_buffer[out_idx], depth_int);

    if (depth_int >= old_depth) {
        // This point is closest (or equal), render it
        // Apply color filter if enabled (filter_mode 1-12)
        var final_color = color;
        if (params.filter_mode > 0u && params.filter_mode <= 12u) {
            let tex_coords = vec2<f32>(
                f32(screen_x) / f32(params.output_width),
                f32(screen_y) / f32(params.output_height)
            );
            final_color = vec4<f32>(apply_filter(color.rgb, params.filter_mode, tex_coords), color.a);
        }
        textureStore(output_texture, vec2<i32>(i32(screen_x), i32(screen_y)), final_color);
    }
}

// Second pass: clear depth buffer and optionally fill holes
@compute @workgroup_size(16, 16)
fn clear_buffers(@builtin(global_invocation_id) global_id: vec3<u32>) {
    let x = global_id.x;
    let y = global_id.y;

    if (x >= params.output_width || y >= params.output_height) {
        return;
    }

    let idx = y * params.output_width + x;
    atomicStore(&depth_buffer[idx], 0u);

    // Clear output to dark background
    textureStore(output_texture, vec2<i32>(i32(x), i32(y)), vec4<f32>(0.1, 0.1, 0.1, 1.0));
}
