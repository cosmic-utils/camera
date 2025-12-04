// SPDX-License-Identifier: GPL-3.0-only

//! GStreamer pipeline for virtual camera output
//!
//! Creates a pipeline that:
//! 1. Receives RGBA frames from the app (via appsrc)
//! 2. Converts to a format supported by the sink (via videoconvert)
//! 3. Outputs to either a PipeWire virtual camera node or V4L2 loopback device

use crate::backends::camera::types::{BackendError, BackendResult};
use crate::constants::VirtualCameraOutput;
use gstreamer::prelude::*;
use gstreamer_app::AppSrc;
use std::sync::atomic::{AtomicU64, Ordering};
use tracing::{debug, error, info, warn};

static FRAME_COUNTER: AtomicU64 = AtomicU64::new(0);

/// Virtual camera GStreamer pipeline
///
/// Supports two output modes:
/// - PipeWire: Uses pipewiresink to create a virtual camera for PipeWire-aware apps
/// - V4L2Loopback: Uses v4l2sink to write to a v4l2loopback device for wider compatibility
///
/// Accepts RGBA frames from the app and uses videoconvert for proper format negotiation.
pub struct VirtualCameraPipeline {
    pipeline: gstreamer::Pipeline,
    appsrc: AppSrc,
    width: u32,
    height: u32,
    #[allow(dead_code)] // Stored for potential future use (e.g., logging, diagnostics)
    output_type: VirtualCameraOutput,
}

impl VirtualCameraPipeline {
    /// Create a new virtual camera pipeline
    ///
    /// The pipeline accepts RGBA frames and outputs them to either:
    /// - PipeWire: A virtual camera node named "Camera (Virtual)"
    /// - V4L2Loopback: A /dev/video* loopback device
    ///
    /// Uses videoconvert for proper format negotiation with the sink.
    pub fn new(width: u32, height: u32, output_type: VirtualCameraOutput) -> BackendResult<Self> {
        info!(width, height, ?output_type, "Creating virtual camera pipeline");

        // Initialize GStreamer if needed
        gstreamer::init().map_err(|e| {
            BackendError::InitializationFailed(format!("GStreamer init failed: {}", e))
        })?;

        // Create pipeline elements
        let pipeline = gstreamer::Pipeline::new();

        // appsrc: receives RGBA frames from the app
        let appsrc = gstreamer::ElementFactory::make("appsrc")
            .name("virtual_camera_src")
            .build()
            .map_err(|e| {
                BackendError::InitializationFailed(format!("Failed to create appsrc: {}", e))
            })?;

        // videoconvert: handles format negotiation with sink
        let videoconvert = gstreamer::ElementFactory::make("videoconvert")
            .name("virtual_camera_convert")
            .build()
            .map_err(|e| {
                BackendError::InitializationFailed(format!("Failed to create videoconvert: {}", e))
            })?;

        // Create sink based on output type
        let sink = Self::create_sink(output_type)?;

        // Configure appsrc
        let appsrc = appsrc.downcast::<AppSrc>().map_err(|_| {
            BackendError::InitializationFailed("Failed to downcast to AppSrc".into())
        })?;

        // Set caps for RGBA input
        let caps = gstreamer::Caps::builder("video/x-raw")
            .field("format", "RGBA")
            .field("width", width as i32)
            .field("height", height as i32)
            .field("framerate", gstreamer::Fraction::new(30, 1))
            .build();

        appsrc.set_caps(Some(&caps));
        appsrc.set_format(gstreamer::Format::Time);
        appsrc.set_is_live(true);
        appsrc.set_do_timestamp(true);
        // Disable blocking to prevent stalls
        appsrc.set_property("block", false);
        // Set stream type to stream for live data
        appsrc.set_property_from_str("stream-type", "stream");

        // Add elements to pipeline
        pipeline
            .add_many([appsrc.upcast_ref(), &videoconvert, &sink])
            .map_err(|e| {
                BackendError::InitializationFailed(format!("Failed to add elements: {}", e))
            })?;

        // Link elements: appsrc -> videoconvert -> sink
        appsrc.link(&videoconvert).map_err(|e| {
            BackendError::InitializationFailed(format!(
                "Failed to link appsrc to videoconvert: {}",
                e
            ))
        })?;
        videoconvert.link(&sink).map_err(|e| {
            BackendError::InitializationFailed(format!(
                "Failed to link videoconvert to sink: {}",
                e
            ))
        })?;

        let sink_name = match output_type {
            VirtualCameraOutput::PipeWire => "pipewiresink",
            VirtualCameraOutput::V4L2Loopback => "v4l2sink",
        };
        info!(
            "Virtual camera pipeline created successfully (appsrc -> videoconvert -> {})",
            sink_name
        );

        Ok(Self {
            pipeline,
            appsrc,
            width,
            height,
            output_type,
        })
    }

