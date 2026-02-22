// SPDX-License-Identifier: GPL-3.0-only
// Shared types for camera backend abstraction

//! Shared types for camera backends

use gstreamer::buffer::{MappedBuffer, Readable};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use std::time::Instant;

/// Frame data storage - either pre-copied bytes or zero-copy GStreamer buffer
///
/// This enum allows frames to be passed around without copying the underlying
/// pixel data when coming from GStreamer pipelines. The `Mapped` variant keeps
/// the GStreamer buffer mapped and alive until all references are dropped.
#[derive(Clone)]
pub enum FrameData {
    /// Pre-copied bytes (used for photo capture, file sources, tests, etc.)
    Copied(Arc<[u8]>),
    /// Zero-copy mapped GStreamer buffer - no data copy, just reference counting
    Mapped(Arc<MappedBuffer<Readable>>),
}

impl FrameData {
    /// Create FrameData from a mapped GStreamer buffer (zero-copy)
    pub fn from_mapped_buffer(buffer: MappedBuffer<Readable>) -> Self {
        FrameData::Mapped(Arc::new(buffer))
    }

    /// Get the length of the frame data in bytes
    pub fn len(&self) -> usize {
        match self {
            FrameData::Copied(data) => data.len(),
            FrameData::Mapped(buf) => buf.len(),
        }
    }

    /// Check if the frame data is empty
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

impl std::fmt::Debug for FrameData {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            FrameData::Copied(data) => write!(f, "FrameData::Copied({} bytes)", data.len()),
            FrameData::Mapped(buf) => write!(f, "FrameData::Mapped({} bytes)", buf.len()),
        }
    }
}

impl AsRef<[u8]> for FrameData {
    fn as_ref(&self) -> &[u8] {
        match self {
            FrameData::Copied(data) => data.as_ref(),
            FrameData::Mapped(buf) => buf.as_slice(),
        }
    }
}

impl std::ops::Deref for FrameData {
    type Target = [u8];

    fn deref(&self) -> &[u8] {
        self.as_ref()
    }
}

/// Camera backend type
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
pub enum CameraBackendType {
    /// PipeWire backend (modern Linux standard)
    #[default]
    PipeWire,
    /// libcamera backend via native libcamera-rs bindings (multi-stream capture)
    Libcamera,
}

impl std::fmt::Display for CameraBackendType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CameraBackendType::PipeWire => write!(f, "PipeWire"),
            CameraBackendType::Libcamera => write!(f, "libcamera"),
        }
    }
}

/// Device information from V4L2 capability
#[derive(Debug, Clone, Default)]
pub struct DeviceInfo {
    /// Name of the device (V4L2 card)
    pub card: String,
    /// Driver name (V4L2 driver)
    pub driver: String,
    /// Device path (e.g., /dev/video0)
    pub path: String,
    /// Real device path (resolved symlinks)
    pub real_path: String,
}

/// Sensor rotation in degrees (clockwise)
///
/// Camera sensors may be physically mounted at various angles relative to the device.
/// This is common on mobile devices where sensors are rotated 90° or 270° relative
/// to the display orientation.
///
/// The rotation value comes from:
/// - libcamera's `api.libcamera.rotation` property in PipeWire
/// - Device tree sensor rotation values
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SensorRotation {
    /// No rotation (sensor is oriented correctly)
    #[default]
    None,
    /// 90 degrees clockwise
    Rotate90,
    /// 180 degrees (upside down)
    Rotate180,
    /// 270 degrees clockwise (90 degrees counter-clockwise)
    Rotate270,
}

impl SensorRotation {
    /// Create rotation from an integer degree value (normalised to 0-360).
    pub fn from_degrees_int(degrees: i32) -> Self {
        match degrees.rem_euclid(360) {
            90 => SensorRotation::Rotate90,
            180 => SensorRotation::Rotate180,
            270 => SensorRotation::Rotate270,
            _ => SensorRotation::None,
        }
    }

    /// Parse rotation from a string value (degrees)
    pub fn from_degrees(degrees: &str) -> Self {
        match degrees.trim() {
            "90" => SensorRotation::Rotate90,
            "180" => SensorRotation::Rotate180,
            "270" => SensorRotation::Rotate270,
            "0" | "" => SensorRotation::None,
            other => {
                if let Ok(deg) = other.parse::<i32>() {
                    Self::from_degrees_int(deg)
                } else {
                    SensorRotation::None
                }
            }
        }
    }

