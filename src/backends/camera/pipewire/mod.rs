// SPDX-License-Identifier: GPL-3.0-only
// PipeWire backend implementation with some features for future use
#![allow(dead_code)]

//! PipeWire camera backend
//!
//! This backend uses PipeWire for camera enumeration, format detection, and capture.
//! It's the modern, recommended approach for Linux camera access.
//!
//! ## Depth Camera Device Handling
//!
//! Depth cameras are automatically routed to the native freedepth backend:
//! - Depth cameras are enumerated via freedepth (not V4L2/PipeWire)
//! - When a depth camera is selected, NativeDepthBackend is used
//! - This provides simultaneous RGB + depth streaming at 30fps
//! - V4L2 kernel driver is NOT used for depth cameras

mod enumeration;
mod pipeline;

pub use enumeration::{enumerate_pipewire_cameras, get_pipewire_formats, is_pipewire_available};
pub use pipeline::PipeWirePipeline;

use super::CameraBackend;
#[cfg(all(target_arch = "x86_64", feature = "freedepth"))]
use super::depth_controller::DepthController;
use super::types::*;
#[cfg(all(target_arch = "x86_64", feature = "freedepth"))]
use super::v4l2_depth::{DepthFrameReceiver, DepthFrameSender, V4l2DepthPipeline};
use super::{NativeDepthBackend, is_depth_native_device};
#[cfg(all(target_arch = "x86_64", feature = "freedepth"))]
use super::{enumerate_depth_cameras, get_depth_formats};
#[cfg(all(target_arch = "x86_64", feature = "freedepth"))]
use crate::shaders::depth::{is_depth_colormap_enabled, is_depth_only_mode, unpack_y10b_gpu};
use std::path::PathBuf;
#[cfg(all(target_arch = "x86_64", feature = "freedepth"))]
use std::sync::Arc;
#[cfg(all(target_arch = "x86_64", feature = "freedepth"))]
use tracing::warn;
use tracing::{debug, info};

use super::V4l2KernelDepthBackend;

/// Active capture pipeline (GStreamer, V4L2 depth, or native depth camera)
enum ActivePipeline {
    /// Standard GStreamer pipeline via PipeWire
    GStreamer(pipeline::PipeWirePipeline),
    /// Direct V4L2 depth capture for Y10B format (freedepth only)
    #[cfg(all(target_arch = "x86_64", feature = "freedepth"))]
    Depth(V4l2DepthPipeline),
    /// Native depth camera backend (freedepth - bypasses V4L2 entirely)
    DepthCamera(NativeDepthBackend),
    /// V4L2 kernel depth backend (uses kernel driver with depth controls)
    KernelDepth(V4l2KernelDepthBackend),
}

/// PipeWire backend implementation
pub struct PipeWireBackend {
    /// Currently active camera device (if initialized)
    current_device: Option<CameraDevice>,
    /// Currently active format (if initialized)
    current_format: Option<CameraFormat>,
    /// Active capture pipeline
    active_pipeline: Option<ActivePipeline>,
    /// Frame sender for preview stream
    frame_sender: Option<FrameSender>,
    /// Frame receiver for preview stream (given to UI)
    frame_receiver: Option<FrameReceiver>,
    /// Depth frame processing task handle (for Y10B)
    depth_task_handle: Option<tokio::task::JoinHandle<()>>,
}

impl PipeWireBackend {
    /// Create a new PipeWire backend
    pub fn new() -> Self {
        Self {
            current_device: None,
            current_format: None,
            active_pipeline: None,
            frame_sender: None,
            frame_receiver: None,
            depth_task_handle: None,
        }
    }

    /// Check if format is Y10B depth sensor
    #[cfg(all(target_arch = "x86_64", feature = "freedepth"))]
    fn is_depth_format(format: &CameraFormat) -> bool {
        format.pixel_format == "Y10B"
    }

