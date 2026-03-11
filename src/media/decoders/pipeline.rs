// SPDX-License-Identifier: GPL-3.0-only

//! GStreamer pipeline construction for PipeWire camera backend
//!
//! This module handles the creation of GStreamer pipelines using pipewiresrc,
//! with appropriate decoder selection and format negotiation.

use super::PipelineBackend;
use crate::constants::{pipeline, timing};
use gstreamer::prelude::*;
use std::sync::RwLock;
use tracing::{error, info, warn};

/// Format category for pipeline construction
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FormatCategory {
    /// Format supported by GPU shader - pass through without conversion
    ShaderSupported,
    /// Encoded format that needs decoding (MJPEG, H264, etc.)
    Encoded,
    /// Bayer pattern that needs conversion to RGB
    Bayer,
    /// Unsupported raw format - convert to NV12 for GPU processing
    ConvertToNv12,
}

/// Determine format category from pixel format string
fn get_format_category(pixel_format: Option<&str>) -> FormatCategory {
    match pixel_format {
        // Encoded formats - need decoding first
        Some("MJPG") | Some("MJPEG") | Some("H264") | Some("H265") | Some("HEVC") => {
            FormatCategory::Encoded
        }

        // Bayer patterns - need bayer2rgb conversion
        Some(fmt) if fmt.starts_with("BA") || fmt.contains("bayer") || fmt.contains("BAYER") => {
            FormatCategory::Bayer
        }

        // Shader-supported RGBA variants (passthrough as RGBA)
        Some("RGBA") | Some("RGBx") | Some("BGRx") | Some("BGRA") | Some("ARGB") | Some("ABGR")
        | Some("xRGB") | Some("xBGR") => FormatCategory::ShaderSupported,

        // Shader-supported YUV formats
        Some("NV12") | Some("NV21") | Some("I420") | Some("YV12") => {
            FormatCategory::ShaderSupported
        }

        // Shader-supported packed 4:2:2 formats
        Some("YUYV") | Some("YUY2") | Some("UYVY") | Some("YVYU") | Some("VYUY") => {
            FormatCategory::ShaderSupported
        }

        // Shader-supported grayscale
        Some("GRAY8") | Some("GREY") | Some("Y8") => FormatCategory::ShaderSupported,

        // Shader-supported RGB24 (no alpha)
        Some("RGB") | Some("BGR") => FormatCategory::ShaderSupported,

        // All other raw formats - convert to NV12 via GStreamer
        // This includes: Y42B, NV16, NV61, Y444, NV24, high bit-depth formats, etc.
        Some(_) => FormatCategory::ConvertToNv12,

        // No format specified - let GStreamer auto-negotiate
        None => FormatCategory::ShaderSupported,
    }
}

/// Full GStreamer pipeline string (for insights)
static FULL_PIPELINE_STRING: RwLock<Option<String>> = RwLock::new(None);

/// Get the full GStreamer pipeline string
pub fn get_full_pipeline_string() -> Option<String> {
    FULL_PIPELINE_STRING
        .read()
        .ok()
        .and_then(|guard| guard.clone())
}

/// Try to create a GStreamer pipeline for camera capture using PipeWire
///
/// This function creates pipelines using pipewiresrc, handling format negotiation
/// and decoder selection automatically. PipeWire-only application.
///
/// Rotation is NOT applied in the preview pipeline - it's handled by the GPU shader
/// for better performance.
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

/// Maximum retries for pipeline creation (handles PipeWire race conditions)
const PIPELINE_CREATE_RETRIES: u32 = 5;
/// Delay between retries in milliseconds (needs to be long enough for camera mode switch)
const PIPELINE_RETRY_DELAY_MS: u64 = 500;

