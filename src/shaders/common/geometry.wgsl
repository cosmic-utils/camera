// SPDX-License-Identifier: GPL-3.0-only
//
// Shared 3D Geometry Utilities
// ============================
//
// These functions are concatenated into point_cloud.wgsl and mesh.wgsl at build time.
// They require a `params` struct to be defined with the following fields:
//   fx, fy, cx, cy: Camera intrinsics
//   output_width, output_height: Render dimensions
//   view_distance: Camera Z position
//   fov: Field of view

// Create rotation matrix from pitch and yaw
fn rotation_matrix(pitch: f32, yaw: f32) -> mat3x3<f32> {
    let cp = cos(pitch);
    let sp = sin(pitch);
    let cy = cos(yaw);
    let sy = sin(yaw);

    // Combined rotation: first yaw (around Y), then pitch (around X)
    return mat3x3<f32>(
        vec3<f32>(cy, 0.0, sy),
        vec3<f32>(sp * sy, cp, -sp * cy),
        vec3<f32>(-cp * sy, sp, cp * cy)
    );
}

// Project 2D pixel + depth to 3D point
// Note: Y is negated to convert from image coordinates (Y down) to 3D (Y up)
fn unproject(u: f32, v: f32, depth: f32) -> vec3<f32> {
    let x = (u - params.cx) * depth / params.fx;
    let y = -(v - params.cy) * depth / params.fy;
    let z = depth;
    return vec3<f32>(x, y, z);
}

// Apply perspective projection to get screen coordinates
fn project_to_screen(point: vec3<f32>) -> vec3<f32> {
    // Camera position: view_distance moves INTO the scene
    let camera_z = params.view_distance;
    let p = vec3<f32>(point.x, point.y, point.z - camera_z);

    // Perspective division
    if (p.z <= 0.01) {
        return vec3<f32>(-1.0, -1.0, -1.0); // Behind camera
    }

    let aspect = f32(params.output_width) / f32(params.output_height);
    let fov_factor = tan(params.fov * 0.5);

    let screen_x = p.x / (p.z * fov_factor * aspect);
    let screen_y = p.y / (p.z * fov_factor);

    // Convert from [-1, 1] to pixel coordinates
    let px = (screen_x * 0.5 + 0.5) * f32(params.output_width);
    let py = (0.5 - screen_y * 0.5) * f32(params.output_height);

    return vec3<f32>(px, py, p.z);
}

// Unpack RGBA from u32
fn unpack_rgba(packed: u32) -> vec4<f32> {
    let r = f32((packed >> 0u) & 0xFFu) / 255.0;
    let g = f32((packed >> 8u) & 0xFFu) / 255.0;
    let b = f32((packed >> 16u) & 0xFFu) / 255.0;
    let a = f32((packed >> 24u) & 0xFFu) / 255.0;
    return vec4<f32>(r, g, b, a);
}

// Kinect invalid depth marker (1023 in 10-bit, shifted to 16-bit = 65472)
const DEPTH_INVALID_16BIT: u32 = 65472u;
