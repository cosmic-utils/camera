// SPDX-License-Identifier: GPL-3.0-only
// Camera backend using native libcamera-rs bindings

//! Camera backend abstraction
//!
//! This module provides the libcamera camera backend with trait-based abstraction.
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
//!       ┌──────────┐
//!       │libcamera │  ← Native libcamera-rs bindings
//!       └──────────┘
//! ```

pub mod libcamera;
pub mod manager;
pub mod types;
pub mod v4l2_controls;
pub mod v4l2_utils;

pub use manager::CameraBackendManager;
pub use types::*;

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

    /// Capture a single photo frame (blocking, up to 2 seconds)
    ///
    /// This captures a single frame with the current camera settings.
    /// The frame data is copied immediately, so the camera preview is not blocked.
    /// The frame format depends on the camera (RGBA, Bayer, or YUV).
    ///
    /// # Returns
    /// * `Ok(CameraFrame)` - Frame captured successfully
    /// * `Err(BackendError)` - Capture failed
    fn capture_photo(&self) -> BackendResult<CameraFrame>;

    /// Request a still capture (non-blocking).
    /// Returns Ok(()) if the request was accepted.
    fn request_still_capture(&self) -> BackendResult<()>;

    /// Poll for a still frame without blocking. Returns `None` if not yet available.
    fn poll_still_frame(&self) -> Option<CameraFrame>;

    /// Poll for the latest preview frame without blocking.
    fn poll_preview_frame(&self) -> Option<CameraFrame>;

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

    /// Check if this backend is available on the current system
    fn is_available(&self) -> bool;

    /// Get the currently active camera device (if initialized)
    fn current_device(&self) -> Option<&CameraDevice>;

    /// Get the currently active format (if initialized)
    fn current_format(&self) -> Option<&CameraFormat>;
}

/// Create a new backend instance
pub fn create_backend() -> Box<dyn CameraBackend> {
    Box::new(libcamera::LibcameraBackend::new())
}
