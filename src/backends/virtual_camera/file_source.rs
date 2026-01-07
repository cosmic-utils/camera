// SPDX-License-Identifier: GPL-3.0-only

//! File source streaming for virtual camera
//!
//! This module provides frame streaming from image and video files
//! for use with the virtual camera output. Videos also stream audio
//! to a virtual microphone via PipeWire.

use crate::backends::camera::types::{BackendError, BackendResult, CameraFrame, PixelFormat};
use crate::constants::{file_formats, virtual_camera as vc_timing};
use std::path::Path;
use std::sync::Arc;
use std::time::Instant;
use tracing::{debug, info, warn};

/// Load a preview frame from an image or video file
///
/// For images, loads the full image. For videos, extracts the first frame.
/// This is useful for showing a preview before streaming starts.
pub fn load_preview_frame(path: &Path) -> BackendResult<CameraFrame> {
    let extension = path
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_lowercase())
        .unwrap_or_default();

    if file_formats::is_image_extension(&extension) {
        load_image_as_frame(path)
    } else if file_formats::is_video_extension(&extension) {
        load_video_first_frame(path)
    } else {
        Err(BackendError::Other(format!(
            "Unsupported file format: {}",
            extension
        )))
    }
}

/// Wait for pipeline to reach async done state
fn wait_for_pipeline_ready(pipeline: &gstreamer::Pipeline, timeout_secs: u64) -> BackendResult<()> {
    use gstreamer::prelude::*;

    let bus = pipeline
        .bus()
        .ok_or_else(|| BackendError::Other("No bus on pipeline".into()))?;
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(timeout_secs);

    while std::time::Instant::now() < deadline {
        if let Some(msg) = bus.timed_pop(gstreamer::ClockTime::from_mseconds(100)) {
            use gstreamer::MessageView;
            match msg.view() {
                MessageView::Error(err) => {
                    return Err(BackendError::Other(format!(
                        "Pipeline error: {}",
                        err.error()
                    )));
                }
                MessageView::AsyncDone(_) => return Ok(()),
                _ => {}
            }
        }
    }
    Ok(()) // Timeout is not necessarily an error
}

/// Load a video frame at a specific position in seconds
///
/// Creates a temporary decoder, seeks to the position, and extracts a frame.
/// Useful for preview scrubbing.
pub fn load_video_frame_at_position(path: &Path, position_secs: f64) -> BackendResult<CameraFrame> {
    use gstreamer::prelude::*;

    debug!(path = %path.display(), position_secs, "Loading video frame at position");

    let (pipeline, appsink) = create_frame_extraction_pipeline(path)?;

    // Start pipeline in paused state first
    pipeline
        .set_state(gstreamer::State::Paused)
        .map_err(|e| BackendError::Other(format!("Failed to pause pipeline: {:?}", e)))?;

    // Wait for pipeline to be ready
    if let Err(e) = wait_for_pipeline_ready(&pipeline, 5) {
        let _ = pipeline.set_state(gstreamer::State::Null);
        return Err(e);
    }

    // Seek to the desired position
    let position = gstreamer::ClockTime::from_nseconds((position_secs * 1_000_000_000.0) as u64);
    if let Err(e) = pipeline.seek_simple(
        gstreamer::SeekFlags::FLUSH | gstreamer::SeekFlags::KEY_UNIT,
        position,
    ) {
        warn!(?e, "Seek failed, trying to get frame anyway");
    }

    // Set to playing to get the frame
    pipeline
        .set_state(gstreamer::State::Playing)
        .map_err(|e| BackendError::Other(format!("Failed to start pipeline: {:?}", e)))?;

    // Wait for frame with timeout
    let sample = appsink
        .try_pull_sample(gstreamer::ClockTime::from_seconds(3))
        .ok_or_else(|| BackendError::Other("Timeout waiting for video frame at position".into()))?;

    let frame = extract_frame_from_sample(&sample)?;
    let _ = pipeline.set_state(gstreamer::State::Null);

    debug!(
        width = frame.width,
        height = frame.height,
        position_secs,
        "Video frame at position loaded successfully"
    );
    Ok(frame)
}

