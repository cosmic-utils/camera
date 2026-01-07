// SPDX-License-Identifier: GPL-3.0-only

//! V4L2 Kernel Depth Camera Backend
//!
//! This backend uses the V4L2 kernel driver with depth control extensions
//! for depth camera streaming. It's preferred over the freedepth userspace
//! library when the kernel driver is available.
//!
//! # Detection
//!
//! The kernel driver is detected by checking for V4L2_CID_DEPTH_SENSOR_TYPE
//! control on the device. If present, this backend is used; otherwise,
//! we fall back to freedepth.
//!
//! # Advantages over freedepth
//!
//! - No need to unbind/rebind kernel driver
//! - Better integration with V4L2 ecosystem
//! - Calibration data exposed via standard V4L2 controls
//! - Works with standard V4L2 tools (v4l2-ctl, etc.)

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::Instant;

use tracing::{debug, error, info, warn};
use v4l::buffer::Type;
use v4l::io::mmap::Stream;
use v4l::io::traits::CaptureStream;
use v4l::prelude::*;
use v4l::video::Capture;
use v4l::{Format, FourCC};

use super::format_converters::{
    self, grbg_to_rgba, unpack_y10b, uyvy_to_rgba, DepthVisualizationOptions,
};
use super::types::*;
use super::v4l2_depth_controls::{
    self, DepthCapabilities, DepthIntrinsics, KernelRegistrationData, V4l2DeviceInfo,
};
use super::CameraBackend;

/// Path prefix for kernel depth camera devices (single depth device)
pub const KERNEL_DEPTH_PREFIX: &str = "v4l2-depth:";

/// Path prefix for kernel Kinect paired devices (color:depth)
pub const KERNEL_KINECT_PREFIX: &str = "v4l2-kinect:";

/// A paired Kinect device (color + depth) from kernel driver
///
/// The kernel Kinect driver creates two separate V4L2 video devices:
/// - Color camera (SGRBG8/UYVY formats)
/// - Depth camera (Y10B/Y16 formats, with depth controls)
///
/// They share the same `bus_info` (e.g., "usb-1.11") which allows pairing.
#[derive(Debug, Clone)]
pub struct KinectDevicePair {
    /// Path to color camera device (e.g., /dev/video4)
    pub color_path: String,
    /// Path to depth camera device (e.g., /dev/video5)
    pub depth_path: String,
    /// USB bus info shared by both devices (e.g., "usb-0000:00:14.0-11")
    pub bus_info: String,
    /// Card name (e.g., "Kinect")
    pub card_name: String,
}

/// Device type detected during scanning
#[derive(Debug, Clone)]
enum KinectDeviceType {
    /// Color camera (has Bayer/UYVY formats, no depth controls)
    Color,
    /// Depth camera (has Y10B/Y16 formats and depth controls)
    Depth,
}

/// Candidate device found during scanning
#[derive(Debug)]
struct KinectCandidate {
    path: String,
    device_type: KinectDeviceType,
    info: V4l2DeviceInfo,
}

