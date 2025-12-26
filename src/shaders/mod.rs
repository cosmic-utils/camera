// SPDX-License-Identifier: GPL-3.0-only
//! Shared shader definitions and GPU filter pipeline
//!
//! This module provides the single source of truth for filter implementations.
//! All components (preview, photo capture, virtual camera) use these shared shaders.
//!
//! All filters operate directly on RGBA textures for simplicity and efficiency.

pub mod common;
pub mod depth;
mod gpu_filter;
mod gpu_processor;
pub mod gpu_utils;
pub mod mesh;
pub mod point_cloud;
pub mod yuv_convert;

pub use gpu_processor::{
    CachedDimensions, compute_dispatch_size, create_rgba_storage_texture, create_staging_buffer,
    create_storage_buffer, read_buffer_async, read_texture_async,
};

pub use depth::{DepthProcessor, kinect, unpack_y10b_gpu};
pub use gpu_filter::{GpuFilterPipeline, apply_filter_gpu_rgba, get_gpu_filter_pipeline};
pub use mesh::{MeshProcessor, MeshResult, render_mesh, set_mesh_registration_data};
pub use point_cloud::{
    DepthFormat, PointCloudProcessor, PointCloudResult, RegistrationData,
    get_point_cloud_registration_data, has_point_cloud_registration_data, render_point_cloud,
    set_point_cloud_registration_data,
};
pub use yuv_convert::{
    YuvConvertProcessor, YuvConvertResult, YuvFormat, convert_yuv_to_rgba_gpu,
    convert_yuv_to_rgba_gpu_texture,
};

/// Shared filter functions (WGSL)
/// Contains: luminance(), hash(), apply_filter()
/// Used by: preview shaders, photo capture, virtual camera
pub const FILTER_FUNCTIONS: &str = include_str!("filters.wgsl");
