// SPDX-License-Identifier: GPL-3.0-only

//! Video recording pipeline with intelligent encoder selection
//!
//! This module implements video recording with:
//! - Automatic hardware encoder detection and selection
//! - Preview continues during recording (tee-based pipeline)
//! - Audio integration
//! - Quality presets

use super::encoder_selection::{EncoderConfig, select_encoders};
use super::muxer::{create_muxer, link_audio_to_muxer, link_muxer_to_sink, link_video_to_muxer};
use crate::backends::camera::types::{CameraFrame, FrameData, SensorRotation};
use gstreamer as gst;
use gstreamer::prelude::*;
use gstreamer_app as gst_app;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use tracing::{debug, error, info, warn};

/// Live audio level data shared between the GStreamer pipeline and the UI.
///
/// Updated by a GStreamer bus watcher when `level` elements post messages.
#[derive(Debug, Clone)]
pub struct AudioLevels {
    /// Per-input-channel peak levels in dB (before mono mix).
    /// One entry per source channel (e.g. 6 for Scarlett pro-audio, 2 for stereo).
    pub input_peak_db: Vec<f64>,
    /// Per-input-channel RMS levels in dB (before mono mix).
    pub input_rms_db: Vec<f64>,
    /// Mono output peak level in dB (after mix).
    pub output_peak_db: f64,
    /// Mono output RMS level in dB (after mix).
    pub output_rms_db: f64,
}

impl Default for AudioLevels {
    fn default() -> Self {
        Self {
            input_peak_db: Vec::new(),
            input_rms_db: Vec::new(),
            output_peak_db: -100.0,
            output_rms_db: -100.0,
        }
    }
}

/// Thread-safe handle to live audio levels.
pub type SharedAudioLevels = Arc<Mutex<AudioLevels>>;

/// Common recording configuration shared by both PipeWire and appsrc paths.
pub struct RecorderConfig<'a> {
    /// Video width
    pub width: u32,
    /// Video height
    pub height: u32,
    /// Video framerate
    pub framerate: u32,
    /// Output file path
    pub output_path: PathBuf,
    /// Encoder configuration
    pub encoder_config: EncoderConfig,
    /// Whether to record audio
    pub enable_audio: bool,
    /// Optional audio device path
    pub audio_device: Option<&'a str>,
    /// Specific encoder info (if None, auto-select)
    pub encoder_info: Option<&'a crate::media::encoders::video::EncoderInfo>,
    /// Sensor rotation to correct video orientation
    pub rotation: SensorRotation,
    /// Pre-created shared audio levels handle (UI reads this for live meters)
    pub audio_levels: SharedAudioLevels,
}

/// PipeWire-specific recording configuration.
pub struct PipeWireRecorderConfig<'a> {
    /// Common recording settings
    pub base: RecorderConfig<'a>,
    /// Camera device path
    pub device_path: &'a str,
    /// Optional metadata path for PipeWire
    pub metadata_path: Option<&'a str>,
    /// Pixel format (e.g., "NV12", "MJPEG")
    pub pixel_format: &'a str,
    /// Optional preview frame sender
    pub preview_sender: Option<tokio::sync::mpsc::Sender<CameraFrame>>,
}

/// Appsrc-specific recording configuration (libcamera backend).
///
/// Frames are pushed from the application via a `tokio::sync::mpsc` channel
/// instead of using `pipewiresrc`. This avoids camera contention when the
/// native libcamera pipeline already holds the device.
pub struct AppsrcRecorderConfig<'a> {
    /// Common recording settings
    pub base: RecorderConfig<'a>,
    /// Pixel format of incoming frames
    pub pixel_format: crate::backends::camera::types::PixelFormat,
}

/// Video recorder using the new pipeline architecture
#[derive(Debug)]
pub struct VideoRecorder {
    pipeline: gst::Pipeline,
    file_path: PathBuf,
    #[allow(dead_code)]
    _preview_task: Option<tokio::task::JoinHandle<()>>,
    /// Shared live audio levels (updated by bus sync handler, read by UI via its own clone)
    #[allow(dead_code)]
    audio_levels: SharedAudioLevels,
}

/// Map sensor rotation to the GStreamer videoflip `video-direction` value.
/// Returns None for SensorRotation::None (no rotation needed).
fn rotation_to_flip_direction(rotation: SensorRotation) -> Option<&'static str> {
    match rotation {
        SensorRotation::Rotate90 => Some("90l"),
        SensorRotation::Rotate180 => Some("180"),
        SensorRotation::Rotate270 => Some("90r"),
        SensorRotation::None => None,
    }
}

/// OpenH264 maximum pixel count (roughly 3072x3072).
const OPENH264_MAX_PIXELS: u32 = 9_437_184;

/// Downscale dimensions if they exceed OpenH264's pixel limit.
/// Returns the original dimensions if the encoder is not OpenH264 or the limit is not exceeded.
fn openh264_downscale(base_width: u32, base_height: u32, encoder_name: &str) -> (u32, u32) {
    let pixels = base_width * base_height;
    if encoder_name == "openh264enc" && pixels > OPENH264_MAX_PIXELS {
        let aspect_ratio = base_width as f64 / base_height as f64;
        let target_width = 1920u32;
        let target_height = (target_width as f64 / aspect_ratio) as u32 & !1; // even height
        warn!(
            "OpenH264 resolution limit exceeded ({}x{} = {} pixels > {} max), downscaling to {}x{}",
            base_width, base_height, pixels, OPENH264_MAX_PIXELS, target_width, target_height,
        );
        (target_width, target_height)
    } else {
        (base_width, base_height)
    }
}