/// Get the duration of a video file in seconds
///
/// Creates a temporary pipeline to query the duration without decoding frames.
pub fn get_video_duration(path: &Path) -> BackendResult<f64> {
    use gstreamer::prelude::*;

    info!(path = %path.display(), "Querying video duration");

    gstreamer::init().map_err(|e| BackendError::Other(format!("GStreamer init failed: {}", e)))?;

    let path_str = path.to_string_lossy();

    // Create a simple discoverer pipeline to get duration
    let pipeline_str = format!("filesrc location=\"{}\" ! decodebin ! fakesink", path_str);

    let pipeline = gstreamer::parse::launch(&pipeline_str)
        .map_err(|e| BackendError::Other(format!("Failed to create pipeline: {}", e)))?
        .downcast::<gstreamer::Pipeline>()
        .map_err(|_| BackendError::Other("Failed to downcast to Pipeline".into()))?;

    // Set to PAUSED to get duration without playing
    pipeline
        .set_state(gstreamer::State::Paused)
        .map_err(|e| BackendError::Other(format!("Failed to pause pipeline: {:?}", e)))?;

    // Wait for state change and query duration
    let bus = pipeline.bus().unwrap();
    let deadline = std::time::Instant::now()
        + std::time::Duration::from_secs(vc_timing::DURATION_QUERY_TIMEOUT_SECS);

    while std::time::Instant::now() < deadline {
        if let Some(msg) = bus.timed_pop(gstreamer::ClockTime::from_mseconds(100)) {
            use gstreamer::MessageView;
            match msg.view() {
                MessageView::Error(err) => {
                    let _ = pipeline.set_state(gstreamer::State::Null);
                    return Err(BackendError::Other(format!(
                        "Pipeline error: {}",
                        err.error()
                    )));
                }
                MessageView::AsyncDone(_) => {
                    // Pipeline is ready, try to get duration
                    if let Some(duration) = pipeline.query_duration::<gstreamer::ClockTime>() {
                        let _ = pipeline.set_state(gstreamer::State::Null);
                        let duration_secs = duration.seconds_f64();
                        info!(duration_secs, "Video duration queried successfully");
                        return Ok(duration_secs);
                    }
                }
                _ => {}
            }
        }
    }

    let _ = pipeline.set_state(gstreamer::State::Null);
    Err(BackendError::Other(
        "Timeout waiting for video duration".into(),
    ))
}

/// Extract frame dimensions and data from a GStreamer sample
fn extract_frame_from_sample(sample: &gstreamer::Sample) -> BackendResult<CameraFrame> {
    let caps = sample
        .caps()
        .ok_or_else(|| BackendError::Other("No caps on sample".into()))?;
    let structure = caps
        .structure(0)
        .ok_or_else(|| BackendError::Other("No structure in caps".into()))?;
    let width = structure
        .get::<i32>("width")
        .map_err(|_| BackendError::Other("No width in caps".into()))? as u32;
    let height = structure
        .get::<i32>("height")
        .map_err(|_| BackendError::Other("No height in caps".into()))? as u32;

    let buffer = sample
        .buffer()
        .ok_or_else(|| BackendError::Other("No buffer in sample".into()))?;
    let map = buffer
        .map_readable()
        .map_err(|_| BackendError::Other("Failed to map buffer".into()))?;
    let data: Vec<u8> = map.as_slice().to_vec();

    Ok(CameraFrame {
        data: Arc::from(data.into_boxed_slice()),
        width,
        height,
        stride: width * 4,
        format: PixelFormat::RGBA,
        captured_at: Instant::now(),
        depth_data: None,
        depth_width: 0,
        depth_height: 0,
        video_timestamp: None,
    })
}

