// SPDX-License-Identifier: MPL-2.0

//! Media processing utilities for encoding, decoding, and color conversion
//!
//! This module provides low-level media processing capabilities used by
//! the camera pipelines:
//!
//! # Color Space Conversion
//!
//! Camera frames arrive in NV12 format (YUV 4:2:0), which must be converted
//! to RGB for display and photo saving. The [`nv12_converter`] module provides
//! GPU-accelerated conversion using wgpu compute shaders, with CPU fallback.
//!
//! # Video Encoding
//!
//! The [`encoders`] module handles video and audio encoding for recording:
//! - **Video**: H.264/H.265 with hardware acceleration (VA-API, NVENC)
//! - **Audio**: AAC encoding with configurable quality
//!
//! # Format Detection
//!
//! The [`formats`] module provides codec metadata and format conversion utilities
//! for working with various pixel formats (NV12, MJPEG, YUYV, etc.).
//!
//! # Modules
//!
//! - [`decoders`]: Hardware decoder detection and pipeline creation
//! - [`encoders`]: Video/audio encoder selection and configuration
//! - [`formats`]: Codec metadata and format conversion utilities
//! - [`nv12_converter`]: GPU-accelerated NV12 to RGB conversion

pub mod decoders;
pub mod encoders;
pub mod formats;
pub mod nv12_converter;

// Re-export commonly used types
pub use decoders::{PipelineBackend, detect_hw_decoders, try_create_pipeline};
pub use formats::Codec;
pub use nv12_converter::convert_nv12_to_rgb;
