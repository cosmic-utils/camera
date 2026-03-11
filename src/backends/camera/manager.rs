// SPDX-License-Identifier: GPL-3.0-only

//! Camera backend lifecycle manager
//!
//! The manager provides:
//! - Backend lifecycle management (initialization, shutdown)
//! - Thread-safe backend access

use super::types::*;
use super::{CameraBackend, get_backend_for_type};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::Mutex;
use tracing::info;

/// Shared recording sender type.
///
/// This Arc lives in the manager (not inside any pipeline) so it survives
/// pipeline restarts and is accessible from both the subscription's capture
/// thread and the recording start/stop code.
pub type SharedRecordingSender = Arc<Mutex<Option<tokio::sync::mpsc::Sender<Arc<CameraFrame>>>>>;

/// Internal manager state
struct ManagerState {
    /// The active backend instance
    backend: Box<dyn CameraBackend>,
    /// Backend type
    backend_type: CameraBackendType,
}

/// Camera backend manager
///
/// Manages backend lifecycle.
/// Thread-safe and can be shared across threads.
#[derive(Clone)]
pub struct CameraBackendManager {
    state: Arc<Mutex<ManagerState>>,
    /// Shared recording sender — written by recording start/stop,
    /// read by the capture thread (via Arc clone passed to the pipeline).
    recording_sender: SharedRecordingSender,
}

impl CameraBackendManager {
    /// Create a new backend manager
    ///
    /// # Arguments
    /// * `backend_type` - The type of backend to use
    pub fn new(backend_type: CameraBackendType) -> Self {
        info!(backend = %backend_type, "Creating camera backend manager");

        let backend = get_backend_for_type(backend_type);

        let state = ManagerState {
            backend,
            backend_type,
        };

        Self {
            state: Arc::new(Mutex::new(state)),
            recording_sender: Arc::new(Mutex::new(None)),
        }
    }

    /// Get the backend type
    pub fn backend_type(&self) -> CameraBackendType {
        self.state.lock().unwrap().backend_type
    }

    /// Check if the backend is available on this system
    pub fn is_available(&self) -> bool {
        self.state.lock().unwrap().backend.is_available()
    }

    /// Enumerate available cameras
    pub fn enumerate_cameras(&self) -> BackendResult<Vec<CameraDevice>> {
        let state = self.state.lock().unwrap();

        // Only call enumerate once - it spawns pw-cli subprocess
        let cameras = state.backend.enumerate_cameras();
        if cameras.is_empty() {
            Err(BackendError::DeviceNotFound("No cameras found".to_string()))
        } else {
            Ok(cameras)
        }
    }

    /// Get supported formats for a camera
    pub fn get_formats(&self, device: &CameraDevice, video_mode: bool) -> Vec<CameraFormat> {
        let state = self.state.lock().unwrap();
        state.backend.get_formats(device, video_mode)
    }

    /// Initialize the backend
    pub fn initialize(&self, device: &CameraDevice, format: &CameraFormat) -> BackendResult<()> {
        info!(device = %device.name, format = %format, "Initializing backend");

        let mut state = self.state.lock().unwrap();
        state.backend.initialize(device, format)
    }

    /// Shutdown the backend
    pub fn shutdown(&self) -> BackendResult<()> {
        info!("Shutting down backend");

        let mut state = self.state.lock().unwrap();
        state.backend.shutdown()
    }

    /// Check if initialized
    pub fn is_initialized(&self) -> bool {
        self.state.lock().unwrap().backend.is_initialized()
    }

    /// Switch to a different camera
    pub fn switch_camera(&self, device: &CameraDevice) -> BackendResult<()> {
        info!(device = %device.name, "Switching camera");

        let mut state = self.state.lock().unwrap();
        state.backend.switch_camera(device)
    }

    /// Apply a different format
    pub fn apply_format(&self, format: &CameraFormat) -> BackendResult<()> {
        info!(format = %format, "Applying format");

        let mut state = self.state.lock().unwrap();
        state.backend.apply_format(format)
    }

    /// Capture a photo
    pub fn capture_photo(&self) -> BackendResult<CameraFrame> {
        let state = self.state.lock().unwrap();
        state.backend.capture_photo()
    }

    /// Start video recording
    pub fn start_recording(&self, output_path: PathBuf) -> BackendResult<()> {
        let mut state = self.state.lock().unwrap();
        state.backend.start_recording(output_path)
    }

    /// Stop video recording
    pub fn stop_recording(&self) -> BackendResult<PathBuf> {
        let mut state = self.state.lock().unwrap();
        state.backend.stop_recording()
    }

    /// Check if currently recording
    pub fn is_recording(&self) -> bool {
        self.state.lock().unwrap().backend.is_recording()
    }

    /// Set (or clear) the direct recording sender.
    ///
    /// This writes to a shared Arc that the capture thread reads from,
    /// independent of which pipeline instance is active.
    pub fn set_recording_sender(
        &self,
        sender: Option<tokio::sync::mpsc::Sender<Arc<CameraFrame>>>,
    ) {
        *self.recording_sender.lock().unwrap() = sender;
    }

    /// Get a clone of the shared recording sender Arc.
    ///
    /// Pass this to the pipeline so the capture thread can read from it.
    pub fn recording_sender(&self) -> SharedRecordingSender {
        Arc::clone(&self.recording_sender)
    }

    /// Get current device
    pub fn current_device(&self) -> Option<CameraDevice> {
        self.state.lock().unwrap().backend.current_device().cloned()
    }

    /// Get current format
    pub fn current_format(&self) -> Option<CameraFormat> {
        self.state.lock().unwrap().backend.current_format().cloned()
    }

    /// Change backend type
    ///
    /// This shuts down the current backend and switches to a new one.
    /// The new backend will need to be initialized before use.
    pub fn change_backend(&self, new_backend_type: CameraBackendType) -> BackendResult<()> {
        info!(old = %self.backend_type(), new = %new_backend_type, "Changing backend");

        let mut state = self.state.lock().unwrap();

        // Shutdown current backend
        let _ = state.backend.shutdown(); // Ignore errors during shutdown

        // Create new backend for the specified type
        let new_backend = get_backend_for_type(new_backend_type);

        state.backend = new_backend;
        state.backend_type = new_backend_type;

        info!("Backend changed successfully");
        Ok(())
    }
}

impl std::fmt::Debug for CameraBackendManager {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let state = self.state.lock().unwrap();
        f.debug_struct("CameraBackendManager")
            .field("backend_type", &state.backend_type)
            .field("initialized", &state.backend.is_initialized())
            .finish()
    }
}
