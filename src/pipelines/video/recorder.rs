// SPDX-License-Identifier: GPL-3.0-only

//! Video recording pipeline with intelligent encoder selection
//!
//! This module implements video recording with:
//! - Automatic hardware encoder detection and selection
//! - Preview continues during recording (tee-based pipeline)
//! - Audio integration
//! - Quality presets

use super::encoder_selection::{EncoderConfig, select_encoders};
use super::muxer::link_audio_to_muxer;
use crate::backends::camera::types::{CameraFrame, PixelFormat, RecordingFrame, SensorRotation};
use crate::media::encoders::video::SelectedVideoEncoder;
use gstreamer as gst;
use gstreamer::prelude::*;
use gstreamer_app as gst_app;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use tracing::{debug, error, info, warn};

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// How often to emit periodic progress log messages (every Nth frame).
const LOG_EVERY_N_FRAMES: u64 = 60;

/// Minimum elapsed seconds before computing effective FPS (avoids division by near-zero).
const MIN_ELAPSED_FOR_FPS: f64 = 0.1;

// ---------------------------------------------------------------------------
// Global recording pipeline diagnostics (read by the insights handler)
// ---------------------------------------------------------------------------

use std::sync::RwLock;
use std::sync::atomic::{AtomicU64, Ordering};

/// Snapshot of the active recording pipeline for the insights drawer.
#[derive(Debug, Clone, Default)]
pub struct RecordingDiagnostics {
    /// Human-readable recording mode (e.g. "VA-API JPEG zero-copy", "NV12 pusher", "Legacy")
    pub mode: String,
    /// GStreamer pipeline description string
    pub pipeline_string: String,
    /// Video encoder element name (e.g. "vah265enc", "openh264enc")
    pub encoder: String,
    /// Recording resolution
    pub resolution: String,
    /// Recording framerate
    pub framerate: u32,
}

/// Live per-step counters for the recording pipeline.
///
/// Updated atomically on the hot path (every frame) by the capture thread
/// and the appsrc pusher task.  Read by the insights handler every tick.
pub struct RecordingPipelineStats {
    /// Frames successfully sent from capture thread → channel
    pub capture_sent: AtomicU64,
    /// Frames dropped at capture thread (channel full)
    pub capture_dropped: AtomicU64,
    /// Frames pushed into GStreamer appsrc
    pub pusher_pushed: AtomicU64,
    /// Frames skipped by pusher (pre-PLAYING or wrong variant)
    pub pusher_skipped: AtomicU64,
    /// Most recent PTS assigned (nanoseconds)
    pub last_pts_ns: AtomicU64,
    /// Most recent processing delay (CLOCK_BOOTTIME - sensor_ts) in microseconds
    pub last_processing_delay_us: AtomicU64,
    /// Pusher start time (nanos since UNIX epoch, 0 = not started)
    pub pusher_start_epoch_ns: AtomicU64,
    /// NV12 conversion time for the most recent frame (microseconds, 0 = N/A)
    pub last_convert_time_us: AtomicU64,
}

/// Snapshot of live recording stats (read by the UI).
#[derive(Debug, Clone, Default)]
pub struct RecordingStatsSnapshot {
    pub capture_sent: u64,
    pub capture_dropped: u64,
    pub pusher_pushed: u64,
    pub pusher_skipped: u64,
    pub last_pts_ms: u64,
    pub last_processing_delay_us: u64,
    pub effective_fps: f64,
    pub last_convert_time_us: u64,
    /// Approximate channel occupancy (sent - dropped - pushed - skipped)
    pub channel_backlog: u64,
}

static RECORDING_DIAGNOSTICS: RwLock<Option<RecordingDiagnostics>> = RwLock::new(None);

static RECORDING_STATS: RecordingPipelineStats = RecordingPipelineStats {
    capture_sent: AtomicU64::new(0),
    capture_dropped: AtomicU64::new(0),
    pusher_pushed: AtomicU64::new(0),
    pusher_skipped: AtomicU64::new(0),
    last_pts_ns: AtomicU64::new(0),
    last_processing_delay_us: AtomicU64::new(0),
    pusher_start_epoch_ns: AtomicU64::new(0),
    last_convert_time_us: AtomicU64::new(0),
};

/// Publish recording pipeline diagnostics (called when recorder is created).
fn publish_recording_diagnostics(diag: RecordingDiagnostics) {
    if let Ok(mut d) = RECORDING_DIAGNOSTICS.write() {
        *d = Some(diag);
    }
}

/// Clear recording pipeline diagnostics and reset stats (called when recording stops).
pub fn clear_recording_diagnostics() {
    if let Ok(mut d) = RECORDING_DIAGNOSTICS.write() {
        *d = None;
    }
    reset_recording_stats();
}

/// Reset all live counters to zero.
fn reset_recording_stats() {
    RECORDING_STATS.capture_sent.store(0, Ordering::Relaxed);
    RECORDING_STATS.capture_dropped.store(0, Ordering::Relaxed);
    RECORDING_STATS.pusher_pushed.store(0, Ordering::Relaxed);
    RECORDING_STATS.pusher_skipped.store(0, Ordering::Relaxed);
    RECORDING_STATS.last_pts_ns.store(0, Ordering::Relaxed);
    RECORDING_STATS
        .last_processing_delay_us
        .store(0, Ordering::Relaxed);
    RECORDING_STATS
        .pusher_start_epoch_ns
        .store(0, Ordering::Relaxed);
    RECORDING_STATS
        .last_convert_time_us
        .store(0, Ordering::Relaxed);
}

/// Increment the capture-sent counter (called from capture thread).
pub fn rec_stats_capture_sent() {
    RECORDING_STATS.capture_sent.fetch_add(1, Ordering::Relaxed);
}

/// Increment the capture-dropped counter (called from capture thread).
pub fn rec_stats_capture_dropped() {
    RECORDING_STATS
        .capture_dropped
        .fetch_add(1, Ordering::Relaxed);
}

