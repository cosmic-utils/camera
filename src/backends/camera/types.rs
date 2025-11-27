// SPDX-License-Identifier: MPL-2.0
// Shared types for camera backend abstraction
#![allow(dead_code)]

//! Shared types for camera backends

use serde::{Deserialize, Serialize};
use std::sync::Arc;
use std::time::Instant;

/// Camera backend type (PipeWire only)
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum CameraBackendType {
    /// PipeWire backend (modern Linux standard)
    PipeWire,
}

impl Default for CameraBackendType {
    fn default() -> Self {
        Self::PipeWire
    }
}

impl std::fmt::Display for CameraBackendType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "PipeWire")
    }
}

/// Represents a camera device
#[derive(Debug, Clone)]
pub struct CameraDevice {
    pub name: String,
    pub path: String,                  // Path to capture device (pipewire node ID)
    pub metadata_path: Option<String>, // Path to metadata/control device or node ID
}

/// Camera format specification
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CameraFormat {
    pub width: u32,
    pub height: u32,
    pub framerate: Option<u32>,     // None for photo mode
    pub hardware_accelerated: bool, // True for MJPEG and raw formats with HW support
    pub pixel_format: String,       // FourCC code (e.g., "MJPG", "H264", "YUYV")
}

impl std::fmt::Display for CameraFormat {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if let Some(fps) = self.framerate {
            write!(f, "{}x{} @ {}fps", self.width, self.height, fps)
        } else {
            write!(f, "{}x{}", self.width, self.height)
        }
    }
}

/// Pixel format for camera frames
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PixelFormat {
    /// NV12 - 4:2:0 semi-planar YUV (1.5 bytes per pixel)
    NV12,
    /// RGBA - 32-bit with alpha (4 bytes per pixel)
    RGBA,
}

/// A single frame from the camera
#[derive(Debug, Clone)]
pub struct CameraFrame {
    pub width: u32,
    pub height: u32,
    pub data: Arc<[u8]>,      // Zero-copy frame data
    pub format: PixelFormat,  // Pixel format of the data
    pub stride_y: u32,        // Row stride for Y plane (bytes per row, may include padding)
    pub stride_uv: u32,       // Row stride for UV plane (bytes per row, may include padding)
    pub offset_uv: usize,     // Byte offset where UV plane starts
    pub captured_at: Instant, // Timestamp when frame was captured (for latency diagnostics)
}

/// Frame receiver type for preview streams
pub type FrameReceiver = cosmic::iced::futures::channel::mpsc::Receiver<CameraFrame>;

/// Frame sender type for preview streams
pub type FrameSender = cosmic::iced::futures::channel::mpsc::Sender<CameraFrame>;

/// Result type for backend operations
pub type BackendResult<T> = Result<T, BackendError>;

/// Error types for backend operations
#[derive(Debug, Clone)]
pub enum BackendError {
    /// Backend is not available on this system
    NotAvailable(String),
    /// Failed to initialize backend
    InitializationFailed(String),
    /// Camera device not found
    DeviceNotFound(String),
    /// Format not supported
    FormatNotSupported(String),
    /// Backend crashed or became unresponsive
    Crashed(String),
    /// Recording already in progress
    RecordingInProgress,
    /// No recording in progress
    NoRecordingInProgress,
    /// General I/O error
    IoError(String),
    /// Other errors
    Other(String),
}

impl std::fmt::Display for BackendError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            BackendError::NotAvailable(msg) => write!(f, "Backend not available: {}", msg),
            BackendError::InitializationFailed(msg) => write!(f, "Initialization failed: {}", msg),
            BackendError::DeviceNotFound(msg) => write!(f, "Device not found: {}", msg),
            BackendError::FormatNotSupported(msg) => write!(f, "Format not supported: {}", msg),
            BackendError::Crashed(msg) => write!(f, "Backend crashed: {}", msg),
            BackendError::RecordingInProgress => write!(f, "Recording already in progress"),
            BackendError::NoRecordingInProgress => write!(f, "No recording in progress"),
            BackendError::IoError(msg) => write!(f, "I/O error: {}", msg),
            BackendError::Other(msg) => write!(f, "Error: {}", msg),
        }
    }
}

impl std::error::Error for BackendError {}
