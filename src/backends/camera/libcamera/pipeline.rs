// SPDX-License-Identifier: GPL-3.0-only

//! Libcamera GStreamer pipeline for camera capture
//!
//! This module handles the creation of GStreamer pipelines using libcamerasrc,
//! specifically designed for mobile Linux devices with libcamera support.

use super::super::types::*;
use crate::constants::{pipeline, timing};
use gstreamer::prelude::*;
use gstreamer_app::AppSink;
use gstreamer_video::VideoInfo;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;
use tracing::{debug, error, info, warn};

static FRAME_COUNTER: AtomicU64 = AtomicU64::new(0);
static DECODE_TIME_US: AtomicU64 = AtomicU64::new(0);
static SEND_TIME_US: AtomicU64 = AtomicU64::new(0);

/// Libcamera camera pipeline
///
/// GStreamer pipeline implementation using libcamerasrc for camera capture.
/// Designed for mobile Linux devices (PinePhone, Librem 5, etc.)
pub struct LibcameraPipeline {
    pipeline: gstreamer::Pipeline,
    _appsink: AppSink,
    recording: bool,
}

impl LibcameraPipeline {
    /// Create a new libcamera pipeline
    pub fn new(
        device: &CameraDevice,
        format: &CameraFormat,
        frame_sender: FrameSender,
    ) -> BackendResult<Self> {
        info!(
            device = %device.name,
            format = %format,
            "Creating libcamera pipeline"
        );

        // Initialize GStreamer
        debug!("Initializing GStreamer");
        gstreamer::init().map_err(|e| BackendError::InitializationFailed(e.to_string()))?;
        debug!("GStreamer initialized successfully");

        // Build the pipeline string
        let pipeline_str = build_libcamera_pipeline_string(device, format);
        info!(pipeline = %pipeline_str, "Creating libcamera pipeline");

        // Parse and create the pipeline
        let pipeline = gstreamer::parse::launch(&pipeline_str)
            .map_err(|e| {
                BackendError::InitializationFailed(format!("Failed to parse pipeline: {}", e))
            })?
            .dynamic_cast::<gstreamer::Pipeline>()
            .map_err(|_| {
                BackendError::InitializationFailed("Failed to cast to pipeline".to_string())
            })?;

        // Get the appsink element
        debug!("Getting appsink element");
        let appsink = pipeline
            .by_name("sink")
            .ok_or_else(|| BackendError::InitializationFailed("Failed to get appsink".to_string()))?
            .dynamic_cast::<AppSink>()
            .map_err(|_| {
                BackendError::InitializationFailed("Failed to cast appsink".to_string())
            })?;
        debug!("Got appsink element");

        // Configure appsink for maximum performance
        debug!("Configuring appsink");
        appsink.set_property("emit-signals", true);
        appsink.set_property("sync", false); // Disable sync for lowest latency

        // Mobile devices typically have lower framerates, use less buffering
        let buffer_count = if format.framerate.unwrap_or(0) > 30 {
            3
        } else {
            pipeline::MAX_BUFFERS
        };
        appsink.set_property("max-buffers", buffer_count);
        appsink.set_property("drop", true);
        appsink.set_property("enable-last-sample", false);

        debug!(
            buffer_count,
            framerate = format.framerate,
            "Appsink configured for libcamera"
        );

        // Set up callback for new samples with performance tracking
        debug!("Setting up frame callback");
        appsink.set_callbacks(
            gstreamer_app::AppSinkCallbacks::builder()
                .new_sample(move |appsink| {
                    let frame_start = Instant::now();
                    let frame_num = FRAME_COUNTER.fetch_add(1, Ordering::Relaxed);

                    // Pull and decode sample
                    let sample = match appsink.pull_sample() {
                        Ok(s) => s,
                        Err(e) => {
                            if frame_num % 30 == 0 {
                                error!(frame = frame_num, error = ?e, "Failed to pull sample");
                            }
                            return Err(gstreamer::FlowError::Eos);
                        }
                    };

                    let buffer = sample.buffer().ok_or_else(|| {
                        if frame_num % 30 == 0 {
                            error!(frame = frame_num, "No buffer in sample");
                        }
                        gstreamer::FlowError::Error
                    })?;

                    // Check buffer flags for incomplete/corrupted frames
                    let buffer_flags = buffer.flags();
                    if buffer_flags.contains(gstreamer::BufferFlags::CORRUPTED) {
                        if frame_num % 30 == 0 {
                            warn!(frame = frame_num, "Buffer marked as corrupted, skipping frame");
                        }
                        return Err(gstreamer::FlowError::Error);
                    }

                    let caps = sample.caps().ok_or_else(|| {
                        if frame_num % 30 == 0 {
                            error!(frame = frame_num, "No caps in sample");
                        }
                        gstreamer::FlowError::Error
                    })?;

                    let video_info = VideoInfo::from_caps(caps).map_err(|e| {
                        if frame_num % 30 == 0 {
                            error!(frame = frame_num, error = ?e, "Failed to get video info");
                        }
                        gstreamer::FlowError::Error
                    })?;

                    let map = buffer.map_readable().map_err(|e| {
                        if frame_num % 30 == 0 {
                            error!(frame = frame_num, error = ?e, "Failed to map buffer");
                        }
                        gstreamer::FlowError::Error
                    })?;

                    let decode_time = frame_start.elapsed();
                    DECODE_TIME_US.store(decode_time.as_micros() as u64, Ordering::Relaxed);

                    // Extract stride information for RGBA format
                    let stride = video_info.stride()[0] as u32;

                    // Log stride info every 60 frames for debugging
                    if frame_num % 60 == 0 {
                        info!(
                            frame = frame_num,
                            width = video_info.width(),
                            height = video_info.height(),
                            stride,
                            "Frame stride information (libcamera)"
                        );
                    }

                    // Use Arc::from to avoid intermediate Vec allocation
                    let frame = CameraFrame {
                        width: video_info.width(),
                        height: video_info.height(),
                        data: Arc::from(map.as_slice()),
                        format: PixelFormat::RGBA,
                        stride,
                        captured_at: frame_start,
                    };

                    // Send frame to the app (non-blocking using try_send)
                    let send_start = Instant::now();
                    let mut sender = frame_sender.clone();
                    match sender.try_send(frame) {
                        Ok(_) => {
                            let send_time = send_start.elapsed();
                            SEND_TIME_US.store(send_time.as_micros() as u64, Ordering::Relaxed);

                            // Performance stats every N frames
                            if frame_num % timing::FRAME_LOG_INTERVAL == 0 {
                                let total_time = frame_start.elapsed();
                                debug!(
                                    frame = frame_num,
                                    decode_us = decode_time.as_micros(),
                                    send_us = send_time.as_micros(),
                                    total_us = total_time.as_micros(),
                                    width = video_info.width(),
                                    height = video_info.height(),
                                    size_kb = map.as_slice().len() / 1024,
                                    "Frame performance (libcamera)"
                                );
                            }
                        }
                        Err(e) => {
                            if frame_num % 30 == 0 {
                                debug!(frame = frame_num, error = ?e, "Frame dropped (channel full)");
                            }
                        }
                    }

                    Ok(gstreamer::FlowSuccess::Ok)
                })
                .build(),
        );
        debug!("Frame callback set up with performance tracking");

        // Start the pipeline
        debug!("Setting pipeline to PLAYING state");
        pipeline.set_state(gstreamer::State::Playing).map_err(|e| {
            BackendError::InitializationFailed(format!("Failed to start pipeline: {}", e))
        })?;

        // Wait for state change to complete
        let (result, state, pending) = pipeline.state(gstreamer::ClockTime::from_seconds(
            timing::START_TIMEOUT_SECS,
        ));
        debug!(result = ?result, state = ?state, pending = ?pending, "Pipeline state");
        if state != gstreamer::State::Playing {
            warn!("Pipeline is not in PLAYING state");
        }

        info!("Libcamera camera initialization complete");

        Ok(Self {
            pipeline,
            _appsink: appsink,
            recording: false,
        })
    }