/// Read the current recording pipeline diagnostics (called by insights handler).
pub fn get_recording_diagnostics() -> Option<RecordingDiagnostics> {
    RECORDING_DIAGNOSTICS.read().ok()?.clone()
}

/// Read a snapshot of the live recording stats (called by insights handler).
pub fn get_recording_stats() -> RecordingStatsSnapshot {
    let sent = RECORDING_STATS.capture_sent.load(Ordering::Relaxed);
    let dropped = RECORDING_STATS.capture_dropped.load(Ordering::Relaxed);
    let pushed = RECORDING_STATS.pusher_pushed.load(Ordering::Relaxed);
    let skipped = RECORDING_STATS.pusher_skipped.load(Ordering::Relaxed);
    let start_ns = RECORDING_STATS
        .pusher_start_epoch_ns
        .load(Ordering::Relaxed);

    let effective_fps = if pushed > 0 && start_ns > 0 {
        let now_ns = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos() as u64)
            .unwrap_or(0);
        let elapsed_s = (now_ns.saturating_sub(start_ns)) as f64 / 1_000_000_000.0;
        if elapsed_s > MIN_ELAPSED_FOR_FPS {
            pushed as f64 / elapsed_s
        } else {
            0.0
        }
    } else {
        0.0
    };

    let backlog = sent
        .saturating_sub(dropped)
        .saturating_sub(pushed)
        .saturating_sub(skipped);

    RecordingStatsSnapshot {
        capture_sent: sent,
        capture_dropped: dropped,
        pusher_pushed: pushed,
        pusher_skipped: skipped,
        last_pts_ms: RECORDING_STATS.last_pts_ns.load(Ordering::Relaxed) / 1_000_000,
        last_processing_delay_us: RECORDING_STATS
            .last_processing_delay_us
            .load(Ordering::Relaxed),
        effective_fps,
        last_convert_time_us: RECORDING_STATS.last_convert_time_us.load(Ordering::Relaxed),
        channel_backlog: backlog,
    }
}

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

/// Common recording configuration.
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
    /// Live filter code (read each frame via AtomicU32, updated by UI thread).
    /// Value is `FilterType::gpu_filter_code()`. 0 = Standard (no filter).
    pub live_filter_code: Arc<std::sync::atomic::AtomicU32>,
}

