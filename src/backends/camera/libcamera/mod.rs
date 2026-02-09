// SPDX-License-Identifier: GPL-3.0-only

//! Direct libcamera backend
//!
//! This backend uses libcamera-rs directly for camera access, bypassing GStreamer/PipeWire.
//! It provides:
//! - Direct access to all libcamera features
//! - Multi-stream capture (4K stills + 1080p preview simultaneously)
//! - Per-frame controls with metadata feedback
//! - Lower latency potential through direct buffer management
//!
//! # Architecture
//!
//! ```text
//! ┌─────────────────────┐
//! │   LibcameraBackend  │  ← Implements CameraBackend trait
//! └──────────┬──────────┘
//!            │
//!            ▼
//! ┌─────────────────────┐
//! │  LibcameraPipeline  │  ← Request cycling, buffer management
//! └──────────┬──────────┘
//!            │
//!            ▼
//! ┌─────────────────────┐
//! │    libcamera-rs     │  ← Native bindings
//! └─────────────────────┘
//! ```

mod pipeline;

pub use pipeline::LibcameraPipeline;

use super::types::*;
use super::CameraBackend;
use libcamera::{
    camera_manager::CameraManager,
    properties,
    stream::StreamRole,
};
use std::path::PathBuf;
use tracing::{debug, info, warn};

/// Direct libcamera backend implementation
///
/// This backend provides direct access to libcamera without GStreamer/PipeWire
/// abstraction layers. It's ideal for:
/// - Maximum control over camera settings
/// - Multi-stream capture (preview + high-res stills)
/// - Lower latency applications
/// - Per-frame control and metadata
///
/// Note: CameraManager is created on-demand because it's not Send+Sync.
pub struct LibcameraBackend {
    /// Currently active camera ID (if initialized)
    current_camera: Option<String>,
    /// Current device info
    current_device: Option<CameraDevice>,
    /// Current format
    current_format: Option<CameraFormat>,
    /// Active pipeline
    pipeline: Option<LibcameraPipeline>,
    /// Frame sender for preview stream
    frame_sender: Option<FrameSender>,
    /// Frame receiver for preview stream (given to UI)
    frame_receiver: Option<FrameReceiver>,
}

impl Default for LibcameraBackend {
    fn default() -> Self {
        Self::new()
    }
}

impl LibcameraBackend {
    /// Create a new libcamera backend
    pub fn new() -> Self {
        Self {
            current_camera: None,
            current_device: None,
            current_format: None,
            pipeline: None,
            frame_sender: None,
            frame_receiver: None,
        }
    }

    /// Get libcamera camera ID from CameraDevice path
    fn find_camera_id(device: &CameraDevice) -> Option<String> {
        let manager = CameraManager::new().ok()?;
        let cameras = manager.cameras();

        // Try to match by path (which may be the libcamera ID directly)
        for cam in cameras.iter() {
            let cam_id = cam.id();
            if cam_id == device.path {
                return Some(cam_id.to_string());
            }
        }

        // Try to match by serial number or model name
        for cam in cameras.iter() {
            if let Ok(model) = cam.properties().get::<properties::Model>()
                && (device.name.contains(&*model) || model.contains(&device.name))
            {
                return Some(cam.id().to_string());
            }
        }

        // If only one camera, use it
        if cameras.len() == 1 {
            return cameras.get(0).map(|c| c.id().to_string());
        }

        None
    }

    /// Create the streaming pipeline
    fn create_pipeline(&mut self) -> BackendResult<()> {
        let device = self
            .current_device
            .as_ref()
            .ok_or_else(|| BackendError::Other("No device set".to_string()))?;
        let format = self
            .current_format
            .as_ref()
            .ok_or_else(|| BackendError::Other("No format set".to_string()))?;

        let camera_id = Self::find_camera_id(device)
            .ok_or_else(|| BackendError::DeviceNotFound(device.name.clone()))?;

        // Create frame channel (small capacity for low latency)
        let (sender, receiver) = cosmic::iced::futures::channel::mpsc::channel(
            crate::constants::latency::FRAME_CHANNEL_CAPACITY,
        );

        // Create pipeline (it will create its own CameraManager in its thread)
        let pipeline = LibcameraPipeline::new(&camera_id, format, sender.clone())?;

        self.pipeline = Some(pipeline);
        self.frame_sender = Some(sender);
        self.frame_receiver = Some(receiver);
        self.current_camera = Some(camera_id);

        Ok(())
    }
}