/// Find Kinect device pairs from the kernel driver
///
/// Scans /dev/video* devices for the Kinect kernel driver signature:
/// - Devices with V4L2_CID_DEPTH_SENSOR_TYPE control are depth devices
/// - Devices with "kinect" driver and color formats are color devices
/// - Devices are paired by matching `bus_info`
///
/// Returns a list of paired devices. Each pair represents one physical Kinect.
pub fn find_kernel_kinect_pairs() -> Vec<KinectDevicePair> {
    use std::collections::HashMap;
    use v4l::prelude::*;

    let mut candidates: Vec<KinectCandidate> = Vec::new();

    // Scan /dev/video* devices
    let entries: Vec<_> = std::fs::read_dir("/dev")
        .into_iter()
        .flatten()
        .flatten()
        .filter(|e| {
            e.path()
                .file_name()
                .and_then(|n| n.to_str())
                .map(|n| n.starts_with("video"))
                .unwrap_or(false)
        })
        .collect();

    for entry in entries {
        let path = entry.path();
        let path_str = path.to_string_lossy().to_string();

        // Get device info via QUERYCAP
        let Some(info) = v4l2_depth_controls::query_device_info(&path_str) else {
            continue;
        };

        // Check if this is a Kinect driver device
        if info.driver != "kinect" {
            continue;
        }

        // Determine device type by checking video formats
        // Note: Both devices may have depth controls, so we MUST check formats first
        // - Depth camera: has Y10B/Y16 formats
        // - Color camera: has GRBG/UYVY formats
        let device_type = if let Ok(dev) = Device::with_path(&path) {
            let formats: Vec<_> = dev.enum_formats().into_iter().flatten().collect();

            // FourCC constants for comparison
            let fourcc_y10b = FourCC::new(b"Y10B");
            let fourcc_y16 = FourCC::new(b"Y16 ");
            let fourcc_grbg = FourCC::new(b"GRBG");
            let fourcc_uyvy = FourCC::new(b"UYVY");

            // Check for depth formats (Y10B, Y16)
            let has_depth_format = formats.iter().any(|f| {
                f.fourcc == fourcc_y10b || f.fourcc == fourcc_y16
            });

            // Check for color formats (Bayer GRBG or UYVY)
            let has_color_format = formats.iter().any(|f| {
                f.fourcc == fourcc_grbg || f.fourcc == fourcc_uyvy
            });

            if has_depth_format {
                KinectDeviceType::Depth
            } else if has_color_format {
                KinectDeviceType::Color
            } else {
                debug!(path = %path_str, "Unknown Kinect device type - no recognized formats");
                continue;
            }
        } else {
            continue;
        };

        debug!(
            path = %path_str,
            device_type = ?device_type,
            bus_info = %info.bus_info,
            card = %info.card,
            "Found Kinect kernel driver device"
        );

        candidates.push(KinectCandidate {
            path: path_str,
            device_type,
            info,
        });
    }

    // Group by bus_info to find pairs
    let mut by_bus: HashMap<String, Vec<KinectCandidate>> = HashMap::new();
    for candidate in candidates {
        by_bus
            .entry(candidate.info.bus_info.clone())
            .or_default()
            .push(candidate);
    }

    // Build pairs from grouped devices
    let mut pairs = Vec::new();
    for (bus_info, devices) in by_bus {
        let mut color_path: Option<String> = None;
        let mut depth_path: Option<String> = None;
        let mut card_name = String::from("Kinect");

        for dev in devices {
            match dev.device_type {
                KinectDeviceType::Color => {
                    color_path = Some(dev.path);
                    card_name = dev.info.card.clone();
                }
                KinectDeviceType::Depth => {
                    depth_path = Some(dev.path);
                }
            }
        }

        // Only create a pair if we have both color and depth
        if let (Some(color), Some(depth)) = (color_path, depth_path) {
            info!(
                color_path = %color,
                depth_path = %depth,
                bus_info = %bus_info,
                "Found Kinect device pair"
            );
            pairs.push(KinectDevicePair {
                color_path: color,
                depth_path: depth,
                bus_info,
                card_name,
            });
        }
    }

    pairs
}

/// Check if there are any Kinect kernel driver devices available
pub fn has_kernel_kinect_devices() -> bool {
    !find_kernel_kinect_pairs().is_empty()
}


/// V4L2 Kernel Depth Backend
///
/// Uses the V4L2 kernel driver with depth control extensions for
/// depth camera streaming. Supports dual-stream capture (color + depth)
/// when initialized with a device pair.
pub struct V4l2KernelDepthBackend {
    /// Current device (combined representation)
    device: Option<CameraDevice>,
    /// Current format
    format: Option<CameraFormat>,
    /// Depth capabilities from V4L2 controls
    capabilities: Option<DepthCapabilities>,
    /// Registration data from kernel calibration (for depth-RGB alignment)
    registration_data: Option<KernelRegistrationData>,
    /// Device pair (if using kernel driver with color + depth)
    device_pair: Option<KinectDevicePair>,
    /// Depth camera V4L2 path
    depth_v4l2_path: Option<String>,
    /// Color camera V4L2 path
    color_v4l2_path: Option<String>,
    /// Depth capture thread handle
    depth_capture_thread: Option<JoinHandle<()>>,
    /// Color capture thread handle
    color_capture_thread: Option<JoinHandle<()>>,
    /// Signal to stop capture
    stop_signal: Arc<AtomicBool>,
    /// Latest combined frame (color + depth visualization)
    latest_frame: Arc<Mutex<Option<CameraFrame>>>,
    /// Latest depth frame data
    latest_depth_frame: Arc<Mutex<Option<DepthFrameData>>>,
    /// Latest color frame data
    latest_color_frame: Arc<Mutex<Option<VideoFrameData>>>,
    /// Frame receiver for preview
    frame_receiver: Option<FrameReceiver>,
    /// Frame sender for preview
    frame_sender: Option<FrameSender>,
}