/// Video recorder using the new pipeline architecture
#[derive(Debug)]
pub struct VideoRecorder {
    pipeline: gst::Pipeline,
    file_path: PathBuf,
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

/// Shared state from the common recorder preparation phase.
///
/// Both `new_from_appsrc` and `new_from_appsrc_jpeg` begin with the same
/// sequence: encoder selection, V4L2 fallback, audio branch creation, and
/// output path resolution. This struct captures the results so each
/// constructor only handles its format-specific pipeline description and
/// pusher spawn.
struct RecorderSetup {
    audio_elements: Option<AudioBranch>,
    encoder_name: String,
    parser_str: String,
    muxer_name: String,
    output_path: PathBuf,
    frame_duration_ns: i64,
}

/// Common setup for both appsrc recorder constructors.
///
/// Handles encoder selection, V4L2 fallback, audio branch creation,
/// and output path resolution. Format-specific encoder overrides
/// (e.g. NVIDIA domain matching) should be applied to the returned
/// [`RecorderSetup`] before building the pipeline description.
fn prepare_recorder(
    encoder_info: Option<&crate::media::encoders::video::EncoderInfo>,
    encoder_config: &EncoderConfig,
    enable_audio: bool,
    audio_device: Option<&str>,
    output_path: PathBuf,
    framerate: u32,
) -> Result<RecorderSetup, String> {
    let encoders = select_encoder_set(encoder_info, encoder_config, enable_audio)?;

    let audio_elements = if let Some(audio_encoder_config) = encoders.audio {
        VideoRecorder::create_audio_branch(audio_device, audio_encoder_config)?
    } else {
        None
    };

    info!(
        video_codec = ?encoders.video.codec,
        audio = audio_elements.is_some(),
        container = ?encoders.video.container,
        "Selected encoders"
    );

    let output_path = output_path.with_extension(encoders.video.extension);
    let frame_duration_ns = 1_000_000_000i64 / framerate as i64;

    let selected_encoder = encoders
        .video
        .encoder
        .factory()
        .map(|f| f.name().to_string())
        .unwrap_or_else(|| "openh264enc".to_string());

    let (encoder_name, parser_str, muxer_name) = if selected_encoder.starts_with("v4l2") {
        warn!(
            selected = %selected_encoder,
            "V4L2 encoder not compatible with appsrc pipeline, falling back to openh264enc"
        );
        (
            "openh264enc".to_string(),
            "! h264parse".to_string(),
            "mp4mux".to_string(),
        )
    } else {
        let (parser, muxer) = parser_and_muxer_names(&encoders.video);
        // Probe hardware encoders to catch cases where the element exists in
        // the registry but can't actually encode (e.g. VA-API backed by NVENC
        // in a flatpak sandbox that lacks libnvidia-encode.so).
        let is_software = selected_encoder == "openh264enc"
            || selected_encoder == "x264enc"
            || selected_encoder == "x265enc";
        if !is_software
            && !crate::media::encoders::detection::probe_single_encoder(&selected_encoder)
        {
            warn!(
                selected = %selected_encoder,
                "Hardware encoder probe failed, falling back to openh264enc"
            );
            (
                "openh264enc".to_string(),
                "! h264parse".to_string(),
                "mp4mux".to_string(),
            )
        } else {
            (selected_encoder, parser, muxer)
        }
    };

    Ok(RecorderSetup {
        audio_elements,
        encoder_name,
        parser_str,
        muxer_name,
        output_path,
        frame_duration_ns,
    })
}

/// Extract parser name (with `! ` prefix) and muxer name from a selected video encoder.
fn parser_and_muxer_names(video: &SelectedVideoEncoder) -> (String, String) {
    let parser = video
        .parser
        .as_ref()
        .and_then(|p| p.factory().map(|f| format!("! {}", f.name())))
        .unwrap_or_default();
    let muxer = video
        .muxer
        .factory()
        .map(|f| f.name().to_string())
        .unwrap_or_else(|| "mp4mux".to_string());
    (parser, muxer)
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

/// Add audio branch elements to the pipeline, link the chain, connect to
/// the muxer, and install the level sync handler.
fn add_audio_branch_to_pipeline(
    pipeline: &gst::Pipeline,
    audio_branch: &AudioBranch,
    audio_levels: &SharedAudioLevels,
) -> Result<(), String> {
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

    VideoRecorder::link_audio_chain(audio_branch)?;

    let muxer = pipeline
        .by_name("recording-muxer")
        .ok_or("Failed to find recording-muxer for audio linking")?;
    link_audio_to_muxer(&audio_branch.encoder, &muxer)?;

    VideoRecorder::install_level_sync_handler(pipeline, audio_levels);

    Ok(())
}

/// Parse a GStreamer pipeline description and perform common configuration.
///
/// Returns the pipeline and appsrc element after:
/// - Parsing the pipeline description
/// - Extracting the `camera-appsrc` element
/// - Configuring the video encoder (bitrate / quality)
/// - Adding the audio branch (if present)
/// - Installing muxer fixup probes
fn build_recorder_pipeline(
    pipeline_desc: &str,
    encoder_name: &str,
    encoder_config: &EncoderConfig,
    encode_width: u32,
    encode_height: u32,
    audio_elements: Option<&AudioBranch>,
    audio_levels: &SharedAudioLevels,
) -> Result<(gst::Pipeline, gst_app::AppSrc), String> {
    let pipeline = gst::parse::launch(pipeline_desc)
        .map_err(|e| format!("Failed to parse pipeline: {}", e))?
        .dynamic_cast::<gst::Pipeline>()
        .map_err(|_| "Failed to cast to Pipeline")?;

    let appsrc = pipeline
        .by_name("camera-appsrc")
        .ok_or("Failed to find camera-appsrc in pipeline")?
        .dynamic_cast::<gst_app::AppSrc>()
        .map_err(|_| "Failed to cast to AppSrc")?;

    if let Some(enc_element) = pipeline.by_name("recording-encoder") {
        crate::media::encoders::video::configure_video_encoder(
            &enc_element,
            encoder_name,
            encoder_config.video_quality,
            encode_width,
            encode_height,
            encoder_config.bitrate_override_kbps,
        );
    }

    if let Some(audio_branch) = audio_elements {
        add_audio_branch_to_pipeline(&pipeline, audio_branch, audio_levels)?;
        info!("Audio branch added to recording pipeline");
    }

    install_muxer_fixup_probes(&pipeline);

    Ok((pipeline, appsrc))
}

/// Result of PTS computation for a single frame.
enum PtsResult {
    /// Computed PTS in nanoseconds — push this buffer.
    Pts(u64),
    /// Frame should be skipped (pipeline not yet PLAYING).
    Skip,
}

/// Compute PTS for a recording frame using sensor timestamps and pipeline
/// running-time for A/V sync. Falls back to frame-count-based PTS if
/// sensor timestamps are unavailable.
fn compute_pts(
    appsrc: &gst_app::AppSrc,
    sensor_ts: Option<u64>,
    frame_count: u64,
    frame_duration_ns: u64,
    pipeline_playing: &mut bool,
    ts_offset: &mut Option<(u64, u64)>,
) -> PtsResult {
    let Some(ts) = sensor_ts else {
        return PtsResult::Pts(frame_count * frame_duration_ns);
    };

    // Skip frames until pipeline is PLAYING.
    if !*pipeline_playing {
        if appsrc.current_running_time().is_none() {
            RECORDING_STATS
                .pusher_skipped
                .fetch_add(1, Ordering::Relaxed);
            return PtsResult::Skip;
        }
        *pipeline_playing = true;
        info!("Pipeline is PLAYING, starting video capture");
    }
    let rt = match appsrc.current_running_time() {
        Some(t) => t.nseconds(),
        None => {
            RECORDING_STATS
                .pusher_skipped
                .fetch_add(1, Ordering::Relaxed);
            return PtsResult::Skip;
        }
    };

    // Record processing delay for diagnostics
    let now_boot = read_clock_boottime_ns();
    let processing_delay = now_boot.saturating_sub(ts);
    RECORDING_STATS
        .last_processing_delay_us
        .store(processing_delay / 1_000, Ordering::Relaxed);

    // On first frame, establish PTS base accounting for processing
    // delay so video timestamps reflect actual capture time.
    let is_first = ts_offset.is_none();
    let (first_ts, pts_base) = *ts_offset.get_or_insert((ts, rt.saturating_sub(processing_delay)));
    let pts = pts_base + ts.saturating_sub(first_ts);
    if is_first {
        warn!(
            running_time_ms = rt / 1_000_000,
            processing_delay_ms = processing_delay / 1_000_000,
            pts_base_ms = pts_base / 1_000_000,
            pts_ms = pts / 1_000_000,
            sensor_ts_ms = ts / 1_000_000,
            "First video frame A/V sync: pts_base = running_time - processing_delay"
        );
    }
    PtsResult::Pts(pts)
}

/// Install a read-only PTS/DTS trace probe on a named element's src pad.
///
/// Logs timestamps at `debug!()` level for the first 5 frames and every
/// [`LOG_EVERY_N_FRAMES`] frames thereafter.
fn install_pts_trace_probe(element: &gst::Element, stage: &'static str) {
    let Some(src_pad) = element.static_pad("src") else {
        return;
    };
    let frame_count = std::sync::Arc::new(AtomicU64::new(0));
    let fc = frame_count.clone();
    src_pad.add_probe(gst::PadProbeType::BUFFER, move |_pad, info| {
        let n = fc.fetch_add(1, Ordering::Relaxed);
        if (n < 5 || n.is_multiple_of(LOG_EVERY_N_FRAMES))
            && let Some(buffer) = info.buffer()
        {
            debug!(
                frame = n,
                pts_ms = buffer.pts().map(|p| p.mseconds()),
                dts_ms = buffer.dts().map(|d| d.mseconds()),
                stage,
                "PTS trace"
            );
        }
        gst::PadProbeReturn::Ok
    });
}

/// Install muxer sink pad probes that fix PTS=NONE (copies DTS → PTS).
///
/// NVENC encoders (nvh265enc/nvh264enc) add a 3 600 000 s offset to PTS/DTS
/// **and** to the segment event.  The aggregator-based mp4mux in GStreamer 1.28
/// converts PTS to running-time via `PTS − segment.start`, so the offset
/// cancels out.  Stripping the offset from buffers without also adjusting the
/// segment causes the muxer to clip every video buffer as "outside segment",
/// resulting in 0 video samples in the output file.
fn install_muxer_fixup_probes(pipeline: &gst::Pipeline) {
    let Some(muxer) = pipeline.by_name("recording-muxer") else {
        return;
    };
    for pad in muxer.sink_pads() {
        let pad_name = pad.name().to_string();
        let mux_probe_count = std::sync::Arc::new(AtomicU64::new(0));
        let mpc = mux_probe_count.clone();
        pad.add_probe(gst::PadProbeType::BUFFER, move |_pad, info| {
            if let Some(buffer) = info.buffer_mut() {
                let buf = buffer.make_mut();
                // Fix PTS=NONE (some encoders set only DTS)
                if buf.pts().is_none()
                    && let Some(dts) = buf.dts()
                {
                    buf.set_pts(dts);
                }
            }
            let n = mpc.fetch_add(1, Ordering::Relaxed);
            if n < 3
                && let Some(buffer) = info.buffer()
            {
                warn!(
                    pad = pad_name.as_str(),
                    frame = n,
                    pts_ms = buffer.pts().map(|p| p.mseconds()),
                    dts_ms = buffer.dts().map(|d| d.mseconds()),
                    "Muxer sink pad buffer"
                );
            }
            gst::PadProbeReturn::Ok
        });
    }
}

/// Convert a camera frame to tightly-packed RGBA using the GPU compute shader.
///
/// For frames already in RGBA format, strips stride padding.
/// For YUV and other formats, uses the GPU compute pipeline.
async fn convert_frame_to_rgba(frame: &CameraFrame) -> Result<Vec<u8>, String> {
    if frame.format == PixelFormat::RGBA {
        let row_bytes = (frame.width * 4) as usize;
        let stride = frame.stride as usize;
        if stride <= row_bytes {
            return Ok(frame.data.to_vec());
        }
        let mut out = Vec::with_capacity(row_bytes * frame.height as usize);
        for y in 0..frame.height as usize {
            out.extend_from_slice(&frame.data[y * stride..y * stride + row_bytes]);
        }
        return Ok(out);
    }

    let input = crate::shaders::GpuFrameInput::from_camera_frame(frame)?;

    let mut pipeline_guard = crate::shaders::get_gpu_convert_pipeline()
        .await
        .map_err(|e| format!("Failed to get GPU convert pipeline: {}", e))?;

    let pipeline = pipeline_guard
        .as_mut()
        .ok_or("GPU convert pipeline not initialized")?;

    pipeline
        .convert(&input)
        .map_err(|e| format!("GPU conversion failed: {}", e))?;

    pipeline
        .read_rgba_to_cpu(frame.width, frame.height)
        .await
        .map_err(|e| format!("Failed to read RGBA from GPU: {}", e))
}

/// Frame data prepared by a format-specific closure for the common pusher loop.
struct PusherFrame {
    buffer: gst::Buffer,
    sensor_ts: Option<u64>,
    sequence: Option<u32>,
}

/// Spawn a tokio task that reads `RecordingFrame`s from a channel, prepares
/// them via `prepare_frame`, and pushes them into the GStreamer `appsrc`.
///
/// The `prepare_frame` closure extracts format-specific data from each
/// `RecordingFrame` and creates a `gst::Buffer`. Return `None` to skip a
/// frame (e.g. wrong variant). The common loop handles PTS computation,
/// buffer timestamping, stats updates, periodic logging, and EOS teardown.
fn spawn_pusher<F>(
    appsrc: gst_app::AppSrc,
    mut frame_rx: tokio::sync::mpsc::Receiver<RecordingFrame>,
    framerate: u32,
    label: &'static str,
    mut prepare_frame: F,
) -> tokio::task::JoinHandle<()>
where
    F: FnMut(RecordingFrame, &gst_app::AppSrc) -> Option<PusherFrame> + Send + 'static,
{
    tokio::spawn(async move {
        info!(label, "Appsrc pusher task started");

        let start_epoch_ns = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos() as u64)
            .unwrap_or(0);
        RECORDING_STATS
            .pusher_start_epoch_ns
            .store(start_epoch_ns, Ordering::Relaxed);

        let mut frame_count: u64 = 0;
        let start_time = std::time::Instant::now();
        let frame_duration_ns = 1_000_000_000u64 / framerate as u64;
        let mut pipeline_playing = false;
        let mut ts_offset: Option<(u64, u64)> = None;

        while let Some(rec_frame) = frame_rx.recv().await {
            let Some(PusherFrame {
                mut buffer,
                sensor_ts,
                sequence,
            }) = prepare_frame(rec_frame, &appsrc)
            else {
                continue;
            };

            let pts_ns = match compute_pts(
                &appsrc,
                sensor_ts,
                frame_count,
                frame_duration_ns,
                &mut pipeline_playing,
                &mut ts_offset,
            ) {
                PtsResult::Pts(pts) => pts,
                PtsResult::Skip => continue,
            };

            {
                let buf_ref = buffer.get_mut().unwrap();
                buf_ref.set_pts(gst::ClockTime::from_nseconds(pts_ns));
                buf_ref.set_duration(gst::ClockTime::from_nseconds(frame_duration_ns));
            }

            RECORDING_STATS.last_pts_ns.store(pts_ns, Ordering::Relaxed);

            if appsrc.push_buffer(buffer).is_err() {
                warn!(label, "Failed to push buffer to appsrc, stopping pusher");
                break;
            }

            RECORDING_STATS
                .pusher_pushed
                .fetch_add(1, Ordering::Relaxed);
            frame_count += 1;
            if frame_count.is_multiple_of(LOG_EVERY_N_FRAMES) {
                let elapsed = start_time.elapsed().as_secs_f64();
                warn!(
                    label,
                    frames = frame_count,
                    seq = ?sequence,
                    sensor_ts_ms = ?sensor_ts.map(|t| t / 1_000_000),
                    pts_ms = pts_ns / 1_000_000,
                    elapsed_secs = format!("{:.1}", elapsed),
                    effective_fps = format!("{:.1}", frame_count as f64 / elapsed),
                    "Pusher progress"
                );
            }
        }

        info!(
            label,
            total_frames = frame_count,
            "Frame channel closed, sending EOS to appsrc"
        );
        let _ = appsrc.end_of_stream();
    })
}

impl VideoRecorder {
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
        frame_rx: tokio::sync::mpsc::Receiver<RecordingFrame>,
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
            live_filter_code,
        } = config;

