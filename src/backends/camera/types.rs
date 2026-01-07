// SPDX-License-Identifier: GPL-3.0-only
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

/// Device information from V4L2 capability
#[derive(Debug, Clone, Default)]
pub struct DeviceInfo {
    /// Name of the device (V4L2 card)
    pub card: String,
    /// Driver name (V4L2 driver)
    pub driver: String,
    /// Device path (e.g., /dev/video0)
    pub path: String,
    /// Real device path (resolved symlinks)
    pub real_path: String,
}

/// Represents a camera device
#[derive(Debug, Clone)]
pub struct CameraDevice {
    pub name: String,
    pub path: String,                    // Path to capture device (pipewire node ID)
    pub metadata_path: Option<String>,   // Path to metadata/control device or node ID
    pub device_info: Option<DeviceInfo>, // V4L2 device information (card, driver, path, real_path)
}

/// Type of sensor (distinguishes RGB camera from depth/IR sensors)
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SensorType {
    /// Standard RGB camera (video/photo)
    #[default]
    Rgb,
    /// Depth sensor (e.g., Kinect Y10B)
    Depth,
    /// Infrared sensor (e.g., Kinect IR)
    Ir,
}

impl std::fmt::Display for SensorType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SensorType::Rgb => write!(f, "Camera"),
            SensorType::Depth => write!(f, "Depth"),
            SensorType::Ir => write!(f, "IR"),
        }
    }
}

/// Camera format specification
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CameraFormat {
    pub width: u32,
    pub height: u32,
    pub framerate: Option<u32>,     // None for photo mode
    pub hardware_accelerated: bool, // True for MJPEG and raw formats with HW support
    pub pixel_format: String,       // FourCC code (e.g., "MJPG", "H264", "YUYV")
    pub sensor_type: SensorType,    // RGB camera vs depth sensor
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
    /// RGBA - 32-bit with alpha (4 bytes per pixel)
    /// This is the native format used throughout the pipeline
    RGBA,
    /// Depth16 - 16-bit grayscale depth data
    /// Used for depth sensors like the Kinect Y10B format
    Depth16,
}

/// A single frame from the camera
#[derive(Debug, Clone)]
pub struct CameraFrame {
    pub width: u32,
    pub height: u32,
    pub data: Arc<[u8]>,      // Zero-copy frame data (RGBA format for preview)
    pub format: PixelFormat,  // Pixel format of the data (RGBA or Depth16)
    pub stride: u32,          // Row stride (bytes per row, may include padding)
    pub captured_at: Instant, // Timestamp when frame was captured (for latency diagnostics)
    /// Optional 16-bit depth data for depth sensor frames
    /// Contains the full precision depth values when available
    pub depth_data: Option<Arc<[u16]>>,
    /// Depth dimensions (may differ from RGB width/height)
    /// Only set when depth_data is Some
    pub depth_width: u32,
    pub depth_height: u32,
    /// Video frame timestamp from hardware (for synchronizing depth/color at different frame rates)
    /// Only used by Kinect native backend where depth (30fps) and color (10fps high-res) differ
    pub video_timestamp: Option<u32>,
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

/// Processed video/color frame data ready for rendering
///
/// Used by depth camera backends to provide color frame output.
/// Note: Pixel format varies by backend:
/// - NativeDepthBackend: RGB (3 bytes per pixel)
/// - V4l2KernelDepthBackend: RGBA (4 bytes per pixel)
#[derive(Debug, Clone)]
pub struct VideoFrameData {
    /// Frame width
    pub width: u32,
    /// Frame height
    pub height: u32,
    /// Pixel data (format varies by backend)
    pub data: Vec<u8>,
    /// Frame timestamp
    pub timestamp: u32,
}

/// Processed depth frame data ready for 3D rendering
///
/// Used by depth camera backends to provide depth frame output
#[derive(Debug, Clone)]
pub struct DepthFrameData {
    /// Frame width
    pub width: u32,
    /// Frame height
    pub height: u32,
    /// Depth values in millimeters (u16 per pixel)
    pub depth_mm: Vec<u16>,
    /// Frame timestamp
    pub timestamp: u32,
}