impl CameraBackend for LibcameraBackend {
    fn enumerate_cameras(&self) -> Vec<CameraDevice> {
        debug!("Enumerating cameras via direct libcamera backend");

        let manager = match CameraManager::new() {
            Ok(m) => m,
            Err(e) => {
                warn!(?e, "Failed to create CameraManager for enumeration");
                return Vec::new();
            }
        };

        let cameras = manager.cameras();
        let mut devices = Vec::new();

        for cam in cameras.iter() {
            let id = cam.id().to_string();

            // Get model name from properties
            let name = cam
                .properties()
                .get::<properties::Model>()
                .map(|m| (*m).clone())
                .unwrap_or_else(|_| id.clone());

            // Get rotation from properties
            let rotation = cam
                .properties()
                .get::<properties::Rotation>()
                .map(|r| SensorRotation::from_degrees(&r.to_string()))
                .unwrap_or_default();

            devices.push(CameraDevice {
                name,
                path: id.clone(),
                metadata_path: Some(id),
                device_info: None,
                rotation,
            });
        }

        debug!(count = devices.len(), "Enumerated libcamera cameras");
        devices
    }

    fn get_formats(&self, device: &CameraDevice, video_mode: bool) -> Vec<CameraFormat> {
        info!(device_path = %device.path, video_mode, "Getting formats via libcamera backend");

        let camera_id = match Self::find_camera_id(device) {
            Some(id) => id,
            None => {
                warn!("Camera not found for format enumeration: {}", device.path);
                return Vec::new();
            }
        };

        let manager = match CameraManager::new() {
            Ok(m) => m,
            Err(e) => {
                warn!(?e, "Failed to create CameraManager for format enumeration");
                return Vec::new();
            }
        };

        let cameras = manager.cameras();
        let cam = match cameras.iter().find(|c| c.id() == camera_id) {
            Some(c) => c,
            None => return Vec::new(),
        };

        // Acquire camera temporarily for configuration inspection
        let cam = match cam.acquire() {
            Ok(c) => c,
            Err(e) => {
                warn!(?e, "Failed to acquire camera for format enumeration");
                return Vec::new();
            }
        };

        let role = if video_mode {
            StreamRole::VideoRecording
        } else {
            StreamRole::ViewFinder
        };

        let cfgs = match cam.generate_configuration(&[role]) {
            Some(c) => c,
            None => {
                warn!("Failed to generate camera configuration");
                return Vec::new();
            }
        };

        let mut formats = Vec::new();

        if let Some(stream_cfg) = cfgs.get(0) {
            // Get available formats from stream configuration
            let stream_formats = stream_cfg.formats();
            let pixel_fmts = stream_formats.pixel_formats();

            for i in 0..pixel_fmts.len() {
                if let Some(fmt) = pixel_fmts.get(i) {
                    let sizes = stream_formats.sizes(fmt);
                    for j in 0..sizes.len() {
                        if let Some(size) = sizes.get(j) {
                            // libcamera doesn't expose discrete framerates like V4L2,
                            // but we can estimate based on typical values
                            let framerate = if video_mode {
                                Some(Framerate::from_int(30))
                            } else {
                                None // Photo mode
                            };

                            formats.push(CameraFormat {
                                width: size.width,
                                height: size.height,
                                framerate,
                                hardware_accelerated: true, // libcamera handles HW acceleration
                                pixel_format: format!("{:?}", fmt),
                            });
                        }
                    }
                }
            }
        }

        // Sort by resolution (highest first)
        formats.sort_by(|a, b| (b.width * b.height).cmp(&(a.width * a.height)));

        // Remove duplicates
        formats.dedup_by(|a, b| a.width == b.width && a.height == b.height);

        debug!(count = formats.len(), "Enumerated formats via libcamera");
        formats
    }

