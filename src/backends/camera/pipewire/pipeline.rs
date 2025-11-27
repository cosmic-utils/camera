// SPDX-License-Identifier: MPL-2.0

//! PipeWire GStreamer pipeline for camera capture

use super::super::types::*;
use crate::constants::{pipeline, timing};
use crate::media::{Codec, PipelineBackend, detect_hw_decoders, try_create_pipeline};
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

/// PipeWire camera pipeline
///
/// Native GStreamer pipeline implementation using pipewiresrc for camera capture.
/// Handles preview streaming with hardware-accelerated decoding.
pub struct PipeWirePipeline {
    pipeline: gstreamer::Pipeline,
    _appsink: AppSink,
    pub decoder: String,
    recording: bool,
}

impl PipeWirePipeline {
    /// Create a new PipeWire pipeline
    pub fn new(
        device: &CameraDevice,
        format: &CameraFormat,
        frame_sender: FrameSender,
    ) -> BackendResult<Self> {
        info!(
            device = %device.name,
            format = %format,
            "Creating PipeWire pipeline"
        );

        // Initialize GStreamer
        debug!("Initializing GStreamer");
        gstreamer::init().map_err(|e| BackendError::InitializationFailed(e.to_string()))?;
        debug!("GStreamer initialized successfully");

        // Extract parameters
        let device_path = if device.path.is_empty() {
            None
        } else {
            Some(device.path.as_str())
        };

        let width = Some(format.width);
        let height = Some(format.height);
        let framerate = format.framerate;
        let pixel_format = Some(format.pixel_format.as_str());

        // Build caps string for resolution and framerate
        let caps_filter = match (width, height, framerate) {
            (Some(w), Some(h), Some(fps)) => {
                format!(
                    "width=(int){},height=(int){},framerate=(fraction){}/1",
                    w, h, fps
                )
            }
            (Some(w), Some(h), None) => {
                format!("width=(int){},height=(int){}", w, h)
            }
            _ => String::new(),
        };

        info!(?device_path, caps_filter, "Initializing PipeWire camera");

        // Check if format needs a decoder
        let needs_decoder = pixel_format
            .map(|fmt| Codec::from_fourcc(fmt).needs_decoder())
            .unwrap_or(true);

        let mut pipeline: Option<gstreamer::Pipeline> = None;
        let mut selected_decoder: Option<String> = None;
        let mut last_error = None;

        if needs_decoder {
            // Detect available hardware decoders
            let hw_decoders = detect_hw_decoders();

            // Try decoders in order: hardware first, then software fallback
            let mut decoders_to_try = hw_decoders;
            decoders_to_try.push("avdec_mjpeg"); // FFmpeg software decoder
            decoders_to_try.push("jpegdec"); // GStreamer software decoder

            for decoder in &decoders_to_try {
                debug!(decoder = %decoder, "Attempting to create pipeline with decoder");
                match try_create_pipeline(
                    device_path,
                    &caps_filter,
                    decoder,
                    pixel_format,
                    PipelineBackend::PipeWire,
                ) {
                    Ok(p) => {
                        info!(decoder = %decoder, "Successfully created pipeline");
                        pipeline = Some(p);
                        selected_decoder = Some(decoder.to_string());
                        break;
                    }
                    Err(e) => {
                        debug!(decoder = %decoder, error = %e, "Failed with decoder");
                        last_error = Some(e);
                    }
                }
            }
        } else {
            // Raw format - no decoder needed
            info!(pixel_format = ?pixel_format, "Using raw format, no decoder needed");
            match try_create_pipeline(
                device_path,
                &caps_filter,
                "",
                pixel_format,
                PipelineBackend::PipeWire,
            ) {
                Ok(p) => {
                    pipeline = Some(p);
                    selected_decoder = Some("none (raw format)".to_string());
                }
                Err(e) => {
                    last_error = Some(e);
                }
            }
        }

        let (pipeline, decoder_name) = match (pipeline, selected_decoder) {
            (Some(p), Some(d)) => {
                info!(decoder = %d, "Pipeline created successfully");
                (p, d)
            }
            _ => {
                error!("All decoders failed");
                return Err(BackendError::InitializationFailed(
                    last_error
                        .map(|e| e.to_string())
                        .unwrap_or_else(|| "No decoders available".to_string()),
                ));
            }
        };

        debug!("Pipeline ready");

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

        // Adjust buffering based on framerate to ensure complete frame delivery
        // At high framerates (>30 FPS), use more buffering to prevent incomplete frames
        let buffer_count = if framerate.unwrap_or(0) > 30 {
            3 // More buffering at high FPS to ensure DMA transfers complete
        } else {
            pipeline::MAX_BUFFERS
        };
        appsink.set_property("max-buffers", buffer_count);
        appsink.set_property("drop", true); // Drop old frames if processing is slow
        appsink.set_property("enable-last-sample", false); // Don't keep last sample in memory

        debug!(
            buffer_count,
            framerate, "Appsink configured for maximum performance"
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
                    // This can happen at high framerates when DMA transfers aren't complete
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

                    // Extract stride information for NV12 format
                    let stride_y = video_info.stride()[0] as u32;
                    let stride_uv = video_info.stride()[1] as u32;
                    let offset_uv = video_info.offset()[1];

                    // Log stride info every 60 frames for debugging
                    if frame_num % 60 == 0 {
                        info!(
                            frame = frame_num,
                            width = video_info.width(),
                            height = video_info.height(),
                            stride_y,
                            stride_uv,
                            offset_uv,
                            "Frame stride information"
                        );
                    }

                    // Use Arc::from to avoid intermediate Vec allocation
                    let frame = CameraFrame {
                        width: video_info.width(),
                        height: video_info.height(),
                        data: Arc::from(map.as_slice()),
                        format: PixelFormat::NV12,  // Pipeline outputs NV12
                        stride_y,
                        stride_uv,
                        offset_uv,
                        captured_at: frame_start, // Use frame_start as capture timestamp
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
                                    "Frame performance"
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

        info!("PipeWire camera initialization complete");

        Ok(Self {
            pipeline,
            _appsink: appsink,
            decoder: decoder_name,
            recording: false,
        })
    }

    /// Start the pipeline (already started in new())
    pub fn start(&self) -> BackendResult<()> {
        info!("PipeWire pipeline already started");
        Ok(())
    }

    /// Stop the pipeline
    pub fn stop(self) -> BackendResult<()> {
        info!("Stopping PipeWire pipeline");

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
                info!(state = ?state, "PipeWire pipeline stopped successfully");
            }
            Err(e) => {
                debug!(error = ?e, state = ?state, "Pipeline state change had issues");
            }
        }

        // PipeWire manages device access - no need for artificial delays
        // (V4L2 needed this, but PipeWire handles it automatically)
        info!("PipeWire will release camera when ready");

        Ok(())
    }

