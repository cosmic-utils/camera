// SPDX-License-Identifier: GPL-3.0-only

//! GPU-accelerated point cloud rendering for 3D depth visualization
//!
//! This module provides GPU-based point cloud rendering from depth + RGB data,
//! creating an interactive 3D view that can be rotated with mouse input.

mod processor;

pub use processor::{
    DepthFormat, PointCloudProcessor, PointCloudResult, RegistrationData,
    get_point_cloud_registration_data, has_point_cloud_registration_data, render_point_cloud,
    set_point_cloud_registration_data,
};

use std::sync::OnceLock;

/// Shared geometry functions (rotation_matrix, unproject, project_to_screen, unpack_rgba)
const GEOMETRY_WGSL: &str = include_str!("../common/geometry.wgsl");

/// Point cloud shader main entry points
const POINT_CLOUD_MAIN_WGSL: &str = include_str!("point_cloud_main.wgsl");

/// Combined point cloud shader (geometry + main)
static POINT_CLOUD_SHADER_COMBINED: OnceLock<String> = OnceLock::new();

/// Get the combined point cloud shader source
pub fn point_cloud_shader() -> &'static str {
    POINT_CLOUD_SHADER_COMBINED
        .get_or_init(|| format!("{}\n\n{}", POINT_CLOUD_MAIN_WGSL, GEOMETRY_WGSL))
}
