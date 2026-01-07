// SPDX-License-Identifier: GPL-3.0-only

#![cfg(target_arch = "x86_64")]

//! GPU-accelerated mesh rendering for 3D depth visualization
//!
//! Uses grid-based triangulation with depth discontinuity handling to create
//! a triangulated mesh from depth camera data with RGB texture colors.

mod processor;

pub use processor::{MeshProcessor, MeshResult, render_mesh, set_mesh_registration_data};

use std::sync::OnceLock;

/// Shared geometry functions (rotation_matrix, unproject, project_to_screen, unpack_rgba)
const GEOMETRY_WGSL: &str = include_str!("../common/geometry.wgsl");

/// Shared filter functions (luminance, hash, apply_filter)
const FILTERS_WGSL: &str = include_str!("../filters.wgsl");

/// Mesh shader main entry points
const MESH_MAIN_WGSL: &str = include_str!("mesh_main.wgsl");

/// Combined mesh shader (geometry + filters + main)
static MESH_SHADER_COMBINED: OnceLock<String> = OnceLock::new();

/// Get the combined mesh shader source
pub fn mesh_shader() -> &'static str {
    MESH_SHADER_COMBINED.get_or_init(|| {
        format!(
            "{}\n\n{}\n\n{}",
            MESH_MAIN_WGSL, GEOMETRY_WGSL, FILTERS_WGSL
        )
    })
}