    fn initialize(&mut self, device: &CameraDevice, format: &CameraFormat) -> BackendResult<()> {
        info!(
            device = %device.name,
            format = %format,
            "Initializing libcamera backend"
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

        // Start the pipeline
        if let Some(ref mut pipeline) = self.pipeline {
            pipeline.start()?;
        }

        info!("libcamera backend initialized successfully");
        Ok(())
    }

    fn shutdown(&mut self) -> BackendResult<()> {
        info!("Shutting down libcamera backend");

        // Stop pipeline
        if let Some(pipeline) = self.pipeline.take() {
            pipeline.stop()?;
        }

        // Clear state
        self.frame_sender = None;
        self.frame_receiver = None;
        self.current_device = None;
        self.current_format = None;
        self.current_camera = None;

        info!("libcamera backend shut down");
        Ok(())
    }

    fn is_initialized(&self) -> bool {
        self.pipeline.is_some() && self.current_device.is_some()
    }

    fn recover(&mut self) -> BackendResult<()> {
        info!("Attempting to recover libcamera backend");

        let device = self
            .current_device
            .clone()
            .ok_or_else(|| BackendError::Other("No device to recover".to_string()))?;
        let format = self
            .current_format
            .clone()
            .ok_or_else(|| BackendError::Other("No format to recover".to_string()))?;

        // Shutdown and reinitialize
        let _ = self.shutdown();
        self.initialize(&device, &format)
    }

    fn switch_camera(&mut self, device: &CameraDevice) -> BackendResult<()> {
        info!(device = %device.name, "Switching to new camera");

        let formats = self.get_formats(device, false);
        if formats.is_empty() {
            return Err(BackendError::FormatNotSupported(
                "No formats available for device".to_string(),
            ));
        }

        // Select highest resolution format
        let format = formats
            .into_iter()
            .max_by_key(|f| f.width * f.height)
            .ok_or_else(|| BackendError::Other("Failed to select format".to_string()))?;

        self.initialize(device, &format)
    }

    fn apply_format(&mut self, format: &CameraFormat) -> BackendResult<()> {
        info!(format = %format, "Applying new format");

        let device = self
            .current_device
            .clone()
            .ok_or_else(|| BackendError::Other("No active device".to_string()))?;

        self.initialize(&device, format)
    }

    fn capture_photo(&self) -> BackendResult<CameraFrame> {
        debug!("Capturing photo via libcamera backend");

        let pipeline = self
            .pipeline
            .as_ref()
            .ok_or_else(|| BackendError::Other("Pipeline not initialized".to_string()))?;

        pipeline.capture_frame()
    }

    fn start_recording(&mut self, output_path: PathBuf) -> BackendResult<()> {
        info!(path = %output_path.display(), "Starting recording");

        // Note: Video recording should use the separate VideoRecorder
        // This is a placeholder for future direct libcamera recording
        Err(BackendError::Other(
            "Use VideoRecorder for recording (libcamera backend recording not yet implemented)".to_string(),
        ))
    }

    fn stop_recording(&mut self) -> BackendResult<PathBuf> {
        info!("Stopping recording");
        Err(BackendError::NoRecordingInProgress)
    }

    fn is_recording(&self) -> bool {
        false
    }

    fn get_preview_receiver(&self) -> Option<FrameReceiver> {
        None // Handled via subscription
    }

    fn backend_type(&self) -> CameraBackendType {
        CameraBackendType::PipeWire // TODO: Add Libcamera variant
    }

    fn is_available(&self) -> bool {
        CameraManager::new()
            .map(|mgr| !mgr.cameras().is_empty())
            .unwrap_or(false)
    }

    fn current_device(&self) -> Option<&CameraDevice> {
        self.current_device.as_ref()
    }

    fn current_format(&self) -> Option<&CameraFormat> {
        self.current_format.as_ref()
    }
}
