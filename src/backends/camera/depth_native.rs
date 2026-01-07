// SPDX-License-Identifier: GPL-3.0-only

#![cfg(all(target_arch = "x86_64", feature = "freedepth"))]

//! Native depth camera backend for simultaneous RGB and depth streaming
//!
//! This backend uses freedepth's KinectStreamer to bypass V4L2 and
//! stream both RGB video and depth data simultaneously.
//!
//! # Key Features
//!
//! - Direct USB isochronous streaming via nusb (not V4L2)
//! - Simultaneous RGB + depth capture at 30fps
//! - Automatic kernel driver unbind/rebind
//! - Full calibration support for accurate depth values
//!
//! # Architecture
//!
//! Depth camera devices are identified by path prefix followed by the
//! freedepth device index. When a depth camera is selected, this backend
//! is used instead of PipeWire/GStreamer.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use std::sync::mpsc::Receiver;

use freedepth::{
    DepthFormat, DepthFrame, DepthRegistration, KinectStreamer, Resolution, VideoFormat, VideoFrame,
};
use tracing::{debug, info, warn};

use super::format_converters::{
    self, ir_8bit_to_rgb, ir_10bit_to_rgb, ir_10bit_unpacked_to_rgb, DepthVisualizationOptions,
};
use super::CameraBackend;
use super::types::*;

/// Path prefix for depth camera devices to distinguish from PipeWire cameras
pub const DEPTH_PATH_PREFIX: &str = "kinect:";

/// Enumerate depth cameras via freedepth
///
/// Returns camera devices for each connected depth sensor.
/// These devices use the native USB backend, bypassing V4L2 entirely.
pub fn enumerate_depth_cameras() -> Vec<CameraDevice> {
    let devices = match freedepth::enumerate_devices() {
        Ok(d) => d,
        Err(e) => {
            debug!("Failed to enumerate depth cameras: {}", e);
            return Vec::new();
        }
    };

    devices
        .iter()
        .map(|dev| {
            let name = dev.name.clone();
            let path = format!("{}{}", DEPTH_PATH_PREFIX, dev.index);
            let serial = dev
                .id
                .serial
                .clone()
                .unwrap_or_else(|| "unknown".to_string());

            info!(
                name = %name,
                path = %path,
                serial = %serial,
                "Found depth camera via freedepth"
            );

            CameraDevice {
                name,
                path,
                metadata_path: Some(serial),
                device_info: Some(DeviceInfo {
                    card: dev.id.family.to_string(),
                    driver: "freedepth".to_string(),
                    path: format!("bus {} addr {}", dev.id.bus, dev.id.address),
                    real_path: format!("usb:{}:{}", dev.id.bus, dev.id.address),
                }),
            }
        })
        .collect()
}

/// Check if a device path refers to a depth camera (native backend)
pub fn is_depth_native_device(path: &str) -> bool {
    path.starts_with(DEPTH_PATH_PREFIX)
}

/// Extract the depth camera device index from a path
pub fn depth_device_index(path: &str) -> Option<usize> {
    path.strip_prefix(DEPTH_PATH_PREFIX)?.parse().ok()
}