/// Select encoder set: use a specific encoder if provided, otherwise auto-select.
fn select_encoder_set(
    encoder_info: Option<&crate::media::encoders::video::EncoderInfo>,
    encoder_config: &EncoderConfig,
    enable_audio: bool,
) -> Result<super::encoder_selection::SelectedEncoders, String> {
    if let Some(enc_info) = encoder_info {
        super::encoder_selection::select_encoders_with_video(encoder_config, enc_info, enable_audio)
    } else {
        select_encoders(encoder_config, enable_audio)
    }
}

impl VideoRecorder {
    /// Create a new video recorder with intelligent encoder selection
    ///
    /// # Arguments
    /// * `config` - PipeWire recorder configuration
    ///
    /// # Returns
    /// * `Ok(VideoRecorder)` - Video recorder instance
    /// * `Err(String)` - Error message
    pub fn new(config: PipeWireRecorderConfig<'_>) -> Result<Self, String> {
        let PipeWireRecorderConfig {
            base:
                RecorderConfig {
                    width,
                    height,
                    framerate,
                    output_path,
                    encoder_config,
                    enable_audio,
                    audio_device,
                    encoder_info,
                    rotation,
                    audio_levels,
                },
            device_path,
            metadata_path,
            pixel_format,
            preview_sender,
        } = config;

        info!(
            device = %device_path,
            metadata = ?metadata_path,
            width,
            height,
            framerate,
            format = %pixel_format,
            output = %output_path.display(),
            audio = enable_audio,
            "Creating video recorder with new pipeline"
        );

        let encoders = select_encoder_set(encoder_info, &encoder_config, enable_audio)?;

        info!(
            video_codec = ?encoders.video.codec,
            audio_codec = ?encoders.audio.as_ref().map(|a| a.codec),
            container = ?encoders.video.container,
            "Selected encoders"
        );

        // Update output path with correct extension
        let output_path = output_path.with_extension(encoders.video.extension);

        // Create pipeline
        let pipeline = gst::Pipeline::new();

        // Create PipeWire video source (PipeWire-only application)
        let source = Self::create_video_source(device_path)?;

        // Create JPEG decoder if needed
        let jpeg_decoder = if pixel_format == "MJPG" || pixel_format == "MJPEG" {
            info!("Adding JPEG decoder for MJPEG source");
            Some(
                gst::ElementFactory::make("jpegdec")
                    .build()
                    .map_err(|e| format!("Failed to create jpegdec: {}", e))?,
            )
        } else {
            None
        };

        // Video processing elements
        let videoconvert = gst::ElementFactory::make("videoconvert")
            .build()
            .map_err(|e| format!("Failed to create videoconvert: {}", e))?;

        // Create videoflip element if rotation is needed.
        // The sensor Rotation property gives the physical mounting angle.
        // To correct, we apply the INVERSE rotation.
        let videoflip = if let Some(flip_method) = rotation_to_flip_direction(rotation) {
            info!(
                rotation = %rotation,
                flip_method,
                "Adding videoflip element to correct sensor rotation"
            );
            Some(
                gst::ElementFactory::make("videoflip")
                    .property_from_str("video-direction", flip_method)
                    .build()
                    .map_err(|e| format!("Failed to create videoflip: {}", e))?,
            )
        } else {
            None
        };

        let videoscale = gst::ElementFactory::make("videoscale")
            .build()
            .map_err(|e| format!("Failed to create videoscale: {}", e))?;

        // Account for dimension swap when rotation is 90° or 270°
        let (base_width, base_height) = if rotation.swaps_dimensions() {
            (height, width) // Swap dimensions for rotated video
        } else {
            (width, height)
        };

        // OpenH264 has a maximum resolution limit — downscale if exceeded
        let enc_name = encoders
            .video
            .encoder
            .factory()
            .map(|f| f.name().to_string())
            .unwrap_or_default();
        let (final_width, final_height) = openh264_downscale(base_width, base_height, &enc_name);

        // Set desired output caps
        let output_caps = gst::Caps::builder("video/x-raw")
            .field("width", final_width as i32)
            .field("height", final_height as i32)
            .field("framerate", gst::Fraction::new(framerate as i32, 1))
            .build();

        let capsfilter = gst::ElementFactory::make("capsfilter")
            .property("caps", &output_caps)
            .build()
            .map_err(|e| format!("Failed to create capsfilter: {}", e))?;

        // Tee to split stream into recording and preview branches
        let tee = gst::ElementFactory::make("tee")
            .build()
            .map_err(|e| format!("Failed to create tee: {}", e))?;

        // Recording branch queue
        let record_queue = gst::ElementFactory::make("queue")
            .build()
            .map_err(|e| format!("Failed to create record queue: {}", e))?;

        // Preview branch (if enabled)
        let preview_elements = Self::create_preview_branch(preview_sender.as_ref())?;

        // Get encoder elements
        let video_encoder = encoders.video.encoder;
        let video_parser = encoders.video.parser;

        // Create muxer
        let muxer_config = create_muxer(encoders.video.muxer, output_path.clone())?;

        // Audio branch (if enabled)
        let audio_elements = if let Some(audio_encoder_config) = encoders.audio {
            Self::create_audio_branch(audio_device, audio_encoder_config)?
        } else {
            None
        };

        // Add all elements to pipeline
        let mut elements: Vec<&gst::Element> = vec![&source];

        if let Some(ref decoder) = jpeg_decoder {
            elements.push(decoder);
        }

        elements.push(&videoconvert);

        if let Some(ref flip) = videoflip {
            elements.push(flip);
        }

        elements.extend_from_slice(&[
            &videoscale,
            &capsfilter,
            &tee,
            &record_queue,
            &video_encoder,
        ]);

        if let Some(ref parser) = video_parser {
            elements.push(parser);
        }

        elements.push(&muxer_config.muxer);
        elements.push(&muxer_config.filesink);

        if let Some((ref preview_queue, ref appsink)) = preview_elements {
            elements.push(preview_queue);
            elements.push(appsink.upcast_ref::<gst::Element>());
        }

        if let Some(ref audio_branch) = audio_elements {
            elements.push(&audio_branch.source);
            elements.push(&audio_branch.queue);
            elements.push(&audio_branch.convert);
            elements.push(&audio_branch.resample);
            elements.push(&audio_branch.level_input);
            elements.push(&audio_branch.capsfilter);
            elements.push(&audio_branch.level_output);
            elements.push(&audio_branch.encoder);
        }

        pipeline
            .add_many(&elements)
            .map_err(|e| format!("Failed to add elements to pipeline: {}", e))?;

        // Link video chain
        Self::link_video_chain(
            &source,
            jpeg_decoder.as_ref(),
            &videoconvert,
            videoflip.as_ref(),
            &videoscale,
            &capsfilter,
            &tee,
        )?;

        // Link recording branch
        Self::link_recording_branch(
            &tee,
            &record_queue,
            &video_encoder,
            video_parser.as_ref(),
            &muxer_config.muxer,
        )?;

        // Link muxer to filesink
        link_muxer_to_sink(&muxer_config.muxer, &muxer_config.filesink)?;

        // Link preview branch if enabled
        let preview_task = Self::link_preview_branch(&tee, preview_elements, preview_sender)?;

        // Link audio branch if enabled
        if let Some(audio_branch) = audio_elements {
            Self::link_audio_chain(&audio_branch)?;
            link_audio_to_muxer(&audio_branch.encoder, &muxer_config.muxer)?;
            Self::install_level_sync_handler(&pipeline, &audio_levels);
        }

        Ok(VideoRecorder {
            pipeline,
            file_path: output_path,
            _preview_task: preview_task,
            audio_levels,
        })
    }