/// Try to create a PipeWire pipeline
fn try_create_pipewire_pipeline(
    device_path: Option<&str>,
    caps_filter: &str,
    pixel_format: Option<&str>,
) -> Result<gstreamer::Pipeline, Box<dyn std::error::Error>> {
    // Check if PipeWire source is available (factory check only, no element instantiation)
    gstreamer::ElementFactory::find("pipewiresrc")
        .ok_or_else(|| "pipewiresrc not available: factory not found".to_string())?;

    info!("✓ PipeWire available - creating camera pipeline");

    // Determine PipeWire path based on device_path format
    let pw_path_prop = determine_pipewire_path(device_path);

    // Log requested format
    log_requested_format(caps_filter);

    // Build PipeWire pipeline based on pixel format
    // Note: Rotation is handled by the GPU shader for better performance
    let pipewire_pipeline =
        build_pipewire_pipeline_string(&pw_path_prop, caps_filter, pixel_format);

    // Store full pipeline string for insights
    if let Ok(mut guard) = FULL_PIPELINE_STRING.write() {
        *guard = Some(pipewire_pipeline.clone());
    }

    // Try launching with retries to handle PipeWire race conditions
    let mut last_error = None;
    for attempt in 1..=PIPELINE_CREATE_RETRIES {
        info!(pipeline = %pipewire_pipeline, attempt, "Attempting to launch pipeline");
        match try_launch_pipeline_with_bus_errors(&pipewire_pipeline) {
            Ok(pipeline) => return Ok(pipeline),
            Err(e) => {
                if attempt < PIPELINE_CREATE_RETRIES {
                    warn!(
                        attempt,
                        max_attempts = PIPELINE_CREATE_RETRIES,
                        error = %e,
                        "Pipeline launch failed, retrying after {}ms",
                        PIPELINE_RETRY_DELAY_MS
                    );
                    std::thread::sleep(std::time::Duration::from_millis(PIPELINE_RETRY_DELAY_MS));
                }
                last_error = Some(e);
            }
        }
    }

    Err(last_error.unwrap_or_else(|| "Pipeline creation failed".into()))
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
        } else if part.contains("framerate=")
            && let Some(fps_str) = part.split("framerate=(fraction)").nth(1)
            && let Some(fps) = fps_str.split('/').next()
        {
            framerate_opt = fps.parse::<u32>().ok();
        }
    }

    if let (Some(w), Some(h), Some(fps)) = (width_opt, height_opt, framerate_opt) {
        info!(width = w, height = h, fps, "Requested format from PipeWire");
    }
}

