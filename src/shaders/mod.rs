// SPDX-License-Identifier: GPL-3.0-only
//! Shared shader definitions and GPU filter pipeline
//!
//! This module provides the single source of truth for filter implementations.
//! All components (preview, photo capture, virtual camera) use these shared shaders.
//!
//! All filters operate directly on RGBA textures for simplicity and efficiency.

pub mod common;
// Y10B depth processing requires freedepth for unpacking
#[cfg(all(target_arch = "x86_64", feature = "freedepth"))]
pub mod depth;
mod gpu_filter;
mod gpu_processor;
pub mod gpu_utils;
// Kinect intrinsics are shared across all backends (freedepth and kernel driver)
#[cfg(target_arch = "x86_64")]
pub mod kinect_intrinsics;
// Mesh and point cloud work with both freedepth and kernel driver
#[cfg(target_arch = "x86_64")]
pub mod mesh;
#[cfg(target_arch = "x86_64")]
pub mod point_cloud;
pub mod yuv_convert;

pub use gpu_processor::{CachedDimensions, compute_dispatch_size, read_buffer_async};

#[cfg(all(target_arch = "x86_64", feature = "freedepth"))]
pub use depth::{DepthProcessor, kinect, unpack_y10b_gpu};
pub use gpu_filter::{GpuFilterPipeline, apply_filter_gpu_rgba, get_gpu_filter_pipeline};
#[cfg(target_arch = "x86_64")]
pub use mesh::{MeshProcessor, MeshResult, render_mesh, set_mesh_registration_data};
#[cfg(target_arch = "x86_64")]
pub use point_cloud::{
    DepthFormat, PointCloudProcessor, PointCloudResult, RegistrationData,
    get_point_cloud_registration_data, has_point_cloud_registration_data, render_point_cloud,
    set_point_cloud_registration_data,
};
pub use yuv_convert::{
    YuvConvertProcessor, YuvConvertResult, YuvFormat, convert_yuv_to_rgba_gpu,
    convert_yuv_to_rgba_gpu_texture,
};

// Kinect intrinsics - re-export for non-x86_64 builds
#[cfg(not(target_arch = "x86_64"))]
pub use kinect_intrinsics as kinect;

#[cfg(not(target_arch = "x86_64"))]
pub struct DepthProcessor;

#[cfg(not(target_arch = "x86_64"))]
pub async fn unpack_y10b_gpu(
    _y10b_data: &[u8],
    _width: u32,
    _height: u32,
) -> Result<(Vec<u8>, Vec<u16>), String> {
    Err("freedepth feature not enabled".to_string())
}

#[cfg(not(target_arch = "x86_64"))]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DepthFormat {
    Y10B,
    Z16,
    Millimeters,
    Disparity16,
}

#[cfg(not(target_arch = "x86_64"))]
#[derive(Clone)]
pub struct RegistrationData {
    pub registration_table: Vec<[i32; 2]>,
    pub depth_to_rgb_shift: Vec<i32>,
    pub target_offset: u32,
}

#[cfg(not(target_arch = "x86_64"))]
pub async fn render_point_cloud(
    _rgb_data: &[u8],
    _depth_data: &[u16],
    _rgb_width: u32,
    _rgb_height: u32,
    _depth_width: u32,
    _depth_height: u32,
    _output_width: u32,
    _output_height: u32,
    _pitch: f32,
    _yaw: f32,
    _zoom: f32,
    _depth_format: DepthFormat,
    _mirror: bool,
    _apply_rgb_registration: bool,
    _filter_mode: u32,
) -> Result<PointCloudResult, String> {
    Err("freedepth feature not enabled".to_string())
}

#[cfg(not(target_arch = "x86_64"))]
pub async fn set_point_cloud_registration_data(_data: &RegistrationData) -> Result<(), String> {
    Err("freedepth feature not enabled".to_string())
}

#[cfg(not(target_arch = "x86_64"))]
pub fn has_point_cloud_registration_data() -> bool {
    false
}

#[cfg(not(target_arch = "x86_64"))]
pub async fn get_point_cloud_registration_data() -> Result<Option<RegistrationData>, String> {
    Ok(None)
}

#[cfg(not(target_arch = "x86_64"))]
pub struct PointCloudProcessor;

#[cfg(not(target_arch = "x86_64"))]
pub struct PointCloudResult {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

#[cfg(not(target_arch = "x86_64"))]
pub async fn render_mesh(
    _rgb_data: &[u8],
    _depth_data: &[u16],
    _rgb_width: u32,
    _rgb_height: u32,
    _depth_width: u32,
    _depth_height: u32,
    _output_width: u32,
    _output_height: u32,
    _pitch: f32,
    _yaw: f32,
    _zoom: f32,
    _depth_format: DepthFormat,
    _mirror: bool,
    _apply_rgb_registration: bool,
    _depth_discontinuity_threshold: f32,
    _filter_mode: u32,
) -> Result<MeshResult, String> {
    Err("freedepth feature not enabled".to_string())
}

#[cfg(not(target_arch = "x86_64"))]
pub async fn set_mesh_registration_data(_data: &RegistrationData) -> Result<(), String> {
    Err("freedepth feature not enabled".to_string())
}

#[cfg(not(target_arch = "x86_64"))]
pub struct MeshProcessor;

#[cfg(not(target_arch = "x86_64"))]
pub struct MeshResult {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

/// Shared filter functions (WGSL)
/// Contains: luminance(), hash(), apply_filter()
/// Used by: preview shaders, photo capture, virtual camera
pub const FILTER_FUNCTIONS: &str = include_str!("filters.wgsl");