/// Get supported formats for a depth camera device
///
/// Returns native formats supported by the depth sensor.
/// Only exposes formats the camera hardware natively outputs:
/// - Bayer: Raw sensor data (640x480 @ 30fps, 1280x1024 @ 10fps)
/// - RGB: Demosaiced Bayer (software conversion, commonly expected output)
/// - YUV: ISP-processed UYVY (640x480 @ 15fps only)
/// - IR 10-bit packed: Native IR sensor output (640x488 @ 30fps, 1280x1024 @ 10fps)
/// - Depth: 11-bit depth data (640x480 @ 30fps)
///
/// Note: IR 8-bit is NOT included as it's a software conversion from 10-bit packed.
/// Note: IR shares the video endpoint with Bayer/YUV - can only stream one at a time.
pub fn get_depth_formats(_device: &CameraDevice) -> Vec<CameraFormat> {
    vec![
        // Bayer GRBG: 640x480 @ 30fps (raw sensor data, requires demosaicing)
        CameraFormat {
            width: 640,
            height: 480,
            framerate: Some(30),
            hardware_accelerated: true,
            pixel_format: "GRBG".to_string(),
            sensor_type: SensorType::Rgb,
        },
        // Bayer GRBG: 1280x1024 @ 10fps (high-res raw sensor data)
        CameraFormat {
            width: 1280,
            height: 1024,
            framerate: Some(10),
            hardware_accelerated: true,
            pixel_format: "GRBG".to_string(),
            sensor_type: SensorType::Rgb,
        },
        // RGB demosaiced: 640x480 @ 30fps
        CameraFormat {
            width: 640,
            height: 480,
            framerate: Some(30),
            hardware_accelerated: true,
            pixel_format: "RGB3".to_string(),
            sensor_type: SensorType::Rgb,
        },
        // RGB demosaiced: 1280x1024 @ 10fps
        CameraFormat {
            width: 1280,
            height: 1024,
            framerate: Some(10),
            hardware_accelerated: true,
            pixel_format: "RGB3".to_string(),
            sensor_type: SensorType::Rgb,
        },
        // YUV UYVY: 640x480 @ 15fps (ISP processed, better colors)
        CameraFormat {
            width: 640,
            height: 480,
            framerate: Some(15),
            hardware_accelerated: true,
            pixel_format: "UYVY".to_string(),
            sensor_type: SensorType::Rgb,
        },
        // IR 10-bit packed: 640x488 @ 30fps (note: height is 488)
        CameraFormat {
            width: 640,
            height: 488,
            framerate: Some(30),
            hardware_accelerated: true,
            pixel_format: "IR10".to_string(),
            sensor_type: SensorType::Ir,
        },
        // IR 10-bit packed: 1280x1024 @ 10fps (high-res requires firmware workaround)
        CameraFormat {
            width: 1280,
            height: 1024,
            framerate: Some(10),
            hardware_accelerated: true,
            pixel_format: "IR10".to_string(),
            sensor_type: SensorType::Ir,
        },
        // Depth 11-bit: 640x480 @ 30fps
        CameraFormat {
            width: 640,
            height: 480,
            framerate: Some(30),
            hardware_accelerated: true,
            pixel_format: "Y10B".to_string(),
            sensor_type: SensorType::Depth,
        },
    ]
}

/// Native depth camera backend state
///
/// Note: The frame receivers are NOT stored here because `mpsc::Receiver` is not `Sync`.
/// They are moved directly to the frame processing thread in `start()`.
pub struct NativeDepthBackend {
    /// The freedepth streamer
    streamer: Option<KinectStreamer>,
    /// Running flag
    running: Arc<AtomicBool>,
    /// Depth-only mode flag (when Y10B format is selected)
    depth_only_mode: Arc<AtomicBool>,
    /// Last video frame (for preview)
    last_video_frame: Arc<Mutex<Option<VideoFrameData>>>,
    /// Last depth frame (for 3D preview)
    last_depth_frame: Arc<Mutex<Option<DepthFrameData>>>,
    /// Frame processing thread
    frame_thread: Option<JoinHandle<()>>,
    /// Currently active device (for CameraBackend trait)
    current_device: Option<CameraDevice>,
    /// Currently active format (for CameraBackend trait)
    current_format: Option<CameraFormat>,
    /// Depth registration data for depth-to-RGB alignment (from device calibration)
    /// Uses trait object for device-agnostic access
    registration: Option<Box<dyn DepthRegistration>>,
}


impl NativeDepthBackend {
    /// Create a new native depth camera backend
    pub fn new() -> Self {
        Self {
            streamer: None,
            running: Arc::new(AtomicBool::new(false)),
            depth_only_mode: Arc::new(AtomicBool::new(false)),
            last_video_frame: Arc::new(Mutex::new(None)),
            last_depth_frame: Arc::new(Mutex::new(None)),
            frame_thread: None,
            current_device: None,
            current_format: None,
            registration: None,
        }
    }

