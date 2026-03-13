// SPDX-License-Identifier: GPL-3.0-only

//! Camera backend lifecycle manager
//!
//! The manager provides:
//! - Backend lifecycle management (initialization, shutdown)
//! - Thread-safe backend access

use super::types::*;
use super::{CameraBackend, create_backend};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, RwLock};
use tracing::info;

/// Shared recording sender type.
///
/// This Arc lives in the manager (not inside any pipeline) so it survives
/// pipeline restarts and is accessible from both the subscription's capture
/// thread and the recording start/stop code.
pub type SharedRecordingSender = Arc<Mutex<Option<tokio::sync::mpsc::Sender<RecordingFrame>>>>;

/// Internal manager state
struct ManagerState {
    /// The active backend instance
    backend: Box<dyn CameraBackend>,
}

/// Camera backend manager
///
/// Manages backend lifecycle.
/// Thread-safe and can be shared across threads.
#[derive(Clone)]
pub struct CameraBackendManager {
    state: Arc<RwLock<ManagerState>>,
    /// Shared recording sender — written by recording start/stop,
    /// read by the capture thread (via Arc clone passed to the pipeline).
    recording_sender: SharedRecordingSender,
    /// When true, the capture thread sends raw JPEG bytes (not decoded frames)
    /// to the recording channel for GPU-accelerated decode via VA-API.
    jpeg_recording_mode: Arc<AtomicBool>,
}

impl Default for CameraBackendManager {
    fn default() -> Self {
        Self::new()
    }
}

impl CameraBackendManager {
    /// Create a new backend manager
    pub fn new() -> Self {
        info!("Creating camera backend manager (libcamera)");

        let backend = create_backend();
        let state = ManagerState { backend };

        Self {
            state: Arc::new(RwLock::new(state)),
            recording_sender: Arc::new(Mutex::new(None)),
            jpeg_recording_mode: Arc::new(AtomicBool::new(false)),
        }
    }

    /// Check if the backend is available on this system
    pub fn is_available(&self) -> bool {
        self.state.read().unwrap().backend.is_available()
    }

    /// Enumerate available cameras
    pub fn enumerate_cameras(&self) -> BackendResult<Vec<CameraDevice>> {
        let state = self.state.read().unwrap();
        let cameras = state.backend.enumerate_cameras();
        if cameras.is_empty() {
            Err(BackendError::DeviceNotFound("No cameras found".to_string()))
        } else {
            Ok(cameras)
        }
    }

    /// Get supported formats for a camera
    pub fn get_formats(&self, device: &CameraDevice, video_mode: bool) -> Vec<CameraFormat> {
        let state = self.state.read().unwrap();
        state.backend.get_formats(device, video_mode)
    }

    /// Initialize the backend
    pub fn initialize(&self, device: &CameraDevice, format: &CameraFormat) -> BackendResult<()> {
        info!(device = %device.name, format = %format, "Initializing backend");

        let mut state = self.state.write().unwrap();
        state.backend.initialize(device, format)
    }

    /// Shutdown the backend
    pub fn shutdown(&self) -> BackendResult<()> {
        info!("Shutting down backend");

        let mut state = self.state.write().unwrap();
        state.backend.shutdown()
    }

    /// Check if initialized
    pub fn is_initialized(&self) -> bool {
        self.state.read().unwrap().backend.is_initialized()
    }

    /// Switch to a different camera
    pub fn switch_camera(&self, device: &CameraDevice) -> BackendResult<()> {
        info!(device = %device.name, "Switching camera");

        let mut state = self.state.write().unwrap();
        state.backend.switch_camera(device)
    }

    /// Apply a different format
    pub fn apply_format(&self, format: &CameraFormat) -> BackendResult<()> {
        info!(format = %format, "Applying format");

        let mut state = self.state.write().unwrap();
        state.backend.apply_format(format)
    }

    /// Capture a photo
    pub fn capture_photo(&self) -> BackendResult<CameraFrame> {
        let state = self.state.read().unwrap();
        state.backend.capture_photo()
    }

    /// Set (or clear) the direct recording sender.
    ///
    /// This writes to a shared Arc that the capture thread reads from,
    /// independent of which pipeline instance is active.
    pub fn set_recording_sender(&self, sender: Option<tokio::sync::mpsc::Sender<RecordingFrame>>) {
        *self.recording_sender.lock().unwrap() = sender;
    }

    /// Enable or disable JPEG recording mode.
    ///
    /// When enabled, the capture thread sends raw JPEG bytes instead of
    /// CPU-decoded frames to the recording channel.
    pub fn set_jpeg_recording_mode(&self, enabled: bool) {
        self.jpeg_recording_mode.store(enabled, Ordering::Relaxed);
    }

    /// Get a clone of the jpeg_recording_mode flag Arc.
    pub fn jpeg_recording_mode(&self) -> Arc<AtomicBool> {
        Arc::clone(&self.jpeg_recording_mode)
    }

    /// Get a clone of the shared recording sender Arc.
    ///
    /// Pass this to the pipeline so the capture thread can read from it.
    pub fn recording_sender(&self) -> SharedRecordingSender {
        Arc::clone(&self.recording_sender)
    }

    /// Get current device
    pub fn current_device(&self) -> Option<CameraDevice> {
        self.state.read().unwrap().backend.current_device().cloned()
    }

    /// Get current format
    pub fn current_format(&self) -> Option<CameraFormat> {
        self.state.read().unwrap().backend.current_format().cloned()
    }
}

impl std::fmt::Debug for CameraBackendManager {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let state = self.state.read().unwrap();
        f.debug_struct("CameraBackendManager")
            .field("initialized", &state.backend.is_initialized())
            .finish()
    }
}