    /// Get the rotation in degrees
    pub fn degrees(&self) -> u32 {
        match self {
            SensorRotation::None => 0,
            SensorRotation::Rotate90 => 90,
            SensorRotation::Rotate180 => 180,
            SensorRotation::Rotate270 => 270,
        }
    }

    /// Check if rotation swaps width and height
    pub fn swaps_dimensions(&self) -> bool {
        matches!(self, SensorRotation::Rotate90 | SensorRotation::Rotate270)
    }

    /// Get the rotation as a GPU shader code (0=None, 1=90CW, 2=180, 3=270CW)
    pub fn gpu_rotation_code(&self) -> u32 {
        match self {
            SensorRotation::None => 0,
            SensorRotation::Rotate90 => 1,
            SensorRotation::Rotate180 => 2,
            SensorRotation::Rotate270 => 3,
        }
    }
}

impl std::fmt::Display for SensorRotation {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}°", self.degrees())
    }
}

/// Represents a camera device
#[derive(Debug, Clone)]
pub struct CameraDevice {
    pub name: String,
    pub path: String,                    // Path to capture device (pipewire node ID)
    pub metadata_path: Option<String>,   // Path to metadata/control device or node ID
    pub device_info: Option<DeviceInfo>, // V4L2 device information (card, driver, path, real_path)
    pub rotation: SensorRotation,        // Sensor rotation from libcamera/device tree
    pub pipeline_handler: Option<String>, // libcamera pipeline handler (e.g., "simple", "vc4", "ipu3")
    pub supports_multistream: bool, // True if camera supports simultaneous preview + raw capture
    pub sensor_model: Option<String>, // Sensor model from device tree compatible (e.g., "sony,imx371")
    pub camera_location: Option<String>, // Camera location: "front", "back", or "external"
    pub libcamera_version: Option<String>, // libcamera version from GStreamer plugin
    pub lens_actuator_path: Option<String>, // V4L2 subdevice for lens focus control
}

/// Framerate as a fraction (numerator/denominator)
/// Stores exact framerate to handle NTSC rates like 59.94fps (60000/1001)
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Framerate {
    pub num: u32,
    pub denom: u32,
}

impl Framerate {
    /// Create a new framerate from numerator and denominator
    pub fn new(num: u32, denom: u32) -> Self {
        Self {
            num,
            denom: if denom == 0 { 1 } else { denom },
        }
    }

    /// Create a framerate from an integer (e.g., 30 becomes 30/1)
    pub fn from_int(fps: u32) -> Self {
        Self { num: fps, denom: 1 }
    }

    /// Get the framerate as a floating point value
    pub fn as_f64(&self) -> f64 {
        self.num as f64 / self.denom as f64
    }

    /// Get the rounded integer framerate (for backwards compatibility)
    pub fn as_int(&self) -> u32 {
        self.num / self.denom
    }

    /// Format as GStreamer fraction string (e.g., "60000/1001")
    pub fn as_gst_fraction(&self) -> String {
        format!("{}/{}", self.num, self.denom)
    }

    /// Check if this framerate matches an integer fps (for config compatibility)
    /// Returns true if the integer part matches (e.g., 59.94fps matches 59)
    pub fn matches_int(&self, fps: u32) -> bool {
        self.as_int() == fps
    }
}

impl std::fmt::Display for Framerate {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let fps = self.as_f64();
        // Show decimal for non-integer framerates (NTSC)
        if self.denom != 1 {
            write!(f, "{:.2}", fps)
        } else {
            write!(f, "{}", self.num)
        }
    }
}

impl Default for Framerate {
    fn default() -> Self {
        Self { num: 30, denom: 1 }
    }
}

/// Camera format specification
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CameraFormat {
    pub width: u32,
    pub height: u32,
    pub framerate: Option<Framerate>, // None for photo mode
    pub hardware_accelerated: bool,   // True for MJPEG and raw formats with HW support
    pub pixel_format: String,         // FourCC code (e.g., "MJPG", "H264", "YUYV")
}

impl std::fmt::Display for CameraFormat {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if let Some(fps) = &self.framerate {
            write!(f, "{}x{} @ {}fps", self.width, self.height, fps)
        } else {
            write!(f, "{}x{}", self.width, self.height)
        }
    }
}