    /// Start the pipeline (already started in new())
    pub fn start(&self) -> BackendResult<()> {
        info!("Libcamera pipeline already started");
        Ok(())
    }

    /// Stop the pipeline
    pub fn stop(self) -> BackendResult<()> {
        info!("Stopping libcamera pipeline");

        // Clear appsink callbacks to release all references
        debug!("Clearing appsink callbacks");
        self._appsink
            .set_callbacks(gstreamer_app::AppSinkCallbacks::builder().build());

        // Set pipeline to NULL state to release camera
        self.pipeline
            .set_state(gstreamer::State::Null)
            .map_err(|e| BackendError::Other(format!("Failed to stop pipeline: {}", e)))?;

        // Wait for state change to complete
        let (result, state, _) = self.pipeline.state(gstreamer::ClockTime::from_seconds(
            timing::STOP_TIMEOUT_SECS,
        ));
        match result {
            Ok(_) => {
                info!(state = ?state, "Libcamera pipeline stopped successfully");
            }
            Err(e) => {
                debug!(error = ?e, state = ?state, "Pipeline state change had issues");
            }
        }

        info!("Libcamera will release camera when ready");

        Ok(())
    }

    /// Capture a single frame
    pub fn capture_frame(&self) -> BackendResult<CameraFrame> {
        Err(BackendError::Other(
            "Photo capture not yet implemented for libcamera backend".to_string(),
        ))
    }

