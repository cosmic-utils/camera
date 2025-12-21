// SPDX-License-Identifier: GPL-3.0-only
//
// Mesh rendering compute shader - Main entry points
//
// Requires geometry.wgsl to be prepended for shared functions:
// rotation_matrix, unproject, project_to_screen, unpack_rgba, DEPTH_INVALID_16BIT

struct MeshParams {
    // Depth input dimensions (used for iteration and unprojection)
    input_width: u32,
    input_height: u32,
    // Output dimensions
    output_width: u32,
    output_height: u32,
    // RGB input dimensions (may differ from depth)
    rgb_width: u32,
    rgb_height: u32,
    // Camera intrinsics (Kinect defaults)
    fx: f32,  // Focal length X (594.21 for Kinect)
    fy: f32,  // Focal length Y (591.04 for Kinect)
    cx: f32,  // Principal point X (339.5 for Kinect 640x480)
    cy: f32,  // Principal point Y (242.7 for Kinect 640x480)
    // Depth format flag: 0 = millimeters, 1 = disparity (10-bit shifted to 16-bit)
    depth_format: u32,
    // Depth conversion coefficients (only used for disparity format)
    depth_coeff_a: f32,
    depth_coeff_b: f32,
    min_depth: f32,
    max_depth: f32,
    // Rotation (radians)
    pitch: f32,
    yaw: f32,
    // Rendering
    fov: f32,
    view_distance: f32,
    // Registration parameters
    use_registration_tables: u32,
    target_offset: u32,
    reg_x_val_scale: i32,
    mirror: u32,
    // High-res RGB scaling for registration
    // Registration tables are built for 640x480 RGB (from 1280x960 crop, scaled 0.5x)
    // For 1280x1024: scale both X and Y by 2.0 to get 1280x960, top-aligned in 1024
    reg_scale_x: f32,              // X scale factor (1.0 for 640, 2.0 for 1280)
    reg_scale_y: f32,              // Y scale factor (same as X to maintain aspect)
    reg_y_offset: i32,             // Y offset (0 for top-aligned crop)
    // Mesh-specific parameters
    depth_discontinuity_threshold: f32,  // 0.1m - don't connect points with larger depth diff
}

// Input: RGB data (RGBA format)
@group(0) @binding(0)
var<storage, read> input_rgb: array<u32>;

// Input: Depth data (16-bit values stored in u32)
@group(0) @binding(1)
var<storage, read> input_depth: array<u32>;

// Output: Rendered mesh image
@group(0) @binding(2)
var output_texture: texture_storage_2d<rgba8unorm, write>;

// Depth buffer for z-ordering (atomic min for nearest point)
@group(0) @binding(3)
var<storage, read_write> depth_buffer: array<atomic<u32>>;

// Parameters
@group(0) @binding(4)
var<uniform> params: MeshParams;

// Registration table: 640*480 [x_scaled, y] pairs for depth-RGB alignment
@group(0) @binding(5)
var<storage, read> registration_table: array<vec2<i32>>;

// Depth-to-RGB shift table: 10001 i32 values indexed by depth in mm (0-10000)
@group(0) @binding(6)
var<storage, read> depth_to_rgb_shift: array<i32>;

// Get depth in meters for a pixel (returns -1 for invalid)
fn get_depth_meters(x: u32, y: u32) -> f32 {
    if (x >= params.input_width || y >= params.input_height) {
        return -1.0;
    }

    let pixel_idx = y * params.input_width + x;
    let depth_u16 = input_depth[pixel_idx] & 0xFFFFu;

    var depth_m: f32;

    if (params.depth_format == 0u) {
        if (depth_u16 == 0u || depth_u16 >= 10000u) {
            return -1.0;
        }
        depth_m = f32(depth_u16) / 1000.0;
    } else {
        if (depth_u16 >= DEPTH_INVALID_16BIT) {
            return -1.0;
        }
        let depth_raw = f32(depth_u16 >> 6u);
        let denom = depth_raw * params.depth_coeff_a + params.depth_coeff_b;
        if (denom <= 0.01) {
            return -1.0;
        }
        depth_m = 1.0 / denom;
    }

    if (depth_m < params.min_depth || depth_m > params.max_depth) {
        return -1.0;
    }

    return depth_m;
}