    /// Internal method to create preview pipeline
    fn create_pipeline(&mut self) -> BackendResult<()> {
        use super::is_kernel_depth_device;

        let device = self
            .current_device
            .clone()
            .ok_or_else(|| BackendError::Other("No device set".to_string()))?;
        let format = self
            .current_format
            .clone()
            .ok_or_else(|| BackendError::Other("No format set".to_string()))?;

        debug!(
            device_path = %device.path,
            is_kernel_depth = is_kernel_depth_device(&device.path),
            is_freedepth = is_depth_native_device(&device.path),
            "create_pipeline checking device type"
        );

        // Check if this is a kernel depth device - use V4L2 kernel backend
        if is_kernel_depth_device(&device.path) {
            info!(device = %device.name, "Using V4L2 kernel depth backend");
            return self.create_kernel_depth_pipeline(&device, &format);
        }

        // Check if this is a freedepth depth camera device (only when no kernel driver)
        #[cfg(all(target_arch = "x86_64", feature = "freedepth"))]
        if is_depth_native_device(&device.path) {
            info!(device = %device.name, "Using native depth camera backend (freedepth)");
            return self.create_depth_camera_pipeline(&device, &format);
        }

        #[cfg(all(target_arch = "x86_64", feature = "freedepth"))]
        {
            if Self::is_depth_format(&format) {
                // Y10B depth format - use V4L2 direct capture
                return self.create_depth_pipeline(&device, &format);
            }
        }
        // Standard format - use GStreamer via PipeWire
        self.create_gstreamer_pipeline(&device, &format)
    }

    /// Create kernel depth camera pipeline via V4L2 with depth controls
    fn create_kernel_depth_pipeline(
        &mut self,
        device: &CameraDevice,
        format: &CameraFormat,
    ) -> BackendResult<()> {
        use super::V4l2KernelDepthBackend;

        info!(
            device = %device.name,
            format = %format,
            "Creating kernel depth camera pipeline"
        );

        // Create kernel depth backend
        let mut kernel_backend = V4l2KernelDepthBackend::new();
        kernel_backend.initialize(device, format)?;

        // Use the KernelDepth pipeline variant
        self.active_pipeline = Some(ActivePipeline::KernelDepth(kernel_backend));

        info!("Kernel depth camera pipeline created successfully");
        Ok(())
    }

    /// Create native depth camera pipeline via freedepth
    fn create_depth_camera_pipeline(
        &mut self,
        device: &CameraDevice,
        format: &CameraFormat,
    ) -> BackendResult<()> {
        info!(
            device = %device.name,
            format = %format,
            "Creating native depth camera pipeline via freedepth"
        );

        // Create native depth camera backend
        let mut depth_backend = NativeDepthBackend::new();
        depth_backend.initialize(device, format)?;

        self.active_pipeline = Some(ActivePipeline::DepthCamera(depth_backend));

        info!("Native depth camera pipeline created successfully");
        Ok(())
    }

    /// Create GStreamer pipeline for standard formats
    fn create_gstreamer_pipeline(
        &mut self,
        device: &CameraDevice,
        format: &CameraFormat,
    ) -> BackendResult<()> {
        // Create frame channel
        let (sender, receiver) = cosmic::iced::futures::channel::mpsc::channel(100);

        // Create pipeline
        let pipeline = pipeline::PipeWirePipeline::new(device, format, sender.clone())?;
        pipeline.start()?;

        self.active_pipeline = Some(ActivePipeline::GStreamer(pipeline));
        self.frame_sender = Some(sender);
        self.frame_receiver = Some(receiver);

        Ok(())
    }

