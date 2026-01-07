// SPDX-License-Identifier: GPL-3.0-only

//! GPU-accelerated YUV to RGBA conversion
//!
//! This module provides compute shader-based conversion of YUV 4:2:2 formats
//! to RGBA, keeping the output on GPU for efficient display or further processing.

mod processor;

pub use processor::{
    YuvConvertProcessor, YuvConvertResult, YuvFormat, convert_yuv_to_rgba_gpu,
    convert_yuv_to_rgba_gpu_texture,
};