    /// Create the appropriate sink element based on output type
    fn create_sink(output_type: VirtualCameraOutput) -> BackendResult<gstreamer::Element> {
        match output_type {
            VirtualCameraOutput::PipeWire => Self::create_pipewire_sink(),
            VirtualCameraOutput::V4L2Loopback => Self::create_v4l2_sink(),
        }
    }

    /// Create a PipeWire sink for virtual camera output
    fn create_pipewire_sink() -> BackendResult<gstreamer::Element> {
        let pipewiresink = gstreamer::ElementFactory::make("pipewiresink")
            .name("virtual_camera_sink")
            .build()
            .map_err(|e| {
                BackendError::InitializationFailed(format!("Failed to create pipewiresink: {}", e))
            })?;

        // Configure pipewiresink for virtual camera mode
        // "provide" mode creates a video source that other applications can use
        pipewiresink.set_property_from_str("mode", "provide");

        // Create stream properties as a GstStructure
        // media.role = "Camera" is required for xdg-desktop-portal to recognize this as a camera
        let stream_props = gstreamer::Structure::builder("props")
            .field("media.class", "Video/Source")
            .field("media.role", "Camera")
            .field("node.name", "camera-virtual")
            .field("node.description", "Camera (Virtual)")
            .build();
        pipewiresink.set_property("stream-properties", &stream_props);

        Ok(pipewiresink)
    }

    /// Create a V4L2 sink for v4l2loopback device output
    fn create_v4l2_sink() -> BackendResult<gstreamer::Element> {
        // Find the v4l2loopback device
        let device_path = VirtualCameraOutput::v4l2loopback_device().ok_or_else(|| {
            BackendError::InitializationFailed(
                "No v4l2loopback device found. Please load the v4l2loopback kernel module.".into(),
            )
        })?;

        info!(device = %device_path, "Using v4l2loopback device");

        let v4l2sink = gstreamer::ElementFactory::make("v4l2sink")
            .name("virtual_camera_sink")
            .build()
            .map_err(|e| {
                BackendError::InitializationFailed(format!("Failed to create v4l2sink: {}", e))
            })?;

        // Set the device path
        v4l2sink.set_property("device", &device_path);

        // Enable async mode for better performance
        v4l2sink.set_property("async", false);
        v4l2sink.set_property("sync", false);

        Ok(v4l2sink)
    }

    /// Start the pipeline
    pub fn start(&self) -> BackendResult<()> {
        debug!("Starting virtual camera pipeline");

        self.pipeline
            .set_state(gstreamer::State::Playing)
            .map_err(|e| {
                BackendError::InitializationFailed(format!("Failed to start pipeline: {}", e))
            })?;

        // For live pipelines with appsrc, we don't need to wait for the state
        // change to complete - we can start pushing frames immediately and
        // GStreamer will buffer them while the pipeline finishes transitioning.
        // This avoids blocking for up to 5 seconds while PipeWire negotiates.

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

    /// Push an RGBA frame to the virtual camera
    ///
    /// The frame data must be in RGBA format with the correct dimensions.
    /// RGBA format: 4 bytes per pixel (width * height * 4 bytes total)
    ///
    /// This method accepts owned data to enable zero-copy buffer passing to GStreamer.
    /// The data is wrapped directly without copying.
    pub fn push_frame_rgba<T: AsRef<[u8]> + Send + 'static>(
        &self,
        rgba_data: T,
        width: u32,
        height: u32,
    ) -> BackendResult<()> {
        // Validate dimensions match
        if width != self.width || height != self.height {
            return Err(BackendError::FormatNotSupported(format!(
                "Frame size {}x{} doesn't match pipeline {}x{}",
                width, height, self.width, self.height
            )));
        }

        // Validate data size (RGBA = 4 bytes per pixel)
        let expected_size = (width * height * 4) as usize;
        let data_ref = rgba_data.as_ref();
        if data_ref.len() != expected_size {
            return Err(BackendError::FormatNotSupported(format!(
                "Frame data size {} doesn't match expected {} for {}x{} RGBA",
                data_ref.len(),
                expected_size,
                width,
                height
            )));
        }

        // Create buffer from owned data - zero-copy wrapping
        // GStreamer will manage the memory and free it when done
        let buffer = gstreamer::Buffer::from_slice(rgba_data);

        // Push buffer to appsrc
        match self.appsrc.push_buffer(buffer) {
            Ok(_) => {
                let count = FRAME_COUNTER.fetch_add(1, Ordering::Relaxed);
                if count % 100 == 0 {
                    debug!(
                        frame = count,
                        "Virtual camera frames pushed (RGBA zero-copy)"
                    );
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
