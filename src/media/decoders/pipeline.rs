// SPDX-License-Identifier: MPL-2.0

//! GStreamer pipeline construction for PipeWire camera backend
//!
//! This module handles the creation of GStreamer pipelines using pipewiresrc,
//! with appropriate decoder selection and format negotiation.

use super::PipelineBackend;
use crate::constants::{pipeline, timing};
use gstreamer::prelude::*;
use tracing::{error, info, warn};

/// Try to create a GStreamer pipeline for camera capture using PipeWire
///
/// This function creates pipelines using pipewiresrc, handling format negotiation
/// and decoder selection automatically. PipeWire-only application.
///
/// # Arguments
/// * `device_path` - Device path (e.g., "pipewire-serial-12345" or "/dev/video0" via PipeWire)
/// * `caps_filter` - GStreamer caps filter string (e.g., "width=1920,height=1080")
/// * `_decoder` - Decoder element name (unused, kept for API compatibility)
/// * `pixel_format` - Pixel format FourCC (e.g., "MJPG", "H264", "YUYV")
/// * `_backend` - Backend type (unused, always PipeWire)
///
/// # Returns
/// * `Ok(Pipeline)` - Successfully created and started pipeline
/// * `Err` - Pipeline creation or startup failed
pub fn try_create_pipeline(
    device_path: Option<&str>,
    caps_filter: &str,
    _decoder: &str,
    pixel_format: Option<&str>,
    _backend: PipelineBackend,
) -> Result<gstreamer::Pipeline, Box<dyn std::error::Error>> {
    try_create_pipewire_pipeline(device_path, caps_filter, pixel_format)
}

/// Try to create a PipeWire pipeline
fn try_create_pipewire_pipeline(
    device_path: Option<&str>,
    caps_filter: &str,
    pixel_format: Option<&str>,
) -> Result<gstreamer::Pipeline, Box<dyn std::error::Error>> {
    // Check if PipeWire source is available
    gstreamer::ElementFactory::make("pipewiresrc")
        .build()
        .map_err(|e| format!("pipewiresrc not available: {}", e))?;

    info!("✓ PipeWire available - creating camera pipeline");

    // Determine PipeWire path based on device_path format
    let pw_path_prop = determine_pipewire_path(device_path);

    // Log requested format
    log_requested_format(caps_filter);

    // Build PipeWire pipeline based on pixel format
    let pipewire_pipeline =
        build_pipewire_pipeline_string(&pw_path_prop, caps_filter, pixel_format);

    info!(pipeline = %pipewire_pipeline, "Creating PipeWire pipeline");
    try_launch_pipeline_with_bus_errors(&pipewire_pipeline)
}

/// Determine PipeWire path property from device path
fn determine_pipewire_path(device_path: Option<&str>) -> String {
    if let Some(dev_path) = device_path {
        if dev_path.is_empty() {
            // Empty path = PipeWire auto-select default camera
            info!("Using default PipeWire camera (auto-select)");
            String::new()
        } else if dev_path.starts_with("v4l2:") {
            // PipeWire object.path format
            let pw_path = format!("path={} ", dev_path);
            info!(object_path = %dev_path, "Using PipeWire object.path");
            pw_path
        } else if dev_path.starts_with("pipewire-serial-") {
            // PipeWire object.serial
            let serial = dev_path
                .strip_prefix("pipewire-serial-")
                .unwrap_or(dev_path);
            let pw_path = format!("target-object={} ", serial);
            info!(serial, "Using PipeWire object.serial");
            pw_path
        } else if dev_path.starts_with("pipewire-") {
            // PipeWire node ID
            let node_id = dev_path.strip_prefix("pipewire-").unwrap_or(dev_path);
            let pw_path = format!("target-object={} ", node_id);
            info!(node_id, "Using PipeWire node ID");
            pw_path
        } else if dev_path.starts_with("/dev/video") {
            // V4L2 device path exposed through PipeWire
            // PipeWire can expose V4L2 devices, so this is valid for PipeWire-only apps
            let pw_path = format!("path=v4l2:{} ", dev_path);
            info!(dev_path, pw_path = %pw_path, "Using V4L2 device via PipeWire (PipeWire manages access)");
            pw_path
        } else {
            // Unknown format - use as-is with path property
            warn!(dev_path, "Unknown device path format, using path property");
            format!("path={} ", dev_path)
        }
    } else {
        // No path specified - use default camera
        info!("Using default PipeWire camera");
        String::new()
    }
}