/// Create a video frame extraction pipeline with appsink
fn create_frame_extraction_pipeline(
    path: &Path,
) -> BackendResult<(gstreamer::Pipeline, gstreamer_app::AppSink)> {
    use gstreamer::prelude::*;

    gstreamer::init().map_err(|e| BackendError::Other(format!("GStreamer init failed: {}", e)))?;

    let path_str = path.to_string_lossy();
    let pipeline_str = format!(
        "filesrc location=\"{}\" ! decodebin ! \
         videoconvert ! video/x-raw,format=RGBA ! \
         appsink name=sink max-buffers=1 drop=true sync=false",
        path_str
    );

    let pipeline = gstreamer::parse::launch(&pipeline_str)
        .map_err(|e| BackendError::Other(format!("Failed to create pipeline: {}", e)))?
        .downcast::<gstreamer::Pipeline>()
        .map_err(|_| BackendError::Other("Failed to downcast to Pipeline".into()))?;

    let appsink = pipeline
        .by_name("sink")
        .ok_or_else(|| BackendError::Other("Failed to find appsink".into()))?
        .downcast::<gstreamer_app::AppSink>()
        .map_err(|_| BackendError::Other("Failed to downcast to AppSink".into()))?;

    Ok((pipeline, appsink))
}

/// Load the first frame from a video file
///
/// Creates a temporary decoder to extract just the first frame.
fn load_video_first_frame(path: &Path) -> BackendResult<CameraFrame> {
    use gstreamer::prelude::*;

    info!(path = %path.display(), "Loading first frame from video");

    let (pipeline, appsink) = create_frame_extraction_pipeline(path)?;

    pipeline
        .set_state(gstreamer::State::Playing)
        .map_err(|e| BackendError::Other(format!("Failed to start pipeline: {:?}", e)))?;

    let sample = appsink
        .try_pull_sample(gstreamer::ClockTime::from_seconds(
            vc_timing::VIDEO_FRAME_TIMEOUT_SECS,
        ))
        .ok_or_else(|| BackendError::Other("Timeout waiting for first video frame".into()))?;

    let frame = extract_frame_from_sample(&sample)?;
    let _ = pipeline.set_state(gstreamer::State::Null);

    info!(
        width = frame.width,
        height = frame.height,
        "Video first frame loaded successfully"
    );
    Ok(frame)
}

/// Load an image file and convert it to a CameraFrame
///
/// Supports common image formats: PNG, JPEG, GIF, BMP, WebP
pub fn load_image_as_frame(path: &Path) -> BackendResult<CameraFrame> {
    info!(path = %path.display(), "Loading image file");

    let img = image::open(path).map_err(|e| {
        BackendError::Other(format!("Failed to load image '{}': {}", path.display(), e))
    })?;

    let rgba = img.to_rgba8();
    let width = rgba.width();
    let height = rgba.height();
    let data: Vec<u8> = rgba.into_raw();

    info!(width, height, "Image loaded successfully");

    Ok(CameraFrame {
        data: Arc::from(data.into_boxed_slice()),
        width,
        height,
        stride: width * 4, // RGBA = 4 bytes per pixel
        format: PixelFormat::RGBA,
        captured_at: Instant::now(),
        depth_data: None,
        depth_width: 0,
        depth_height: 0,
        video_timestamp: None,
    })
}

/// Video file decoder for streaming frames with audio
///
/// Uses GStreamer to decode video files and provides frames as CameraFrame.
/// Audio is forwarded to a PipeWire virtual microphone.
/// Videos are looped automatically.
pub struct VideoDecoder {
    /// Main pipeline for video decoding
    video_pipeline: gstreamer::Pipeline,
    /// Video appsink for frame extraction
    video_appsink: gstreamer_app::AppSink,
    /// Audio pipeline for virtual microphone (separate for independent control)
    audio_pipeline: Option<gstreamer::Pipeline>,
    /// Video dimensions
    width: u32,
    height: u32,
    /// Whether the video has audio
    has_audio: bool,
}