    /// Create an appsrc-based video recorder for the libcamera backend.
    ///
    /// Frames from the native capture pipeline are received via `frame_rx` and
    /// pushed into a GStreamer encoding pipeline through `appsrc`. The preview
    /// continues uninterrupted because the same frames are displayed in the UI
    /// and forwarded here.
    ///
    /// The returned recorder must be started with `.start()`. When the `frame_rx`
    /// channel closes (sender dropped), the pusher task sends EOS and the
    /// pipeline finalizes gracefully.
    pub fn new_from_appsrc(
        config: AppsrcRecorderConfig<'_>,
        frame_rx: tokio::sync::mpsc::Receiver<Arc<CameraFrame>>,
    ) -> Result<Self, String> {
        let AppsrcRecorderConfig {
            base:
                RecorderConfig {
                    width,
                    height,
                    framerate,
                    output_path,
                    encoder_config,
                    enable_audio,
                    audio_device,
                    encoder_info,
                    rotation,
                    audio_levels,
                },
            pixel_format,
        } = config;

        info!(
            width,
            height,
            framerate,
            format = ?pixel_format,
            output = %output_path.display(),
            audio = enable_audio,
            audio_device = ?audio_device,
            rotation = %rotation,
            "Creating appsrc-based video recorder (libcamera backend)"
        );

        let encoders = select_encoder_set(encoder_info, &encoder_config, enable_audio)?;

        // Create audio branch (reuses the same pulsesrc-based chain as the
        // PipeWire recording path).  The branch is added to the pipeline after
        // gst_parse_launch creates the video portion.
        let audio_elements = if let Some(audio_encoder_config) = encoders.audio {
            Self::create_audio_branch(audio_device, audio_encoder_config)?
        } else {
            None
        };

        info!(
            video_codec = ?encoders.video.codec,
            audio = audio_elements.is_some(),
            container = ?encoders.video.container,
            "Selected encoders for appsrc pipeline"
        );

        let output_path = output_path.with_extension(encoders.video.extension);

        // Determine video parameters
        let initial_gst_format = pixel_format.to_gst_format_string();
        let frame_duration_ns = 1_000_000_000i64 / framerate as i64;

        let (base_width, base_height) = if rotation.swaps_dimensions() {
            (height, width)
        } else {
            (width, height)
        };

        // Inverse rotation: sensor mounting angle → correction direction
        let flip_str = rotation_to_flip_direction(rotation)
            .map(|dir| format!("! videoflip video-direction={dir}"))
            .unwrap_or_default();

        // Get encoder element name.
        // V4L2 hardware encoders are probed at startup and removed from the
        // list if broken. This is a safety net in case the probe hasn't
        // completed yet or the encoder fails differently with appsrc.
        let selected_encoder = encoders
            .video
            .encoder
            .factory()
            .map(|f| f.name().to_string())
            .unwrap_or_else(|| "openh264enc".to_string());
        let (encoder_name, parser_str, muxer_name) = if selected_encoder.starts_with("v4l2") {
            warn!(
                selected = %selected_encoder,
                "V4L2 encoder not compatible with appsrc, falling back to openh264enc"
            );
            (
                "openh264enc".to_string(),
                "! h264parse".to_string(),
                "mp4mux".to_string(),
            )
        } else {
            let parser = encoders
                .video
                .parser
                .as_ref()
                .and_then(|p| p.factory().map(|f| format!("! {}", f.name())))
                .unwrap_or_default();
            let muxer = encoders
                .video
                .muxer
                .factory()
                .map(|f| f.name().to_string())
                .unwrap_or_else(|| "mp4mux".to_string());
            (selected_encoder, parser, muxer)
        };

        // OpenH264 has a maximum resolution limit — downscale if exceeded
        let (final_width, final_height) =
            openh264_downscale(base_width, base_height, &encoder_name);

        // Build the pipeline using gst_parse_launch — this correctly handles
        // caps negotiation (including appsrc's internal_get_caps quirks in
        // GStreamer 1.28 where current_caps is only set when data flows).
        let output_path_str = output_path.display().to_string();
        let pipeline_desc = format!(
            "appsrc name=camera-appsrc \
               caps=video/x-raw,format={fmt},width={w},height={h},framerate={fps}/1 \
               is-live=true do-timestamp=false format=time \
               min-latency={lat} max-latency={lat} \
             ! queue max-size-buffers=5 max-size-time=1000000000 leaky=downstream \
             ! videoconvert \
             {flip} \
             ! videoscale \
             ! capsfilter caps=video/x-raw,format=I420,width={fw},height={fh},framerate={fps}/1 \
             ! videoconvert \
             ! {encoder} name=recording-encoder \
             {parser} \
             ! {muxer} name=recording-muxer \
             ! filesink location={loc}",
            fmt = initial_gst_format,
            w = width,
            h = height,
            fps = framerate,
            lat = frame_duration_ns,
            flip = flip_str,
            fw = final_width,
            fh = final_height,
            encoder = encoder_name,
            parser = parser_str,
            muxer = muxer_name,
            loc = output_path_str,
        );

        info!(desc = %pipeline_desc, "Launching appsrc pipeline");

        let pipeline = gst::parse::launch(&pipeline_desc)
            .map_err(|e| format!("Failed to parse pipeline: {}", e))?
            .dynamic_cast::<gst::Pipeline>()
            .map_err(|_| "Failed to cast to Pipeline")?;

        let appsrc = pipeline
            .by_name("camera-appsrc")
            .ok_or("Failed to find camera-appsrc in pipeline")?
            .dynamic_cast::<gst_app::AppSrc>()
            .map_err(|_| "Failed to cast to AppSrc")?;

        // Configure the encoder element with bitrate and quality settings.
        // gst_parse_launch creates a fresh element — the one configured by
        // select_encoders_with_video() was discarded (we only used its name).
        if let Some(enc_element) = pipeline.by_name("recording-encoder") {
            crate::media::encoders::video::configure_video_encoder(
                &enc_element,
                &encoder_name,
                encoder_config.video_quality,
                final_width,
                final_height,
                encoder_config.bitrate_override_kbps,
            );
        }

        // Some encoders (x265enc, vaapih265enc, x264enc) output encoded
        // buffers with valid DTS but PTS=NONE on non-keyframes.  mp4mux
        // requires every buffer to carry a PTS.  Fix this by installing a
        // pad probe on the muxer's video sink pad that copies DTS → PTS
        // when PTS is missing.
        if let Some(muxer) = pipeline.by_name("recording-muxer") {
            for pad in muxer.sink_pads() {
                pad.add_probe(gst::PadProbeType::BUFFER, move |_pad, info| {
                    if let Some(buffer) = info.buffer_mut()
                        && buffer.pts().is_none()
                        && let Some(dts) = buffer.dts()
                    {
                        let buf = buffer.make_mut();
                        buf.set_pts(dts);
                    }
                    gst::PadProbeReturn::Ok
                });
            }
        }

        // Add audio branch to the pipeline (elements created before parse_launch,
        // linked here so they feed into the named muxer).
        if let Some(ref audio_branch) = audio_elements {
            pipeline
                .add_many([
                    &audio_branch.source,
                    &audio_branch.queue,
                    &audio_branch.convert,
                    &audio_branch.resample,
                    &audio_branch.level_input,
                    &audio_branch.capsfilter,
                    &audio_branch.level_output,
                    &audio_branch.encoder,
                ])
                .map_err(|e| format!("Failed to add audio elements to pipeline: {}", e))?;

            Self::link_audio_chain(audio_branch)?;

            let muxer = pipeline
                .by_name("recording-muxer")
                .ok_or("Failed to find recording-muxer for audio linking")?;
            link_audio_to_muxer(&audio_branch.encoder, &muxer)?;

            Self::install_level_sync_handler(&pipeline, &audio_levels);

            info!("Audio branch added to appsrc recording pipeline");
        }

        // Spawn the pusher task that reads frames from channel and pushes to appsrc.
        // The pusher polls current_running_time() to detect when the pipeline
        // reaches PLAYING, skipping frames until then.
        let pusher_task =
            Self::spawn_appsrc_pusher(appsrc, frame_rx, pixel_format, width, height, framerate);

        Ok(VideoRecorder {
            pipeline,
            file_path: output_path,
            _preview_task: Some(pusher_task),
            audio_levels,
        })
    }