/// Build PipeWire pipeline string based on pixel format
///
/// For MJPEG and raw YUV formats (YUYV), the pipeline outputs native YUV
/// which is then converted to RGBA by a GPU compute shader. This is much
/// faster than CPU-based videoconvert.
///
/// For unsupported raw formats, the pipeline converts to NV12 via videoconvert
/// which the GPU shader can then process.
///
/// Note: Rotation is NOT applied in the pipeline - it's handled by the GPU shader
/// for better performance (zero CPU overhead per frame).
fn build_pipewire_pipeline_string(
    pw_path_prop: &str,
    caps_filter: &str,
    pixel_format: Option<&str>,
) -> String {
    if !caps_filter.is_empty() {
        let category = get_format_category(pixel_format);
        info!(?pixel_format, ?category, "Building pipeline for format");

        match (category, pixel_format) {
            // Encoded formats - MJPEG
            (FormatCategory::Encoded, Some("MJPG") | Some("MJPEG")) => {
                // MJPEG: decode to native YUV format (GPU will convert to RGBA)
                // Prefer CPU decoders (jpegdec, avdec_mjpeg) for reliability
                let decoder_chain = build_mjpeg_decoder_chain();
                info!(decoder = %decoder_chain, "MJPEG pipeline: native YUV output (GPU conversion)");
                format!(
                    "pipewiresrc {}do-timestamp=true ! \
                    queue max-size-buffers=2 leaky=downstream ! \
                    identity sync=true ! \
                    image/jpeg,{} ! \
                    jpegparse ! \
                    {} ! \
                    queue max-size-buffers={} leaky=downstream ! \
                    appsink name=sink",
                    pw_path_prop,
                    caps_filter,
                    decoder_chain,
                    pipeline::MAX_BUFFERS
                )
            }

            // Encoded formats - H264
            (FormatCategory::Encoded, Some("H264")) => {
                // H264: decode to native YUV format with hardware acceleration preference
                // h264parse config-interval=-1 inserts SPS/PPS before each keyframe for decoder robustness
                // Try hardware decoders first (VA-API), fall back to software (avdec_h264) only as last resort
                let decoder_chain = build_h264_decoder_chain();
                info!(decoder = %decoder_chain, "H264 pipeline: native YUV output (GPU conversion)");
                format!(
                    "pipewiresrc {}do-timestamp=true ! video/x-h264,{} ! \
                     h264parse config-interval=-1 ! \
                     queue max-size-buffers=0 max-size-bytes=0 max-size-time=0 ! \
                     {} ! \
                     video/x-raw ! \
                     queue max-size-buffers=8 leaky=downstream ! \
                     appsink name=sink sync=false",
                    pw_path_prop, caps_filter, decoder_chain
                )
            }

            // Encoded formats - H265/HEVC
            (FormatCategory::Encoded, Some("H265") | Some("HEVC")) => {
                // H265: decode to native YUV format with hardware acceleration preference
                let decoder_chain = build_h265_decoder_chain();
                info!(decoder = %decoder_chain, "H265 pipeline: native YUV output (GPU conversion)");
                format!(
                    "pipewiresrc {}do-timestamp=true ! video/x-h265,{} ! \
                     h265parse config-interval=-1 ! \
                     queue max-size-buffers=0 max-size-bytes=0 max-size-time=0 ! \
                     {} ! \
                     video/x-raw ! \
                     queue max-size-buffers=8 leaky=downstream ! \
                     appsink name=sink sync=false",
                    pw_path_prop, caps_filter, decoder_chain
                )
            }

            // Bayer patterns - convert to RGBA via bayer2rgb
            (FormatCategory::Bayer, Some(fmt)) => {
                info!(
                    format = fmt,
                    "Bayer pipeline: converting to RGBA via bayer2rgb"
                );
                format!(
                    "pipewiresrc {}do-timestamp=true ! \
                     video/x-bayer,{} ! \
                     bayer2rgb ! \
                     video/x-raw,format=RGBA ! \
                     appsink name=sink",
                    pw_path_prop, caps_filter
                )
            }

            // Shader-supported packed 4:2:2 formats (passthrough to GPU)
            (
                FormatCategory::ShaderSupported,
                Some(fmt @ ("YUYV" | "YUY2" | "UYVY" | "YVYU" | "VYUY")),
            ) => {
                // Map YUYV to YUY2 for GStreamer (they're the same format)
                let gst_fmt = if fmt == "YUYV" { "YUY2" } else { fmt };
                info!(
                    format = fmt,
                    "Packed 4:2:2 pipeline: native passthrough (GPU conversion)"
                );
                format!(
                    "pipewiresrc {}do-timestamp=true ! \
                    video/x-raw,format={},{} ! \
                    appsink name=sink",
                    pw_path_prop, gst_fmt, caps_filter
                )
            }

            // Shader-supported semi-planar/planar YUV formats (passthrough to GPU)
            (FormatCategory::ShaderSupported, Some(fmt @ ("NV12" | "NV21" | "I420" | "YV12"))) => {
                info!(
                    format = fmt,
                    "YUV planar pipeline: native passthrough (GPU conversion)"
                );
                format!(
                    "pipewiresrc {}do-timestamp=true ! \
                    video/x-raw,format={},{} ! \
                    appsink name=sink",
                    pw_path_prop, fmt, caps_filter
                )
            }

            // Shader-supported grayscale
            (FormatCategory::ShaderSupported, Some("GRAY8") | Some("GREY") | Some("Y8")) => {
                info!("Gray8 pipeline: native passthrough (GPU conversion)");
                format!(
                    "pipewiresrc {}do-timestamp=true ! \
                    video/x-raw,format=GRAY8,{} ! \
                    appsink name=sink",
                    pw_path_prop, caps_filter
                )
            }

            // Shader-supported RGB24/BGR (convert to RGBA for easier GPU handling)
            (FormatCategory::ShaderSupported, Some(fmt @ ("RGB" | "BGR"))) => {
                info!(
                    format = fmt,
                    "RGB24 pipeline: convert to RGBA (GPU passthrough)"
                );
                format!(
                    "pipewiresrc {}do-timestamp=true ! \
                    video/x-raw,format={},{} ! \
                    videoconvert n-threads={} ! \
                    video/x-raw,format=RGBA ! \
                    appsink name=sink",
                    pw_path_prop,
                    fmt,
                    caps_filter,
                    pipeline::videoconvert_threads()
                )
            }

            // Shader-supported RGBA variants - direct passthrough
            (
                FormatCategory::ShaderSupported,
                Some("RGBA") | Some("RGBx") | Some("BGRx") | Some("BGRA") | Some("ARGB")
                | Some("ABGR") | Some("xRGB") | Some("xBGR"),
            ) => {
                let fmt = pixel_format.unwrap();
                info!(format = fmt, "RGBA variant pipeline: native passthrough");
                format!(
                    "pipewiresrc {}do-timestamp=true ! \
                    video/x-raw,format={},{} ! \
                    appsink name=sink",
                    pw_path_prop, fmt, caps_filter
                )
            }

            // Unsupported raw formats - convert to NV12 for GPU processing
            (FormatCategory::ConvertToNv12, Some(fmt)) => {
                info!(
                    format = fmt,
                    "Unsupported format: converting to NV12 via videoconvert"
                );
                format!(
                    "pipewiresrc {}do-timestamp=true ! \
                    video/x-raw,format={},{} ! \
                    videoconvert n-threads={} ! \
                    video/x-raw,format=NV12 ! \
                    appsink name=sink",
                    pw_path_prop,
                    fmt,
                    caps_filter,
                    pipeline::videoconvert_threads()
                )
            }

            // Fallback for any remaining cases
            _ => {
                info!("Generic pipeline with auto-negotiation");
                format!(
                    "pipewiresrc {}do-timestamp=true ! video/x-raw,{} ! \
                     videoconvert n-threads={} ! video/x-raw,format=NV12 ! appsink name=sink",
                    pw_path_prop,
                    caps_filter,
                    pipeline::videoconvert_threads()
                )
            }
        }
    } else {
        // No specific format - let PipeWire auto-negotiate, decode if needed, output NV12
        info!("No format specified: using decodebin with NV12 output");
        format!(
            "pipewiresrc {}do-timestamp=true ! decodebin ! \
             videoconvert n-threads={} ! video/x-raw,format=NV12 ! appsink name=sink",
            pw_path_prop,
            pipeline::videoconvert_threads()
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
                        // Wait for Null state to complete so GStreamer releases all buffers
                        let _ = pipeline.state(gstreamer::ClockTime::from_seconds(2));
                        Err(format!(
                            "Pipeline failed to start (state: {:?}, result: {:?})",
                            state, result
                        )
                        .into())
                    }
                }
                Err(e) => {
                    error!(error = %e, "✗ Failed to set pipeline to PLAYING state");
                    // Check bus for the actual error reason
                    check_bus_for_errors(&pipeline);
                    let _ = pipeline.set_state(gstreamer::State::Null);
                    // Wait for Null state to complete so GStreamer releases all buffers
                    let _ = pipeline.state(gstreamer::ClockTime::from_seconds(2));
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
    if let Some(bus) = pipeline.bus()
        && let Some(msg) = bus.timed_pop_filtered(
            gstreamer::ClockTime::from_mseconds(100),
            &[
                gstreamer::MessageType::Error,
                gstreamer::MessageType::Warning,
            ],
        )
    {
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

/// Build the MJPEG decoder chain using shared definitions
fn build_mjpeg_decoder_chain() -> String {
    super::definitions::find_available_decoder(super::definitions::MJPEG_DECODERS)
}

/// Build the H264 decoder chain using shared definitions
fn build_h264_decoder_chain() -> String {
    super::definitions::find_available_decoder(super::definitions::H264_DECODERS)
}

/// Build the H265/HEVC decoder chain using shared definitions
fn build_h265_decoder_chain() -> String {
    super::definitions::find_available_decoder(super::definitions::H265_DECODERS)
}