        // Always use the filtered (RGBA) pipeline so the user can toggle
        // filters mid-recording and have them apply to the output file.
        let initial_filter_code = live_filter_code.load(std::sync::atomic::Ordering::Relaxed);

        info!(
            width,
            height,
            framerate,
            format = ?pixel_format,
            initial_filter = initial_filter_code,
            output = %output_path.display(),
            audio = enable_audio,
            audio_device = ?audio_device,
            rotation = %rotation,
            "Creating appsrc-based video recorder (libcamera backend)"
        );

        let setup = prepare_recorder(
            encoder_info,
            &encoder_config,
            enable_audio,
            audio_device,
            output_path,
            framerate,
        )?;

        let (base_width, base_height) = if rotation.swaps_dimensions() {
            (height, width)
        } else {
            (width, height)
        };

        // Inverse rotation: sensor mounting angle → correction direction
        let flip_str = rotation_to_flip_direction(rotation)
            .map(|dir| format!("! videoflip video-direction={dir}"))
            .unwrap_or_default();

        // OpenH264 has a maximum resolution limit — downscale if exceeded
        let (final_width, final_height) =
            openh264_downscale(base_width, base_height, &setup.encoder_name);

        // Only insert videoconvert/videoscale/capsfilter when actually needed.
        // Skipping these for the common case (no rotation, no scaling, I420 input)
        // eliminates ~3 software passthrough elements at 12MP+ resolutions.
        let needs_rotation = !flip_str.is_empty();
        let needs_scaling = final_width != base_width || final_height != base_height;