    /// Read `CLOCK_BOOTTIME` in nanoseconds (same clock domain as libcamera
    /// sensor timestamps).
    fn read_clock_boottime_ns() -> u64 {
        use std::mem::MaybeUninit;
        unsafe {
            let mut ts = MaybeUninit::<libc::timespec>::uninit();
            if libc::clock_gettime(libc::CLOCK_BOOTTIME, ts.as_mut_ptr()) != 0 {
                return 0;
            }
            let ts = ts.assume_init();
            ts.tv_sec as u64 * 1_000_000_000 + ts.tv_nsec as u64
        }
    }

    /// Spawn a tokio task that reads `CameraFrame`s from the channel and pushes
    /// them into the GStreamer `appsrc` element as `gst::Buffer`s.
    ///
    /// Frames arriving before the pipeline reaches PLAYING are skipped.
    /// PTS is computed per-frame in the pipeline's running-time domain:
    ///   PTS = current_running_time - (CLOCK_BOOTTIME_now - sensor_ts)
    /// This places video timestamps in the same clock domain as pulsesrc
    /// audio, achieving A/V sync regardless of processing delay.
    /// Falls back to frame-count-based PTS if sensor timestamps are unavailable.
    ///
    /// On first frame the appsrc caps may be corrected (to handle MJPEG chroma
    /// subsampling detection). When the channel closes (sender dropped), EOS is
    /// sent to finalize the file.
    fn spawn_appsrc_pusher(
        appsrc: gst_app::AppSrc,
        mut frame_rx: tokio::sync::mpsc::Receiver<Arc<CameraFrame>>,
        pixel_format: crate::backends::camera::types::PixelFormat,
        width: u32,
        height: u32,
        framerate: u32,
    ) -> tokio::task::JoinHandle<()> {
        tokio::spawn(async move {
            info!("Appsrc pusher task started");

            let mut caps_corrected = false;
            let mut frame_count: u64 = 0;
            let start_time = std::time::Instant::now();
            let initial_format = pixel_format.to_gst_format_string();
            let frame_duration_ns = 1_000_000_000u64 / framerate as u64;

            // Whether the pipeline has reached PLAYING (running_time available).
            let mut pipeline_playing = false;

            while let Some(frame) = frame_rx.recv().await {
                // On first frame, check if the actual format differs from the
                // initial caps (e.g. MJPEG decoded to I422 instead of I420).
                if !caps_corrected {
                    let actual_format = frame.gst_format_string();
                    if actual_format != initial_format {
                        let caps = gst::Caps::builder("video/x-raw")
                            .field("format", actual_format)
                            .field("width", width as i32)
                            .field("height", height as i32)
                            .field("framerate", gst::Fraction::new(framerate as i32, 1))
                            .build();
                        info!(
                            initial = initial_format,
                            actual = actual_format,
                            "Correcting appsrc caps: {}",
                            caps
                        );
                        appsrc.set_property("caps", &caps);
                    }
                    caps_corrected = true;
                }

                // Compute PTS in the pipeline's running-time domain so that
                // video timestamps are directly comparable to pulsesrc audio.
                //
                // For each frame we query the pipeline's current_running_time()
                // and subtract the processing delay (wall-clock time between
                // sensor capture and this moment).  This gives us the
                // running-time at which the sensor actually captured the frame,
                // placing it in the same clock domain as the audio PTS.
                let pts_ns = if let Some(ts) = frame.sensor_timestamp_ns {
                    // Skip frames until pipeline is PLAYING.
                    if !pipeline_playing {
                        if appsrc.current_running_time().is_none() {
                            continue;
                        }
                        pipeline_playing = true;
                        info!("Pipeline is PLAYING, starting video capture");
                    }
                    let rt = match appsrc.current_running_time() {
                        Some(t) => t.nseconds(),
                        None => continue, // pipeline paused/stopped
                    };
                    let now_boot = Self::read_clock_boottime_ns();
                    // processing_delay = time from sensor capture to now
                    let processing_delay = now_boot.saturating_sub(ts);
                    // running-time at which the sensor captured this frame
                    rt.saturating_sub(processing_delay)
                } else {
                    frame_count * frame_duration_ns
                };

                let data = (*frame.data).to_vec();
                let mut buffer = gst::Buffer::from_mut_slice(data);
                {
                    let buf_ref = buffer.get_mut().unwrap();
                    buf_ref.set_pts(gst::ClockTime::from_nseconds(pts_ns));
                    buf_ref.set_duration(gst::ClockTime::from_nseconds(frame_duration_ns));
                }

                // Push buffer to appsrc
                if appsrc.push_buffer(buffer).is_err() {
                    warn!("Failed to push buffer to appsrc, stopping pusher");
                    break;
                }

                frame_count += 1;
                if frame_count.is_multiple_of(LOG_EVERY_N_FRAMES) {
                    let elapsed = start_time.elapsed().as_secs_f64();
                    debug!(
                        frames = frame_count,
                        elapsed_secs = format!("{:.1}", elapsed),
                        effective_fps = format!("{:.1}", frame_count as f64 / elapsed),
                        "Appsrc pusher progress"
                    );
                }
            }

            // Channel closed — sender was dropped (recording stopped)
            info!(
                total_frames = frame_count,
                "Frame channel closed, sending EOS to appsrc"
            );
            let _ = appsrc.end_of_stream();
        })
    }