    /// Create V4L2 depth pipeline for Y10B format
    #[cfg(all(target_arch = "x86_64", feature = "freedepth"))]
    fn create_depth_pipeline(
        &mut self,
        device: &CameraDevice,
        format: &CameraFormat,
    ) -> BackendResult<()> {
        info!(
            device = %device.name,
            format = %format,
            "Creating V4L2 depth pipeline for Y10B format"
        );

        // Create channels for depth frames and processed frames
        let (depth_sender, mut depth_receiver): (DepthFrameSender, DepthFrameReceiver) =
            cosmic::iced::futures::channel::mpsc::channel(10);
        let (frame_sender, frame_receiver): (FrameSender, FrameReceiver) =
            cosmic::iced::futures::channel::mpsc::channel(100);

        // Create V4L2 depth pipeline
        let depth_pipeline = V4l2DepthPipeline::new(device, format, depth_sender)?;

        // Spawn task to process depth frames with GPU shader
        let width = format.width;
        let height = format.height;
        let mut frame_sender_clone = frame_sender.clone();

        let task_handle = tokio::spawn(async move {
            use futures::StreamExt;

            info!("Depth frame processing task started");

            while let Some(depth_frame) = depth_receiver.next().await {
                // Process with GPU shader
                // Check visualization modes from shared state
                let use_colormap = is_depth_colormap_enabled();
                let depth_only = is_depth_only_mode();
                match unpack_y10b_gpu(
                    &depth_frame.raw_data,
                    width,
                    height,
                    use_colormap,
                    depth_only,
                )
                .await
                {
                    Ok(result) => {
                        // Apply depth camera calibration if available to convert raw depth to mm
                        let calibrated_depth = if DepthController::is_initialized() {
                            // Convert raw 10-bit values (shifted to 16-bit) to millimeters
                            DepthController::convert_depth_to_mm(&result.depth_u16)
                                .map(|mm_values| Arc::from(mm_values.into_boxed_slice()))
                        } else {
                            // No calibration available, use raw values
                            Some(Arc::from(result.depth_u16.into_boxed_slice()))
                        };

                        // Create CameraFrame with RGBA preview and depth data
                        let camera_frame = CameraFrame {
                            width: result.width,
                            height: result.height,
                            data: Arc::from(result.rgba_preview.into_boxed_slice()),
                            format: PixelFormat::Depth16,
                            stride: result.width * 4,
                            captured_at: depth_frame.captured_at,
                            depth_data: calibrated_depth,
                            depth_width: result.width,
                            depth_height: result.height,
                            video_timestamp: None,
                        };

                        // Send to preview channel
                        if frame_sender_clone.try_send(camera_frame).is_err() {
                            debug!("Preview channel full, dropping depth frame");
                        }
                    }
                    Err(e) => {
                        warn!(error = %e, "Failed to unpack Y10B frame with GPU");
                    }
                }
            }

            info!("Depth frame processing task ended");
        });

        self.active_pipeline = Some(ActivePipeline::Depth(depth_pipeline));
        self.frame_sender = Some(frame_sender);
        self.frame_receiver = Some(frame_receiver);
        self.depth_task_handle = Some(task_handle);

        info!("V4L2 depth pipeline created successfully");
        Ok(())
    }
}

impl CameraBackend for PipeWireBackend {
    fn enumerate_cameras(&self) -> Vec<CameraDevice> {
        use super::{V4l2KernelDepthBackend, has_kernel_depth_driver};

        info!("Enumerating cameras (PipeWire + depth cameras)");

        let mut cameras = Vec::new();

        // Check if kernel depth driver is available - if so, use it exclusively
        // and skip freedepth entirely
        if has_kernel_depth_driver() {
            info!("Kernel depth driver detected - using V4L2 kernel backend for depth cameras");
            let kernel_backend = V4l2KernelDepthBackend::new();
            let kernel_cameras = kernel_backend.enumerate_cameras();
            if !kernel_cameras.is_empty() {
                info!(
                    count = kernel_cameras.len(),
                    "Found depth cameras via kernel driver"
                );
                cameras.extend(kernel_cameras);
            }
        } else {
            // No kernel driver - use freedepth on x86_64 (if feature enabled)
            #[cfg(all(target_arch = "x86_64", feature = "freedepth"))]
            {
                let depth_cameras = enumerate_depth_cameras();
                if !depth_cameras.is_empty() {
                    info!(
                        count = depth_cameras.len(),
                        "Found depth cameras via freedepth (no kernel driver)"
                    );
                    cameras.extend(depth_cameras);
                }
            }
        }

        // Then enumerate other cameras via PipeWire (excludes depth camera V4L2 devices)
        if let Some(pw_cameras) = enumerate_pipewire_cameras() {
            info!(count = pw_cameras.len(), "Found cameras via PipeWire");
            cameras.extend(pw_cameras);
        }

        info!(total = cameras.len(), "Total cameras enumerated");
        cameras
    }