        // Always use RGBA input: the filtered pusher converts each frame to RGBA
        // (via GPU compute shader), applies the current filter, and pushes RGBA.
        // This lets the user toggle filters mid-recording.
        let initial_gst_format = "RGBA";

        let processing_chain = if needs_rotation || needs_scaling {
            format!(
                "! videoconvert {flip} ! videoscale \
                 ! capsfilter caps=video/x-raw,format=I420,width={fw},height={fh},framerate={fps}/1 \
                 ! videoconvert",
                flip = flip_str,
                fw = final_width,
                fh = final_height,
                fps = framerate,
            )
        } else {
            "! videoconvert".to_string()
        };

        let pipeline_desc = format!(
            "appsrc name=camera-appsrc \
               caps=video/x-raw,format={fmt},width={w},height={h},framerate={fps}/1 \
               is-live=true do-timestamp=false format=time \
               min-latency={lat} max-latency={lat} \
             ! queue max-size-buffers=5 max-size-time=1000000000 \
             {processing} \
             ! {encoder} name=recording-encoder \
             {parser} \
             ! {muxer} name=recording-muxer \
             ! filesink location={loc}",
            fmt = initial_gst_format,
            w = width,
            h = height,
            fps = framerate,
            lat = setup.frame_duration_ns,
            processing = processing_chain,
            encoder = setup.encoder_name,
            parser = setup.parser_str,
            muxer = setup.muxer_name,
            loc = setup.output_path.display(),
        );

        info!(desc = %pipeline_desc, "Launching appsrc pipeline");

        let (pipeline, appsrc) = build_recorder_pipeline(
            &pipeline_desc,
            &setup.encoder_name,
            &encoder_config,
            final_width,
            final_height,
            setup.audio_elements.as_ref(),
            &audio_levels,
        )?;

        info!(
            initial_filter = initial_filter_code,
            "Pusher will apply live GPU filter (RGBA output)"
        );
        drop(Self::spawn_filtered_pusher(
            appsrc,
            frame_rx,
            framerate,
            live_filter_code,
        ));

        // Publish diagnostics for the insights drawer
        let mode = if needs_rotation || needs_scaling {
            "Filtered RGBA (videoconvert + rotation/scale)"
        } else {
            "Filtered RGBA (videoconvert)"
        };
        publish_recording_diagnostics(RecordingDiagnostics {
            mode: mode.to_string(),
            pipeline_string: pipeline_desc.clone(),
            encoder: setup.encoder_name.clone(),
            resolution: format!("{}x{}", final_width, final_height),
            framerate,
        });

        let recorder = VideoRecorder {
            pipeline,
            file_path: setup.output_path,
        };

        // Eagerly start: if a hardware encoder fails (e.g. VA-API backed by
        // NVENC in a flatpak sandbox), return Err so the caller can retry.
        recorder.start()?;