    /// Create PipeWire video source element
    fn create_video_source(device_path: &str) -> Result<gst::Element, String> {
        let mut builder = gst::ElementFactory::make("pipewiresrc").property("do-timestamp", true);

        // pipewiresrc target-object expects serial number or node name, not node ID
        // Prefer serial number from device_path
        if device_path.starts_with("pipewire-serial-") {
            if let Some(serial) = device_path.strip_prefix("pipewire-serial-") {
                info!("Using PipeWire target-object serial: {}", serial);
                builder = builder.property("target-object", serial);
            }
        } else if device_path.starts_with("pipewire-")
            && let Some(node) = device_path.strip_prefix("pipewire-")
        {
            info!("Using PipeWire target-object node name: {}", node);
            builder = builder.property("target-object", node);
        }
        // If device_path is empty, PipeWire will use the default camera

        builder
            .build()
            .map_err(|e| format!("Failed to create pipewiresrc: {}", e))
    }

    /// Create preview branch elements
    fn create_preview_branch(
        preview_sender: Option<&tokio::sync::mpsc::Sender<CameraFrame>>,
    ) -> Result<Option<(gst::Element, gst_app::AppSink)>, String> {
        if preview_sender.is_none() {
            return Ok(None);
        }

        let preview_queue = gst::ElementFactory::make("queue")
            .build()
            .map_err(|e| format!("Failed to create preview queue: {}", e))?;

        let appsink = gst::ElementFactory::make("appsink")
            .build()
            .map_err(|e| format!("Failed to create appsink: {}", e))?
            .dynamic_cast::<gst_app::AppSink>()
            .map_err(|_| "Failed to cast to AppSink")?;

        // Configure appsink for RGBA format
        let preview_caps = gst::Caps::builder("video/x-raw")
            .field("format", "RGBA")
            .build();
        appsink.set_caps(Some(&preview_caps));
        appsink.set_property("emit-signals", false);
        appsink.set_property("max-buffers", 2u32);
        appsink.set_property("drop", true);

        Ok(Some((preview_queue, appsink)))
    }