    /// Capture a single frame
    pub fn capture_frame(&self) -> BackendResult<CameraFrame> {
        // For now, return an error - photo capture needs to be implemented differently
        // The current architecture doesn't support pull-based frame capture from the pipeline
        Err(BackendError::Other(
            "Photo capture not yet implemented for PipeWire backend".to_string(),
        ))
    }

    /// Start recording video
    pub fn start_recording(&mut self, _output_path: PathBuf) -> BackendResult<()> {
        if self.recording {
            return Err(BackendError::RecordingInProgress);
        }

        // TODO: Implement recording for PipeWire
        // For now, return error
        Err(BackendError::Other(
            "Video recording not yet implemented for PipeWire backend".to_string(),
        ))
    }

    /// Stop recording video
    pub fn stop_recording(&mut self) -> BackendResult<PathBuf> {
        if !self.recording {
            return Err(BackendError::NoRecordingInProgress);
        }

        // TODO: Implement recording stop
        Err(BackendError::Other(
            "Video recording not yet implemented for PipeWire backend".to_string(),
        ))
    }

    /// Check if currently recording
    pub fn is_recording(&self) -> bool {
        self.recording
    }

    /// Get decoder name
    pub fn decoder_name(&self) -> &str {
        &self.decoder
    }
}

impl Drop for PipeWirePipeline {
    fn drop(&mut self) {
        info!("Dropping PipeWire pipeline - explicitly stopping");
        // Clear callbacks first
        self._appsink
            .set_callbacks(gstreamer_app::AppSinkCallbacks::builder().build());
        // Explicitly set pipeline to Null to release device immediately
        let _ = self.pipeline.set_state(gstreamer::State::Null);
        info!("PipeWire pipeline stopped");
    }
}
