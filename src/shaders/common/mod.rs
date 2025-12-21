// SPDX-License-Identifier: GPL-3.0-only

//! Shared shader utilities
//!
//! Common WGSL functions shared between point cloud and mesh rendering shaders.
//! These are concatenated with shader-specific code at compile time.

mod params;

pub use params::Render3DParams;

/// Shared geometry functions for 3D rendering
///
/// Includes:
/// - `rotation_matrix(pitch, yaw)` - Creates rotation matrix from Euler angles
/// - `unproject(u, v, depth, cx, cy, fx, fy)` - Projects 2D pixel to 3D point
/// - `project_to_screen(point, view_distance, fov, width, height)` - Perspective projection
/// - `unpack_rgba(packed)` - Unpacks RGBA from u32
pub const GEOMETRY_FUNCTIONS: &str = include_str!("geometry.wgsl");