    /// Create audio branch elements
    ///
    /// Uses `pulsesrc` (PipeWire's PulseAudio compatibility layer) instead of
    /// `pipewiresrc` because the latter has unreliable device targeting and data
    /// flow issues. `pulsesrc` reliably captures from all device types including
    /// pro-audio (multi-channel) and standard stereo/mono sources.
    ///
    /// All input channels are mixed down to mono via a capsfilter, using the
    /// hardware input gains as-is (no software volume adjustment).
    fn create_audio_branch(
        audio_device: Option<&str>,
        audio_encoder_config: crate::media::encoders::audio::SelectedAudioEncoder,
    ) -> Result<Option<AudioBranch>, String> {
        let mut source_builder = gst::ElementFactory::make("pulsesrc");

        // pulsesrc `device` property takes the PipeWire/PulseAudio node name
        // (e.g. "alsa_input.usb-Focusrite_Scarlett_4i4_4th_Gen_...-00.pro-input-0")
        if let Some(device) = audio_device {
            if !device.is_empty() {
                info!(device = %device, "Using audio source device");
                source_builder = source_builder.property("device", device);
            }
        } else {
            info!("Using default audio source");
        }

        let source = source_builder
            .build()
            .map_err(|e| format!("Failed to create audio source: {}", e))?;

        let queue = gst::ElementFactory::make("queue")
            .property("max-size-buffers", 200u32)
            .property("max-size-time", 2_000_000_000u64)
            .build()
            .map_err(|e| format!("Failed to create audio queue: {}", e))?;

        let convert = gst::ElementFactory::make("audioconvert")
            .build()
            .map_err(|e| format!("Failed to create audioconvert: {}", e))?;

        let resample = gst::ElementFactory::make("audioresample")
            .build()
            .map_err(|e| format!("Failed to create audioresample: {}", e))?;

        // Level meter BEFORE mono mix — reports per-channel input levels
        let level_input = gst::ElementFactory::make("level")
            .name("audio-level-input")
            .property("post-messages", true)
            .property("interval", 100_000_000u64) // 100ms
            .build()
            .map_err(|e| format!("Failed to create input level meter: {}", e))?;

        // Force mono output — mixes all input channels (stereo, 6ch pro-audio, etc.)
        // down to a single channel using the hardware input levels as-is.
        let capsfilter = gst::ElementFactory::make("capsfilter")
            .property(
                "caps",
                gst::Caps::builder("audio/x-raw")
                    .field("channels", 1i32)
                    .build(),
            )
            .build()
            .map_err(|e| format!("Failed to create audio capsfilter: {}", e))?;

        // Level meter AFTER mono mix — reports mono output level
        let level_output = gst::ElementFactory::make("level")
            .name("audio-level-output")
            .property("post-messages", true)
            .property("interval", 100_000_000u64) // 100ms
            .build()
            .map_err(|e| format!("Failed to create output level meter: {}", e))?;

        let encoder = audio_encoder_config.encoder;

        Ok(Some(AudioBranch {
            source,
            queue,
            convert,
            resample,
            level_input,
            capsfilter,
            level_output,
            encoder,
        }))
    }

    /// Link video chain
    fn link_video_chain(
        source: &gst::Element,
        jpeg_decoder: Option<&gst::Element>,
        videoconvert: &gst::Element,
        videoflip: Option<&gst::Element>,
        videoscale: &gst::Element,
        capsfilter: &gst::Element,
        tee: &gst::Element,
    ) -> Result<(), String> {
        if let Some(decoder) = jpeg_decoder {
            source
                .link(decoder)
                .map_err(|_| "Failed to link source to jpegdec")?;
            decoder
                .link(videoconvert)
                .map_err(|_| "Failed to link jpegdec to videoconvert")?;
        } else {
            source
                .link(videoconvert)
                .map_err(|_| "Failed to link source to videoconvert")?;
        }

        // Link videoconvert -> (optional videoflip) -> videoscale
        if let Some(flip) = videoflip {
            videoconvert
                .link(flip)
                .map_err(|_| "Failed to link videoconvert to videoflip")?;
            flip.link(videoscale)
                .map_err(|_| "Failed to link videoflip to videoscale")?;
        } else {
            videoconvert
                .link(videoscale)
                .map_err(|_| "Failed to link videoconvert to videoscale")?;
        }

        videoscale
            .link(capsfilter)
            .map_err(|_| "Failed to link videoscale to capsfilter")?;
        capsfilter
            .link(tee)
            .map_err(|_| "Failed to link capsfilter to tee")?;

        Ok(())
    }