/// Pixel format for camera frames
///
/// Supports both direct RGBA and various YUV formats for GPU conversion.
/// YUV formats are converted to RGBA by a GPU compute shader before use
/// by downstream consumers (filters, histogram, photo capture, etc.)
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PixelFormat {
    /// RGBA - 32-bit with alpha (4 bytes per pixel)
    /// This is the canonical format used throughout the pipeline after conversion
    RGBA,
    /// NV12 - Semi-planar 4:2:0 (Y plane + interleaved UV plane)
    /// Common output from MJPEG decoders
    NV12,
    /// I420 - Planar 4:2:0 (separate Y, U, V planes)
    /// Common output from software JPEG decoders
    I420,
    /// YUYV - Packed 4:2:2 (Y0 U Y1 V interleaved)
    /// Common raw format from webcam sensors
    YUYV,
    /// UYVY - Packed 4:2:2 (U Y0 V Y1 interleaved)
    /// Common alternative to YUYV
    UYVY,
    /// Gray8 - 8-bit grayscale (single channel)
    /// Used for monochrome cameras, depth sensors, IR cameras
    Gray8,
    /// RGB24 - 24-bit RGB (3 bytes per pixel, no alpha)
    /// Direct RGB without alpha channel
    RGB24,
    /// NV21 - Semi-planar 4:2:0 (Y plane + interleaved VU plane)
    /// Like NV12 but with V and U swapped
    NV21,
    /// YVYU - Packed 4:2:2 (Y0 V Y1 U interleaved)
    /// Variant of YUYV with U/V swapped
    YVYU,
    /// VYUY - Packed 4:2:2 (V Y0 U Y1 interleaved)
    /// Variant with V first
    VYUY,
    /// ABGR8888 - 32-bit with alpha (A B G R byte order)
    /// Native format from libcamera, converted to RGBA by shader
    ABGR,
    /// BGRA - 32-bit with alpha (B G R A byte order)
    /// Common format from some cameras, converted to RGBA by shader
    BGRA,
    /// RGGB Bayer pattern - Raw sensor data requiring debayering
    /// Row 0: R G R G..., Row 1: G B G B...
    BayerRGGB,
    /// BGGR Bayer pattern - Raw sensor data requiring debayering
    /// Row 0: B G B G..., Row 1: G R G R...
    BayerBGGR,
    /// GRBG Bayer pattern - Raw sensor data requiring debayering
    /// Row 0: G R G R..., Row 1: B G B G...
    BayerGRBG,
    /// GBRG Bayer pattern - Raw sensor data requiring debayering
    /// Row 0: G B G B..., Row 1: R G R G...
    BayerGBRG,
}

impl PixelFormat {
    /// Check if this format is a YUV format requiring GPU conversion
    pub fn is_yuv(&self) -> bool {
        matches!(
            self,
            Self::NV12
                | Self::I420
                | Self::YUYV
                | Self::UYVY
                | Self::NV21
                | Self::YVYU
                | Self::VYUY
        )
    }

    /// Check if this format is a raw Bayer pattern requiring debayering
    pub fn is_bayer(&self) -> bool {
        matches!(
            self,
            Self::BayerRGGB | Self::BayerBGGR | Self::BayerGRBG | Self::BayerGBRG
        )
    }

    /// Get Bayer pattern code for debayer shader (0=RGGB, 1=BGGR, 2=GRBG, 3=GBRG)
    pub fn bayer_pattern_code(&self) -> Option<u32> {
        match self {
            Self::BayerRGGB => Some(0),
            Self::BayerBGGR => Some(1),
            Self::BayerGRBG => Some(2),
            Self::BayerGBRG => Some(3),
            _ => None,
        }
    }

    /// Check if this format requires GPU conversion (YUV, ABGR, BGRA, Gray8, RGB24, Bayer)
    /// These formats go through the compute shader for conversion to RGBA
    pub fn needs_gpu_conversion(&self) -> bool {
        !matches!(self, Self::RGBA)
    }

