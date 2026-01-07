// SPDX-License-Identifier: GPL-3.0-only

//! Unified 3D rendering parameters
//!
//! Shared parameter struct for point cloud and mesh rendering.
//! These processors share 90% of their parameters, so we unify them.

/// Unified 3D rendering parameters for point cloud and mesh shaders
///
/// This struct is used by both PointCloudProcessor and MeshProcessor.
/// Each processor uses the relevant fields for its rendering mode.
#[repr(C)]
#[derive(Debug, Copy, Clone, bytemuck::Pod, bytemuck::Zeroable)]
pub struct Render3DParams {
    // === Dimensions ===
    /// Depth input width
    pub input_width: u32,
    /// Depth input height
    pub input_height: u32,
    /// Output image width
    pub output_width: u32,
    /// Output image height
    pub output_height: u32,
    /// RGB input width (may differ from depth)
    pub rgb_width: u32,
    /// RGB input height
    pub rgb_height: u32,

    // === Camera intrinsics ===
    /// Focal length X (pixels)
    pub fx: f32,
    /// Focal length Y (pixels)
    pub fy: f32,
    /// Principal point X (pixels)
    pub cx: f32,
    /// Principal point Y (pixels)
    pub cy: f32,

    // === Depth format and conversion ===
    /// Depth format: 0 = millimeters, 1 = disparity (10-bit shifted to 16-bit)
    pub depth_format: u32,
    /// Depth conversion coefficient A (for disparity: 1/depth = raw * A + B)
    pub depth_coeff_a: f32,
    /// Depth conversion coefficient B
    pub depth_coeff_b: f32,
    /// Minimum valid depth (meters)
    pub min_depth: f32,
    /// Maximum valid depth (meters)
    pub max_depth: f32,

    // === View transform ===
    /// Rotation around X axis (radians)
    pub pitch: f32,
    /// Rotation around Y axis (radians)
    pub yaw: f32,
    /// Field of view (radians)
    pub fov: f32,
    /// Camera distance from scene center
    pub view_distance: f32,

    // === Registration parameters ===
    /// Whether to use registration lookup tables (1) or simple shift (0)
    pub use_registration_tables: u32,
    /// Y offset from pad_info for registration tables
    pub target_offset: u32,
    /// Fixed-point scale factor (typically 256)
    pub reg_x_val_scale: i32,
    /// Mirror horizontally (1 = yes, 0 = no)
    pub mirror: u32,
    /// X scale factor for high-res RGB (1.0 for 640, 2.0 for 1280)
    pub reg_scale_x: f32,
    /// Y scale factor for high-res RGB
    pub reg_scale_y: f32,
    /// Y offset for high-res (32 for 1280x1024, 0 for 640x480)
    pub reg_y_offset: i32,

    // === Rendering mode-specific ===
    /// Point size (point cloud only, 0.0 for mesh)
    pub point_size: f32,
    /// Depth discontinuity threshold in meters (mesh only, 0.0 for point cloud)
    pub depth_discontinuity_threshold: f32,
    /// Color filter mode (0 = none, 1-14 = various filters)
    pub filter_mode: u32,
}

impl Default for Render3DParams {
    fn default() -> Self {
        Self {
            input_width: 640,
            input_height: 480,
            output_width: 640,
            output_height: 480,
            rgb_width: 640,
            rgb_height: 480,
            // Default Kinect intrinsics
            fx: 594.21,
            fy: 591.04,
            cx: 339.5,
            cy: 242.7,
            depth_format: 0,
            depth_coeff_a: -0.0030711,
            depth_coeff_b: 3.3309495,
            min_depth: 0.4,
            max_depth: 4.0,
            pitch: 0.0,
            yaw: 0.0,
            fov: std::f32::consts::FRAC_PI_4,
            view_distance: 2.0,
            use_registration_tables: 0,
            target_offset: 0,
            reg_x_val_scale: 256,
            mirror: 0,
            reg_scale_x: 1.0,
            reg_scale_y: 1.0,
            reg_y_offset: 0,
            point_size: 2.0,
            depth_discontinuity_threshold: 0.1,
            filter_mode: 0,
        }
    }
}

impl Render3DParams {
    /// Create params for point cloud rendering
    pub fn for_point_cloud(point_size: f32) -> Self {
        Self {
            point_size,
            depth_discontinuity_threshold: 0.0,
            ..Default::default()
        }
    }

    /// Create params for mesh rendering
    pub fn for_mesh(discontinuity_threshold: f32) -> Self {
        Self {
            point_size: 0.0,
            depth_discontinuity_threshold: discontinuity_threshold,
            ..Default::default()
        }
    }
}