    /// Link recording branch
    fn link_recording_branch(
        tee: &gst::Element,
        record_queue: &gst::Element,
        encoder: &gst::Element,
        parser: Option<&gst::Element>,
        muxer: &gst::Element,
    ) -> Result<(), String> {
        tee.link(record_queue)
            .map_err(|_| "Failed to link tee to record_queue")?;
        record_queue
            .link(encoder)
            .map_err(|_| "Failed to link record_queue to encoder")?;

        if let Some(parser) = parser {
            encoder
                .link(parser)
                .map_err(|_| "Failed to link encoder to parser")?;
            link_video_to_muxer(parser, muxer)?;
        } else {
            link_video_to_muxer(encoder, muxer)?;
        }

        Ok(())
    }

    /// Link preview branch and spawn frame extraction task
    fn link_preview_branch(
        tee: &gst::Element,
        preview_elements: Option<(gst::Element, gst_app::AppSink)>,
        preview_sender: Option<tokio::sync::mpsc::Sender<CameraFrame>>,
    ) -> Result<Option<tokio::task::JoinHandle<()>>, String> {
        if let Some((preview_queue, appsink)) = preview_elements {
            tee.link(&preview_queue)
                .map_err(|_| "Failed to link tee to preview_queue")?;
            preview_queue
                .link(appsink.upcast_ref::<gst::Element>())
                .map_err(|_| "Failed to link preview_queue to appsink")?;

            if let Some(sender) = preview_sender {
                let task = Self::spawn_preview_task(appsink, sender);
                return Ok(Some(task));
            }
        }

        Ok(None)
    }

    /// Spawn task to extract preview frames from appsink
    fn spawn_preview_task(
        appsink: gst_app::AppSink,
        preview_sender: tokio::sync::mpsc::Sender<CameraFrame>,
    ) -> tokio::task::JoinHandle<()> {
        tokio::spawn(async move {
            info!("Preview frame extraction task started");
            loop {
                match appsink.try_pull_sample(gst::ClockTime::from_mseconds(100)) {
                    Some(sample) => {
                        if let Some(buffer) = sample.buffer()
                            && let Some(caps) = sample.caps()
                            && let Ok(map) = buffer.map_readable()
                        {
                            use gstreamer_video::VideoInfo;

                            if let Ok(video_info) = VideoInfo::from_caps(caps) {
                                let stride = video_info.stride()[0] as u32;

                                let frame = CameraFrame {
                                    data: FrameData::Copied(Arc::from(
                                        map.as_slice().to_vec().into_boxed_slice(),
                                    )),
                                    width: video_info.width(),
                                    height: video_info.height(),
                                    format: crate::backends::camera::types::PixelFormat::RGBA,
                                    stride,
                                    yuv_planes: None,
                                    captured_at: std::time::Instant::now(),
                                    sensor_timestamp_ns: None,
                                    libcamera_metadata: None,
                                };

                                let _ = preview_sender.send(frame).await;
                            }
                        }
                    }
                    None => {
                        tokio::time::sleep(tokio::time::Duration::from_millis(10)).await;
                    }
                }
            }
        })
    }

    /// Link audio chain:
    /// source → queue → convert → resample → level(input) → capsfilter(mono) → level(output) → encoder
    fn link_audio_chain(audio_branch: &AudioBranch) -> Result<(), String> {
        gst::Element::link_many([
            &audio_branch.source,
            &audio_branch.queue,
            &audio_branch.convert,
            &audio_branch.resample,
            &audio_branch.level_input,
            &audio_branch.capsfilter,
            &audio_branch.level_output,
            &audio_branch.encoder,
        ])
        .map_err(|_| "Failed to link audio chain")?;

        Ok(())
    }

    /// Install a bus sync handler that intercepts `level` element messages
    /// in the GStreamer streaming thread and updates [`SharedAudioLevels`].
    ///
    /// Level messages are handled and dropped before they reach the async bus
    /// queue. All other messages (Eos, Error, Warning, etc.) pass through
    /// normally, so `stop()` can use `timed_pop_filtered` without races.
    fn install_level_sync_handler(pipeline: &gst::Pipeline, levels: &SharedAudioLevels) {
        let Some(bus) = pipeline.bus() else { return };
        let levels = Arc::clone(levels);

        bus.set_sync_handler(move |_, msg| {
            let gst::MessageView::Element(e) = msg.view() else {
                return gst::BusSyncReply::Pass;
            };
            let Some(structure) = e.structure() else {
                return gst::BusSyncReply::Pass;
            };
            if structure.name() != "level" {
                return gst::BusSyncReply::Pass;
            }

            let src_name = msg.src().map(|s| s.name().to_string()).unwrap_or_default();

            let peak_db = structure
                .get::<gst::glib::ValueArray>("peak")
                .ok()
                .map(|list| list.iter().filter_map(|v| v.get::<f64>().ok()).collect())
                .unwrap_or_default();
            let rms_db = structure
                .get::<gst::glib::ValueArray>("rms")
                .ok()
                .map(|list| list.iter().filter_map(|v| v.get::<f64>().ok()).collect())
                .unwrap_or_default();

            if let Ok(mut lock) = levels.lock() {
                if src_name == "audio-level-input" {
                    lock.input_peak_db = peak_db;
                    lock.input_rms_db = rms_db;
                } else if src_name == "audio-level-output" {
                    lock.output_peak_db = peak_db.first().copied().unwrap_or(-100.0);
                    lock.output_rms_db = rms_db.first().copied().unwrap_or(-100.0);
                }
            }

            // Drop level messages — don't clutter the bus queue
            gst::BusSyncReply::Drop
        });
    }

