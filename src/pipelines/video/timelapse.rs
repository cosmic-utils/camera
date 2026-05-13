// SPDX-License-Identifier: GPL-3.0-only

//! Timelapse video encoder
//!
//! Receives camera frames in real-time via a channel, converts each to
//! RGBA using the app's GPU compute pipeline (supporting all formats
//! including packed Bayer), and encodes them into a video at 30 fps.
//!
//! Pipeline: appsrc (RGBA) → videoconvert → encoder → muxer → filesink

use super::encoder_selection::{EncoderConfig, select_encoders, select_encoders_with_video};
use super::muxer::{create_muxer, link_muxer_to_sink, link_video_to_muxer};
use super::recorder::convert_frame_to_rgba;
use crate::backends::camera::types::{CameraFrame, SensorRotation};
use crate::media::encoders::video::EncoderInfo;
use gstreamer as gst;
use gstreamer::prelude::*;
use gstreamer_app as gst_app;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};
use tracing::{error, info, warn};

/// Target framerate for the output timelapse video.
const TIMELAPSE_FPS: u32 = 30;

/// Run the timelapse encoding loop.
///
/// Receives `CameraFrame`s from `frame_rx`, converts each to RGBA via
/// the GPU compute pipeline (handles YUV, Bayer, and all other formats),
/// pushes it into a GStreamer encoding pipeline at [`TIMELAPSE_FPS`], and
/// finalises the file when the channel closes.
///
/// This function is intended to be spawned as an async task.
pub async fn run_timelapse_encoder(
    mut frame_rx: tokio::sync::mpsc::UnboundedReceiver<Arc<CameraFrame>>,
    output_path: PathBuf,
    encoder_info: Option<EncoderInfo>,
    bitrate_kbps: Option<u32>,
    live_filter_code: Arc<AtomicU32>,
    rotation: SensorRotation,
    mirror_horizontal: bool,
) -> Result<String, String> {
    // Wait for the first frame so we know the dimensions.
    let first_frame = frame_rx
        .recv()
        .await
        .ok_or_else(|| "Channel closed before first frame".to_string())?;

    let width = first_frame.width;
    let height = first_frame.height;

    info!(
        width, height,
        fps = TIMELAPSE_FPS,
        format = ?first_frame.format,
        output = %output_path.display(),
        "Starting timelapse encoder"
    );

    // --- rotation handling -----------------------------------------------------
    let flip_direction = match rotation {
        SensorRotation::Rotate90 => Some("90l"),
        SensorRotation::Rotate180 => Some("180"),
        SensorRotation::Rotate270 => Some("90r"),
        SensorRotation::None => None,
    };

    // Swap output dimensions for 90°/270° rotations
    let (out_width, out_height) = if rotation.swaps_dimensions() {
        (height, width)
    } else {
        (width, height)
    };

    // --- encoder selection ---------------------------------------------------
    let encoder_config = EncoderConfig {
        width: out_width,
        height: out_height,
        bitrate_override_kbps: bitrate_kbps,
        ..Default::default()
    };

    let encoders = if let Some(ref info) = encoder_info {
        select_encoders_with_video(&encoder_config, info, false)
    } else {
        select_encoders(&encoder_config, false)
    }
    .map_err(|e| format!("Encoder selection failed: {e}"))?;

    let video_enc = encoders.video;

    let enc_name = video_enc
        .encoder
        .factory()
        .map(|f| f.name().to_string())
        .unwrap_or_default();
    let extension = video_enc.extension;

    let mut final_output = output_path;
    final_output.set_extension(extension);

    info!(encoder = %enc_name, output = %final_output.display(),
          "Timelapse encoder selected");

    // --- build GStreamer pipeline (always RGBA input) -------------------------
    let pipeline = gst::Pipeline::new();

    let appsrc = gst_app::AppSrc::builder()
        .caps(
            &gst::Caps::builder("video/x-raw")
                .field("format", "RGBA")
                .field("width", width as i32)
                .field("height", height as i32)
                .field("framerate", gst::Fraction::new(TIMELAPSE_FPS as i32, 1))
                .build(),
        )
        .format(gst::Format::Time)
        .is_live(false)
        .build();

    let videoconvert = gst::ElementFactory::make("videoconvert")
        .build()
        .map_err(|e| format!("videoconvert: {e}"))?;

    // Optional videoflip for sensor rotation correction. If we also need to
    // mirror horizontally, chain a second videoflip after rotation.
    let videoflip = if let Some(direction) = flip_direction {
        let flip = gst::ElementFactory::make("videoflip")
            .property_from_str("video-direction", direction)
            .build()
            .map_err(|e| format!("videoflip: {e}"))?;

        // Capsfilter after flip to enforce rotated dimensions
        let capsfilter = gst::ElementFactory::make("capsfilter")
            .property(
                "caps",
                gst::Caps::builder("video/x-raw")
                    .field("width", out_width as i32)
                    .field("height", out_height as i32)
                    .build(),
            )
            .build()
            .map_err(|e| format!("capsfilter: {e}"))?;

        Some((flip, capsfilter))
    } else {
        None
    };

    let mirror_flip = if mirror_horizontal {
        Some(
            gst::ElementFactory::make("videoflip")
                .property_from_str("video-direction", "horiz")
                .build()
                .map_err(|e| format!("videoflip(mirror): {e}"))?,
        )
    } else {
        None
    };

    let encoder_elem = video_enc.encoder;
    let parser = video_enc.parser;
    let muxer_elem = video_enc.muxer;
    let muxer_cfg = create_muxer(muxer_elem, final_output.clone())?;

    // Add elements
    pipeline
        .add_many([
            appsrc.upcast_ref(),
            &videoconvert,
            &encoder_elem,
            &muxer_cfg.muxer,
            &muxer_cfg.filesink,
        ])
        .map_err(|e| format!("pipeline add: {e}"))?;

    if let Some((ref flip, ref capsfilter)) = videoflip {
        pipeline
            .add_many([flip, capsfilter])
            .map_err(|e| format!("pipeline add flip/capsfilter: {e}"))?;
    }
    if let Some(ref mirror) = mirror_flip {
        pipeline
            .add_many([mirror])
            .map_err(|e| format!("pipeline add mirror: {e}"))?;
    }

    // Link
    appsrc
        .link(&videoconvert)
        .map_err(|_| "link appsrc→videoconvert")?;

    // Chain: videoconvert → [videoflip → capsfilter →] [mirror →] encoder
    let after_rotation: &gst::Element = if let Some((ref flip, ref capsfilter)) = videoflip {
        videoconvert
            .link(flip)
            .map_err(|_| "link videoconvert→videoflip")?;
        flip.link(capsfilter)
            .map_err(|_| "link videoflip→capsfilter")?;
        capsfilter
    } else {
        &videoconvert
    };
    let pre_encoder: &gst::Element = if let Some(ref mirror) = mirror_flip {
        after_rotation
            .link(mirror)
            .map_err(|_| "link rotation→mirror")?;
        mirror
    } else {
        after_rotation
    };

    if let Some(ref p) = parser {
        pipeline
            .add(p)
            .map_err(|e| format!("pipeline add parser: {e}"))?;
        pre_encoder
            .link(&encoder_elem)
            .map_err(|_| "link pre_encoder→encoder")?;
        encoder_elem.link(p).map_err(|_| "link encoder→parser")?;
        link_video_to_muxer(p, &muxer_cfg.muxer)?;
    } else {
        pre_encoder
            .link(&encoder_elem)
            .map_err(|_| "link pre_encoder→encoder")?;
        link_video_to_muxer(&encoder_elem, &muxer_cfg.muxer)?;
    }
    link_muxer_to_sink(&muxer_cfg.muxer, &muxer_cfg.filesink)?;

    // Start
    pipeline
        .set_state(gst::State::Playing)
        .map_err(|e| format!("set PLAYING: {e:?}"))?;

    // --- push frames --------------------------------------------------------
    let frame_duration_ns: u64 = 1_000_000_000 / TIMELAPSE_FPS as u64;
    let frame_duration = gst::ClockTime::from_nseconds(frame_duration_ns);
    let mut frame_index: u64 = 0;

    // Push the first frame we already received
    let rgba = convert_and_filter(&first_frame, &live_filter_code).await?;
    push_rgba(&appsrc, rgba, frame_index, frame_duration)?;
    frame_index += 1;

    // Receive remaining frames until the channel is closed (timelapse stopped)
    while let Some(frame) = frame_rx.recv().await {
        if frame.width != width || frame.height != height {
            warn!(
                expected_w = width,
                expected_h = height,
                actual_w = frame.width,
                actual_h = frame.height,
                "Skipping frame with mismatched dimensions"
            );
            continue;
        }
        match convert_and_filter(&frame, &live_filter_code).await {
            Ok(rgba) => {
                if let Err(e) = push_rgba(&appsrc, rgba, frame_index, frame_duration) {
                    error!(error = %e, frame = frame_index, "Failed to push frame, stopping");
                    break;
                }
                frame_index += 1;
            }
            Err(e) => {
                warn!(error = %e, frame = frame_index, "Skipping frame (conversion failed)");
                continue;
            }
        }

        if frame_index.is_multiple_of(30) {
            info!(frames = frame_index, "Timelapse encoding progress");
        }
    }

    info!(
        total_frames = frame_index,
        "Channel closed, finalising timelapse video"
    );

    if frame_index == 0 {
        pipeline.set_state(gst::State::Null).ok();
        let _ = std::fs::remove_file(&final_output);
        return Err("No frames were encoded".into());
    }

    // Signal end of stream
    let _ = appsrc.end_of_stream();

    // Wait for EOS
    let bus = pipeline.bus().ok_or("No pipeline bus")?;
    let timeout = gst::ClockTime::from_seconds(30);
    loop {
        match bus.timed_pop(timeout) {
            Some(msg) => match msg.view() {
                gst::MessageView::Eos(..) => {
                    info!("Timelapse video EOS received");
                    break;
                }
                gst::MessageView::Error(e) => {
                    let err_msg = format!(
                        "GStreamer error: {} ({})",
                        e.error(),
                        e.debug().unwrap_or_default()
                    );
                    pipeline.set_state(gst::State::Null).ok();
                    return Err(err_msg);
                }
                _ => {}
            },
            None => {
                pipeline.set_state(gst::State::Null).ok();
                return Err("Timeout waiting for pipeline EOS".into());
            }
        }
    }

    pipeline
        .set_state(gst::State::Null)
        .map_err(|e| format!("set NULL: {e:?}"))?;

    info!(path = %final_output.display(), frames = frame_index, "Timelapse video saved");
    Ok(final_output.display().to_string())
}