    /// Get the format code for the GPU compute shader
    pub fn gpu_format_code(&self) -> u32 {
        match self {
            Self::RGBA => 0,
            Self::NV12 => 1,
            Self::I420 => 2,
            Self::YUYV => 3,
            Self::UYVY => 4,
            Self::Gray8 => 5,
            Self::RGB24 => 6,
            Self::NV21 => 7,
            Self::YVYU => 8,
            Self::VYUY => 9,
            Self::ABGR => 10,
            Self::BGRA => 11,
            Self::BayerRGGB => 12,
            Self::BayerBGGR => 13,
            Self::BayerGRBG => 14,
            Self::BayerGBRG => 15,
        }
    }

    /// Average bytes per pixel (accounting for chroma subsampling)
    pub fn bytes_per_pixel(&self) -> f32 {
        match self {
            Self::RGBA | Self::ABGR | Self::BGRA => 4.0, // 4 bytes per pixel
            Self::NV12 | Self::NV21 | Self::I420 => 1.5, // 4:2:0 subsampling
            Self::YUYV | Self::UYVY | Self::YVYU | Self::VYUY => 2.0, // 4:2:2 subsampling
            Self::Gray8 => 1.0,                          // Single channel
            Self::RGB24 => 3.0,                          // 3 bytes per pixel
            // Bayer patterns: 1 byte per pixel (8-bit) or 2 bytes (10/12/16-bit)
            Self::BayerRGGB | Self::BayerBGGR | Self::BayerGRBG | Self::BayerGBRG => 1.0,
        }
    }

    /// Convert to a GStreamer video/x-raw format string.
    ///
    /// Used when setting caps on an appsrc element to feed native frames
    /// into a GStreamer encoding pipeline.
    pub fn to_gst_format_string(&self) -> &'static str {
        match self {
            Self::RGBA => "RGBA",
            Self::ABGR => "ABGR",
            Self::BGRA => "BGRA",
            Self::NV12 => "NV12",
            Self::NV21 => "NV21",
            Self::I420 => "I420",
            Self::YUYV => "YUY2",
            Self::UYVY => "UYVY",
            Self::YVYU => "YVYU",
            Self::VYUY => "VYUY",
            Self::Gray8 => "GRAY8",
            Self::RGB24 => "RGB",
            // Bayer formats — GStreamer uses video/x-bayer, not video/x-raw,
            // so these cannot appear in raw caps; return a placeholder.
            Self::BayerRGGB => "RGGB",
            Self::BayerBGGR => "BGGR",
            Self::BayerGRBG => "GRBG",
            Self::BayerGBRG => "GBRG",
        }
    }

    /// Parse format from GStreamer format string
    pub fn from_gst_format(format: &str) -> Option<Self> {
        match format {
            "RGBA" | "RGBx" | "xRGB" | "ARGB" => Some(Self::RGBA),
            "ABGR" | "xBGR" => Some(Self::ABGR),
            "BGRA" | "BGRx" => Some(Self::BGRA),
            "NV12" => Some(Self::NV12),
            "NV21" => Some(Self::NV21),
            "I420" | "YV12" => Some(Self::I420),
            "YUYV" | "YUY2" => Some(Self::YUYV),
            "UYVY" => Some(Self::UYVY),
            "YVYU" => Some(Self::YVYU),
            "VYUY" => Some(Self::VYUY),
            "GRAY8" | "GREY" | "Y8" => Some(Self::Gray8),
            "RGB" | "BGR" => Some(Self::RGB24),
            // Bayer formats from GStreamer video/x-bayer
            "rggb" | "RGGB" => Some(Self::BayerRGGB),
            "bggr" | "BGGR" => Some(Self::BayerBGGR),
            "grbg" | "GRBG" => Some(Self::BayerGRBG),
            "gbrg" | "GBRG" => Some(Self::BayerGBRG),
            _ => None,
        }
    }
}

/// Autofocus state reported in frame metadata
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum AfState {
    #[default]
    Idle,
    Scanning,
    Focused,
    Failed,
}

/// Auto exposure state reported in frame metadata
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum AeState {
    #[default]
    Inactive,
    Searching,
    Converged,
    Locked,
}

/// Auto white balance state reported in frame metadata
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum AwbState {
    #[default]
    Inactive,
    Searching,
    Converged,
    Locked,
}

