// SPDX-License-Identifier: MPL-2.0

//! Hardware and software decoder utilities
//!
//! This module provides utilities for detecting and managing video decoders,
//! particularly hardware-accelerated decoders for formats like MJPEG, H.264, etc.

mod hardware;
mod pipeline;

pub use hardware::detect_hw_decoders;
pub use pipeline::try_create_pipeline;

/// Pipeline backend selector
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PipelineBackend {
    /// PipeWire backend (allows simultaneous preview + recording)
    PipeWire,
}

// Conversion from CameraBackendType (for backward compatibility)
impl From<crate::backends::camera::CameraBackendType> for PipelineBackend {
    fn from(_backend: crate::backends::camera::CameraBackendType) -> Self {
        // Only PipeWire is supported
        PipelineBackend::PipeWire
    }
}