    /// Initialize and start streaming with default settings
    ///
    /// This will unbind the kernel driver and start native USB streaming.
    /// Uses Bayer format at Medium resolution by default.
    ///
    /// # Arguments
    /// * `device_index` - The depth camera device index
    pub fn start(&mut self, device_index: usize) -> Result<(), String> {
        self.start_with_format(device_index, VideoFormat::Bayer, Resolution::Medium)
    }

    /// Initialize and start streaming with specific format and resolution
    ///
    /// # Arguments
    /// * `device_index` - The depth camera device index
    /// * `video_format` - The video format to use
    /// * `resolution` - The resolution to use (Medium or High)
    fn start_with_format(
        &mut self,
        device_index: usize,
        video_format: VideoFormat,
        resolution: Resolution,
    ) -> Result<(), String> {
        if self.running.load(Ordering::SeqCst) {
            return Err("Already running".to_string());
        }

        info!(
            device = device_index,
            format = ?video_format,
            resolution = ?resolution,
            "Starting native depth camera backend"
        );

        // Create the streamer (unbinds kernel driver)
        let mut streamer = KinectStreamer::new(device_index)
            .map_err(|e| format!("Failed to create depth camera streamer: {}", e))?;

        // Start streaming with selected video format and resolution
        let (video_rx, depth_rx) = streamer
            .start(video_format, resolution, DepthFormat::Depth11Bit)
            .map_err(|e| format!("Failed to start streaming: {}", e))?;

        // Fetch device-specific registration data for depth-to-RGB alignment
        let registration = streamer.create_depth_registration();

        // Extract the calibrated depth-to-mm converter before boxing
        // (uses Kinect-specific method, but the trait provides generic GPU data access)
        let depth_converter = Arc::new(registration.depth_to_mm().clone());

        // Log using trait methods for device-agnostic access
        let summary = registration.registration_summary();
        info!(
            "Fetched depth registration with target_offset={}, from_device={}",
            registration.target_offset(),
            summary.from_device
        );

        // Store as trait object for generic access
        self.registration = Some(Box::new(registration));

        // Register USB device for global motor control access
        if let Some(usb) = streamer.usb_device() {
            super::motor_control::set_motor_usb_device(usb);
        }

        self.streamer = Some(streamer);
        self.running.store(true, Ordering::SeqCst);

        // Start frame processing thread
        // Note: receivers are moved directly to the thread (not stored in struct)
        // because mpsc::Receiver is not Sync
        let running = Arc::clone(&self.running);
        let last_video = Arc::clone(&self.last_video_frame);
        let last_depth = Arc::clone(&self.last_depth_frame);

        let thread = thread::spawn(move || {
            frame_processing_thread(
                running,
                video_rx,
                depth_rx,
                last_video,
                last_depth,
                depth_converter,
            );
        });

        self.frame_thread = Some(thread);

        info!("Native depth camera backend started");
        Ok(())
    }

    /// Stop streaming and rebind the kernel driver
    pub fn stop(&mut self) {
        if !self.running.load(Ordering::SeqCst) {
            return;
        }

        info!("Stopping native depth camera backend");
        self.running.store(false, Ordering::SeqCst);

        // Wait for frame thread to finish
        if let Some(thread) = self.frame_thread.take() {
            let _ = thread.join();
        }

        // Clear global motor control reference
        super::motor_control::clear_motor_usb_device();

        // Stop streamer (this rebinds the driver)
        if let Some(mut streamer) = self.streamer.take() {
            streamer.stop();
            if let Err(e) = streamer.rebind_driver() {
                warn!("Failed to rebind kernel driver: {}", e);
            }
        }

        // Clear frame data
        *self.last_video_frame.lock().unwrap() = None;
        *self.last_depth_frame.lock().unwrap() = None;

        info!("Native depth camera backend stopped");
    }

    /// Check if the backend is running
    pub fn is_running(&self) -> bool {
        self.running.load(Ordering::SeqCst)
    }