        Ok(recorder)
    }

    /// Spawn the legacy (decoded-frame) pusher task.
    ///
    /// Spawn a pusher task that converts frames to RGBA and applies the live
    /// GPU filter before pushing to appsrc.
    ///
    /// Reads the current filter code from `live_filter_code` each frame so
    /// filter changes during recording are reflected in the output file.
    /// When filter code is 0 (Standard), the RGBA data is pushed without
    /// running the filter shader.
    fn spawn_filtered_pusher(
        appsrc: gst_app::AppSrc,
        mut frame_rx: tokio::sync::mpsc::Receiver<RecordingFrame>,
        framerate: u32,
        live_filter_code: Arc<std::sync::atomic::AtomicU32>,
    ) -> tokio::task::JoinHandle<()> {
        tokio::spawn(async move {
            let initial = live_filter_code.load(std::sync::atomic::Ordering::Relaxed);
            info!(
                initial_filter_code = initial,
                "Filtered appsrc pusher task started"
            );

            let start_epoch_ns = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos() as u64)
                .unwrap_or(0);
            RECORDING_STATS
                .pusher_start_epoch_ns
                .store(start_epoch_ns, Ordering::Relaxed);

            let mut frame_count: u64 = 0;
            let start_time = std::time::Instant::now();
            let frame_duration_ns = 1_000_000_000u64 / framerate as u64;
            let mut pipeline_playing = false;
            let mut ts_offset: Option<(u64, u64)> = None;

            while let Some(rec_frame) = frame_rx.recv().await {
                let frame = match rec_frame {
                    RecordingFrame::Decoded(f) => f,
                    RecordingFrame::Jpeg { .. } => continue,
                };

                let sensor_ts = frame.sensor_timestamp_ns;
                let sequence = frame.libcamera_metadata.as_ref().and_then(|m| m.sequence);

                // Convert to RGBA via GPU compute shader
                let t0 = std::time::Instant::now();
                let rgba = match convert_frame_to_rgba(&frame).await {
                    Ok(data) => data,
                    Err(e) => {
                        warn!(error = %e, "Failed to convert frame to RGBA, skipping");
                        continue;
                    }
                };

                // Read current filter from shared atomic (UI thread updates this)
                let filter_code = live_filter_code.load(std::sync::atomic::Ordering::Relaxed);
                let filter_type = crate::app::FilterType::from_gpu_filter_code(filter_code);

                // Apply GPU filter (skip for Standard — just use the RGBA as-is)
                let filtered = if filter_type == crate::app::FilterType::Standard {
                    rgba
                } else {
                    match crate::shaders::apply_filter_gpu_rgba(
                        &rgba,
                        frame.width,
                        frame.height,
                        filter_type,
                    )
                    .await
                    {
                        Ok(data) => data,
                        Err(e) => {
                            warn!(error = %e, "Failed to apply filter, using unfiltered RGBA");
                            rgba
                        }
                    }
                };

                RECORDING_STATS
                    .last_convert_time_us
                    .store(t0.elapsed().as_micros() as u64, Ordering::Relaxed);

                let pts_ns = match compute_pts(
                    &appsrc,
                    sensor_ts,
                    frame_count,
                    frame_duration_ns,
                    &mut pipeline_playing,
                    &mut ts_offset,
                ) {
                    PtsResult::Pts(pts) => pts,
                    PtsResult::Skip => continue,
                };

                let mut buffer = gst::Buffer::from_mut_slice(filtered);
                {
                    let buf_ref = buffer.get_mut().unwrap();
                    buf_ref.set_pts(gst::ClockTime::from_nseconds(pts_ns));
                    buf_ref.set_duration(gst::ClockTime::from_nseconds(frame_duration_ns));
                }

                RECORDING_STATS.last_pts_ns.store(pts_ns, Ordering::Relaxed);

                if appsrc.push_buffer(buffer).is_err() {
                    warn!("Filtered pusher: failed to push buffer, stopping");
                    break;
                }

                RECORDING_STATS
                    .pusher_pushed
                    .fetch_add(1, Ordering::Relaxed);
                frame_count += 1;
                if frame_count.is_multiple_of(LOG_EVERY_N_FRAMES) {
                    let elapsed = start_time.elapsed().as_secs_f64();
                    warn!(
                        frames = frame_count,
                        seq = ?sequence,
                        sensor_ts_ms = ?sensor_ts.map(|t| t / 1_000_000),
                        pts_ms = pts_ns / 1_000_000,
                        elapsed_secs = format!("{:.1}", elapsed),
                        effective_fps = format!("{:.1}", frame_count as f64 / elapsed),
                        filter_time_us = t0.elapsed().as_micros(),
                        "Filtered pusher progress"
                    );
                }
            }

            info!(
                total_frames = frame_count,
                "Frame channel closed, sending EOS to filtered appsrc"
            );
            let _ = appsrc.end_of_stream();
        })
    }

    /// Create a VA-API JPEG zero-copy recording pipeline.
    ///
    /// Instead of CPU-decoding MJPEG and converting to NV12, this pipeline sends
    /// raw JPEG bytes through `appsrc` → `vajpegdec` (GPU decode) → `vah265enc`.
    /// The GPU decoder outputs NV12 in VA-API memory that the encoder consumes
    /// zero-copy, eliminating both the turbojpeg CPU decode and the I420→NV12
    /// software conversion from the recording path.
    ///
    /// Falls back to `None` if the pipeline cannot be constructed (caller should
    /// retry with the legacy `new_from_appsrc` path).
    pub fn new_from_appsrc_jpeg(
        config: AppsrcRecorderConfig<'_>,
        va_jpeg_dec: &str,
        frame_rx: tokio::sync::mpsc::Receiver<RecordingFrame>,
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
                    rotation: _,
                    audio_levels,
                },
            pixel_format: _,
            live_filter_code,
        } = config;

        if live_filter_code.load(std::sync::atomic::Ordering::Relaxed) != 0 {
            return Err(
                "VA-API JPEG pipeline does not support filters; falling back to legacy".to_string(),
            );
        }

        info!(
            width,
            height,
            framerate,
            va_jpeg_dec,
            output = %output_path.display(),
            audio = enable_audio,
            "Creating VA-API JPEG zero-copy recording pipeline"
        );

        let mut setup = prepare_recorder(
            encoder_info,
            &encoder_config,
            enable_audio,
            audio_device,
            output_path,
            framerate,
        )?;

        // Match encoder to decoder memory domain to avoid implicit GPU memory
        // transfers that cause frame stalls:
        //   nvjpegdec (CUDA memory)   → nvh265enc/nvh264enc (CUDA memory)
        //   vajpegdec (VA-API memory) → vah265enc/vah264enc (VA-API memory)
        let is_nvidia_decoder = va_jpeg_dec.starts_with("nv");
        if is_nvidia_decoder && !setup.encoder_name.starts_with("nv") {
            use crate::media::encoders::detection::probe_single_encoder;
            if probe_single_encoder("nvh265enc") {
                warn!(
                    decoder = va_jpeg_dec,
                    selected = %setup.encoder_name,
                    override_to = "nvh265enc",
                    "Overriding encoder to match NVIDIA decoder memory domain"
                );
                setup.encoder_name = "nvh265enc".to_string();
                setup.parser_str = "! h265parse".to_string();
                setup.muxer_name = "mp4mux".to_string();
            } else if probe_single_encoder("nvh264enc") {
                warn!(
                    decoder = va_jpeg_dec,
                    selected = %setup.encoder_name,
                    override_to = "nvh264enc",
                    "Overriding encoder to match NVIDIA decoder memory domain"
                );
                setup.encoder_name = "nvh264enc".to_string();
                setup.parser_str = "! h264parse".to_string();
                setup.muxer_name = "mp4mux".to_string();
            } else {
                warn!(
                    decoder = va_jpeg_dec,
                    "NVIDIA encoders not functional, using selected encoder with potential memory transfer"
                );
            }
        }

        let pipeline_desc = format!(
            "appsrc name=camera-appsrc \
               caps=image/jpeg,width={w},height={h},framerate={fps}/1 \
               is-live=true do-timestamp=false format=time \
               min-latency={lat} max-latency={lat} \
             ! queue max-size-buffers=60 max-size-time=3000000000 \
             ! {decoder} name=jpeg-decoder \
             ! videoconvert \
             ! {encoder} name=recording-encoder \
             {parser} \
             ! {muxer} name=recording-muxer \
             ! filesink location={loc}",
            w = width,
            h = height,
            fps = framerate,
            lat = setup.frame_duration_ns,
            decoder = va_jpeg_dec,
            encoder = setup.encoder_name,
            parser = setup.parser_str,
            muxer = setup.muxer_name,
            loc = setup.output_path.display(),
        );

        info!(desc = %pipeline_desc, "Launching JPEG zero-copy pipeline");

        if setup.audio_elements.is_some() {
            warn!("A/V sync: audio branch active, video PTS compensated in compute_pts");
        }

        let (pipeline, appsrc) = build_recorder_pipeline(
            &pipeline_desc,
            &setup.encoder_name,
            &encoder_config,
            width,
            height,
            setup.audio_elements.as_ref(),
            &audio_levels,
        )?;

        // JPEG-specific PTS verification probes
        if let Some(decoder) = pipeline.by_name("jpeg-decoder") {
            install_pts_trace_probe(&decoder, "decoder-out");
        }
        if let Some(enc_element) = pipeline.by_name("recording-encoder") {
            install_pts_trace_probe(&enc_element, "encoder-out");
        }

        drop(Self::spawn_appsrc_jpeg_pusher(appsrc, frame_rx, framerate));

        publish_recording_diagnostics(RecordingDiagnostics {
            mode: format!("JPEG zero-copy ({} → {})", va_jpeg_dec, setup.encoder_name),
            pipeline_string: pipeline_desc.clone(),
            encoder: setup.encoder_name.clone(),
            resolution: format!("{}x{}", width, height),
            framerate,
        });

        let recorder = VideoRecorder {
            pipeline,
            file_path: setup.output_path,
        };

        // Eagerly start the pipeline so failures (e.g. NVIDIA encoder not
        // functional in a flatpak sandbox) are caught here and the caller
        // can fall back to the legacy appsrc path.
        recorder.start()?;

        Ok(recorder)
    }

    /// Spawn the JPEG (zero-copy) pusher task.
    ///
    /// Passes raw JPEG bytes straight through via [`spawn_pusher`].
    fn spawn_appsrc_jpeg_pusher(
        appsrc: gst_app::AppSrc,
        frame_rx: tokio::sync::mpsc::Receiver<RecordingFrame>,
        framerate: u32,
    ) -> tokio::task::JoinHandle<()> {
        spawn_pusher(
            appsrc,
            frame_rx,
            framerate,
            "JPEG recorder",
            |rec_frame, _appsrc| match rec_frame {
                RecordingFrame::Jpeg {
                    data,
                    sensor_timestamp_ns,
                    sequence,
                    ..
                } => Some(PusherFrame {
                    buffer: gst::Buffer::from_slice(data),
                    sensor_ts: sensor_timestamp_ns,
                    sequence,
                }),
                RecordingFrame::Decoded(_) => None,
            },
        )
    }

    /// Create audio branch elements
    ///
    /// Uses `pulsesrc` (PipeWire's PulseAudio compatibility layer) for reliable
    /// audio capture from all device types including pro-audio (multi-channel)
    /// and standard stereo/mono sources.
    ///
    /// All input channels are mixed down to mono via a capsfilter, using the
    /// hardware input gains as-is (no software volume adjustment).
    fn create_audio_branch(
        audio_device: Option<&str>,
        audio_encoder_config: crate::media::encoders::audio::SelectedAudioEncoder,
    ) -> Result<Option<AudioBranch>, String> {
        let mut source_builder = gst::ElementFactory::make("pulsesrc")
            // Use pipeline clock for timestamps instead of device clock.
            // `re-timestamp` makes pulsesrc stamp buffers when they arrive,
            // giving consistent A/V sync regardless of PipeWire routing
            // latency for non-default devices. Tradeoff: audio latency
            // through PipeWire (~10-50ms) is absorbed into timestamps, and
            // long recordings may drift if PipeWire and pipeline clocks
            // diverge. If drift is observed, consider `slave-method=skew`.
            .property_from_str("slave-method", "re-timestamp")
            .property("provide-clock", false);

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

    /// Start recording (idempotent — no-op if already playing)
    pub fn start(&self) -> Result<(), String> {
        // Skip if already playing (e.g. JPEG zero-copy path starts eagerly)
        if self.pipeline.current_state() == gst::State::Playing {
            info!("Pipeline already playing, skipping start");
            return Ok(());
        }

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
        clear_recording_diagnostics();

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

        let mut eos_timeout = false;

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
                        eos_timeout = true;
                    }
                    _ => {}
                },
                None => {
                    warn!("Timeout (10s) waiting for pipeline EOS, forcing shutdown");
                    eos_timeout = true;
                }
            }
        } else {
            // Fallback: no bus available, use fixed sleep
            warn!("No pipeline bus available, using fixed sleep fallback");
            std::thread::sleep(std::time::Duration::from_millis(1000));
            eos_timeout = true;
        }

        // Set pipeline to NULL state - this will trigger final cleanup
        info!("Setting pipeline to NULL state");
        self.pipeline
            .set_state(gst::State::Null)
            .map_err(|e| format!("Failed to stop pipeline: {}", e))?;

        let file_path = std::mem::take(&mut self.file_path);
        if eos_timeout {
            warn!(path = %file_path.display(), "Recording may be incomplete (EOS timeout)");
            Err(format!(
                "Recording saved but may be incomplete: {}",
                file_path.display()
            ))
        } else {
            info!(path = %file_path.display(), "Recording saved");
            Ok(file_path)
        }
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

