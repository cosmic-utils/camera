// SPDX-License-Identifier: GPL-3.0-only
// Shared types for camera backend abstraction
#![allow(dead_code)]

//! Shared types for camera backends

use gstreamer::buffer::{MappedBuffer, Readable};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use std::time::Instant;

/// Frame data storage - either pre-copied bytes or zero-copy GStreamer buffer
///
/// This enum allows frames to be passed around without copying the underlying
/// pixel data when coming from GStreamer pipelines. The `Mapped` variant keeps
/// the GStreamer buffer mapped and alive until all references are dropped.
#[derive(Clone)]
pub enum FrameData {
    /// Pre-copied bytes (used for photo capture, file sources, tests, etc.)
    Copied(Arc<[u8]>),
    /// Zero-copy mapped GStreamer buffer - no data copy, just reference counting
    Mapped(Arc<MappedBuffer<Readable>>),
}

impl FrameData {
    /// Create FrameData from pre-copied bytes
    pub fn from_bytes(data: Arc<[u8]>) -> Self {
        FrameData::Copied(data)
    }

    /// Create FrameData from a mapped GStreamer buffer (zero-copy)
    pub fn from_mapped_buffer(buffer: MappedBuffer<Readable>) -> Self {
        FrameData::Mapped(Arc::new(buffer))
    }

    /// Get the length of the frame data in bytes
    pub fn len(&self) -> usize {
        match self {
            FrameData::Copied(data) => data.len(),
            FrameData::Mapped(buf) => buf.len(),
        }
    }

    /// Check if the frame data is empty
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Get a raw pointer to the data for deduplication checks
    pub fn as_ptr(&self) -> *const u8 {
        match self {
            FrameData::Copied(data) => data.as_ptr(),
            FrameData::Mapped(buf) => buf.as_ptr(),
        }
    }
}

impl std::fmt::Debug for FrameData {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            FrameData::Copied(data) => write!(f, "FrameData::Copied({} bytes)", data.len()),
            FrameData::Mapped(buf) => write!(f, "FrameData::Mapped({} bytes)", buf.len()),
        }
    }
}

impl AsRef<[u8]> for FrameData {
    fn as_ref(&self) -> &[u8] {
        match self {
            FrameData::Copied(data) => data.as_ref(),
            FrameData::Mapped(buf) => buf.as_slice(),
        }
    }
}

impl std::ops::Deref for FrameData {
    type Target = [u8];

    fn deref(&self) -> &[u8] {
        self.as_ref()
    }
}

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

/// Sensor rotation in degrees (clockwise)
///
/// Camera sensors may be physically mounted at various angles relative to the device.
/// This is common on mobile devices where sensors are rotated 90° or 270° relative
/// to the display orientation.
///
/// The rotation value comes from:
/// - libcamera's `api.libcamera.rotation` property in PipeWire
/// - Device tree sensor rotation values
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SensorRotation {
    /// No rotation (sensor is oriented correctly)
    #[default]
    None,
    /// 90 degrees clockwise
    Rotate90,
    /// 180 degrees (upside down)
    Rotate180,
    /// 270 degrees clockwise (90 degrees counter-clockwise)
    Rotate270,
}

impl SensorRotation {
    /// Parse rotation from a string value (degrees)
    pub fn from_degrees(degrees: &str) -> Self {
        match degrees.trim() {
            "90" => SensorRotation::Rotate90,
            "180" => SensorRotation::Rotate180,
            "270" => SensorRotation::Rotate270,
            "0" | "" => SensorRotation::None,
            other => {
                // Try to parse as integer and normalize
                if let Ok(deg) = other.parse::<i32>() {
                    match deg.rem_euclid(360) {
                        90 => SensorRotation::Rotate90,
                        180 => SensorRotation::Rotate180,
                        270 => SensorRotation::Rotate270,
                        _ => SensorRotation::None,
                    }
                } else {
                    SensorRotation::None
                }
            }
        }
    }

    /// Get the rotation in degrees
    pub fn degrees(&self) -> u32 {
        match self {
            SensorRotation::None => 0,
            SensorRotation::Rotate90 => 90,
            SensorRotation::Rotate180 => 180,
            SensorRotation::Rotate270 => 270,
        }
    }

    /// Check if rotation swaps width and height
    pub fn swaps_dimensions(&self) -> bool {
        matches!(self, SensorRotation::Rotate90 | SensorRotation::Rotate270)
    }
}

impl std::fmt::Display for SensorRotation {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}°", self.degrees())
    }
}

/// Represents a camera device
#[derive(Debug, Clone)]
pub struct CameraDevice {
    pub name: String,
    pub path: String,                    // Path to capture device (pipewire node ID)
    pub metadata_path: Option<String>,   // Path to metadata/control device or node ID
    pub device_info: Option<DeviceInfo>, // V4L2 device information (card, driver, path, real_path)
    pub rotation: SensorRotation,        // Sensor rotation from libcamera/device tree
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
    /// RGBA - 32-bit with alpha (4 bytes per pixel)
    /// This is the native format used throughout the pipeline
    RGBA,
}

/// A single frame from the camera
#[derive(Debug, Clone)]
pub struct CameraFrame {
    pub width: u32,
    pub height: u32,
    pub data: FrameData, // Frame data (RGBA format) - zero-copy when from GStreamer
    pub format: PixelFormat, // Pixel format of the data (always RGBA)
    pub stride: u32,     // Row stride (bytes per row, may include padding)
    pub captured_at: Instant, // Timestamp when frame was captured (for latency diagnostics)
}

impl CameraFrame {
    /// Get the frame data as a byte slice
    #[inline]
    pub fn data_slice(&self) -> &[u8] {
        &self.data
    }

    /// Get a raw pointer to the data for deduplication checks
    #[inline]
    pub fn data_ptr(&self) -> usize {
        self.data.as_ptr() as usize
    }
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
