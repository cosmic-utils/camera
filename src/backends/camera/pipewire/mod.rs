// SPDX-License-Identifier: MPL-2.0
// PipeWire backend implementation with some features for future use
#![allow(dead_code)]

//! PipeWire camera backend
//!
//! This backend uses PipeWire for camera enumeration, format detection, and capture.
//! It's the modern, recommended approach for Linux camera access.

mod enumeration;
mod pipeline;

pub use enumeration::{enumerate_pipewire_cameras, get_pipewire_formats, is_pipewire_available};
pub use pipeline::PipeWirePipeline;

use super::CameraBackend;
use super::types::*;
use std::path::PathBuf;
use tracing::{debug, info};

/// PipeWire backend implementation
pub struct PipeWireBackend {
    /// Currently active camera device (if initialized)
    current_device: Option<CameraDevice>,
    /// Currently active format (if initialized)
    current_format: Option<CameraFormat>,
    /// Active GStreamer pipeline for preview
    pipeline: Option<pipeline::PipeWirePipeline>,
    /// Frame sender for preview stream
    frame_sender: Option<FrameSender>,
    /// Frame receiver for preview stream (given to UI)
    frame_receiver: Option<FrameReceiver>,
}

impl PipeWireBackend {
    /// Create a new PipeWire backend
    pub fn new() -> Self {
        Self {
            current_device: None,
            current_format: None,
            pipeline: None,
            frame_sender: None,
            frame_receiver: None,
        }
    }

    /// Internal method to create preview pipeline
    fn create_pipeline(&mut self) -> BackendResult<()> {
        let device = self
            .current_device
            .as_ref()
            .ok_or_else(|| BackendError::Other("No device set".to_string()))?;
        let format = self
            .current_format
            .as_ref()
            .ok_or_else(|| BackendError::Other("No format set".to_string()))?;

        // Create frame channel
        let (sender, receiver) = cosmic::iced::futures::channel::mpsc::channel(100);

        // Create pipeline
        let pipeline = pipeline::PipeWirePipeline::new(device, format, sender.clone())?;

        pipeline.start()?;

        self.pipeline = Some(pipeline);
        self.frame_sender = Some(sender);
        self.frame_receiver = Some(receiver);

        Ok(())
    }
}

impl CameraBackend for PipeWireBackend {
    fn enumerate_cameras(&self) -> Vec<CameraDevice> {
        info!("Using PipeWire backend for camera enumeration");

        if let Some(cameras) = enumerate_pipewire_cameras() {
            info!(count = cameras.len(), "PipeWire cameras enumerated");
            cameras
        } else {
            info!("PipeWire enumeration returned None");
            Vec::new()
        }
    }

    fn get_formats(&self, device: &CameraDevice, _video_mode: bool) -> Vec<CameraFormat> {
        info!(device_path = %device.path, "Getting formats via PipeWire backend");
        get_pipewire_formats(&device.path, device.metadata_path.as_deref())
    }

    fn initialize(&mut self, device: &CameraDevice, format: &CameraFormat) -> BackendResult<()> {
        info!(
            device = %device.name,
            format = %format,
            "Initializing PipeWire backend"
        );

        // Shutdown any existing pipeline
        if self.is_initialized() {
            self.shutdown()?;
        }

        // Store device and format
        self.current_device = Some(device.clone());
        self.current_format = Some(format.clone());

        // Create pipeline
        self.create_pipeline()?;

        info!("PipeWire backend initialized successfully");
        Ok(())
    }

    fn shutdown(&mut self) -> BackendResult<()> {
        info!("Shutting down PipeWire backend");

        // Stop pipeline
        if let Some(pipeline) = self.pipeline.take() {
            pipeline.stop()?;
        }

        // Clear state
        self.frame_sender = None;
        self.frame_receiver = None;
        self.current_device = None;
        self.current_format = None;

        info!("PipeWire backend shut down");
        Ok(())
    }

    fn is_initialized(&self) -> bool {
        self.pipeline.is_some() && self.current_device.is_some()
    }

    fn recover(&mut self) -> BackendResult<()> {
        info!("Attempting to recover PipeWire backend");

        // Get current config
        let device = self
            .current_device
            .clone()
            .ok_or_else(|| BackendError::Other("No device to recover".to_string()))?;
        let format = self
            .current_format
            .clone()
            .ok_or_else(|| BackendError::Other("No format to recover".to_string()))?;

        // Shutdown and reinitialize
        let _ = self.shutdown(); // Ignore errors during recovery shutdown
        self.initialize(&device, &format)
    }

    fn switch_camera(&mut self, device: &CameraDevice) -> BackendResult<()> {
        info!(device = %device.name, "Switching to new camera");

        // Get available formats for new device
        let formats = self.get_formats(device, false);
        if formats.is_empty() {
            return Err(BackendError::FormatNotSupported(
                "No formats available for device".to_string(),
            ));
        }

        // Select highest resolution format
        let format = formats
            .iter()
            .max_by_key(|f| f.width * f.height)
            .cloned()
            .ok_or_else(|| BackendError::Other("Failed to select format".to_string()))?;

        // Reinitialize with new device
        self.initialize(device, &format)
    }

    fn apply_format(&mut self, format: &CameraFormat) -> BackendResult<()> {
        info!(format = %format, "Applying new format");

        let device = self
            .current_device
            .clone()
            .ok_or_else(|| BackendError::Other("No active device".to_string()))?;

        // Reinitialize with new format
        self.initialize(&device, format)
    }

    fn capture_photo(&self) -> BackendResult<CameraFrame> {
        debug!("Capturing photo via PipeWire backend");

        let pipeline = self
            .pipeline
            .as_ref()
            .ok_or_else(|| BackendError::Other("Pipeline not initialized".to_string()))?;

        pipeline.capture_frame()
    }

    fn start_recording(&mut self, output_path: PathBuf) -> BackendResult<()> {
        info!(path = %output_path.display(), "Starting recording");

        let pipeline = self
            .pipeline
            .as_mut()
            .ok_or_else(|| BackendError::Other("Pipeline not initialized".to_string()))?;

        pipeline.start_recording(output_path)
    }

    fn stop_recording(&mut self) -> BackendResult<PathBuf> {
        info!("Stopping recording");

        let pipeline = self
            .pipeline
            .as_mut()
            .ok_or_else(|| BackendError::Other("Pipeline not initialized".to_string()))?;

        pipeline.stop_recording()
    }

    fn is_recording(&self) -> bool {
        self.pipeline
            .as_ref()
            .map(|p| p.is_recording())
            .unwrap_or(false)
    }

    fn get_preview_receiver(&self) -> Option<FrameReceiver> {
        // Note: This returns a clone of the receiver
        // The actual implementation will need to handle this differently
        // For now, we'll return None and handle preview frames via subscription
        None
    }

    fn backend_type(&self) -> CameraBackendType {
        CameraBackendType::PipeWire
    }

    fn is_available(&self) -> bool {
        is_pipewire_available()
    }

    fn current_device(&self) -> Option<&CameraDevice> {
        self.current_device.as_ref()
    }

    fn current_format(&self) -> Option<&CameraFormat> {
        self.current_format.as_ref()
    }
}