/// Log requested format from caps filter
fn log_requested_format(caps_filter: &str) {
    if caps_filter.is_empty() {
        return;
    }

    let mut width_opt = None;
    let mut height_opt = None;
    let mut framerate_opt = None;

    for part in caps_filter.split(',') {
        if part.contains("width=") {
            if let Some(w) = part.split("width=(int)").nth(1) {
                width_opt = w.parse::<u32>().ok();
            }
        } else if part.contains("height=") {
            if let Some(h) = part.split("height=(int)").nth(1) {
                height_opt = h.parse::<u32>().ok();
            }
        } else if part.contains("framerate=") {
            if let Some(fps_str) = part.split("framerate=(fraction)").nth(1) {
                if let Some(fps) = fps_str.split('/').next() {
                    framerate_opt = fps.parse::<u32>().ok();
                }
            }
        }
    }

    if let (Some(w), Some(h), Some(fps)) = (width_opt, height_opt, framerate_opt) {
        info!(width = w, height = h, fps, "Requested format from PipeWire");
    }
}

/// Build PipeWire pipeline string based on pixel format
fn build_pipewire_pipeline_string(
    pw_path_prop: &str,
    caps_filter: &str,
    pixel_format: Option<&str>,
) -> String {
    if !caps_filter.is_empty() {
        match pixel_format {
            Some("MJPG") | Some("MJPEG") => {
                // MJPEG: use jpegdec with queue for proper buffering at high resolutions
                // Queue ensures complete JPEG frames before decoding
                // Allow buffering up to 30MB (enough for ~9 high-res JPEG frames at 4000x3000)
                // identity sync helps reduce artifacts on incomplete jpeg frames
                // stream can still flicker with artifacts when there is high CPU usage. The problem is likely that the jpegdec decoder is producing incomplete/corrupted frames when the system is under load, rather
                //   than dropping them cleanly.
                format!(
                    "pipewiresrc {}do-timestamp=true ! \
                    queue max-size-buffers=2 leaky=downstream ! \
                    identity sync=true ! \
                    image/jpeg,{} ! \
                    jpegparse ! \
                    jpegdec max-errors=-1 ! \
                    queue max-size-buffers={} leaky=downstream ! \
                    videoconvert n-threads={} ! \
                    video/x-raw,format={} ! \
                    appsink name=sink",
                    pw_path_prop,
                    caps_filter,
                    pipeline::MAX_BUFFERS,
                    pipeline::videoconvert_threads(),
                    pipeline::OUTPUT_FORMAT
                )
            }
            Some("H264") => {
                // H264: use decodebin with queue
                // Allow buffering up to 30MB for high-resolution streams
                format!(
                    "pipewiresrc {}do-timestamp=true ! video/x-h264,{} ! \
                     queue max-size-buffers=0 max-size-time=0 max-size-bytes=31457280 leaky=downstream ! \
                     decodebin ! videoconvert n-threads={} ! video/x-raw,format={} ! appsink name=sink",
                    pw_path_prop,
                    caps_filter,
                    pipeline::videoconvert_threads(),
                    pipeline::OUTPUT_FORMAT
                )
            }
            _ => {
                // Raw format: direct conversion (no queue needed for raw formats)
                format!(
                    "pipewiresrc {}do-timestamp=true ! video/x-raw,{} ! videoconvert ! video/x-raw,format={} ! appsink name=sink",
                    pw_path_prop,
                    caps_filter,
                    pipeline::OUTPUT_FORMAT
                )
            }
        }
    } else {
        // No specific format - let PipeWire auto-negotiate
        format!(
            "pipewiresrc {}do-timestamp=true ! decodebin ! videoconvert ! video/x-raw,format={} ! appsink name=sink",
            pw_path_prop,
            pipeline::OUTPUT_FORMAT
        )
    }
}