/// Frame metadata extracted from libcamera completed requests
///
/// This struct contains the actual values applied by the ISP for a given frame,
/// as reported by libcamera. Only populated when using the libcamera backend.
#[derive(Debug, Clone, Default)]
pub struct FrameMetadata {
    /// Actual exposure time applied (microseconds)
    pub exposure_time: Option<u64>,
    /// Actual analogue gain applied
    pub analogue_gain: Option<f32>,
    /// Actual digital gain applied
    pub digital_gain: Option<f32>,
    /// Color temperature (Kelvin)
    pub colour_temperature: Option<u32>,
    /// Lens position (for AF cameras)
    pub lens_position: Option<f32>,
    /// Sensor timestamp (nanoseconds since boot)
    pub sensor_timestamp: Option<u64>,
    /// Frame sequence number
    pub sequence: Option<u32>,
    /// Focus status (scanning, focused, failed)
    pub af_state: Option<AfState>,
    /// Auto exposure state
    pub ae_state: Option<AeState>,
    /// Auto white balance state
    pub awb_state: Option<AwbState>,
    /// ISP white balance gains [R, B] applied by the AWB algorithm
    pub colour_gains: Option<[f32; 2]>,
    /// 3x3 colour correction matrix (row-major) from sensor RGB to sRGB
    pub colour_correction_matrix: Option<[[f32; 3]; 3]>,
    /// Sensor black level (normalized 0..1, from 16-bit SensorBlackLevels)
    pub black_level: Option<f32>,
    /// Scene illuminance (lux)
    pub lux: Option<f32>,
    /// Focus figure of merit (AF quality score)
    pub focus_fom: Option<i32>,
}

/// YUV plane offsets for multi-plane formats (NV12, I420)
///
/// For planar/semi-planar YUV formats, the planes are stored at different offsets
/// within a single contiguous buffer. This struct stores the offsets and strides
/// needed to extract each plane during GPU upload, enabling true zero-copy.
///
/// - NV12: Y plane (full resolution) + UV plane (half resolution, interleaved)
/// - I420: Y plane + U plane + V plane (all separate, U/V at half resolution)
#[derive(Clone, Copy)]
pub struct YuvPlanes {
    /// Y plane offset in bytes from start of buffer
    pub y_offset: usize,
    /// Y plane size in bytes
    pub y_size: usize,
    /// UV plane offset in bytes (NV12: interleaved UV, I420: U plane)
    pub uv_offset: usize,
    /// UV plane size in bytes
    pub uv_size: usize,
    /// UV plane stride in bytes
    pub uv_stride: u32,
    /// V plane offset in bytes (I420 only, 0 for NV12)
    pub v_offset: usize,
    /// V plane size in bytes (I420 only, 0 for NV12)
    pub v_size: usize,
    /// V plane stride in bytes (I420 only)
    pub v_stride: u32,
    /// UV plane width in pixels (derived from subsampling)
    pub uv_width: u32,
    /// UV plane height in pixels (derived from subsampling)
    pub uv_height: u32,
}

impl std::fmt::Debug for YuvPlanes {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("YuvPlanes")
            .field("y_offset", &self.y_offset)
            .field("y_size", &self.y_size)
            .field("uv_offset", &self.uv_offset)
            .field("uv_size", &self.uv_size)
            .field("uv_stride", &self.uv_stride)
            .field("v_offset", &self.v_offset)
            .field("v_size", &self.v_size)
            .field("v_stride", &self.v_stride)
            .field("uv_width", &self.uv_width)
            .field("uv_height", &self.uv_height)
            .finish()
    }
}

/// A single frame from the camera
///
/// Supports both RGBA and YUV formats. For YUV formats:
/// - `data` contains the entire buffer (all planes contiguous, zero-copy)
/// - `yuv_planes` contains offsets to extract Y, UV, V planes during GPU upload
#[derive(Debug, Clone)]
pub struct CameraFrame {
    pub width: u32,
    pub height: u32,
    /// Frame data: RGBA pixels, Y plane (NV12/I420), or packed YUYV
    pub data: FrameData,
    /// Pixel format of the data
    pub format: PixelFormat,
    /// Row stride for the main data (bytes per row, may include padding)
    pub stride: u32,
    /// Additional YUV planes (for NV12/I420 formats)
    pub yuv_planes: Option<YuvPlanes>,
    /// Timestamp when frame was captured (for latency diagnostics)
    pub captured_at: Instant,
    /// Kernel/sensor timestamp in nanoseconds (CLOCK_BOOTTIME).
    /// Used for video recording PTS to maintain A/V sync.
    pub sensor_timestamp_ns: Option<u64>,
    /// libcamera metadata (actual exposure, gain, focus state, etc.)
    /// Only populated when using libcamera control integration
    pub libcamera_metadata: Option<FrameMetadata>,
}

