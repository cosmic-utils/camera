// SPDX-License-Identifier: MPL-2.0
// Camera backend with trait-based abstraction for future multi-backend support
#![allow(dead_code)]

//! Camera backend abstraction
//!
//! This module provides a complete trait-based abstraction for the PipeWire camera backend.
//!
//! # Architecture
//!
//! ```text
//! ┌─────────────────────┐
//! │   UI Layer (App)    │
//! └──────────┬──────────┘
//!            │
//!            ▼
//! ┌─────────────────────┐
//! │ CameraBackendManager│  ← Lifecycle management, crash recovery
//! └──────────┬──────────┘
//!            │
//!            ▼
//! ┌─────────────────────┐
//! │  CameraBackend Trait│  ← Common interface
//! └──────────┬──────────┘
//!            │
//!            ▼
//!       ┌────────┐
//!       │PipeWire│  ← Concrete implementation
//!       └────────┘
//! ```

pub mod manager;
pub mod pipewire;
pub mod types;

pub use manager::CameraBackendManager;
pub use types::*;

use std::path::PathBuf;

/// Complete camera backend trait
///
/// All camera backends must implement this trait to provide:
/// - Device enumeration and format detection
/// - Lifecycle management (initialization, shutdown, recovery)
/// - Camera operations (switching, format changes)
/// - Capture operations (photo, video)
/// - Preview streaming
pub trait CameraBackend: Send + Sync {
    // ===== Enumeration =====

    /// Enumerate available cameras on this backend
    fn enumerate_cameras(&self) -> Vec<CameraDevice>;

    /// Get supported formats for a specific camera device
    ///
    /// # Arguments
    /// * `device` - The camera device to query
    /// * `video_mode` - If true, only return formats suitable for video recording
    fn get_formats(&self, device: &CameraDevice, video_mode: bool) -> Vec<CameraFormat>;

    // ===== Lifecycle =====

    /// Initialize the backend with a specific camera and format
    ///
    /// This creates the preview pipeline and prepares for capture operations.
    /// Must be called before any capture or preview operations.
    ///
    /// # Arguments
    /// * `device` - The camera device to initialize
    /// * `format` - The desired video format (resolution, framerate, pixel format)
    ///
    /// # Returns
    /// * `Ok(())` - Backend initialized successfully
    /// * `Err(BackendError)` - Initialization failed
    fn initialize(&mut self, device: &CameraDevice, format: &CameraFormat) -> BackendResult<()>;

    /// Shutdown the backend and release all resources
    ///
    /// This stops any active preview or recording, closes the camera device,
    /// and releases all resources. After shutdown, the backend must be
    /// reinitialized before use.
    fn shutdown(&mut self) -> BackendResult<()>;

    /// Check if the backend is currently initialized and operational
    fn is_initialized(&self) -> bool;

    /// Attempt to recover from a crash or error state
    ///
    /// This tries to reinitialize the backend with the last known configuration.
    /// Used by the manager for automatic crash recovery.
    fn recover(&mut self) -> BackendResult<()>;

    // ===== Operations =====

    /// Switch to a different camera device
    ///
    /// This shuts down the current camera and initializes the new one.
    /// The format will be automatically selected (max resolution for the new camera).
    ///
    /// # Arguments
    /// * `device` - The camera device to switch to
    fn switch_camera(&mut self, device: &CameraDevice) -> BackendResult<()>;

    /// Apply a different format to the current camera
    ///
    /// This recreates the pipeline with the new format settings.
    /// The camera device remains the same.
    ///
    /// # Arguments
    /// * `format` - The new format to apply
    fn apply_format(&mut self, format: &CameraFormat) -> BackendResult<()>;

    // ===== Capture: Photo =====

    /// Capture a single photo frame
    ///
    /// This captures a single frame with the current camera settings.
    /// The frame data is copied immediately, so the camera preview is not blocked.
    /// The frame is in RGBA format and ready for processing by the photo pipeline.
    ///
    /// # Returns
    /// * `Ok(CameraFrame)` - Frame captured successfully
    /// * `Err(BackendError)` - Capture failed
    fn capture_photo(&self) -> BackendResult<CameraFrame>;

    // ===== Capture: Video =====

    /// Start video recording to a file
    ///
    /// This starts recording video (and audio if configured) to the specified path.
    /// Preview continues uninterrupted during recording.
    /// Only one recording can be active at a time.
    ///
    /// # Arguments
    /// * `output_path` - Path where the video file will be saved
    ///
    /// # Returns
    /// * `Ok(())` - Recording started successfully
    /// * `Err(BackendError::RecordingInProgress)` - Already recording
    /// * `Err(BackendError)` - Failed to start recording
    fn start_recording(&mut self, output_path: PathBuf) -> BackendResult<()>;

    /// Stop video recording and finalize the file
    ///
    /// This stops the active recording, flushes the encoder, and finalizes the output file.
    /// Preview continues uninterrupted.
    ///
    /// # Returns
    /// * `Ok(PathBuf)` - Path to the saved video file
    /// * `Err(BackendError::NoRecordingInProgress)` - No active recording
    /// * `Err(BackendError)` - Failed to stop recording
    fn stop_recording(&mut self) -> BackendResult<PathBuf>;

    /// Check if currently recording
    fn is_recording(&self) -> bool;

    // ===== Preview =====

    /// Get a receiver for preview frames
    ///
    /// The receiver will continuously receive frames while the backend is initialized.
    /// Frames are in RGBA format and ready for display via the preview widget.
    ///
    /// # Returns
    /// * `Some(FrameReceiver)` - Stream of preview frames
    /// * `None` - Backend not initialized or preview not available
    fn get_preview_receiver(&self) -> Option<FrameReceiver>;

    // ===== Metadata =====

    /// Get the backend type identifier
    fn backend_type(&self) -> CameraBackendType;

    /// Check if this backend is available on the current system
    ///
    /// This checks for required system components (PipeWire daemon, V4L2 devices, etc.)
    fn is_available(&self) -> bool;

    /// Get the currently active camera device (if initialized)
    fn current_device(&self) -> Option<&CameraDevice>;

    /// Get the currently active format (if initialized)
    fn current_format(&self) -> Option<&CameraFormat>;
}

/// Get a concrete backend instance (PipeWire only)
pub fn get_backend() -> Box<dyn CameraBackend> {
    Box::new(pipewire::PipeWireBackend::new())
}

/// Get the default backend (PipeWire)
pub fn get_default_backend() -> CameraBackendType {
    CameraBackendType::PipeWire
}
