// SPDX-License-Identifier: MPL-2.0

//! GStreamer pipeline for virtual camera output via PipeWire
//!
//! Creates a pipeline that:
//! 1. Receives NV12 frames from the app (via appsrc)
//! 2. Converts format as needed (via videoconvert)
//! 3. Outputs to a PipeWire virtual camera node

use crate::backends::camera::types::{BackendError, BackendResult};
use gstreamer::prelude::*;
use gstreamer_app::AppSrc;
use std::sync::atomic::{AtomicU64, Ordering};
use tracing::{debug, error, info, warn};

static FRAME_COUNTER: AtomicU64 = AtomicU64::new(0);

/// Virtual camera GStreamer pipeline
///
/// Uses pipewiresink to create a virtual camera device that other
/// applications can use as a video source. Accepts NV12 format directly
/// to avoid any conversion overhead.
pub struct VirtualCameraPipeline {
    pipeline: gstreamer::Pipeline,
    appsrc: AppSrc,
    width: u32,
    height: u32,
}

impl VirtualCameraPipeline {
    /// Create a new virtual camera pipeline
    ///
    /// The pipeline accepts NV12 frames and outputs them directly to a PipeWire
    /// virtual camera node named "COSMIC Camera (Virtual)".
    pub fn new(width: u32, height: u32) -> BackendResult<Self> {
        info!(width, height, "Creating virtual camera pipeline (NV12)");

        // Initialize GStreamer if needed
        gstreamer::init().map_err(|e| {
            BackendError::InitializationFailed(format!("GStreamer init failed: {}", e))
        })?;

        // Create pipeline elements
        let pipeline = gstreamer::Pipeline::new();

        // appsrc: receives NV12 frames from the app
        let appsrc = gstreamer::ElementFactory::make("appsrc")
            .name("virtual_camera_src")
            .build()
            .map_err(|e| {
                BackendError::InitializationFailed(format!("Failed to create appsrc: {}", e))
            })?;

        // videoconvert: handles format negotiation between appsrc and pipewiresink
        let videoconvert = gstreamer::ElementFactory::make("videoconvert")
            .name("virtual_camera_convert")
            .build()
            .map_err(|e| {
                BackendError::InitializationFailed(format!("Failed to create videoconvert: {}", e))
            })?;

        // pipewiresink: output to PipeWire as a virtual camera
        let pipewiresink = gstreamer::ElementFactory::make("pipewiresink")
            .name("virtual_camera_sink")
            .build()
            .map_err(|e| {
                BackendError::InitializationFailed(format!("Failed to create pipewiresink: {}", e))
            })?;

        // Configure appsrc
        let appsrc = appsrc.downcast::<AppSrc>().map_err(|_| {
            BackendError::InitializationFailed("Failed to downcast to AppSrc".into())
        })?;

        // Set caps for NV12 input
        let caps = gstreamer::Caps::builder("video/x-raw")
            .field("format", "NV12")
            .field("width", width as i32)
            .field("height", height as i32)
            .field("framerate", gstreamer::Fraction::new(30, 1))
            .build();

        appsrc.set_caps(Some(&caps));
        appsrc.set_format(gstreamer::Format::Time);
        appsrc.set_is_live(true);
        appsrc.set_do_timestamp(true);

        // Configure pipewiresink for virtual camera mode
        // "provide" mode creates a video source that other applications can use
        // The node name will appear as the camera device name
        pipewiresink.set_property_from_str("mode", "provide");

        // Create stream properties as a GstStructure
        // media.role = "Camera" is required for xdg-desktop-portal to recognize this as a camera
        let stream_props = gstreamer::Structure::builder("props")
            .field("media.class", "Video/Source")
            .field("media.role", "Camera")
            .field("node.name", "cosmic-camera-virtual")
            .field("node.description", "COSMIC Camera (Virtual)")
            .build();
        pipewiresink.set_property("stream-properties", &stream_props);

        // Add elements to pipeline
        pipeline
            .add_many([appsrc.upcast_ref(), &videoconvert, &pipewiresink])
            .map_err(|e| {
                BackendError::InitializationFailed(format!("Failed to add elements: {}", e))
            })?;

        // Link elements: appsrc -> videoconvert -> pipewiresink
        gstreamer::Element::link_many([appsrc.upcast_ref(), &videoconvert, &pipewiresink])
            .map_err(|e| {
                BackendError::InitializationFailed(format!("Failed to link elements: {}", e))
            })?;

        info!(
            "Virtual camera pipeline created successfully (NV12 -> videoconvert -> pipewiresink)"
        );

        Ok(Self {
            pipeline,
            appsrc,
            width,
            height,
        })
    }