impl V4l2KernelDepthBackend {
    /// Create a new V4L2 kernel depth backend
    pub fn new() -> Self {
        Self {
            device: None,
            format: None,
            capabilities: None,
            registration_data: None,
            device_pair: None,
            depth_v4l2_path: None,
            color_v4l2_path: None,
            depth_capture_thread: None,
            color_capture_thread: None,
            stop_signal: Arc::new(AtomicBool::new(false)),
            latest_frame: Arc::new(Mutex::new(None)),
            latest_depth_frame: Arc::new(Mutex::new(None)),
            latest_color_frame: Arc::new(Mutex::new(None)),
            frame_receiver: None,
            frame_sender: None,
        }
    }

    /// Check if a V4L2 device has kernel depth driver support
    pub fn has_kernel_depth_support(v4l2_path: &str) -> bool {
        v4l2_depth_controls::has_depth_controls(v4l2_path)
    }

    /// Get depth capabilities for a device
    pub fn get_capabilities(&self) -> Option<&DepthCapabilities> {
        self.capabilities.as_ref()
    }

    /// Get depth intrinsics (camera calibration)
    pub fn get_intrinsics(&self) -> Option<&DepthIntrinsics> {
        self.capabilities.as_ref()?.intrinsics.as_ref()
    }

    /// Get registration data (for depth-to-RGB alignment)
    ///
    /// Built from kernel intrinsics/extrinsics during initialization.
    /// Returns None if calibration data was not available from the kernel.
    pub fn get_registration_data(&self) -> Option<&KernelRegistrationData> {
        self.registration_data.as_ref()
    }

    /// Get registration data converted to shader format
    pub fn get_shader_registration_data(&self) -> Option<crate::shaders::RegistrationData> {
        self.registration_data.as_ref().map(|r| r.to_shader_format())
    }

    /// Get the device pair if available
    pub fn get_device_pair(&self) -> Option<&KinectDevicePair> {
        self.device_pair.as_ref()
    }

    /// Get latest depth frame data
    pub fn get_latest_depth(&self) -> Option<DepthFrameData> {
        self.latest_depth_frame.lock().ok()?.clone()
    }

    /// Get latest color frame data
    pub fn get_latest_color(&self) -> Option<VideoFrameData> {
        self.latest_color_frame.lock().ok()?.clone()
    }

    /// Get the latest frame for preview (polling method)
    pub fn get_frame(&self) -> Option<CameraFrame> {
        self.latest_frame.lock().ok()?.clone()
    }

    /// Initialize from a device pair (color + depth)
    pub fn initialize_from_pair(&mut self, pair: &KinectDevicePair) -> BackendResult<()> {
        info!(
            color_path = %pair.color_path,
            depth_path = %pair.depth_path,
            "Initializing kernel depth backend from device pair"
        );

        // Query depth capabilities from depth device
        let capabilities = v4l2_depth_controls::query_depth_capabilities(&pair.depth_path);

        // Build registration data from kernel calibration
        let registration_data = if let Some(ref caps) = capabilities {
            info!(
                sensor_type = ?caps.sensor_type,
                min_distance = caps.min_distance_mm,
                max_distance = caps.max_distance_mm,
                invalid_value = caps.invalid_value,
                has_intrinsics = caps.intrinsics.is_some(),
                has_extrinsics = caps.extrinsics.is_some(),
                "Depth camera capabilities"
            );

            // Build registration from intrinsics/extrinsics if available
            caps.intrinsics.as_ref().map(|intrinsics| {
                KernelRegistrationData::from_kernel_calibration(
                    intrinsics,
                    caps.extrinsics.as_ref(),
                )
            })
        } else {
            None
        };

        // Create combined device representation
        let device = CameraDevice {
            name: format!("{} (Kernel)", pair.card_name),
            path: format!("{}{}:{}", KERNEL_KINECT_PREFIX, pair.color_path, pair.depth_path),
            metadata_path: Some(pair.depth_path.clone()),
            device_info: Some(DeviceInfo {
                card: pair.card_name.clone(),
                driver: "kinect".to_string(),
                path: pair.color_path.clone(),
                real_path: pair.depth_path.clone(),
            }),
        };

        // Default format (640x480 depth)
        let format = CameraFormat {
            width: 640,
            height: 480,
            framerate: Some(30),
            hardware_accelerated: true,
            pixel_format: "Y10B".to_string(),
            sensor_type: SensorType::Depth,
        };

        self.device = Some(device);
        self.format = Some(format.clone());
        self.capabilities = capabilities;
        self.registration_data = registration_data;
        self.device_pair = Some(pair.clone());
        self.depth_v4l2_path = Some(pair.depth_path.clone());
        self.color_v4l2_path = Some(pair.color_path.clone());

        // Start dual capture
        self.start_dual_capture(pair, &format)?;

        info!("Kernel depth backend initialized with device pair");
        Ok(())
    }