    fn get_formats(&self, device: &CameraDevice, _video_mode: bool) -> Vec<CameraFormat> {
        use super::{V4l2KernelDepthBackend, is_kernel_depth_device};

        // Check for kernel depth device first
        if is_kernel_depth_device(&device.path) {
            info!(device_path = %device.path, "Getting formats for kernel depth camera device");
            let kernel_backend = V4l2KernelDepthBackend::new();
            return kernel_backend.get_formats(device, _video_mode);
        }

        // Use native formats for freedepth depth camera devices (only if no kernel driver)
        #[cfg(all(target_arch = "x86_64", feature = "freedepth"))]
        if is_depth_native_device(&device.path) {
            info!(device_path = %device.path, "Getting formats for freedepth depth camera device");
            return get_depth_formats(device);
        }

        // Use PipeWire formats for other devices
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
        info!("Shutting down backend");

        // Cancel depth processing task if running
        if let Some(handle) = self.depth_task_handle.take() {
            handle.abort();
        }

        // Stop pipeline
        if let Some(pipeline) = self.active_pipeline.take() {
            match pipeline {
                ActivePipeline::GStreamer(p) => p.stop()?,
                #[cfg(all(target_arch = "x86_64", feature = "freedepth"))]
                ActivePipeline::Depth(p) => p.stop()?,
                ActivePipeline::DepthCamera(mut d) => {
                    d.shutdown()?;
                }
                ActivePipeline::KernelDepth(mut k) => {
                    k.shutdown()?;
                }
            }
        }

        // Clear state
        self.frame_sender = None;
        self.frame_receiver = None;
        self.current_device = None;
        self.current_format = None;

        info!("Backend shut down");
        Ok(())
    }

    fn is_initialized(&self) -> bool {
        self.active_pipeline.is_some() && self.current_device.is_some()
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
        debug!("Capturing photo");

        match &self.active_pipeline {
            Some(ActivePipeline::GStreamer(pipeline)) => pipeline.capture_frame(),
            #[cfg(all(target_arch = "x86_64", feature = "freedepth"))]
            Some(ActivePipeline::Depth(_)) => {
                // For depth sensor, capture is handled via the frame stream
                // The UI should use the last received frame
                Err(BackendError::Other(
                    "Use frame stream for depth capture".to_string(),
                ))
            }
            Some(ActivePipeline::DepthCamera(depth)) => {
                // Capture from native depth camera backend
                depth.capture_photo()
            }
            Some(ActivePipeline::KernelDepth(kernel)) => {
                // Capture from kernel depth camera backend
                kernel.capture_photo()
            }
            None => Err(BackendError::Other("Pipeline not initialized".to_string())),
        }
    }

    fn start_recording(&mut self, output_path: PathBuf) -> BackendResult<()> {
        info!(path = %output_path.display(), "Starting recording");

        match &mut self.active_pipeline {
            Some(ActivePipeline::GStreamer(pipeline)) => pipeline.start_recording(output_path),
            #[cfg(all(target_arch = "x86_64", feature = "freedepth"))]
            Some(ActivePipeline::Depth(_)) => Err(BackendError::Other(
                "Video recording not supported for depth sensor".to_string(),
            )),
            Some(ActivePipeline::DepthCamera(_)) => Err(BackendError::Other(
                "Video recording not yet implemented for native depth camera backend".to_string(),
            )),
            Some(ActivePipeline::KernelDepth(_)) => Err(BackendError::Other(
                "Video recording not yet implemented for kernel depth camera backend".to_string(),
            )),
            None => Err(BackendError::Other("Pipeline not initialized".to_string())),
        }
    }

    fn stop_recording(&mut self) -> BackendResult<PathBuf> {
        info!("Stopping recording");

        match &mut self.active_pipeline {
            Some(ActivePipeline::GStreamer(pipeline)) => pipeline.stop_recording(),
            #[cfg(all(target_arch = "x86_64", feature = "freedepth"))]
            Some(ActivePipeline::Depth(_)) => Err(BackendError::NoRecordingInProgress),
            Some(ActivePipeline::DepthCamera(_)) => Err(BackendError::NoRecordingInProgress),
            Some(ActivePipeline::KernelDepth(_)) => Err(BackendError::NoRecordingInProgress),
            None => Err(BackendError::Other("Pipeline not initialized".to_string())),
        }
    }

    fn is_recording(&self) -> bool {
        match &self.active_pipeline {
            Some(ActivePipeline::GStreamer(pipeline)) => pipeline.is_recording(),
            #[cfg(all(target_arch = "x86_64", feature = "freedepth"))]
            Some(ActivePipeline::Depth(_)) => false,
            Some(ActivePipeline::DepthCamera(_)) => false,
            Some(ActivePipeline::KernelDepth(_)) => false,
            None => false,
        }
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