/// Try to launch pipeline and check bus for detailed error messages
fn try_launch_pipeline_with_bus_errors(
    pipeline_str: &str,
) -> Result<gstreamer::Pipeline, Box<dyn std::error::Error>> {
    info!(pipeline = %pipeline_str, "Attempting to launch pipeline");

    match gstreamer::parse::launch(pipeline_str) {
        Ok(p) => {
            info!("Pipeline parsed successfully");
            let pipeline = p
                .dynamic_cast::<gstreamer::Pipeline>()
                .map_err(|_| "Failed to cast to pipeline")?;

            info!("Cast to Pipeline successful");

            // Try to set to PLAYING to validate it works
            info!("Setting pipeline to PLAYING state");
            match pipeline.set_state(gstreamer::State::Playing) {
                Ok(_) => {
                    info!("set_state() returned successfully");
                    info!(
                        "Waiting for state change with {}ms timeout",
                        timing::STATE_CHANGE_TIMEOUT_MS
                    );
                    let (result, state, pending) = pipeline.state(
                        gstreamer::ClockTime::from_mseconds(timing::STATE_CHANGE_TIMEOUT_MS),
                    );

                    info!(?result, ?state, ?pending, "State query completed");

                    // Only accept Playing state
                    if result.is_ok() && state == gstreamer::State::Playing {
                        info!(?state, "✓ Pipeline reached target state successfully");
                        Ok(pipeline)
                    } else if matches!(result, Ok(gstreamer::StateChangeSuccess::Async))
                        && pending == gstreamer::State::Playing
                    {
                        // Pipeline transitioning asynchronously - accept immediately for fast startup
                        // Frames will arrive when the device is ready
                        info!(
                            ?state,
                            ?pending,
                            "✓ Pipeline transitioning asynchronously (accepted for fast startup)"
                        );
                        Ok(pipeline)
                    } else {
                        error!(
                            ?state,
                            ?result,
                            ?pending,
                            "✗ Pipeline failed to reach PLAYING"
                        );
                        check_bus_for_errors(&pipeline);
                        let _ = pipeline.set_state(gstreamer::State::Null);
                        Err(format!(
                            "Pipeline failed to start (state: {:?}, result: {:?})",
                            state, result
                        )
                        .into())
                    }
                }
                Err(e) => {
                    error!(error = %e, "✗ Failed to set pipeline to PLAYING state");
                    let _ = pipeline.set_state(gstreamer::State::Null);
                    Err(format!("Failed to set pipeline to PLAYING: {}", e).into())
                }
            }
        }
        Err(e) => {
            error!(error = %e, pipeline = %pipeline_str, "✗ Failed to parse pipeline");
            Err(Box::new(e))
        }
    }
}

/// Check bus for error messages
fn check_bus_for_errors(pipeline: &gstreamer::Pipeline) {
    info!("Checking GStreamer bus for error messages");
    if let Some(bus) = pipeline.bus() {
        if let Some(msg) = bus.timed_pop_filtered(
            gstreamer::ClockTime::from_mseconds(100),
            &[
                gstreamer::MessageType::Error,
                gstreamer::MessageType::Warning,
            ],
        ) {
            match msg.view() {
                gstreamer::MessageView::Error(err) => {
                    error!(
                        error = %err.error(),
                        debug = ?err.debug(),
                        source = ?err.src().map(|s| s.name()),
                        "GStreamer ERROR during pipeline start"
                    );
                }
                gstreamer::MessageView::Warning(warn_msg) => {
                    warn!(
                        warning = %warn_msg.error(),
                        debug = ?warn_msg.debug(),
                        "GStreamer WARNING during pipeline start"
                    );
                }
                _ => {}
            }
        }
    }
}