    /// Start dual capture (color + depth streams)
    fn start_dual_capture(&mut self, pair: &KinectDevicePair, format: &CameraFormat) -> BackendResult<()> {
        let stop_signal = self.stop_signal.clone();
        let latest_frame = self.latest_frame.clone();
        let latest_depth = self.latest_depth_frame.clone();
        let latest_color = self.latest_color_frame.clone();

        // Create frame channel
        let (sender, receiver) = cosmic::iced::futures::channel::mpsc::channel(30);
        self.frame_sender = Some(sender.clone());
        self.frame_receiver = Some(receiver);

        // Get capabilities for depth processing
        let capabilities = self.capabilities.clone();

        // Start depth capture thread
        let depth_path = pair.depth_path.clone();
        let depth_width = format.width;
        let depth_height = format.height;
        let depth_stop = stop_signal.clone();
        let depth_latest = latest_depth.clone();
        let depth_frame_latest = latest_frame.clone();
        let depth_sender = sender.clone();
        let depth_caps = capabilities.clone();

        let depth_handle = thread::spawn(move || {
            if let Err(e) = depth_capture_loop(
                &depth_path,
                depth_width,
                depth_height,
                depth_stop,
                depth_latest,
                depth_frame_latest,
                depth_sender,
                depth_caps,
            ) {
                error!(error = %e, "Depth capture loop error");
            }
        });

        self.depth_capture_thread = Some(depth_handle);

        // Start color capture thread
        let color_path = pair.color_path.clone();
        let color_stop = stop_signal.clone();
        let color_latest = latest_color.clone();

        let color_handle = thread::spawn(move || {
            if let Err(e) = color_capture_loop(
                &color_path,
                color_stop,
                color_latest,
            ) {
                error!(error = %e, "Color capture loop error");
            }
        });

        self.color_capture_thread = Some(color_handle);

        info!("Dual capture started (color + depth)");
        Ok(())
    }

    /// Start single depth capture (legacy mode)
    fn start_capture(&mut self, v4l2_path: &str, format: &CameraFormat) -> BackendResult<()> {
        let path = v4l2_path.to_string();
        let width = format.width;
        let height = format.height;
        let stop_signal = self.stop_signal.clone();
        let latest_frame = self.latest_frame.clone();
        let latest_depth = self.latest_depth_frame.clone();

        // Create frame channel
        let (sender, receiver) = cosmic::iced::futures::channel::mpsc::channel(30);
        self.frame_sender = Some(sender.clone());
        self.frame_receiver = Some(receiver);

        // Get capabilities for depth processing
        let capabilities = self.capabilities.clone();

        let handle = thread::spawn(move || {
            if let Err(e) = depth_capture_loop(
                &path,
                width,
                height,
                stop_signal,
                latest_depth,
                latest_frame,
                sender,
                capabilities,
            ) {
                error!(error = %e, "Capture loop error");
            }
        });

        self.depth_capture_thread = Some(handle);
        Ok(())
    }

    /// Stop all capture threads
    fn stop_capture(&mut self) {
        self.stop_signal.store(true, Ordering::SeqCst);

        if let Some(handle) = self.depth_capture_thread.take() {
            let _ = handle.join();
        }

        if let Some(handle) = self.color_capture_thread.take() {
            let _ = handle.join();
        }

        self.stop_signal.store(false, Ordering::SeqCst);
        self.frame_sender = None;
        self.frame_receiver = None;
    }
}

