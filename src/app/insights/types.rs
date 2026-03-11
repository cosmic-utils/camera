// SPDX-License-Identifier: GPL-3.0-only

//! Types for the Insights drawer diagnostic information.

use crate::media::decoders::{DecoderDef, H264_DECODERS, H265_DECODERS, MJPEG_DECODERS};
use crate::pipelines::video::AudioLevels;
use std::sync::OnceLock;

/// Cached decoder availability (checked once at startup, per codec)
static MJPEG_AVAILABILITY: OnceLock<Vec<bool>> = OnceLock::new();
static H264_AVAILABILITY: OnceLock<Vec<bool>> = OnceLock::new();
static H265_AVAILABILITY: OnceLock<Vec<bool>> = OnceLock::new();

/// State for Insights drawer diagnostic information
#[derive(Debug, Clone, Default)]
pub struct InsightsState {
    // Pipeline info
    /// Full GStreamer pipeline string
    pub full_pipeline_string: Option<String>,
    /// Decoder fallback chain status
    pub decoder_chain: Vec<DecoderStatus>,

    // Current format chain
    /// Current format pipeline information
    pub format_chain: FormatChain,

    // Performance metrics
    /// Frame latency in microseconds
    pub frame_latency_us: u64,
    /// Total dropped frames count
    pub dropped_frames: u64,
    /// Frame size after decoding in bytes
    pub frame_size_decoded: usize,
    /// GStreamer decode/conversion time in microseconds
    pub gstreamer_decode_time_us: u64,
    /// GPU compute shader conversion time in microseconds
    pub gpu_conversion_time_us: u64,
    /// Copy time (source to GPU) in microseconds
    pub copy_time_us: u64,
    /// CPU decode time in microseconds (e.g., turbojpeg MJPEG→I420)
    pub cpu_decode_time_us: u64,
    /// CPU processing description (e.g., "MJPEG → I420 (turbojpeg)")
    pub cpu_processing: Option<String>,
    /// Copy bandwidth in MB/s
    pub copy_bandwidth_mbps: f64,

    // Backend info
    /// Active backend type (e.g., "libcamera", "PipeWire")
    pub backend_type: String,
    /// libcamera pipeline handler (e.g., "RPiCFE", "simple")
    pub pipeline_handler: Option<String>,
    /// libcamera version string
    pub libcamera_version: Option<String>,
    /// Sensor model (e.g., "sony,imx371")
    pub sensor_model: Option<String>,
    /// MJPEG decoder name when native libcamera decodes MJPEG (e.g., "turbojpeg (libjpeg-turbo)")
    pub mjpeg_decoder: Option<String>,

    // Per-frame libcamera metadata (updated every frame)
    /// Actual exposure time applied (microseconds)
    pub meta_exposure_us: Option<u64>,
    /// Actual analogue gain applied
    pub meta_analogue_gain: Option<f32>,
    /// Actual digital gain applied
    pub meta_digital_gain: Option<f32>,
    /// Color temperature (Kelvin)
    pub meta_colour_temperature: Option<u32>,
    /// Frame sequence number
    pub meta_sequence: Option<u32>,
    /// ISP white balance gains [R, B]
    pub meta_colour_gains: Option<[f32; 2]>,
    /// Sensor black level (normalized 0..1)
    pub meta_black_level: Option<f32>,
    /// Lens position (dioptres)
    pub meta_lens_position: Option<f32>,
    /// Scene illuminance (lux)
    pub meta_lux: Option<f32>,
    /// Focus figure of merit
    pub meta_focus_fom: Option<i32>,
    /// Whether libcamera metadata is present at all
    pub has_libcamera_metadata: bool,

    // Audio levels (snapshot from SharedAudioLevels)
    /// Last polled audio level data (updated by insights tick)
    pub audio_levels: Option<AudioLevels>,

    // Multi-stream info
    /// Whether dual-stream mode is active via GStreamer
    pub is_multistream: bool,
    /// Whether libcamera itself supports dual-stream (ViewFinder + Raw)
    /// even if GStreamer's Software ISP can't handle it
    pub libcamera_multistream_capable: bool,
    /// Preview stream details
    pub preview_stream: Option<StreamInfo>,
    /// Capture stream details
    pub capture_stream: Option<StreamInfo>,
}