// Get RGB color for a depth pixel
fn get_color(x: u32, y: u32, depth_m: f32) -> vec4<f32> {
    var rgb_x: u32;
    var rgb_y: u32;

    if (params.use_registration_tables == 1u) {
        let reg_idx = y * params.input_width + x;
        let reg = registration_table[reg_idx];

        let depth_mm = u32(depth_m * 1000.0);
        let clamped_depth_mm = clamp(depth_mm, 0u, 10000u);
        let shift = depth_to_rgb_shift[clamped_depth_mm];

        // Calculate base RGB coordinates (in 640x480 space)
        let rgb_x_scaled = reg.x + shift;
        let rgb_x_base = rgb_x_scaled / params.reg_x_val_scale;
        let rgb_y_base = reg.y - i32(params.target_offset);

        // Scale to actual RGB resolution if different from base 640x480
        let rgb_x_i = i32(f32(rgb_x_base) * params.reg_scale_x);
        let rgb_y_i = i32(f32(rgb_y_base) * params.reg_scale_y) + params.reg_y_offset;

        if (rgb_x_i < 0 || rgb_x_i >= i32(params.rgb_width) ||
            rgb_y_i < 0 || rgb_y_i >= i32(params.rgb_height)) {
            return vec4<f32>(0.5, 0.5, 0.5, 1.0);
        }

        rgb_x = u32(rgb_x_i);
        rgb_y = u32(rgb_y_i);
    } else {
        var rgb_x_f = f32(x);
        var rgb_y_f = f32(y);

        if (params.rgb_width != params.input_width || params.rgb_height != params.input_height) {
            rgb_x_f = rgb_x_f * f32(params.rgb_width) / f32(params.input_width);
            rgb_y_f = rgb_y_f * f32(params.rgb_height) / f32(params.input_height);
        }

        rgb_x = u32(clamp(rgb_x_f, 0.0, f32(params.rgb_width - 1u)));
        rgb_y = u32(clamp(rgb_y_f, 0.0, f32(params.rgb_height - 1u)));
    }

    let rgb_idx = rgb_y * params.rgb_width + rgb_x;
    return unpack_rgba(input_rgb[rgb_idx]);
}

// Transform 3D point with rotation and mirroring
fn transform_point(point: vec3<f32>) -> vec3<f32> {
    var p = point;

    // Apply horizontal mirror if enabled
    if (params.mirror == 1u) {
        p.x = -p.x;
    }

    // Apply rotation around scene center
    let rotation_center = 1.5;
    p.z -= rotation_center;
    let rot = rotation_matrix(params.pitch, params.yaw);
    p = rot * p;
    p.z += rotation_center;

    return p;
}

// Compute barycentric coordinates for a point relative to a triangle
fn barycentric(p: vec2<f32>, a: vec2<f32>, b: vec2<f32>, c: vec2<f32>) -> vec3<f32> {
    let v0 = c - a;
    let v1 = b - a;
    let v2 = p - a;

    let dot00 = dot(v0, v0);
    let dot01 = dot(v0, v1);
    let dot02 = dot(v0, v2);
    let dot11 = dot(v1, v1);
    let dot12 = dot(v1, v2);

    let inv_denom = 1.0 / (dot00 * dot11 - dot01 * dot01);
    let u = (dot11 * dot02 - dot01 * dot12) * inv_denom;
    let v = (dot00 * dot12 - dot01 * dot02) * inv_denom;

    return vec3<f32>(1.0 - u - v, v, u);
}

// Convert depth to integer for atomic comparison
fn depth_to_int(depth: f32) -> u32 {
    return u32((1.0 - depth / (params.max_depth * 2.0)) * 4294967295.0);
}