/// Depth capture loop running in a separate thread
fn depth_capture_loop(
    path: &str,
    width: u32,
    height: u32,
    stop_signal: Arc<AtomicBool>,
    latest_depth: Arc<Mutex<Option<DepthFrameData>>>,
    latest_frame: Arc<Mutex<Option<CameraFrame>>>,
    mut sender: FrameSender,
    capabilities: Option<DepthCapabilities>,
) -> Result<(), String> {
    info!(path, width, height, "Starting V4L2 kernel depth capture");

    // Open V4L2 device
    let dev = Device::with_path(path).map_err(|e| format!("Failed to open device: {}", e))?;

    // Set format - try Y10B first (11-bit packed), fall back to Y16
    let fourcc_y10b = FourCC::new(b"Y10B");
    let fourcc_y16 = FourCC::new(b"Y16 ");

    let format = Format::new(width, height, fourcc_y10b);
    let actual_format = match dev.set_format(&format) {
        Ok(f) => f,
        Err(_) => {
            // Try Y16 as fallback
            let format = Format::new(width, height, fourcc_y16);
            dev.set_format(&format)
                .map_err(|e| format!("Failed to set format: {}", e))?
        }
    };

    info!(
        width = actual_format.width,
        height = actual_format.height,
        fourcc = ?actual_format.fourcc,
        "V4L2 depth format configured"
    );

    // Create memory-mapped stream
    let mut stream =
        Stream::with_buffers(&dev, Type::VideoCapture, 4)
            .map_err(|e| format!("Failed to create stream: {}", e))?;

    // Get invalid depth value from capabilities
    let invalid_value = capabilities
        .as_ref()
        .map(|c| c.invalid_value as u16)
        .unwrap_or(2047);

    // Configure depth visualization (auto-range grayscale for kernel backend)
    let viz_options = DepthVisualizationOptions {
        grayscale: true,
        invalid_value,
        ..DepthVisualizationOptions::auto_range()
    };

    info!(invalid_value, "Depth capture loop started");

    while !stop_signal.load(Ordering::SeqCst) {
        // Capture frame
        let (buf, meta) = match stream.next() {
            Ok(frame) => frame,
            Err(e) => {
                warn!(error = %e, "Failed to capture depth frame");
                continue;
            }
        };

        let captured_at = Instant::now();
        let timestamp = (meta.timestamp.sec as u32)
            .wrapping_mul(1_000_000)
            .wrapping_add(meta.timestamp.usec as u32);

        // Process depth data
        let depth_data: Vec<u16> = if actual_format.fourcc == FourCC::new(b"Y16 ") {
            // 16-bit depth - direct copy
            buf.chunks_exact(2)
                .map(|c| u16::from_le_bytes([c[0], c[1]]))
                .collect()
        } else if actual_format.fourcc == FourCC::new(b"Y10B") {
            // 10-bit packed - unpack
            unpack_y10b(buf, width, height)
        } else {
            warn!(fourcc = ?actual_format.fourcc, "Unsupported depth format");
            continue;
        };

        // Update latest depth frame data
        if let Ok(mut guard) = latest_depth.lock() {
            *guard = Some(DepthFrameData {
                width,
                height,
                depth_mm: depth_data.clone(),
                timestamp,
            });
        }

        // Convert depth to RGBA visualization
        let rgba = format_converters::depth_to_rgba(&depth_data, width, height, &viz_options);

        // Create camera frame for preview
        let frame = CameraFrame {
            width,
            height,
            data: Arc::from(rgba.into_boxed_slice()),
            format: PixelFormat::Depth16,
            stride: width * 4,
            captured_at,
            depth_data: Some(Arc::from(depth_data.into_boxed_slice())),
            depth_width: width,
            depth_height: height,
            video_timestamp: Some(timestamp),
        };

        // Update latest frame
        if let Ok(mut guard) = latest_frame.lock() {
            *guard = Some(frame.clone());
        }

        // Send to channel
        if sender.try_send(frame).is_err() {
            debug!("Frame channel full, dropping frame");
        }
    }

    info!("Depth capture loop stopped");
    Ok(())
}