    /// Get the latest video frame (for preview)
    ///
    /// In depth-only mode (Y10B format), this returns a colormap visualization
    /// of the depth data instead of the RGB video.
    pub fn get_video_frame(&self) -> Option<VideoFrameData> {
        if self.depth_only_mode.load(Ordering::Relaxed) {
            // In depth-only mode, convert depth to RGB visualization
            use crate::shaders::depth::{is_depth_grayscale_mode, is_depth_only_mode};

            let depth_frame = self.last_depth_frame.lock().ok()?.clone()?;
            let viz_options = DepthVisualizationOptions {
                grayscale: is_depth_grayscale_mode(),
                quantize: is_depth_only_mode(), // Use quantization in depth-only mode
                ..DepthVisualizationOptions::kinect()
            };
            let rgb_data = format_converters::depth_to_rgb(
                &depth_frame.depth_mm,
                depth_frame.width,
                depth_frame.height,
                &viz_options,
            );
            Some(VideoFrameData {
                width: depth_frame.width,
                height: depth_frame.height,
                data: rgb_data,
                timestamp: depth_frame.timestamp,
            })
        } else {
            self.last_video_frame.lock().ok()?.clone()
        }
    }

    /// Get the latest depth frame (for 3D preview)
    pub fn get_depth_frame(&self) -> Option<DepthFrameData> {
        self.last_depth_frame.lock().ok()?.clone()
    }

    /// Check if in depth-only mode
    pub fn is_depth_only_mode(&self) -> bool {
        self.depth_only_mode.load(Ordering::Relaxed)
    }

    /// Check if depth data is available
    pub fn has_depth(&self) -> bool {
        self.last_depth_frame
            .lock()
            .map(|d| d.is_some())
            .unwrap_or(false)
    }

    // =========================================================================
    // Motor Control
    // =========================================================================

    /// Set the tilt angle in degrees (see freedepth::TILT_MIN_DEGREES/TILT_MAX_DEGREES)
    pub fn set_tilt(&self, degrees: i8) -> Result<(), String> {
        let streamer = self.streamer.as_ref().ok_or("Depth camera not running")?;
        streamer
            .set_tilt(degrees)
            .map_err(|e| format!("Failed to set tilt: {}", e))
    }

    /// Get the current tilt angle in degrees
    pub fn get_tilt(&self) -> Result<i8, String> {
        let streamer = self.streamer.as_ref().ok_or("Depth camera not running")?;
        streamer
            .get_tilt()
            .map_err(|e| format!("Failed to get tilt: {}", e))
    }

    /// Get the full motor/accelerometer state
    pub fn get_tilt_state(&self) -> Result<freedepth::TiltState, String> {
        let streamer = self.streamer.as_ref().ok_or("Depth camera not running")?;
        streamer
            .get_tilt_state()
            .map_err(|e| format!("Failed to get tilt state: {}", e))
    }

    /// Get the USB device Arc for shared motor control access
    ///
    /// Returns an Arc to the USB device that can be used for motor control
    /// even when the backend is owned elsewhere.
    pub fn usb_device(&self) -> Option<std::sync::Arc<std::sync::Mutex<freedepth::UsbDevice>>> {
        self.streamer.as_ref()?.usb_device()
    }

    /// Get the depth registration data
    ///
    /// Returns the registration data fetched from the device calibration,
    /// which can be used for accurate depth-to-RGB alignment.
    /// Uses the generic DepthRegistration trait for device-agnostic access.
    pub fn get_registration(&self) -> Option<&dyn DepthRegistration> {
        self.registration.as_ref().map(|r| r.as_ref())
    }
}

impl Default for NativeDepthBackend {
    fn default() -> Self {
        Self::new()
    }
}

impl Drop for NativeDepthBackend {
    fn drop(&mut self) {
        self.stop();
    }
}

