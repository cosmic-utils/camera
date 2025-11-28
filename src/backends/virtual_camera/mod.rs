// SPDX-License-Identifier: MPL-2.0

//! Virtual camera backend for streaming filtered video to PipeWire
//!
//! This module creates a virtual camera device that other applications (like
//! video conferencing software) can use as a camera source. The video output
//! has filters applied using the shared GPU filter pipeline.
//!
//! # Architecture
//!
//! ```text
//! Camera Frames (RGBA)
//!        │
//!        ▼
//! ┌──────────────────┐
//! │ GPU Filter       │  ← Uses shared filter shaders
//! │ (RGBA → RGBA)    │    Direct RGBA texture processing
//! └──────────────────┘
//!        │
//!        ▼
//! ┌──────────────────┐
//! │ GStreamer Sink   │  ← appsrc → videoconvert → pipewiresink
//! │ (PipeWire)       │    Format negotiation handled by GStreamer
//! └──────────────────┘
//!        │
//!        ▼
//!   Video Apps (Zoom, Teams, etc.)
//! ```

mod gpu_filter;
mod pipeline;

pub use gpu_filter::GpuFilterRenderer;
pub use pipeline::VirtualCameraPipeline;

use crate::app::FilterType;
use crate::backends::camera::types::{BackendError, BackendResult, CameraFrame};
use std::sync::Arc;
use tokio::sync::Mutex;
use tracing::{debug, error, info, warn};

/// Shared GPU filter renderer instance for virtual camera
/// Uses OnceLock + Mutex for lazy initialization and thread-safe access
static GPU_FILTER_RENDERER: std::sync::OnceLock<Arc<Mutex<Option<GpuFilterRenderer>>>> =
    std::sync::OnceLock::new();

/// Virtual camera manager
///
/// Manages the lifecycle of the virtual camera pipeline and handles
/// frame processing with GPU-accelerated filters (with software rendering fallback).
pub struct VirtualCameraManager {
    /// The GStreamer pipeline for virtual camera output
    pipeline: Option<VirtualCameraPipeline>,
    /// Current filter being applied
    current_filter: FilterType,
    /// Whether currently streaming
    streaming: bool,
    /// Output resolution (width, height)
    output_size: (u32, u32),
    /// Whether GPU acceleration is available
    gpu_available: bool,
}

impl VirtualCameraManager {
    /// Create a new virtual camera manager
    pub fn new() -> Self {
        Self {
            pipeline: None,
            current_filter: FilterType::Standard,
            streaming: false,
            output_size: (1280, 720),
            gpu_available: false,
        }
    }

    /// Start streaming to virtual camera
    ///
    /// Creates a PipeWire virtual camera node that will be visible to other applications.
    /// Uses GPU-accelerated filtering with software rendering fallback.
    pub fn start(&mut self, width: u32, height: u32) -> BackendResult<()> {
        if self.streaming {
            return Err(BackendError::Other(
                "Virtual camera already streaming".into(),
            ));
        }

        info!(width, height, "Starting virtual camera");

        // Create and start the pipeline
        let pipeline = VirtualCameraPipeline::new(width, height)?;
        pipeline.start()?;

        self.pipeline = Some(pipeline);
        self.output_size = (width, height);
        self.streaming = true;

        info!("Virtual camera started successfully");
        Ok(())
    }

    /// Stop streaming to virtual camera
    pub fn stop(&mut self) -> BackendResult<()> {
        if !self.streaming {
            return Err(BackendError::Other("Virtual camera not streaming".into()));
        }

        info!("Stopping virtual camera");

        if let Some(pipeline) = self.pipeline.take() {
            pipeline.stop()?;
        }

        self.streaming = false;
        info!("Virtual camera stopped");
        Ok(())
    }

    /// Check if currently streaming
    pub fn is_streaming(&self) -> bool {
        self.streaming
    }

    /// Set the current filter to apply
    pub fn set_filter(&mut self, filter: FilterType) {
        self.current_filter = filter;
        debug!(?filter, "Virtual camera filter changed");
    }