/// Color capture loop running in a separate thread
fn color_capture_loop(
    path: &str,
    stop_signal: Arc<AtomicBool>,
    latest_color: Arc<Mutex<Option<VideoFrameData>>>,
) -> Result<(), String> {
    info!(path, "Starting V4L2 kernel color capture");

    // Open V4L2 device
    let dev = Device::with_path(path).map_err(|e| format!("Failed to open device: {}", e))?;

    // Try to set UYVY format at 640x480 (preferred for color)
    let fourcc_uyvy = FourCC::new(b"UYVY");
    let fourcc_grbg = FourCC::new(b"GRBG");

    let format = Format::new(640, 480, fourcc_uyvy);
    let actual_format = match dev.set_format(&format) {
        Ok(f) => f,
        Err(_) => {
            // Try Bayer GRBG as fallback
            let format = Format::new(640, 480, fourcc_grbg);
            dev.set_format(&format)
                .map_err(|e| format!("Failed to set color format: {}", e))?
        }
    };

    let width = actual_format.width;
    let height = actual_format.height;

    info!(
        width,
        height,
        fourcc = ?actual_format.fourcc,
        "V4L2 color format configured"
    );

    // Create memory-mapped stream
    let mut stream =
        Stream::with_buffers(&dev, Type::VideoCapture, 4)
            .map_err(|e| format!("Failed to create color stream: {}", e))?;

    info!("Color capture loop started");

    while !stop_signal.load(Ordering::SeqCst) {
        // Capture frame
        let (buf, meta) = match stream.next() {
            Ok(frame) => frame,
            Err(e) => {
                warn!(error = %e, "Failed to capture color frame");
                continue;
            }
        };

        let timestamp = (meta.timestamp.sec as u32)
            .wrapping_mul(1_000_000)
            .wrapping_add(meta.timestamp.usec as u32);

        // Convert to RGBA based on format
        let rgba = if actual_format.fourcc == FourCC::new(b"UYVY") {
            uyvy_to_rgba(buf, width, height)
        } else if actual_format.fourcc == FourCC::new(b"GRBG") {
            grbg_to_rgba(buf, width, height)
        } else {
            warn!(fourcc = ?actual_format.fourcc, "Unsupported color format");
            continue;
        };

        // Update latest color frame
        if let Ok(mut guard) = latest_color.lock() {
            *guard = Some(VideoFrameData {
                width,
                height,
                data: rgba,
                timestamp,
            });
        }
    }

    info!("Color capture loop stopped");
    Ok(())
}

impl Default for V4l2KernelDepthBackend {
    fn default() -> Self {
        Self::new()
    }
}

impl CameraBackend for V4l2KernelDepthBackend {
    fn enumerate_cameras(&self) -> Vec<CameraDevice> {
        let mut cameras = Vec::new();

        // First, look for paired Kinect devices (color + depth from kernel driver)
        let pairs = find_kernel_kinect_pairs();
        for pair in pairs {
            let device = CameraDevice {
                name: format!("{} (Kernel)", pair.card_name),
                path: format!("{}{}:{}", KERNEL_KINECT_PREFIX, pair.color_path, pair.depth_path),
                metadata_path: Some(pair.depth_path.clone()),
                device_info: Some(DeviceInfo {
                    card: pair.card_name.clone(),
                    driver: "kinect".to_string(),
                    path: pair.color_path.clone(),
                    real_path: pair.depth_path.clone(),
                }),
            };

            info!(
                name = %device.name,
                path = %device.path,
                color = %pair.color_path,
                depth = %pair.depth_path,
                "Found kernel Kinect device pair"
            );

            cameras.push(device);
        }

        // If we found paired devices, don't scan for individual depth devices
        // (they're already included in pairs)
        if !cameras.is_empty() {
            return cameras;
        }

        // Fallback: scan for individual depth devices (non-paired)
        for entry in std::fs::read_dir("/dev").into_iter().flatten() {
            if let Ok(entry) = entry {
                let path = entry.path();
                if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
                    if name.starts_with("video") {
                        let path_str = path.to_string_lossy();
                        if Self::has_kernel_depth_support(&path_str) {
                            // Get device info
                            if let Ok(dev) = Device::with_path(&path) {
                                if let Ok(caps) = dev.query_caps() {
                                    let device = CameraDevice {
                                        name: caps.card.clone(),
                                        path: format!("{}{}", KERNEL_DEPTH_PREFIX, path_str),
                                        metadata_path: Some(path_str.to_string()),
                                        device_info: Some(DeviceInfo {
                                            card: caps.card.clone(),
                                            driver: caps.driver.clone(),
                                            path: path_str.to_string(),
                                            real_path: path_str.to_string(),
                                        }),
                                    };

                                    info!(
                                        name = %device.name,
                                        path = %device.path,
                                        "Found kernel depth camera"
                                    );

                                    cameras.push(device);
                                }
                            }
                        }
                    }
                }
            }
        }

