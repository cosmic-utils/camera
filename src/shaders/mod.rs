// SPDX-License-Identifier: MPL-2.0
//! Shared shader definitions and GPU filter pipeline
//!
//! This module provides the single source of truth for filter implementations.
//! All components (preview, photo capture, virtual camera) use these shared shaders.
//!
//! All filters operate directly on RGBA textures for simplicity and efficiency.

mod gpu_filter;

pub use gpu_filter::{apply_filter_gpu_rgba, get_gpu_filter_pipeline, GpuFilterPipeline};

/// Shared filter functions (WGSL)
/// Contains: luminance(), hash(), apply_filter()
/// Used by: preview shaders, photo capture, virtual camera
pub const FILTER_FUNCTIONS: &str = include_str!("filters.wgsl");