impl VideoDecoder {
    /// Create a new video decoder for the given file
    ///
    /// The pipeline decodes video to RGBA format and optionally
    /// forwards audio to a PipeWire virtual microphone.
    pub fn new(path: &Path) -> BackendResult<Self> {
        use gstreamer::prelude::*;

        info!(path = %path.display(), "Creating video decoder with audio support");

        gstreamer::init()
            .map_err(|e| BackendError::Other(format!("GStreamer init failed: {}", e)))?;

        let path_str = path.to_string_lossy();

        // Create video pipeline: filesrc → decodebin → videoconvert → appsink
        // Note: sync=true is important to play video at correct speed (matches video's native framerate)
        let video_pipeline_str = format!(
            "filesrc location=\"{}\" ! decodebin name=decode ! \
             queue ! videoconvert ! video/x-raw,format=RGBA ! appsink name=videosink emit-signals=true sync=true",
            path_str
        );

        let video_pipeline = gstreamer::parse::launch(&video_pipeline_str)
            .map_err(|e| BackendError::Other(format!("Failed to create video pipeline: {}", e)))?
            .downcast::<gstreamer::Pipeline>()
            .map_err(|_| BackendError::Other("Failed to downcast to Pipeline".into()))?;

        let video_appsink = video_pipeline
            .by_name("videosink")
            .ok_or_else(|| BackendError::Other("Failed to find video appsink".into()))?
            .downcast::<gstreamer_app::AppSink>()
            .map_err(|_| BackendError::Other("Failed to downcast to AppSink".into()))?;

        // Configure video appsink
        video_appsink.set_max_buffers(1);
        video_appsink.set_drop(true);

        // Start video pipeline
        video_pipeline
            .set_state(gstreamer::State::Playing)
            .map_err(|e| BackendError::Other(format!("Failed to start video pipeline: {:?}", e)))?;

        // Wait for preroll to get video dimensions
        let bus = video_pipeline.bus().unwrap();
        let mut width = 0u32;
        let mut height = 0u32;

        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while std::time::Instant::now() < deadline {
            if let Some(msg) = bus.timed_pop(gstreamer::ClockTime::from_mseconds(100)) {
                use gstreamer::MessageView;
                match msg.view() {
                    MessageView::Error(err) => {
                        let _ = video_pipeline.set_state(gstreamer::State::Null);
                        return Err(BackendError::Other(format!(
                            "Video pipeline error: {}",
                            err.error()
                        )));
                    }
                    MessageView::StateChanged(state) => {
                        if state.src() == Some(video_pipeline.upcast_ref())
                            && state.current() == gstreamer::State::Playing
                        {
                            if let Some(pad) = video_appsink.static_pad("sink") {
                                if let Some(caps) = pad.current_caps() {
                                    if let Some(s) = caps.structure(0) {
                                        width = s.get::<i32>("width").unwrap_or(0) as u32;
                                        height = s.get::<i32>("height").unwrap_or(0) as u32;
                                        if width > 0 && height > 0 {
                                            break;
                                        }
                                    }
                                }
                            }
                        }
                    }
                    _ => {}
                }
            }

            if let Some(pad) = video_appsink.static_pad("sink") {
                if let Some(caps) = pad.current_caps() {
                    if let Some(s) = caps.structure(0) {
                        width = s.get::<i32>("width").unwrap_or(0) as u32;
                        height = s.get::<i32>("height").unwrap_or(0) as u32;
                        if width > 0 && height > 0 {
                            break;
                        }
                    }
                }
            }
        }

        if width == 0 || height == 0 {
            let _ = video_pipeline.set_state(gstreamer::State::Null);
            return Err(BackendError::Other(
                "Failed to determine video dimensions".into(),
            ));
        }

        // Try to create audio pipeline for virtual microphone
        let audio_pipeline = Self::create_audio_pipeline(path);
        let has_audio = audio_pipeline.is_some();

        info!(width, height, has_audio, "Video decoder created");

        Ok(Self {
            video_pipeline,
            video_appsink,
            audio_pipeline,
            width,
            height,
            has_audio,
        })
    }