    /// Push a frame to the virtual camera
    ///
    /// Applies the current filter using the shared GPU filter pipeline
    /// and sends the result to the virtual camera sink.
    ///
    /// This method is synchronous and can be called from a dedicated thread.
    /// GPU initialization happens lazily on first call.
    pub fn push_frame(&mut self, frame: &CameraFrame) -> BackendResult<()> {
        let pipeline = self
            .pipeline
            .as_ref()
            .ok_or_else(|| BackendError::Other("Virtual camera not started".into()))?;

        // For standard filter, just pass through the frame data
        if self.current_filter == FilterType::Standard {
            return self.push_passthrough_frame(pipeline, frame);
        }

        // Try to use GPU filter renderer (initialize lazily if needed)
        let cell = GPU_FILTER_RENDERER.get_or_init(|| Arc::new(Mutex::new(None)));

        // Try to lock without blocking - if locked, skip filtering this frame
        match cell.try_lock() {
            Ok(mut guard) => {
                // Initialize renderer if needed
                if guard.is_none() {
                    // Create a runtime for initialization only
                    match tokio::runtime::Handle::try_current() {
                        Ok(handle) => {
                            match handle.block_on(GpuFilterRenderer::new()) {
                                Ok(renderer) => {
                                    info!("GPU filter renderer initialized for virtual camera");
                                    *guard = Some(renderer);
                                }
                                Err(e) => {
                                    warn!(error = %e, "Failed to initialize GPU filter renderer");
                                }
                            }
                        }
                        Err(_) => {
                            // No tokio runtime, try to create one temporarily
                            match tokio::runtime::Runtime::new() {
                                Ok(rt) => {
                                    match rt.block_on(GpuFilterRenderer::new()) {
                                        Ok(renderer) => {
                                            info!("GPU filter renderer initialized for virtual camera");
                                            *guard = Some(renderer);
                                        }
                                        Err(e) => {
                                            warn!(error = %e, "Failed to initialize GPU filter renderer");
                                        }
                                    }
                                }
                                Err(e) => {
                                    warn!(error = %e, "Failed to create tokio runtime for GPU init");
                                }
                            }
                        }
                    }
                }

                // Apply filter if renderer is available
                if let Some(renderer) = guard.as_mut() {
                    match renderer.apply_filter(frame, self.current_filter) {
                        Ok(rgba_data) => {
                            self.gpu_available = true;
                            // Pass owned Vec directly - zero-copy to GStreamer
                            return pipeline.push_frame_rgba(rgba_data, frame.width, frame.height);
                        }
                        Err(e) => {
                            warn!(error = ?e, "GPU filter failed, using passthrough");
                            self.gpu_available = false;
                        }
                    }
                }
            }
            Err(_) => {
                // Mutex is locked, skip filtering this frame
                debug!("GPU renderer busy, using passthrough");
            }
        }

        // Fallback: passthrough without filter
        self.push_passthrough_frame(pipeline, frame)
    }

    /// Push a frame without filtering (passthrough)
    fn push_passthrough_frame(
        &self,
        pipeline: &VirtualCameraPipeline,
        frame: &CameraFrame,
    ) -> BackendResult<()> {
        // Extract RGBA data with stride handling
        let width = frame.width as usize;
        let height = frame.height as usize;
        let stride = frame.stride as usize;
        let row_bytes = width * 4; // RGBA = 4 bytes per pixel

        // If stride matches expected row size, use data directly
        // Clone the Arc - cheap refcount increment, no data copy
        if stride == row_bytes {
            return pipeline.push_frame_rgba(Arc::clone(&frame.data), frame.width, frame.height);
        }

        // Handle stride padding by copying row by row
        let mut rgba_data = vec![0u8; row_bytes * height];
        for y in 0..height {
            let src_start = y * stride;
            let dst_start = y * row_bytes;
            rgba_data[dst_start..dst_start + row_bytes]
                .copy_from_slice(&frame.data[src_start..src_start + row_bytes]);
        }

        // Pass owned Vec - zero-copy to GStreamer
        pipeline.push_frame_rgba(rgba_data, frame.width, frame.height)
    }

    /// Get the current filter
    pub fn current_filter(&self) -> FilterType {
        self.current_filter
    }

    /// Get the output size
    pub fn output_size(&self) -> (u32, u32) {
        self.output_size
    }

    /// Check if GPU filtering is being used
    pub fn is_gpu_accelerated(&self) -> bool {
        self.gpu_available
    }
}

impl Default for VirtualCameraManager {
    fn default() -> Self {
        Self::new()
    }
}

impl Drop for VirtualCameraManager {
    fn drop(&mut self) {
        if self.streaming {
            if let Err(e) = self.stop() {
                error!(?e, "Failed to stop virtual camera on drop");
            }
        }
    }
}