impl CameraFrame {
    /// Determine the GStreamer format string for this frame.
    ///
    /// For most pixel formats this delegates to `PixelFormat::to_gst_format_string()`.
    /// However, MJPEG-decoded frames are always reported as `I420` but the actual
    /// chroma subsampling varies (4:2:0, 4:2:2, 4:4:4) depending on the JPEG.
    /// We inspect `yuv_planes` to distinguish:
    /// - `uv_width == width && uv_height == height` → Y444 (4:4:4)
    /// - `uv_width < width  && uv_height == height` → Y42B (4:2:2)
    /// - Otherwise → I420 (4:2:0, the common case)
    pub fn gst_format_string(&self) -> &'static str {
        if self.format == PixelFormat::I420
            && let Some(ref planes) = self.yuv_planes
        {
            if planes.uv_width == self.width && planes.uv_height == self.height {
                return "Y444";
            }
            if planes.uv_width < self.width && planes.uv_height == self.height {
                return "Y42B";
            }
        }
        self.format.to_gst_format_string()
    }

    /// Convert to a frame with copied data (safe for background processing)
    ///
    /// Mapped GStreamer buffers become invalid when the pipeline is destroyed.
    /// Use this method before sending frames to background tasks that may outlive
    /// the pipeline.
    pub fn to_copied(&self) -> Self {
        let copied_data = match &self.data {
            FrameData::Copied(data) => FrameData::Copied(Arc::clone(data)),
            FrameData::Mapped(buffer) => {
                // Copy the mapped buffer data to owned memory
                let slice: &[u8] = buffer.as_ref();
                let bytes: Arc<[u8]> = Arc::from(slice);
                FrameData::Copied(bytes)
            }
        };

        Self {
            width: self.width,
            height: self.height,
            data: copied_data,
            format: self.format,
            stride: self.stride,
            yuv_planes: self.yuv_planes,
            captured_at: self.captured_at,
            sensor_timestamp_ns: self.sensor_timestamp_ns,
            libcamera_metadata: self.libcamera_metadata.clone(),
        }
    }
}

/// Frame receiver type for preview streams
pub type FrameReceiver = cosmic::iced::futures::channel::mpsc::Receiver<CameraFrame>;

/// Frame sender type for preview streams
pub type FrameSender = cosmic::iced::futures::channel::mpsc::Sender<CameraFrame>;

/// Result type for backend operations
pub type BackendResult<T> = Result<T, BackendError>;

/// Error types for backend operations
#[derive(Debug, Clone)]
pub enum BackendError {
    /// Backend is not available on this system
    NotAvailable(String),
    /// Failed to initialize backend
    InitializationFailed(String),
    /// Camera device not found
    DeviceNotFound(String),
    /// Format not supported
    FormatNotSupported(String),
    /// Backend crashed or became unresponsive
    Crashed(String),
    /// Recording already in progress
    RecordingInProgress,
    /// No recording in progress
    NoRecordingInProgress,
    /// General I/O error
    IoError(String),
    /// Other errors
    Other(String),
}

impl std::fmt::Display for BackendError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            BackendError::NotAvailable(msg) => write!(f, "Backend not available: {}", msg),
            BackendError::InitializationFailed(msg) => write!(f, "Initialization failed: {}", msg),
            BackendError::DeviceNotFound(msg) => write!(f, "Device not found: {}", msg),
            BackendError::FormatNotSupported(msg) => write!(f, "Format not supported: {}", msg),
            BackendError::Crashed(msg) => write!(f, "Backend crashed: {}", msg),
            BackendError::RecordingInProgress => write!(f, "Recording already in progress"),
            BackendError::NoRecordingInProgress => write!(f, "No recording in progress"),
            BackendError::IoError(msg) => write!(f, "I/O error: {}", msg),
            BackendError::Other(msg) => write!(f, "Error: {}", msg),
        }
    }
}

impl std::error::Error for BackendError {}