    /// Create audio pipeline that sends audio to a PipeWire virtual microphone
    fn create_audio_pipeline(path: &Path) -> Option<gstreamer::Pipeline> {
        use gstreamer::prelude::*;

        let path_str = path.to_string_lossy();

        // Create audio pipeline: filesrc → decodebin → audioconvert → audioresample → pipewiresink
        // The pipewiresink creates a virtual microphone that other apps can use
        let audio_pipeline_str = format!(
            "filesrc location=\"{}\" ! decodebin name=decode ! \
             queue ! audioconvert ! audioresample ! \
             audio/x-raw,format=F32LE,channels=2,rate=48000 ! \
             pipewiresink name=audiosink stream-properties=\"p,media.class=Audio/Source,node.name=Camera Virtual Mic,media.role=Communication\" sync=true",
            path_str
        );

        match gstreamer::parse::launch(&audio_pipeline_str) {
            Ok(element) => {
                match element.downcast::<gstreamer::Pipeline>() {
                    Ok(pipeline) => {
                        // Start the audio pipeline
                        match pipeline.set_state(gstreamer::State::Playing) {
                            Ok(_) => {
                                // Wait a bit and check if the pipeline is actually running
                                std::thread::sleep(vc_timing::AUDIO_PIPELINE_STARTUP_DELAY);

                                // Check for errors on the bus
                                if let Some(bus) = pipeline.bus() {
                                    while let Some(msg) = bus.pop() {
                                        use gstreamer::MessageView;
                                        if let MessageView::Error(err) = msg.view() {
                                            warn!(
                                                error = %err.error(),
                                                debug = ?err.debug(),
                                                "Audio pipeline error, disabling audio"
                                            );
                                            let _ = pipeline.set_state(gstreamer::State::Null);
                                            return None;
                                        }
                                    }
                                }

                                info!("Audio pipeline created - virtual microphone active");
                                Some(pipeline)
                            }
                            Err(e) => {
                                warn!(?e, "Failed to start audio pipeline");
                                None
                            }
                        }
                    }
                    Err(_) => {
                        warn!("Failed to downcast audio pipeline");
                        None
                    }
                }
            }
            Err(e) => {
                debug!(
                    ?e,
                    "Could not create audio pipeline (video may not have audio track)"
                );
                None
            }
        }
    }

    /// Convert a GStreamer sample to a CameraFrame using decoder dimensions
    fn sample_to_frame(&self, sample: gstreamer::Sample) -> Option<CameraFrame> {
        let buffer = sample.buffer()?;
        let map = buffer.map_readable().ok()?;
        let data: Vec<u8> = map.as_slice().to_vec();

        Some(CameraFrame {
            data: Arc::from(data.into_boxed_slice()),
            width: self.width,
            height: self.height,
            stride: self.width * 4,
            format: PixelFormat::RGBA,
            captured_at: Instant::now(),
            depth_data: None,
            depth_width: 0,
            depth_height: 0,
            video_timestamp: None,
        })
    }

    /// Get the preroll frame (first frame available immediately after pipeline starts)
    ///
    /// This should be called once right after creating the decoder to get
    /// the first frame without waiting for sync timing. Returns None if
    /// preroll is not yet available.
    pub fn preroll_frame(&self) -> Option<CameraFrame> {
        self.video_appsink
            .pull_preroll()
            .ok()
            .and_then(|s| self.sample_to_frame(s))
    }

    /// Get the next frame from the video
    ///
    /// Blocks until a frame is available. Returns None if the video has ended
    /// (caller should restart for looping).
    pub fn next_frame(&self) -> Option<CameraFrame> {
        self.video_appsink
            .pull_sample()
            .ok()
            .and_then(|s| self.sample_to_frame(s))
    }

    /// Check if the video has reached the end
    pub fn is_eos(&self) -> bool {
        use gstreamer::prelude::*;

        if let Some(bus) = self.video_pipeline.bus() {
            while let Some(msg) = bus.pop() {
                use gstreamer::MessageView;
                if let MessageView::Eos(_) = msg.view() {
                    return true;
                }
            }
        }
        false
    }