        cameras
    }

    fn get_formats(&self, device: &CameraDevice, _video_mode: bool) -> Vec<CameraFormat> {
        let v4l2_path = device
            .path
            .strip_prefix(KERNEL_DEPTH_PREFIX)
            .unwrap_or(&device.path);

        let dev = match Device::with_path(v4l2_path) {
            Ok(d) => d,
            Err(_) => return Vec::new(),
        };

        let mut formats = Vec::new();

        // Query supported formats
        if let Ok(format_iter) = dev.enum_formats() {
            for fmt_desc in format_iter {
                if let Ok(frame_sizes) = dev.enum_framesizes(fmt_desc.fourcc) {
                    for size in frame_sizes {
                        match size.size {
                            v4l::framesize::FrameSizeEnum::Discrete(discrete) => {
                                // Query framerates
                                if let Ok(intervals) = dev.enum_frameintervals(
                                    fmt_desc.fourcc,
                                    discrete.width,
                                    discrete.height,
                                ) {
                                    for interval in intervals {
                                        let fps = match interval.interval {
                                            v4l::frameinterval::FrameIntervalEnum::Discrete(
                                                frac,
                                            ) => {
                                                if frac.numerator > 0 {
                                                    Some(frac.denominator / frac.numerator)
                                                } else {
                                                    Some(30)
                                                }
                                            }
                                            _ => Some(30),
                                        };

                                        formats.push(CameraFormat {
                                            width: discrete.width,
                                            height: discrete.height,
                                            framerate: fps,
                                            hardware_accelerated: true,
                                            pixel_format: format!("{:?}", fmt_desc.fourcc),
                                            sensor_type: SensorType::Depth,
                                        });
                                    }
                                }
                            }
                            v4l::framesize::FrameSizeEnum::Stepwise(step) => {
                                // Add common resolutions
                                for (w, h) in [(640, 480), (320, 240)] {
                                    if w >= step.min_width
                                        && w <= step.max_width
                                        && h >= step.min_height
                                        && h <= step.max_height
                                    {
                                        formats.push(CameraFormat {
                                            width: w,
                                            height: h,
                                            framerate: Some(30),
                                            hardware_accelerated: true,
                                            pixel_format: format!("{:?}", fmt_desc.fourcc),
                                            sensor_type: SensorType::Depth,
                                        });
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }

        formats
    }

    fn initialize(&mut self, device: &CameraDevice, format: &CameraFormat) -> BackendResult<()> {
        info!(
            device = %device.name,
            format = %format,
            "Initializing V4L2 kernel depth backend"
        );

        // Check if this is a paired device (v4l2-kinect:color:depth format)
        if let Some(paths) = device.path.strip_prefix(KERNEL_KINECT_PREFIX) {
            // Parse color:depth paths
            let parts: Vec<&str> = paths.split(':').collect();
            if parts.len() == 2 {
                let pair = KinectDevicePair {
                    color_path: parts[0].to_string(),
                    depth_path: parts[1].to_string(),
                    bus_info: String::new(), // Not needed for initialization
                    card_name: device.name.clone(),
                };
                return self.initialize_from_pair(&pair);
            }
        }

        // Single depth device path (v4l2-depth:/dev/videoX format)
        let v4l2_path = device
            .path
            .strip_prefix(KERNEL_DEPTH_PREFIX)
            .unwrap_or(&device.path)
            .to_string();

        // Query depth capabilities
        let capabilities = v4l2_depth_controls::query_depth_capabilities(&v4l2_path);

        if let Some(ref caps) = capabilities {
            info!(
                sensor_type = ?caps.sensor_type,
                min_distance = caps.min_distance_mm,
                max_distance = caps.max_distance_mm,
                invalid_value = caps.invalid_value,
                "Depth camera capabilities"
            );
        }

        self.device = Some(device.clone());
        self.format = Some(format.clone());
        self.capabilities = capabilities;
        self.depth_v4l2_path = Some(v4l2_path.clone());

        // Start single depth capture
        self.start_capture(&v4l2_path, format)?;

        info!("V4L2 kernel depth backend initialized");
        Ok(())
    }

    fn shutdown(&mut self) -> BackendResult<()> {
        info!("Shutting down V4L2 kernel depth backend");

        self.stop_capture();

        self.device = None;
        self.format = None;
        self.capabilities = None;
        self.device_pair = None;
        self.depth_v4l2_path = None;
        self.color_v4l2_path = None;

        // Clear frame data
        if let Ok(mut guard) = self.latest_depth_frame.lock() {
            *guard = None;
        }
        if let Ok(mut guard) = self.latest_color_frame.lock() {
            *guard = None;
        }

        Ok(())
    }

    fn is_initialized(&self) -> bool {
        self.device.is_some() && self.depth_capture_thread.is_some()
    }

    fn recover(&mut self) -> BackendResult<()> {
        let device = self
            .device
            .clone()
            .ok_or_else(|| BackendError::Other("No device to recover".to_string()))?;
        let format = self
            .format
            .clone()
            .ok_or_else(|| BackendError::Other("No format to recover".to_string()))?;

        self.shutdown()?;
        self.initialize(&device, &format)
    }

    fn switch_camera(&mut self, device: &CameraDevice) -> BackendResult<()> {
        let formats = self.get_formats(device, false);
        let format = formats
            .first()
            .cloned()
            .ok_or_else(|| BackendError::FormatNotSupported("No formats available".to_string()))?;

        self.initialize(device, &format)
    }

    fn apply_format(&mut self, format: &CameraFormat) -> BackendResult<()> {
        let device = self
            .device
            .clone()
            .ok_or_else(|| BackendError::Other("No active device".to_string()))?;

        self.initialize(&device, format)
    }

    fn capture_photo(&self) -> BackendResult<CameraFrame> {
        self.latest_frame
            .lock()
            .ok()
            .and_then(|guard| guard.clone())
            .ok_or_else(|| BackendError::Other("No frame available".to_string()))
    }

    fn start_recording(&mut self, _output_path: PathBuf) -> BackendResult<()> {
        Err(BackendError::Other(
            "Recording not yet implemented for kernel depth backend".to_string(),
        ))
    }

    fn stop_recording(&mut self) -> BackendResult<PathBuf> {
        Err(BackendError::NoRecordingInProgress)
    }

    fn is_recording(&self) -> bool {
        false
    }

    fn get_preview_receiver(&self) -> Option<FrameReceiver> {
        None // Use subscription model instead
    }

    fn backend_type(&self) -> CameraBackendType {
        CameraBackendType::PipeWire // Reuse existing type
    }

    fn is_available(&self) -> bool {
        // Check if any device has kernel depth support
        for entry in std::fs::read_dir("/dev").into_iter().flatten() {
            if let Ok(entry) = entry {
                let path = entry.path();
                if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
                    if name.starts_with("video") {
                        if Self::has_kernel_depth_support(&path.to_string_lossy()) {
                            return true;
                        }
                    }
                }
            }
        }
        false
    }

    fn current_device(&self) -> Option<&CameraDevice> {
        self.device.as_ref()
    }

    fn current_format(&self) -> Option<&CameraFormat> {
        self.format.as_ref()
    }
}

/// Check if a device path refers to a kernel depth camera
/// Recognizes both single depth devices (v4l2-depth:) and paired devices (v4l2-kinect:)
pub fn is_kernel_depth_device(path: &str) -> bool {
    path.starts_with(KERNEL_DEPTH_PREFIX) || path.starts_with(KERNEL_KINECT_PREFIX)
}

/// Check if a device path refers to a kernel kinect paired device
pub fn is_kernel_kinect_device(path: &str) -> bool {
    path.starts_with(KERNEL_KINECT_PREFIX)
}

/// Extract the V4L2 device path from a kernel depth path
pub fn kernel_depth_v4l2_path(path: &str) -> Option<&str> {
    path.strip_prefix(KERNEL_DEPTH_PREFIX)
}

/// Parse a kernel kinect device path to get color and depth paths
/// Format: v4l2-kinect:/dev/video4:/dev/video5
pub fn parse_kernel_kinect_path(path: &str) -> Option<(String, String)> {
    let paths = path.strip_prefix(KERNEL_KINECT_PREFIX)?;
    let parts: Vec<&str> = paths.split(':').collect();
    if parts.len() == 2 {
        Some((parts[0].to_string(), parts[1].to_string()))
    } else {
        None
    }
}