    /// Start recording video
    pub fn start_recording(&mut self, _output_path: PathBuf) -> BackendResult<()> {
        if self.recording {
            return Err(BackendError::RecordingInProgress);
        }

        Err(BackendError::Other(
            "Video recording not yet implemented for libcamera backend".to_string(),
        ))
    }

    /// Stop recording video
    pub fn stop_recording(&mut self) -> BackendResult<PathBuf> {
        if !self.recording {
            return Err(BackendError::NoRecordingInProgress);
        }

        Err(BackendError::Other(
            "Video recording not yet implemented for libcamera backend".to_string(),
        ))
    }

    /// Check if currently recording
    pub fn is_recording(&self) -> bool {
        self.recording
    }
}

impl Drop for LibcameraPipeline {
    fn drop(&mut self) {
        info!("Dropping libcamera pipeline - explicitly stopping");
        // Clear callbacks first
        self._appsink
            .set_callbacks(gstreamer_app::AppSinkCallbacks::builder().build());
        // Explicitly set pipeline to Null to release device immediately
        let _ = self.pipeline.set_state(gstreamer::State::Null);
        info!("Libcamera pipeline stopped");
    }
}

/// Build the GStreamer pipeline string for libcamera
fn build_libcamera_pipeline_string(device: &CameraDevice, format: &CameraFormat) -> String {
    // Build camera-name property if device path is specified
    let camera_prop = if device.path.is_empty() {
        String::new()
    } else {
        format!("camera-name=\"{}\" ", device.path)
    };

    // Build caps filter for resolution and framerate
    let caps_filter = if let Some(fps) = format.framerate {
        format!(
            "width={},height={},framerate={}/1",
            format.width, format.height, fps
        )
    } else {
        format!("width={},height={}", format.width, format.height)
    };

    // Libcamera typically outputs raw formats (NV12, YUY2, etc.)
    // Use videoconvert to convert to RGBA for display
    match format.pixel_format.as_str() {
        "MJPG" | "MJPEG" => {
            // Some libcamera devices might support MJPEG
            format!(
                "libcamerasrc {}! image/jpeg,{} ! \
                 queue max-size-buffers=2 leaky=downstream ! \
                 jpegdec ! \
                 videoconvert n-threads={} ! \
                 video/x-raw,format={} ! \
                 appsink name=sink",
                camera_prop,
                caps_filter,
                pipeline::videoconvert_threads(),
                pipeline::OUTPUT_FORMAT
            )
        }
        _ => {
            // Raw formats (NV12, YUY2, etc.) - direct conversion
            // This is the common case for mobile cameras with ISP
            format!(
                "libcamerasrc {}! video/x-raw,{} ! \
                 queue max-size-buffers=2 leaky=downstream ! \
                 videoconvert n-threads={} ! \
                 video/x-raw,format={} ! \
                 appsink name=sink",
                camera_prop,
                caps_filter,
                pipeline::videoconvert_threads(),
                pipeline::OUTPUT_FORMAT
            )
        }
    }
}