    /// Restart the video from the beginning (for looping)
    pub fn restart(&self) -> BackendResult<()> {
        use gstreamer::prelude::*;

        debug!("Restarting video for loop");

        // Seek video pipeline to beginning
        if let Err(e) = self.video_pipeline.seek_simple(
            gstreamer::SeekFlags::FLUSH | gstreamer::SeekFlags::KEY_UNIT,
            gstreamer::ClockTime::ZERO,
        ) {
            warn!(?e, "Video seek failed");
        }

        // Also restart audio pipeline if present
        if let Some(ref audio_pipeline) = self.audio_pipeline {
            if let Err(e) = audio_pipeline.seek_simple(
                gstreamer::SeekFlags::FLUSH | gstreamer::SeekFlags::KEY_UNIT,
                gstreamer::ClockTime::ZERO,
            ) {
                warn!(?e, "Audio seek failed");
            }
        }

        Ok(())
    }

    /// Get video dimensions
    pub fn dimensions(&self) -> (u32, u32) {
        (self.width, self.height)
    }

    /// Check if the video has audio
    pub fn has_audio(&self) -> bool {
        self.has_audio
    }

    /// Get the current playback position in seconds
    pub fn position(&self) -> Option<f64> {
        use gstreamer::prelude::*;

        self.video_pipeline
            .query_position::<gstreamer::ClockTime>()
            .map(|pos| pos.seconds_f64())
    }

    /// Get the total video duration in seconds
    pub fn duration(&self) -> Option<f64> {
        use gstreamer::prelude::*;

        self.video_pipeline
            .query_duration::<gstreamer::ClockTime>()
            .map(|dur| dur.seconds_f64())
    }

    /// Get playback progress as a fraction (0.0 to 1.0)
    pub fn progress(&self) -> Option<f64> {
        match (self.position(), self.duration()) {
            (Some(pos), Some(dur)) if dur > 0.0 => Some((pos / dur).clamp(0.0, 1.0)),
            _ => None,
        }
    }

    /// Seek to a specific position in seconds
    pub fn seek(&self, position_secs: f64) {
        use gstreamer::prelude::*;

        let position =
            gstreamer::ClockTime::from_nseconds((position_secs * 1_000_000_000.0) as u64);
        debug!(position_secs, "Seeking video to position");

        // Seek video pipeline
        if let Err(e) = self.video_pipeline.seek_simple(
            gstreamer::SeekFlags::FLUSH | gstreamer::SeekFlags::KEY_UNIT,
            position,
        ) {
            warn!(?e, "Video seek failed");
        }

        // Also seek audio pipeline if present
        if let Some(ref audio_pipeline) = self.audio_pipeline {
            if let Err(e) = audio_pipeline.seek_simple(
                gstreamer::SeekFlags::FLUSH | gstreamer::SeekFlags::KEY_UNIT,
                position,
            ) {
                warn!(?e, "Audio seek failed");
            }
        }
    }

    /// Pause or resume playback
    pub fn set_paused(&self, paused: bool) {
        use gstreamer::prelude::*;

        let state = if paused {
            gstreamer::State::Paused
        } else {
            gstreamer::State::Playing
        };

        debug!(paused, "Setting video pause state");

        if let Err(e) = self.video_pipeline.set_state(state) {
            warn!(?e, "Failed to set video pipeline state");
        }

        // Also pause/resume audio pipeline if present
        if let Some(ref audio_pipeline) = self.audio_pipeline {
            if let Err(e) = audio_pipeline.set_state(state) {
                warn!(?e, "Failed to set audio pipeline state");
            }
        }
    }

    /// Stop the decoder
    pub fn stop(&self) {
        use gstreamer::prelude::*;

        let _ = self.video_pipeline.set_state(gstreamer::State::Null);

        if let Some(ref audio_pipeline) = self.audio_pipeline {
            let _ = audio_pipeline.set_state(gstreamer::State::Null);
            info!("Video decoder stopped (video + audio)");
        } else {
            info!("Video decoder stopped (video only)");
        }
    }
}

impl Drop for VideoDecoder {
    fn drop(&mut self) {
        self.stop();
    }
}
