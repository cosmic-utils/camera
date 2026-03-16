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

mod gpu_convert;
mod gpu_filter;
mod histogram_pipeline;

pub use gpu_convert::{GpuConvertPipeline, GpuFrameInput, get_gpu_convert_pipeline};
pub use gpu_filter::{GpuFilterPipeline, apply_filter_gpu_rgba, get_gpu_filter_pipeline};
pub use histogram_pipeline::{BrightnessMetrics, analyze_brightness_gpu};

/// Precompile all GPU shader pipelines so the first capture doesn't pay compilation cost.
///
/// Triggers device creation and pipeline compilation for both the convert and filter
/// pipelines. Safe to call from an async task at startup.
pub async fn warmup_gpu_pipelines() -> Result<(), String> {
    // 1. Warm up the convert pipeline (debayer, yuv, awb, unpack, filter)
    {
        let mut guard = get_gpu_convert_pipeline().await?;
        if let Some(pipeline) = guard.as_mut() {
            pipeline.warmup_pipelines();
        }
    } // Drop convert lock before acquiring filter lock

    // 2. Warm up the standalone filter pipeline (triggers device + pipeline creation)
    {
        let _guard = get_gpu_filter_pipeline().await?;
    }

    Ok(())
}

/// Shared filter functions (WGSL)
/// Contains: luminance(), hash(), apply_filter()
/// Used by: preview shaders, photo capture, virtual camera
pub const FILTER_FUNCTIONS: &str = include_str!("filters.wgsl");