// Rasterize a single triangle
fn rasterize_triangle(
    s0: vec3<f32>, s1: vec3<f32>, s2: vec3<f32>,
    c0: vec4<f32>, c1: vec4<f32>, c2: vec4<f32>,
) {
    // Skip degenerate or behind-camera triangles
    if (s0.z < 0.0 || s1.z < 0.0 || s2.z < 0.0) {
        return;
    }

    // Compute bounding box
    let min_x = max(0, i32(floor(min(s0.x, min(s1.x, s2.x)))));
    let max_x = min(i32(params.output_width) - 1, i32(ceil(max(s0.x, max(s1.x, s2.x)))));
    let min_y = max(0, i32(floor(min(s0.y, min(s1.y, s2.y)))));
    let max_y = min(i32(params.output_height) - 1, i32(ceil(max(s0.y, max(s1.y, s2.y)))));

    // Skip if bounding box is empty or off-screen
    if (min_x > max_x || min_y > max_y) {
        return;
    }

    // Limit triangle size to avoid excessive iteration (max 64x64 pixels)
    let max_size = 64;
    if (max_x - min_x > max_size || max_y - min_y > max_size) {
        return;
    }

    // Rasterize using barycentric coordinates
    for (var py = min_y; py <= max_y; py = py + 1) {
        for (var px = min_x; px <= max_x; px = px + 1) {
            let p = vec2<f32>(f32(px) + 0.5, f32(py) + 0.5);

            // Compute barycentric coordinates
            let bary = barycentric(p, s0.xy, s1.xy, s2.xy);

            // Check if point is inside triangle (all barycentric coords >= 0)
            if (bary.x >= 0.0 && bary.y >= 0.0 && bary.z >= 0.0) {
                // Interpolate depth
                let depth = bary.x * s0.z + bary.y * s1.z + bary.z * s2.z;

                // Interpolate color
                let color = bary.x * c0 + bary.y * c1 + bary.z * c2;

                // Atomic depth test
                let depth_int = depth_to_int(depth);
                let idx = u32(py) * params.output_width + u32(px);
                let old = atomicMax(&depth_buffer[idx], depth_int);

                if (depth_int >= old) {
                    textureStore(output_texture, vec2<i32>(px, py), color);
                }
            }
        }
    }
}

@compute @workgroup_size(16, 16)
fn main(@builtin(global_invocation_id) global_id: vec3<u32>) {
    let x = global_id.x;
    let y = global_id.y;

    // Each thread processes the bottom-right corner of a 2x2 quad
    // Skip first row/column (need top-left neighbor)
    if (x == 0u || y == 0u || x >= params.input_width || y >= params.input_height) {
        return;
    }

    // Get depths of 2x2 quad: (x-1,y-1), (x,y-1), (x-1,y), (x,y)
    let d00 = get_depth_meters(x - 1u, y - 1u);
    let d10 = get_depth_meters(x, y - 1u);
    let d01 = get_depth_meters(x - 1u, y);
    let d11 = get_depth_meters(x, y);

    // Skip if any depth is invalid
    if (d00 < 0.0 || d10 < 0.0 || d01 < 0.0 || d11 < 0.0) {
        return;
    }

    // Check depth discontinuity - don't connect across large depth jumps
    let max_diff = max(max(abs(d00 - d10), abs(d00 - d01)),
                       max(abs(d11 - d10), abs(d11 - d01)));
    if (max_diff > params.depth_discontinuity_threshold) {
        return;
    }

    // Unproject all 4 corners to 3D and transform
    let p00 = transform_point(unproject(f32(x - 1u), f32(y - 1u), d00));
    let p10 = transform_point(unproject(f32(x), f32(y - 1u), d10));
    let p01 = transform_point(unproject(f32(x - 1u), f32(y), d01));
    let p11 = transform_point(unproject(f32(x), f32(y), d11));

    // Get colors for each corner
    let c00 = get_color(x - 1u, y - 1u, d00);
    let c10 = get_color(x, y - 1u, d10);
    let c01 = get_color(x - 1u, y, d01);
    let c11 = get_color(x, y, d11);

    // Project to screen
    let s00 = project_to_screen(p00);
    let s10 = project_to_screen(p10);
    let s01 = project_to_screen(p01);
    let s11 = project_to_screen(p11);

    // Rasterize two triangles for the quad
    // Triangle 1: (00, 10, 01)
    rasterize_triangle(s00, s10, s01, c00, c10, c01);
    // Triangle 2: (10, 11, 01)
    rasterize_triangle(s10, s11, s01, c10, c11, c01);
}

// Clear pass: clear depth buffer and output texture
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