/// Convert a YUV CameraFrame (I420 or Y42B) to tightly-packed NV12.
#[allow(dead_code)]
///
/// NV12 layout: Y plane (width × height) followed by interleaved UV plane
/// (width × height/2). This eliminates GStreamer's `videoconvert` element
/// from the recording pipeline — the conversion here is a simple memcpy +
/// interleave, much cheaper than videoconvert's generic pixel-format engine.
///
/// - I420 (4:2:0): Y + U + V → Y + interleave(U, V). Lossless.
/// - Y42B (4:2:2): Y + U + V → Y + interleave + vertical subsample UV.
///   Minor chroma quality loss (acceptable for video recording).
fn yuv_to_nv12(frame: &std::sync::Arc<crate::backends::camera::types::CameraFrame>) -> Vec<u8> {
    let w = frame.width as usize;
    let h = frame.height as usize;
    let nv12_uv_h = h / 2;
    let nv12_uv_w = w / 2;
    // NV12 total: Y (w*h) + UV interleaved (w * h/2)
    let nv12_size = w * h + w * nv12_uv_h;
    let mut nv12 = Vec::with_capacity(nv12_size);
    let data: &[u8] = &frame.data;

    if let Some(ref planes) = frame.yuv_planes {
        let y_stride = frame.stride as usize;
        let uv_stride = planes.uv_stride as usize;
        let src_uv_h = planes.uv_height as usize;

        // Copy Y plane (strip stride padding), with bounds check
        if y_stride == w {
            let y_end = planes.y_offset + w * h;
            if y_end <= data.len() {
                nv12.extend_from_slice(&data[planes.y_offset..y_end]);
            } else {
                warn!(expected = y_end, actual = data.len(), "Truncated Y plane");
                nv12.resize(nv12_size, 0);
                return nv12;
            }
        } else {
            for row in 0..h {
                let start = planes.y_offset + row * y_stride;
                let end = start + w;
                if end > data.len() {
                    warn!(row, expected = end, actual = data.len(), "Truncated Y row");
                    nv12.resize(nv12_size, 0);
                    return nv12;
                }
                nv12.extend_from_slice(&data[start..end]);
            }
        }

        // Interleave U and V into NV12 UV plane.
        // I420 (src_uv_h == h/2): copy all rows, interleave U[x] V[x].
        // Y42B (src_uv_h == h): take every other row (vertical subsample).
        let row_step = if src_uv_h > nv12_uv_h { 2 } else { 1 };
        for row in 0..nv12_uv_h {
            let src_row = row * row_step;
            let u_end = planes.uv_offset + src_row * uv_stride + nv12_uv_w;
            let v_end = planes.v_offset + src_row * uv_stride + nv12_uv_w;
            if u_end > data.len() || v_end > data.len() {
                warn!(row, "Truncated UV plane, padding remainder with zeros");
                nv12.resize(nv12_size, 0);
                return nv12;
            }
            let u_row = planes.uv_offset + src_row * uv_stride;
            let v_row = planes.v_offset + src_row * uv_stride;
            for x in 0..nv12_uv_w {
                nv12.push(data[u_row + x]);
                nv12.push(data[v_row + x]);
            }
        }
    } else {
        // No yuv_planes — assume tightly packed I420
        let y_size = w * h;
        let uv_plane_size = nv12_uv_w * nv12_uv_h;
        let required = y_size + 2 * uv_plane_size;

        if data.len() < required {
            warn!(
                expected = required,
                actual = data.len(),
                "Truncated I420 frame"
            );
            nv12.resize(nv12_size, 0);
            return nv12;
        }

        // Copy Y
        nv12.extend_from_slice(&data[..y_size]);

        // Interleave U and V
        let u_start = y_size;
        let v_start = y_size + uv_plane_size;
        for i in 0..uv_plane_size {
            nv12.push(data[u_start + i]);
            nv12.push(data[v_start + i]);
        }
    }

    nv12
}

/// Check which video encoders are available (backward compatibility)
pub fn check_available_encoders() {
    crate::media::encoders::log_available_encoders();
}