/// Information about an individual stream in a multi-stream pipeline
#[derive(Debug, Clone, Default)]
pub struct StreamInfo {
    /// Stream role (e.g., "View-finder", "Raw", "Still-capture")
    pub role: String,
    /// Negotiated resolution (e.g., "1920x1080")
    pub resolution: String,
    /// Negotiated pixel format (e.g., "NV12", "BayerRGGB")
    pub pixel_format: String,
    /// Total frames received on this stream
    pub frame_count: u64,
}

/// Status of a decoder in the fallback chain
#[derive(Debug, Clone)]
pub struct DecoderStatus {
    /// Decoder element name (e.g., "vaapijpegdec")
    pub name: &'static str,
    /// Human-readable description
    pub description: &'static str,
    /// Current state in the fallback chain
    pub state: FallbackState,
}

/// State of a decoder in the fallback chain
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum FallbackState {
    /// Currently active
    Selected,
    /// Available but not selected
    Available,
    /// Not present on the system
    #[default]
    Unavailable,
}

/// Current format pipeline chain
#[derive(Debug, Clone, Default)]
pub struct FormatChain {
    /// Camera source type (e.g., "V4L2 via PipeWire", "libcamera via PipeWire")
    pub source: String,
    /// Current resolution
    pub resolution: String,
    /// Current framerate
    pub framerate: String,
    /// Native format from camera (e.g., "MJPG", "YUYV", "NV12")
    pub native_format: String,
    /// GStreamer output format after decoding (if applicable, e.g., "I420", "NV12")
    pub gstreamer_output: Option<String>,
    /// WGPU processing description (e.g., "I420 → RGBA", "Passthrough")
    pub wgpu_processing: String,
}

/// Get cached decoder availability for a decoder list
fn get_cached_availability(
    decoders: &[DecoderDef],
    cache: &'static OnceLock<Vec<bool>>,
) -> &'static Vec<bool> {
    cache.get_or_init(|| {
        decoders
            .iter()
            .map(|d| gstreamer::ElementFactory::find(d.name).is_some())
            .collect()
    })
}

/// Build a decoder status chain from decoder definitions
///
/// This is the generic builder that replaces the three format-specific methods.
fn build_chain_from_defs(
    decoders: &'static [DecoderDef],
    availability: &[bool],
    full_pipeline: Option<&str>,
) -> Vec<DecoderStatus> {
    // Find which decoder is actually used in the pipeline
    let active_decoder = full_pipeline.and_then(|pipeline| {
        decoders.iter().find_map(|d| {
            // Check for decoder name followed by space, '!', or end of string
            if pipeline.contains(&format!("{} ", d.name))
                || pipeline.contains(&format!("{}!", d.name))
                || pipeline.ends_with(d.name)
            {
                Some(d.name)
            } else {
                None
            }
        })
    });

    decoders
        .iter()
        .enumerate()
        .map(|(i, decoder)| {
            let state = if active_decoder == Some(decoder.name) {
                FallbackState::Selected
            } else if availability.get(i).copied().unwrap_or(false) {
                FallbackState::Available
            } else {
                FallbackState::Unavailable
            };
            DecoderStatus {
                name: decoder.name,
                description: decoder.description,
                state,
            }
        })
        .collect()
}

impl InsightsState {
    /// Build the decoder fallback chain based on pixel format
    ///
    /// `pixel_format` is the camera's native format (e.g., "MJPG", "H264", "YUYV")
    /// `full_pipeline` is the actual GStreamer pipeline string to parse for the active decoder.
    /// Decoder availability is cached on first call since it doesn't change at runtime.
    pub fn build_decoder_chain(
        pixel_format: Option<&str>,
        full_pipeline: Option<&str>,
    ) -> Vec<DecoderStatus> {
        match pixel_format {
            Some("MJPG") | Some("MJPEG") => {
                let availability = get_cached_availability(MJPEG_DECODERS, &MJPEG_AVAILABILITY);
                build_chain_from_defs(MJPEG_DECODERS, availability, full_pipeline)
            }
            Some("H264") => {
                let availability = get_cached_availability(H264_DECODERS, &H264_AVAILABILITY);
                build_chain_from_defs(H264_DECODERS, availability, full_pipeline)
            }
            Some("H265") | Some("HEVC") => {
                let availability = get_cached_availability(H265_DECODERS, &H265_AVAILABILITY);
                build_chain_from_defs(H265_DECODERS, availability, full_pipeline)
            }
            // Raw formats don't need decoders
            _ => Vec::new(),
        }
    }
}