/// Frame processing thread
fn frame_processing_thread(
    running: Arc<AtomicBool>,
    video_rx: Receiver<VideoFrame>,
    depth_rx: Receiver<DepthFrame>,
    last_video: Arc<Mutex<Option<VideoFrameData>>>,
    last_depth: Arc<Mutex<Option<DepthFrameData>>>,
    depth_converter: Arc<freedepth::DepthToMm>,
) {
    info!("Frame processing thread started (using device-calibrated depth converter)");

    let mut video_count = 0u64;
    let mut depth_count = 0u64;

    while running.load(Ordering::Relaxed) {
        // Process video frames
        match video_rx.try_recv() {
            Ok(frame) => {
                video_count += 1;
                if let Some(processed) = process_video_frame(&frame) {
                    if let Ok(mut guard) = last_video.lock() {
                        *guard = Some(processed);
                    }
                }
            }
            Err(std::sync::mpsc::TryRecvError::Empty) => {}
            Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                debug!("Video channel disconnected");
                break;
            }
        }

        // Process depth frames
        match depth_rx.try_recv() {
            Ok(frame) => {
                depth_count += 1;
                if let Some(processed) = process_depth_frame(&frame, &depth_converter) {
                    if let Ok(mut guard) = last_depth.lock() {
                        *guard = Some(processed);
                    }
                }
            }
            Err(std::sync::mpsc::TryRecvError::Empty) => {}
            Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                debug!("Depth channel disconnected");
                break;
            }
        }

        // Small sleep to avoid busy-waiting
        thread::sleep(Duration::from_micros(100));
    }

    info!(
        video_frames = video_count,
        depth_frames = depth_count,
        "Frame processing thread ended"
    );
}

/// Process a video frame from freedepth format to RGB
///
/// For YUV formats, uses GPU-accelerated conversion when available.
/// For IR formats, unpacks data and converts to grayscale RGB.
fn process_video_frame(frame: &VideoFrame) -> Option<VideoFrameData> {
    // Handle different video formats
    // Note: VideoFormat::Rgb from Kinect is actually Bayer data that needs demosaicing.
    // The Kinect hardware doesn't have native RGB output - all formats go through
    // Bayer pattern data, and RGB conversion happens in software.
    let rgb_data = match frame.format {
        VideoFormat::Rgb | VideoFormat::Bayer => {
            // Both formats are actually Bayer data from the hardware - needs demosaicing
            // The Kinect doesn't have native RGB output, so we always demosaic
            let pixels = (frame.width * frame.height) as usize;
            let mut rgb = vec![0u8; pixels * 3];
            freedepth::convert_bayer_to_rgb(&frame.data, &mut rgb, frame.width, frame.height);
            rgb
        }
        VideoFormat::YuvRaw => {
            // YUV format - try GPU conversion first, fall back to CPU
            match try_gpu_yuv_to_rgb(&frame.data, frame.width, frame.height) {
                Some(rgb) => rgb,
                None => {
                    // GPU not available, use CPU conversion
                    frame.yuv_to_rgb()?
                }
            }
        }
        VideoFormat::YuvRgb => {
            // Already converted to RGB by freedepth
            frame.data.clone()
        }
        VideoFormat::Ir8Bit => {
            // 8-bit grayscale - simply expand to RGB (1 byte per pixel input)
            ir_8bit_to_rgb(&frame.data, frame.width, frame.height)
        }
        VideoFormat::Ir10BitPacked => {
            // 10-bit packed IR - unpack using freedepth and convert to grayscale RGB
            let unpacked = freedepth::unpack_10bit_ir(&frame.data, frame.width, frame.height);
            ir_10bit_unpacked_to_rgb(&unpacked, frame.width, frame.height)
        }
        VideoFormat::Ir10Bit => {
            // 10-bit unpacked IR (stored as u16) - convert to grayscale RGB
            ir_10bit_to_rgb(&frame.data, frame.width, frame.height)
        }
    };

    Some(VideoFrameData {
        width: frame.width,
        height: frame.height,
        data: rgb_data,
        timestamp: frame.timestamp,
    })
}

/// Try GPU-accelerated YUV to RGB conversion
///
/// Returns RGB data (3 bytes per pixel) on success, None if GPU not available.
/// Uses UYVY format which is what the Kinect outputs.
fn try_gpu_yuv_to_rgb(yuv_data: &[u8], width: u32, height: u32) -> Option<Vec<u8>> {
    use crate::shaders::{YuvFormat, convert_yuv_to_rgba_gpu};

    // Use pollster to block on the async GPU conversion
    // This is safe because we're in a dedicated frame processing thread
    // Kinect uses UYVY format (U, Y0, V, Y1) per formats.rs
    let rgba = pollster::block_on(async {
        convert_yuv_to_rgba_gpu(yuv_data, width, height, YuvFormat::Uyvy).await
    })
    .ok()?;

    // Convert RGBA to RGB (drop alpha channel)
    let pixels = (width * height) as usize;
    let mut rgb = Vec::with_capacity(pixels * 3);
    for i in 0..pixels {
        rgb.push(rgba[i * 4]); // R
        rgb.push(rgba[i * 4 + 1]); // G
        rgb.push(rgba[i * 4 + 2]); // B
    }

    Some(rgb)
}

