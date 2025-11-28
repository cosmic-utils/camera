// SPDX-License-Identifier: MPL-2.0

//! Video recording pipeline with intelligent encoder selection
//!
//! This module implements video recording with:
//! - Automatic hardware encoder detection and selection
//! - Preview continues during recording (tee-based pipeline)
//! - Audio integration
//! - Quality presets

use super::encoder_selection::{EncoderConfig, select_encoders};
use super::muxer::{create_muxer, link_audio_to_muxer, link_muxer_to_sink, link_video_to_muxer};
use crate::backends::camera::types::CameraFrame;
use gstreamer as gst;
use gstreamer::prelude::*;
use gstreamer_app as gst_app;
use std::path::PathBuf;
use tracing::{debug, error, info, warn};

/// Video recorder using the new pipeline architecture
#[derive(Debug)]
pub struct VideoRecorder {
    pipeline: gst::Pipeline,
    file_path: PathBuf,
    #[allow(dead_code)]
    _preview_task: Option<tokio::task::JoinHandle<()>>,
}

impl VideoRecorder {
    /// Create a new video recorder with intelligent encoder selection
    ///
    /// # Arguments
    /// * `device_path` - Camera device path
    /// * `metadata_path` - Optional metadata path for PipeWire
    /// * `width` - Video width
    /// * `height` - Video height
    /// * `framerate` - Video framerate
    /// * `pixel_format` - Pixel format (e.g., "NV12", "MJPEG")
    /// * `output_path` - Output file path
    /// * `config` - Encoder configuration
    /// * `enable_audio` - Whether to record audio
    /// * `audio_device` - Optional audio device path
    /// * `preview_sender` - Optional preview frame sender
    ///
    /// # Returns
    /// * `Ok(VideoRecorder)` - Video recorder instance
    /// * `Err(String)` - Error message
    pub fn new(
        device_path: &str,
        metadata_path: Option<&str>,
        width: u32,
        height: u32,
        framerate: u32,
        pixel_format: &str,
        output_path: PathBuf,
        config: EncoderConfig,
        enable_audio: bool,
        audio_device: Option<&str>,
        preview_sender: Option<tokio::sync::mpsc::Sender<CameraFrame>>,
        encoder_info: Option<&crate::media::encoders::video::EncoderInfo>,
    ) -> Result<Self, String> {
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

        // Initialize GStreamer
        gst::init().map_err(|e| format!("Failed to initialize GStreamer: {}", e))?;

        // Select encoders (use specific encoder if provided, otherwise auto-select)
        let encoders = if let Some(enc_info) = encoder_info {
            super::encoder_selection::select_encoders_with_video(&config, enc_info, enable_audio)?
        } else {
            select_encoders(&config, enable_audio)?
        };

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
        let source = Self::create_video_source(device_path, metadata_path)?;

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

        let videoscale = gst::ElementFactory::make("videoscale")
            .build()
            .map_err(|e| format!("Failed to create videoscale: {}", e))?;

        // OpenH264 has a maximum resolution limit of 9,437,184 pixels (roughly 3072x3072)
        // If resolution exceeds this and we're using openh264, downscale to 1920x1080
        let (final_width, final_height) = {
            let pixels = width * height;
            const OPENH264_MAX_PIXELS: u32 = 9_437_184;

            let is_openh264 = encoders
                .video
                .encoder
                .factory()
                .map(|f| f.name() == "openh264enc")
                .unwrap_or(false);

            if pixels > OPENH264_MAX_PIXELS && is_openh264 {
                // Downscale to 1920x1080 maintaining aspect ratio
                let aspect_ratio = width as f64 / height as f64;
                let target_width = 1920u32;
                let target_height = (target_width as f64 / aspect_ratio) as u32;
                // Make sure height is even (required for most encoders)
                let target_height = target_height & !1;

                warn!(
                    "OpenH264 resolution limit exceeded ({}x{} = {} pixels > {} max), downscaling to {}x{}",
                    width, height, pixels, OPENH264_MAX_PIXELS, target_width, target_height
                );
                (target_width, target_height)
            } else {
                (width, height)
            }
        };

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
            Self::create_audio_branch(audio_device, audio_encoder_config, enable_audio)?
        } else {
            None
        };

        // Add all elements to pipeline
        let mut elements: Vec<&gst::Element> = vec![&source];

        if let Some(ref decoder) = jpeg_decoder {
            elements.push(decoder);
        }

        elements.extend_from_slice(&[
            &videoconvert,
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
            elements.push(&audio_branch.volume);
            elements.push(&audio_branch.limiter);
            elements.push(&audio_branch.convert);
            elements.push(&audio_branch.resample);
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
        }

        Ok(VideoRecorder {
            pipeline,
            file_path: output_path,
            _preview_task: preview_task,
        })
    }

