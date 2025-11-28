// SPDX-License-Identifier: MPL-2.0

//! Virtual camera backend for streaming filtered video to PipeWire
//!
//! This module creates a virtual camera device that other applications (like
//! video conferencing software) can use as a camera source. The video output
//! has filters applied and uses NV12 pixel format (no conversion needed).
//!
//! # Architecture
//!
//! ```text
//! Camera Frames (NV12)
//!        │
//!        ▼
//! ┌──────────────────┐
//! │ GPU Filter       │  ← Applies selected filter via wgpu
//! │ (NV12 → RGB →    │    (with CPU fallback)
//! │  filtered NV12)  │
//! └──────────────────┘
//!        │
//!        ▼
//! ┌──────────────────┐
//! │ GStreamer Sink   │  ← appsrc (NV12) → pipewiresink
//! │ (PipeWire)       │    No conversion needed!
//! └──────────────────┘
//!        │
//!        ▼
//!   Video Apps (Zoom, Teams, etc.)
//! ```

mod filters;
#[allow(dead_code)]
mod gpu_filter;
mod pipeline;

pub use filters::apply_filter_cpu;
pub use pipeline::VirtualCameraPipeline;

use crate::app::FilterType;
use crate::backends::camera::types::{BackendError, BackendResult, CameraFrame};
use tracing::{debug, error, info};

/// Virtual camera manager
///
/// Manages the lifecycle of the virtual camera pipeline and handles
/// frame processing with CPU-based filters (to avoid GPU readback blocking).
pub struct VirtualCameraManager {
    /// The GStreamer pipeline for virtual camera output
    pipeline: Option<VirtualCameraPipeline>,
    /// Current filter being applied
    current_filter: FilterType,
    /// Whether currently streaming
    streaming: bool,
    /// Output resolution (width, height)
    output_size: (u32, u32),
}

impl VirtualCameraManager {
    /// Create a new virtual camera manager
    pub fn new() -> Self {
        Self {
            pipeline: None,
            current_filter: FilterType::Standard,
            streaming: false,
            output_size: (1280, 720),
        }
    }

    /// Start streaming to virtual camera
    ///
    /// Creates a PipeWire virtual camera node that will be visible to other applications.
    /// Uses CPU-based filtering to avoid GPU readback blocking.
    pub fn start(&mut self, width: u32, height: u32) -> BackendResult<()> {
        if self.streaming {
            return Err(BackendError::Other(
                "Virtual camera already streaming".into(),
            ));
        }

        info!(
            width,
            height, "Starting virtual camera (CPU filtering mode)"
        );

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
    /// Applies the current filter using CPU processing and sends the result
    /// to the virtual camera sink. CPU filtering is used instead of GPU to
    /// avoid blocking GPU readback that would freeze the preview.
    pub fn push_frame(&mut self, frame: &CameraFrame) -> BackendResult<()> {
        let pipeline = self
            .pipeline
            .as_ref()
            .ok_or_else(|| BackendError::Other("Virtual camera not started".into()))?;

        // Apply filter using CPU (runs on dedicated thread, so blocking is OK)
        // This avoids the GPU readback that was causing preview freezes
        let nv12_data = apply_filter_cpu(frame, self.current_filter)?;

        // Push NV12 data to pipeline
        pipeline.push_frame_nv12(&nv12_data, frame.width, frame.height)
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
    /// Currently always returns false (using CPU filtering to avoid blocking)
    pub fn is_gpu_accelerated(&self) -> bool {
        false
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