/// Process a depth frame from freedepth format to mm values
///
/// Uses the device-calibrated depth converter for accurate raw-to-mm conversion.
fn process_depth_frame(
    frame: &DepthFrame,
    converter: &freedepth::DepthToMm,
) -> Option<DepthFrameData> {
    // Get depth as u16 slice
    let depth_raw = frame.as_u16()?;

    // Convert raw 11-bit disparity values to millimeters using device-calibrated lookup table
    let mut depth_mm = vec![0u16; depth_raw.len()];
    converter.convert_frame(depth_raw, &mut depth_mm);

    Some(DepthFrameData {
        width: frame.width,
        height: frame.height,
        depth_mm,
        timestamp: frame.timestamp,
    })
}

/// Check if native depth camera backend can be used
///
/// Returns true if:
/// - A depth camera device is connected
/// - We can likely unbind the kernel driver
pub fn can_use_native_backend() -> bool {
    match freedepth::enumerate_devices() {
        Ok(devices) => !devices.is_empty(),
        Err(_) => false,
    }
}

// Re-export rgb_to_rgba from centralized format converters
pub use super::format_converters::rgb_to_rgba;

impl CameraBackend for NativeDepthBackend {
    fn enumerate_cameras(&self) -> Vec<CameraDevice> {
        enumerate_depth_cameras()
    }

    fn get_formats(&self, device: &CameraDevice, _video_mode: bool) -> Vec<CameraFormat> {
        get_depth_formats(device)
    }

    fn initialize(&mut self, device: &CameraDevice, format: &CameraFormat) -> BackendResult<()> {
        info!(
            device = %device.name,
            format = %format,
            "Initializing native depth camera backend"
        );

        // Shutdown any existing stream
        if self.is_running() {
            self.stop();
        }

        // Extract device index from path
        let device_index = depth_device_index(&device.path).ok_or_else(|| {
            BackendError::DeviceNotFound(format!(
                "Invalid depth camera device path: {}",
                device.path
            ))
        })?;

        // Map FourCC pixel_format to freedepth VideoFormat and Resolution
        // Resolution is determined by width: 640 = Medium, 1280 = High
        let resolution = if format.width >= 1280 {
            Resolution::High
        } else {
            Resolution::Medium
        };

        let (video_format, depth_only) = match format.pixel_format.as_str() {
            // Bayer raw (requires demosaicing)
            "GRBG" => (VideoFormat::Bayer, false),
            // RGB demosaiced (same as Bayer at hardware level, demosaiced in software)
            "RGB3" => (VideoFormat::Rgb, false),
            // YUV UYVY (ISP processed) - only Medium resolution supported
            "UYVY" => {
                if resolution == Resolution::High {
                    return Err(BackendError::FormatNotSupported(
                        "YUV format only supports 640x480 @ 15fps".to_string(),
                    ));
                }
                (VideoFormat::YuvRaw, false)
            }
            // IR 10-bit packed
            "IR10" => (VideoFormat::Ir10BitPacked, false),
            // Depth-only mode (use Bayer internally but display depth visualization)
            "Y10B" => (VideoFormat::Bayer, true),
            _ => {
                return Err(BackendError::FormatNotSupported(format!(
                    "Unknown depth camera format: {} at {}x{}",
                    format.pixel_format, format.width, format.height
                )));
            }
        };

        // Set depth-only mode flag
        self.depth_only_mode.store(depth_only, Ordering::SeqCst);

        // Store device and format
        self.current_device = Some(device.clone());
        self.current_format = Some(format.clone());

        // Start streaming with selected format and resolution
        self.start_with_format(device_index, video_format, resolution)
            .map_err(|e| {
                BackendError::InitializationFailed(format!(
                    "Failed to start depth camera streaming: {}",
                    e
                ))
            })?;

        info!("Native depth camera backend initialized successfully");
        Ok(())
    }