    /// Create PipeWire video source element
    fn create_video_source(
        device_path: &str,
        _metadata_path: Option<&str>,
    ) -> Result<gst::Element, String> {
        let mut builder = gst::ElementFactory::make("pipewiresrc").property("do-timestamp", true);

        // pipewiresrc target-object expects serial number or node name, not node ID
        // Prefer serial number from device_path
        if device_path.starts_with("pipewire-serial-") {
            if let Some(serial) = device_path.strip_prefix("pipewire-serial-") {
                info!("Using PipeWire target-object serial: {}", serial);
                builder = builder.property("target-object", serial);
            }
        } else if device_path.starts_with("pipewire-") {
            if let Some(node) = device_path.strip_prefix("pipewire-") {
                info!("Using PipeWire target-object node name: {}", node);
                builder = builder.property("target-object", node);
            }
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
    fn create_audio_branch(
        audio_device: Option<&str>,
        audio_encoder_config: crate::media::encoders::audio::SelectedAudioEncoder,
        _enable_audio: bool,
    ) -> Result<Option<AudioBranch>, String> {
        // Create audio source (use pipewiresrc for PipeWire audio)
        let mut source_builder = gst::ElementFactory::make("pipewiresrc")
            .property("do-timestamp", true)
            .property("keepalive-time", 1000) // Keep connection alive
            .property("resend-last", false); // Don't resend last buffer on underrun

        // pipewiresrc target-object expects serial number or node name
        // If no device specified, PipeWire will use the default audio source
        if let Some(device) = audio_device {
            // Parse the device identifier (same format as video: "pipewire-serial-{serial}")
            if device.starts_with("pipewire-serial-") {
                if let Some(serial) = device.strip_prefix("pipewire-serial-") {
                    info!("Using PipeWire audio serial: {}", serial);
                    source_builder = source_builder.property("target-object", serial);
                }
            } else if device.starts_with("pipewire-") {
                if let Some(node) = device.strip_prefix("pipewire-") {
                    info!("Using PipeWire audio node name: {}", node);
                    source_builder = source_builder.property("target-object", node);
                }
            } else {
                // Assume it's a node name directly
                info!("Using PipeWire audio node name: {}", device);
                source_builder = source_builder.property("target-object", device);
            }
        } else {
            info!("Using default PipeWire audio source");
        }

        let source = source_builder
            .build()
            .map_err(|e| format!("Failed to create audio source: {}", e))?;

        // Add queue for audio buffering to prevent crackling/underruns
        let queue = gst::ElementFactory::make("queue")
            .property("max-size-buffers", 200u32) // Buffer up to 200 audio buffers
            .property("max-size-time", 2000000000u64) // 2 seconds max
            .build()
            .map_err(|e| format!("Failed to create audio queue: {}", e))?;

        // Add volume element to boost audio signal
        // Note: COSMIC Sound Settings uses 1.5x (150%) max with over-amplification enabled
        // Default is 1.0x (100%) for inputs, which works for normal profiles
        // Pro audio profile bypasses PipeWire's software volume (always 100% hardware)
        let volume = gst::ElementFactory::make("volume")
            .build()
            .map_err(|e| format!("Failed to create volume element: {}", e))?;

        // Apply 1.0x (unity gain) by default to match COSMIC settings behavior
        // Users should adjust input volume in COSMIC Sound Settings if needed
        // (Enable over-amplification there for up to 150% if mic is too quiet)
        let _ = volume.set_property("volume", 1.0f64);
        debug!("Configured audio volume: 1.0x (unity gain, adjust in Sound Settings if needed)");

        // Add audio limiter to prevent clipping and overly loud audio
        // This is especially important when recording from USB microphones or webcams
        // which may output very hot signal levels
        let limiter = gst::ElementFactory::make("rglimiter")
            .build()
            .map_err(|e| format!("Failed to create audio limiter: {}", e))?;
        debug!("Added audio limiter to prevent clipping");

        // Audio convert and resample
        let convert = gst::ElementFactory::make("audioconvert")
            .build()
            .map_err(|e| format!("Failed to create audioconvert: {}", e))?;

        let resample = gst::ElementFactory::make("audioresample")
            .build()
            .map_err(|e| format!("Failed to create audioresample: {}", e))?;

        let encoder = audio_encoder_config.encoder;

        Ok(Some(AudioBranch {
            source,
            queue,
            volume,
            limiter,
            convert,
            resample,
            encoder,
        }))
    }

    /// Link video chain
    fn link_video_chain(
        source: &gst::Element,
        jpeg_decoder: Option<&gst::Element>,
        videoconvert: &gst::Element,
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

        videoconvert
            .link(videoscale)
            .map_err(|_| "Failed to link videoconvert to videoscale")?;
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
                        if let Some(buffer) = sample.buffer() {
                            if let Some(caps) = sample.caps() {
                                if let Ok(map) = buffer.map_readable() {
                                    use gstreamer_video::VideoInfo;

                                    if let Ok(video_info) = VideoInfo::from_caps(caps) {
                                        let stride = video_info.stride()[0] as u32;

                                        let frame = CameraFrame {
                                            data: map.as_slice().to_vec().into(),
                                            width: video_info.width(),
                                            height: video_info.height(),
                                            format:
                                                crate::backends::camera::types::PixelFormat::RGBA,
                                            stride,
                                            captured_at: std::time::Instant::now(),
                                        };

                                        let _ = preview_sender.send(frame).await;
                                    }
                                }
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

    /// Link audio chain
    fn link_audio_chain(audio_branch: &AudioBranch) -> Result<(), String> {
        audio_branch
            .source
            .link(&audio_branch.queue)
            .map_err(|_| "Failed to link audio source to queue")?;
        audio_branch
            .queue
            .link(&audio_branch.volume)
            .map_err(|_| "Failed to link queue to volume")?;
        audio_branch
            .volume
            .link(&audio_branch.limiter)
            .map_err(|_| "Failed to link volume to limiter")?;
        audio_branch
            .limiter
            .link(&audio_branch.convert)
            .map_err(|_| "Failed to link limiter to audioconvert")?;
        audio_branch
            .convert
            .link(&audio_branch.resample)
            .map_err(|_| "Failed to link audioconvert to audioresample")?;
        audio_branch
            .resample
            .link(&audio_branch.encoder)
            .map_err(|_| "Failed to link audioresample to encoder")?;

        Ok(())
    }

    /// Start recording
    pub fn start(&self) -> Result<(), String> {
        info!("Starting video recording");
        self.pipeline
            .set_state(gst::State::Playing)
            .map_err(|e| format!("Failed to start recording: {}", e))?;

        // Check for immediate errors
        let bus = self.pipeline.bus().ok_or("No bus available")?;
        if let Some(msg) = bus.timed_pop_filtered(
            gst::ClockTime::from_mseconds(500),
            &[gst::MessageType::Error, gst::MessageType::Warning],
        ) {
            match msg.view() {
                gst::MessageView::Error(err) => {
                    error!(
                        error = %err.error(),
                        debug = ?err.debug(),
                        source = ?err.src().map(|s| s.name()),
                        "GStreamer error during start"
                    );
                    return Err(format!("Recording start error: {}", err.error()));
                }
                gst::MessageView::Warning(warn) => {
                    error!(
                        warning = %warn.error(),
                        debug = ?warn.debug(),
                        source = ?warn.src().map(|s| s.name()),
                        "GStreamer warning during start"
                    );
                }
                _ => {}
            }
        }

        Ok(())
    }

    /// Stop recording and finalize the file
    pub fn stop(self) -> Result<PathBuf, String> {
        info!("Stopping video recording");

        // Send EOS to trigger graceful shutdown
        info!("Sending EOS to pipeline");
        if !self.pipeline.send_event(gst::event::Eos::new()) {
            warn!("Failed to send EOS event to pipeline");
        }

        // Give pipeline a brief moment to start processing EOS
        // This is crucial for WebM muxer to write duration metadata
        std::thread::sleep(std::time::Duration::from_millis(500));

        // Set pipeline to NULL state - this will trigger final cleanup
        info!("Setting pipeline to NULL state");
        self.pipeline
            .set_state(gst::State::Null)
            .map_err(|e| format!("Failed to stop pipeline: {}", e))?;

        info!(path = %self.file_path.display(), "Recording saved");
        Ok(self.file_path.clone())
    }
}

impl Drop for VideoRecorder {
    fn drop(&mut self) {
        // Ensure pipeline is properly stopped to avoid GStreamer warnings
        let _ = self.pipeline.set_state(gst::State::Null);
    }
}

/// Audio branch elements
struct AudioBranch {
    source: gst::Element,
    queue: gst::Element,
    volume: gst::Element,
    limiter: gst::Element,
    convert: gst::Element,
    resample: gst::Element,
    encoder: gst::Element,
}

/// Check which video encoders are available (backward compatibility)
pub fn check_available_encoders() {
    crate::media::encoders::log_available_encoders();
}