    /// Start the pipeline
    pub fn start(&self) -> BackendResult<()> {
        debug!("Starting virtual camera pipeline");

        self.pipeline
            .set_state(gstreamer::State::Playing)
            .map_err(|e| {
                BackendError::InitializationFailed(format!("Failed to start pipeline: {}", e))
            })?;

        // Wait for state change to complete
        let (result, _state, _pending) = self.pipeline.state(gstreamer::ClockTime::from_seconds(5));
        if result.is_err() {
            return Err(BackendError::InitializationFailed(
                "Pipeline failed to reach Playing state".into(),
            ));
        }

        info!("Virtual camera pipeline started");
        Ok(())
    }

    /// Stop the pipeline
    pub fn stop(&self) -> BackendResult<()> {
        debug!("Stopping virtual camera pipeline");

        // Send EOS to gracefully stop
        self.appsrc
            .end_of_stream()
            .map_err(|e| BackendError::Other(format!("Failed to send EOS: {}", e)))?;

        // Set to Null state
        self.pipeline
            .set_state(gstreamer::State::Null)
            .map_err(|e| BackendError::Other(format!("Failed to stop pipeline: {}", e)))?;

        info!("Virtual camera pipeline stopped");
        Ok(())
    }

    /// Push an NV12 frame to the virtual camera
    ///
    /// The frame data must be in NV12 format with the correct dimensions.
    /// NV12 format: Y plane (width * height bytes) followed by UV plane (width * height / 2 bytes)
    pub fn push_frame_nv12(&self, nv12_data: &[u8], width: u32, height: u32) -> BackendResult<()> {
        // Validate dimensions match
        if width != self.width || height != self.height {
            return Err(BackendError::FormatNotSupported(format!(
                "Frame size {}x{} doesn't match pipeline {}x{}",
                width, height, self.width, self.height
            )));
        }

        // Validate data size (NV12 = 1.5 bytes per pixel)
        let expected_size = (width * height * 3 / 2) as usize;
        if nv12_data.len() != expected_size {
            return Err(BackendError::FormatNotSupported(format!(
                "Frame data size {} doesn't match expected {} for {}x{} NV12",
                nv12_data.len(),
                expected_size,
                width,
                height
            )));
        }

        // Create buffer from data
        let mut buffer = gstreamer::Buffer::with_size(expected_size)
            .map_err(|e| BackendError::Other(format!("Failed to create buffer: {}", e)))?;

        {
            let buffer_ref = buffer.get_mut().ok_or_else(|| {
                BackendError::Other("Failed to get mutable buffer reference".into())
            })?;

            let mut map = buffer_ref
                .map_writable()
                .map_err(|e| BackendError::Other(format!("Failed to map buffer: {}", e)))?;

            map.copy_from_slice(nv12_data);
        }

        // Push buffer to appsrc
        match self.appsrc.push_buffer(buffer) {
            Ok(_) => {
                let count = FRAME_COUNTER.fetch_add(1, Ordering::Relaxed);
                if count % 100 == 0 {
                    debug!(frame = count, "Virtual camera frames pushed (NV12)");
                }
                Ok(())
            }
            Err(e) => {
                warn!(?e, "Failed to push frame to virtual camera");
                Err(BackendError::Other(format!(
                    "Failed to push frame: {:?}",
                    e
                )))
            }
        }
    }

    /// Get the pipeline's current state
    pub fn state(&self) -> gstreamer::State {
        let (_success, state, _pending) = self.pipeline.state(gstreamer::ClockTime::ZERO);
        state
    }
}

impl Drop for VirtualCameraPipeline {
    fn drop(&mut self) {
        debug!("Dropping virtual camera pipeline");
        if let Err(e) = self.pipeline.set_state(gstreamer::State::Null) {
            error!(?e, "Failed to set pipeline to Null on drop");
        }
    }
}
