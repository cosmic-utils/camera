// SPDX-License-Identifier: GPL-3.0-only
//! Shared shader definitions and GPU pipelines
//!
//! This module provides the single source of truth for shader implementations.
//! All components (preview, photo capture, virtual camera) use these shared shaders.
//!
//! ## Pipelines
//!
//! - **YUV Convert**: Converts YUV frames (NV12, I420, YUYV) to RGBA on GPU
//! - **GPU Filter**: Applies visual filters (sepia, mono, etc.) to RGBA frames
//! - **Histogram**: Analyzes brightness distribution for exposure metering
//!
//! All pipelines operate on RGBA textures for uniform downstream processing.

mod gpu_filter;
mod histogram_pipeline;
mod yuv_convert;

pub use gpu_filter::{GpuFilterPipeline, apply_filter_gpu_rgba, get_gpu_filter_pipeline};
pub use histogram_pipeline::{BrightnessMetrics, analyze_brightness_gpu};
pub use yuv_convert::{YuvConvertPipeline, YuvFrameInput, get_yuv_convert_pipeline};

/// Shared filter functions (WGSL)
/// Contains: luminance(), hash(), apply_filter()
/// Used by: preview shaders, photo capture, virtual camera
pub const FILTER_FUNCTIONS: &str = include_str!("filters.wgsl");
