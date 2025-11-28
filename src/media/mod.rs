// SPDX-License-Identifier: MPL-2.0

//! Media processing utilities for encoding, decoding, and format handling
//!
//! This module provides low-level media processing capabilities used by
//! the camera pipelines:
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
//! for working with various pixel formats (RGBA, MJPEG, YUYV, etc.).
//!
//! # Modules
//!
//! - [`decoders`]: Hardware decoder detection and pipeline creation
//! - [`encoders`]: Video/audio encoder selection and configuration
//! - [`formats`]: Codec metadata and format conversion utilities

pub mod decoders;
pub mod encoders;
pub mod formats;

// Re-export commonly used types
pub use decoders::{PipelineBackend, detect_hw_decoders, try_create_pipeline};
pub use formats::Codec;