/// Convert a frame to RGBA and apply the current live filter (if any).
async fn convert_and_filter(
    frame: &CameraFrame,
    live_filter_code: &AtomicU32,
) -> Result<Vec<u8>, String> {
    let mut rgba = convert_frame_to_rgba(frame).await?;

    let filter_code = live_filter_code.load(Ordering::Relaxed);
    if filter_code != 0 {
        let filter = crate::app::FilterType::from_gpu_filter_code(filter_code);
        rgba = crate::shaders::apply_filter_gpu_rgba(&rgba, frame.width, frame.height, filter)
            .await
            .map_err(|e| format!("Filter failed: {e}"))?;
    }

    Ok(rgba)
}

/// Push tightly-packed RGBA bytes into appsrc with correct PTS.
fn push_rgba(
    appsrc: &gst_app::AppSrc,
    rgba: Vec<u8>,
    index: u64,
    duration: gst::ClockTime,
) -> Result<(), String> {
    let pts = duration * index;
    let mut buffer = gst::Buffer::from_mut_slice(rgba);
    {
        let buf = buffer.get_mut().unwrap();
        buf.set_pts(pts);
        buf.set_duration(duration);
    }

    appsrc
        .push_buffer(buffer)
        .map(|_| ())
        .map_err(|e| format!("push_buffer: {e}"))
}