    fn shutdown(&mut self) -> BackendResult<()> {
        info!("Shutting down native depth camera backend");
        self.stop();
        self.current_device = None;
        self.current_format = None;
        Ok(())
    }

    fn is_initialized(&self) -> bool {
        self.is_running() && self.current_device.is_some()
    }

    fn recover(&mut self) -> BackendResult<()> {
        info!("Attempting to recover native depth camera backend");
        let device = self
            .current_device
            .clone()
            .ok_or_else(|| BackendError::Other("No device to recover".to_string()))?;
        let format = self
            .current_format
            .clone()
            .ok_or_else(|| BackendError::Other("No format to recover".to_string()))?;

        self.stop();
        self.initialize(&device, &format)
    }

    fn switch_camera(&mut self, device: &CameraDevice) -> BackendResult<()> {
        info!(device = %device.name, "Switching to new depth camera device");

        let formats = self.get_formats(device, false);
        if formats.is_empty() {
            return Err(BackendError::FormatNotSupported(
                "No formats available for device".to_string(),
            ));
        }

        let format = formats
            .first()
            .cloned()
            .ok_or_else(|| BackendError::Other("Failed to select format".to_string()))?;

        self.initialize(device, &format)
    }

    fn apply_format(&mut self, format: &CameraFormat) -> BackendResult<()> {
        info!(format = %format, "Applying new format to depth camera");

        let device = self
            .current_device
            .clone()
            .ok_or_else(|| BackendError::Other("No active device".to_string()))?;

        self.initialize(&device, format)
    }

    fn capture_photo(&self) -> BackendResult<CameraFrame> {
        debug!("Capturing photo from native depth camera backend");

        // Get the latest video frame
        let video_frame = self
            .get_video_frame()
            .ok_or_else(|| BackendError::Other("No video frame available".to_string()))?;

        // Convert RGB to RGBA
        let rgba_data = rgb_to_rgba(&video_frame.data);

        // Get depth frame and its dimensions
        let depth_frame = self.get_depth_frame();
        let (depth_width, depth_height, depth_data) = match &depth_frame {
            Some(d) => (
                d.width,
                d.height,
                Some(Arc::from(d.depth_mm.clone().into_boxed_slice())),
            ),
            None => (0, 0, None),
        };

        Ok(CameraFrame {
            width: video_frame.width,
            height: video_frame.height,
            data: Arc::from(rgba_data.into_boxed_slice()),
            format: PixelFormat::RGBA,
            stride: video_frame.width * 4,
            captured_at: Instant::now(),
            depth_data,
            depth_width,
            depth_height,
            video_timestamp: Some(video_frame.timestamp),
        })
    }

    fn start_recording(&mut self, _output_path: PathBuf) -> BackendResult<()> {
        // Video recording not yet implemented for native depth cameras
        Err(BackendError::Other(
            "Video recording not yet implemented for native depth camera backend".to_string(),
        ))
    }

    fn stop_recording(&mut self) -> BackendResult<PathBuf> {
        Err(BackendError::NoRecordingInProgress)
    }

    fn is_recording(&self) -> bool {
        false
    }

    fn get_preview_receiver(&self) -> Option<FrameReceiver> {
        // Preview is handled via polling (get_video_frame)
        None
    }

    fn backend_type(&self) -> CameraBackendType {
        CameraBackendType::PipeWire // Report as PipeWire for compatibility
    }

    fn is_available(&self) -> bool {
        can_use_native_backend()
    }

    fn current_device(&self) -> Option<&CameraDevice> {
        self.current_device.as_ref()
    }

    fn current_format(&self) -> Option<&CameraFormat> {
        self.current_format.as_ref()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_backend_creation() {
        let backend = NativeDepthBackend::new();
        assert!(!backend.is_running());
    }
}