    /// Start recording
    pub fn start(&self) -> Result<(), String> {
        info!("Starting video recording pipeline");

        // Log pipeline element names for diagnostics
        let mut element_names = Vec::new();
        for e in self.pipeline.iterate_elements().into_iter().flatten() {
            element_names.push(e.name().to_string());
        }
        info!(elements = ?element_names, "Pipeline elements");

        let result = self
            .pipeline
            .set_state(gst::State::Playing)
            .map_err(|e| format!("Failed to start recording: {}", e))?;
        info!(state_change = ?result, "Pipeline set to Playing");
        Ok(())
    }

    /// Stop recording and finalize the file
    pub fn stop(mut self) -> Result<PathBuf, String> {
        info!("Stopping video recording");

        // Send EOS directly to every source element's src pad.
        // pipeline.send_event(EOS) doesn't reliably reach live sources like pulsesrc,
        // so the muxer (aggregator) never sees EOS on the audio pad and hangs.
        // By pushing EOS on each source's src pad, both appsrc and pulsesrc branches
        // propagate EOS through to the muxer, allowing it to finalize.
        info!("Sending EOS to all source elements");
        let iter = self.pipeline.iterate_sources();
        let mut eos_sent = 0u32;
        for src in iter {
            let Ok(src) = src else { continue };
            let name = src.name().to_string();
            // Use element-level send_event (not pad-level) — for source elements
            // this routes downstream events via gst_pad_push_event on the src pad.
            // pad.send_event() would send upstream, which is wrong for EOS.
            debug!(element = %name, "Sending EOS to source element");
            src.send_event(gst::event::Eos::new());
            eos_sent += 1;
        }
        if eos_sent == 0 {
            // Fallback: send EOS to pipeline
            warn!("No source pads found, sending EOS to pipeline");
            if !self.pipeline.send_event(gst::event::Eos::new()) {
                warn!("Failed to send EOS event to pipeline");
            }
        } else {
            info!(eos_sent, "EOS sent to source elements");
        }

        // Wait for EOS to propagate through the entire pipeline.
        // The bus posts an EOS message only after ALL sink elements have received
        // EOS, which means the muxer has finalized (written moov atom for MP4,
        // duration for WebM, etc.) and the filesink has flushed.
        if let Some(bus) = self.pipeline.bus() {
            info!("Waiting for pipeline EOS on bus...");
            match bus.timed_pop_filtered(
                gst::ClockTime::from_seconds(10),
                &[gst::MessageType::Eos, gst::MessageType::Error],
            ) {
                Some(msg) => match msg.view() {
                    gst::MessageView::Eos(_) => {
                        info!("Pipeline EOS received — file finalized");
                    }
                    gst::MessageView::Error(err) => {
                        error!(
                            error = %err.error(),
                            debug = ?err.debug(),
                            source = ?err.src().map(|s| s.name()),
                            "GStreamer error while waiting for EOS"
                        );
                    }
                    _ => {}
                },
                None => {
                    warn!("Timeout (10s) waiting for pipeline EOS, forcing shutdown");
                }
            }
        } else {
            // Fallback: no bus available, use fixed sleep
            warn!("No pipeline bus available, using fixed sleep fallback");
            std::thread::sleep(std::time::Duration::from_millis(1000));
        }

        // Set pipeline to NULL state - this will trigger final cleanup
        info!("Setting pipeline to NULL state");
        self.pipeline
            .set_state(gst::State::Null)
            .map_err(|e| format!("Failed to stop pipeline: {}", e))?;

        let file_path = std::mem::take(&mut self.file_path);
        info!(path = %file_path.display(), "Recording saved");
        Ok(file_path)
    }
}

impl Drop for VideoRecorder {
    fn drop(&mut self) {
        // Remove the bus sync handler to release its captured references
        if let Some(bus) = self.pipeline.bus() {
            bus.unset_sync_handler();
        }
        // Ensure pipeline is properly stopped — this disconnects pulsesrc from
        // PulseAudio and releases all GStreamer resources.
        let _ = self.pipeline.set_state(gst::State::Null);
    }
}

/// Audio branch elements
struct AudioBranch {
    source: gst::Element,
    queue: gst::Element,
    convert: gst::Element,
    resample: gst::Element,
    /// Level meter before mono mix (per-channel input levels)
    level_input: gst::Element,
    capsfilter: gst::Element,
    /// Level meter after mono mix (mono output level)
    level_output: gst::Element,
    encoder: gst::Element,
}

/// How often to emit periodic progress log messages (every Nth frame).
const LOG_EVERY_N_FRAMES: u64 = 60;

/// Check which video encoders are available (backward compatibility)
pub fn check_available_encoders() {
    crate::media::encoders::log_available_encoders();
}
