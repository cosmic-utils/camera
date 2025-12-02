// SPDX-License-Identifier: GPL-3.0-only

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
    /// Libcamera backend (for mobile Linux devices)
    Libcamera,
}

// Conversion from CameraBackendType
impl From<crate::backends::camera::CameraBackendType> for PipelineBackend {
    fn from(backend: crate::backends::camera::CameraBackendType) -> Self {
        match backend {
            crate::backends::camera::CameraBackendType::PipeWire => PipelineBackend::PipeWire,
            crate::backends::camera::CameraBackendType::Libcamera => PipelineBackend::Libcamera,
        }
    }
}
