// SPDX-License-Identifier: GPL-3.0-only

//! libcamera backend using native libcamera-rs bindings
//!
//! This backend uses direct libcamera-rs bindings for camera access,
//! providing multi-stream capture capabilities (preview + raw still capture)
//! with per-frame metadata (exposure, gain, colour temperature).
//!
//! # Architecture
//!
//! ```text
//! ┌─────────────────────┐
//! │   LibcameraBackend  │  ← Implements CameraBackend trait
//! └──────────┬──────────┘
//!            │
//!            ▼
//! ┌──────────────────────┐
//! │NativeLibcameraPipeline│  ← Direct libcamera-rs
//! └──────────┬───────────┘
//!            │
//!            ▼
//!   ┌────────┴────────┐
//!   │   libcamera-rs  │  ← Native bindings
//!   │  (CameraManager)│
//!   └─────────────────┘
//! ```
//!
//! # Multi-Stream Capture
//!
//! libcamera supports multiple streams per camera:
//! - ViewFinder stream: ISP-processed preview (e.g., ABGR8888)
//! - Raw stream: Bypasses ISP for full-resolution Bayer capture
//!
//! This allows simultaneous 1080p preview and full-resolution photo capture.

pub(crate) mod native;

pub(crate) use native::{NativeLibcameraPipeline, PipelineSharedState};

use super::CameraBackend;
use super::types::*;
use super::v4l2_utils;
use libcamera::camera_manager::CameraManager;
use libcamera::stream::StreamRole;
use native::pixel_formats::{is_bayer_format, pixel_format_name};
use std::path::PathBuf;
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex};
use tracing::{debug, info, warn};

/// Global lock for CameraManager creation.
///
/// libcamera only allows one CameraManager to exist at a time.
/// This mutex ensures that enumeration, format queries, and pipeline creation
/// don't overlap and trigger the "Multiple CameraManager objects" abort.
static CAMERA_MANAGER_LOCK: Mutex<()> = Mutex::new(());

/// Cached enumeration result, returned when capture is active and
/// a new CameraManager cannot be created.
static CACHED_DEVICES: std::sync::RwLock<Vec<CameraDevice>> = std::sync::RwLock::new(Vec::new());

/// Cached format results per device path, returned when capture is active.
/// Key: device path, Value: formats
static CACHED_FORMATS: std::sync::LazyLock<
    std::sync::RwLock<std::collections::HashMap<String, Vec<CameraFormat>>>,
> = std::sync::LazyLock::new(|| std::sync::RwLock::new(std::collections::HashMap::new()));

/// libcamera backend using native libcamera-rs bindings
///
/// Provides:
/// - Multi-stream capture (preview + raw still-capture simultaneously)
/// - Full libcamera property access (model, location, rotation)
/// - Per-frame metadata (exposure, gain, colour temperature)
/// - Direct buffer control without GStreamer overhead
pub struct LibcameraBackend {
    /// Current device info
    current_device: Option<CameraDevice>,
    /// Current preview format
    current_format: Option<CameraFormat>,
    /// Active native pipeline
    pipeline: Option<NativeLibcameraPipeline>,
    /// Frame receiver for preview stream (given to UI once)
    frame_receiver: Mutex<Option<FrameReceiver>>,
}

impl Default for LibcameraBackend {
    fn default() -> Self {
        Self::new()
    }
}

impl LibcameraBackend {
    /// Create a new libcamera backend
    pub fn new() -> Self {
        Self {
            current_device: None,
            current_format: None,
            pipeline: None,
            frame_receiver: Mutex::new(None),
        }
    }

    /// Check if libcamera is available (cameras detected)
    fn is_libcamera_available() -> bool {
        // If a capture pipeline is active, libcamera is definitely available
        if native::is_capture_active() {
            return true;
        }
        let _lock = CAMERA_MANAGER_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        CameraManager::new()
            .map(|mgr| !mgr.cameras().is_empty())
            .unwrap_or(false)
    }

    /// Get still capture format (highest resolution available, prefer Bayer)
    fn get_still_format(&self, device: &CameraDevice) -> Option<CameraFormat> {
        let formats = self.get_formats(device, false);
        // Prefer Bayer (raw) format at highest resolution for capture
        let bayer_format = formats
            .iter()
            .filter(|f| f.pixel_format.starts_with("Bayer"))
            .max_by_key(|f| f.width * f.height)
            .cloned();
        bayer_format.or_else(|| formats.into_iter().max_by_key(|f| f.width * f.height))
    }
}

/// Build a sysfs device-tree path for a camera property.
fn dt_sysfs_path(camera_path: &str, property: &str) -> String {
    if camera_path.starts_with('/') {
        format!("/sys/firmware/devicetree{}/{}", camera_path, property)
    } else {
        format!("/sys/firmware/devicetree/base/{}/{}", camera_path, property)
    }
}

/// Device tree properties read for a camera.
struct DeviceTreeProperties {
    sensor_model: Option<String>,
    location: Option<String>,
    rotation: SensorRotation,
}

/// Read sensor model, location, and rotation from the device tree in one pass.
///
/// - `compatible`: null-terminated strings; first entry is the sensor model.
/// - `orientation`: big-endian u32 — 0=front, 1=back, 2=external.
/// - `rotation`: big-endian u32 — sensor mounting angle in degrees.
fn read_device_tree_properties(camera_path: &str) -> DeviceTreeProperties {
    // Sensor model from `compatible`
    let sensor_model = {
        let dt_path = dt_sysfs_path(camera_path, "compatible");
        match std::fs::read(&dt_path) {
            Ok(bytes) if !bytes.is_empty() => {
                let end = bytes.iter().position(|&b| b == 0).unwrap_or(bytes.len());
                let model = String::from_utf8_lossy(&bytes[..end]).trim().to_string();
                if model.is_empty() {
                    None
                } else {
                    debug!(dt_path = %dt_path, model = %model, "Read sensor model from device tree");
                    Some(model)
                }
            }
            Ok(_) => None,
            Err(e) => {
                debug!(dt_path = %dt_path, error = %e, "Could not read sensor model from device tree");
                None
            }
        }
    };

    // Location from `orientation`
    let location = {
        let dt_path = dt_sysfs_path(camera_path, "orientation");
        match std::fs::read(&dt_path) {
            Ok(bytes) if bytes.len() >= 4 => {
                let value = u32::from_be_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]);
                let loc = match value {
                    0 => "front",
                    1 => "back",
                    _ => "external",
                };
                debug!(dt_path = %dt_path, value, location = loc, "Read orientation from device tree");
                Some(loc.to_string())
            }
            _ => None,
        }
    };

    // Rotation from `rotation`
    let rotation = {
        let dt_path = dt_sysfs_path(camera_path, "rotation");
        match std::fs::read(&dt_path) {
            Ok(bytes) if bytes.len() >= 4 => {
                let degrees = u32::from_be_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]);
                debug!(dt_path = %dt_path, degrees, "Read rotation from device tree");
                SensorRotation::from_degrees_int(degrees as i32)
            }
            _ => SensorRotation::default(),
        }
    };

    DeviceTreeProperties {
        sensor_model,
        location,
        rotation,
    }
}

/// Generate a pretty display name from location (and optionally sensor model)
fn generate_pretty_name(sensor_model: Option<&str>, location: Option<&str>) -> Option<String> {
    match location {
        Some("front") => Some("Front Camera".to_string()),
        Some("back") => Some("Back Camera".to_string()),
        Some("external") => sensor_model.map(|m| m.to_string()),
        _ if sensor_model.is_some() => Some("Camera".to_string()),
        _ => None,
    }
}

/// Prefix non-Bayer-prefixed raw format names with "Bayer".
fn bayer_display_name(format_name: String, is_bayer: bool) -> String {
    if is_bayer && !format_name.starts_with("Bayer") {
        format!("Bayer{}", format_name.to_uppercase())
    } else {
        format_name
    }
}

/// Collect formats from a single stream configuration.
///
/// When `synthesize_intermediate` is true, adds common resolution tiers (1080p, 720p)
/// by scaling the native sensor size. This is only appropriate for ISP-processed
/// (non-Bayer) streams where the hardware can produce any output size.
fn collect_stream_formats(
    config: &libcamera::camera::CameraConfiguration,
    index: usize,
    framerate: Option<Framerate>,
    synthesize_intermediate: bool,
) -> Vec<CameraFormat> {
    let Some(cfg) = config.get(index) else {
        return Vec::new();
    };
    let stream_formats = cfg.formats();
    let pixel_formats = stream_formats.pixel_formats();
    let mut formats = Vec::new();

    for pf in &*pixel_formats {
        let format_name = pixel_format_name(pf);
        let is_bayer = is_bayer_format(pf);
        let display_name = bayer_display_name(format_name, is_bayer);

        let mut max_w = 0u32;
        let mut max_h = 0u32;

        for size in stream_formats.sizes(pf) {
            if size.width * size.height > max_w * max_h {
                max_w = size.width;
                max_h = size.height;
            }
            formats.push(CameraFormat {
                width: size.width,
                height: size.height,
                framerate,
                hardware_accelerated: true,
                pixel_format: display_name.clone(),
            });
        }

        // Synthesize common intermediate resolutions for ISP-processed formats.
        // libcamera's StreamFormats only lists discrete native sizes (e.g., 4656x3496
        // and 640x480), but the ISP can produce any size via downscaling. We add
        // standard resolution tiers by scaling the native size while preserving the
        // sensor's aspect ratio (rounded to even for codec compatibility).
        if synthesize_intermediate && !is_bayer && max_w > 0 && max_h > 0 {
            for &target_h in &[1080u32, 720] {
                if max_h > target_h {
                    let scale = target_h as f64 / max_h as f64;
                    let w = ((max_w as f64 * scale) as u32) & !1;
                    if w > 0 {
                        formats.push(CameraFormat {
                            width: w,
                            height: target_h,
                            framerate,
                            hardware_accelerated: true,
                            pixel_format: display_name.clone(),
                        });
                    }
                }
            }
        }
    }

    formats
}

/// Query formats for a camera using an existing CameraManager reference.
///
/// This avoids creating a new CameraManager and is used both during enumeration
/// (to pre-populate the cache for all cameras) and during direct format queries.
fn query_camera_formats(
    cam: &libcamera::camera::Camera,
    supports_multistream: bool,
    video_mode: bool,
) -> Vec<CameraFormat> {
    let config = match cam.generate_configuration(&[StreamRole::ViewFinder]) {
        Some(config) => config,
        None => return Vec::new(),
    };

    let framerate = if video_mode {
        Some(Framerate::from_int(30))
    } else {
        None
    };

    let mut formats = collect_stream_formats(&config, 0, framerate, true);

    // Also probe raw stream formats if multistream is supported
    if supports_multistream && let Some(raw_config) = cam.generate_configuration(&[StreamRole::Raw])
    {
        formats.extend(collect_stream_formats(&raw_config, 0, framerate, false));
    }

    // Sort by resolution (highest first), then by pixel format (Bayer formats first)
    formats.sort_by(|a, b| {
        (b.width * b.height)
            .cmp(&(a.width * a.height))
            .then_with(|| {
                b.pixel_format
                    .starts_with("Bayer")
                    .cmp(&a.pixel_format.starts_with("Bayer"))
            })
    });
    formats.dedup_by(|a, b| {
        a.width == b.width && a.height == b.height && a.pixel_format == b.pixel_format
    });

    formats
}

impl CameraBackend for LibcameraBackend {
    fn enumerate_cameras(&self) -> Vec<CameraDevice> {
        debug!("Enumerating cameras via libcamera-rs");

        // If a capture pipeline is active, we can't create another CameraManager.
        // Return cached results from the last successful enumeration.
        if native::is_capture_active() {
            debug!("Capture active, returning cached camera list");
            return CACHED_DEVICES.read().map(|g| g.clone()).unwrap_or_default();
        }

        let _lock = CAMERA_MANAGER_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());

        let mgr = match CameraManager::new() {
            Ok(mgr) => mgr,
            Err(e) => {
                warn!(error = %e, "Failed to create CameraManager");
                return Vec::new();
            }
        };

        let libcamera_version = Some(mgr.version().to_string());
        let cameras = mgr.cameras();
        let mut devices = Vec::new();

        for cam in cameras.iter() {
            let camera_id = cam.id().to_string();
            let props = cam.properties();

            // Read device tree properties as fallback for libcamera properties
            let dt = read_device_tree_properties(&camera_id);

            // Get model from libcamera properties, fall back to device tree
            let sensor_model = props
                .get::<libcamera::properties::Model>()
                .ok()
                .map(|m| m.0.clone())
                .or(dt.sensor_model);

            // Get location from libcamera properties, fall back to device tree
            let camera_location = props
                .get::<libcamera::properties::Location>()
                .ok()
                .map(|l| match l {
                    libcamera::properties::Location::CameraFront => "front".to_string(),
                    libcamera::properties::Location::CameraBack => "back".to_string(),
                    libcamera::properties::Location::CameraExternal => "external".to_string(),
                })
                .or(dt.location);

            // Get rotation from libcamera properties, fall back to device tree
            let rotation = props
                .get::<libcamera::properties::Rotation>()
                .ok()
                .map(|r| SensorRotation::from_degrees_int(r.0))
                .unwrap_or(dt.rotation);

            // Generate pretty name
            let pretty_name =
                generate_pretty_name(sensor_model.as_deref(), camera_location.as_deref())
                    .unwrap_or_else(|| sensor_model.as_deref().unwrap_or(&camera_id).to_string());

            // Detect pipeline handler from camera path
            let pipeline_handler = if camera_id.starts_with("/base/") {
                Some("simple".to_string())
            } else {
                None
            };

            let supports_multistream =
                v4l2_utils::supports_multistream(pipeline_handler.as_deref());

            debug!(
                camera_id = %camera_id,
                pretty_name = %pretty_name,
                sensor_model = ?sensor_model,
                camera_location = ?camera_location,
                rotation = %rotation,
                pipeline_handler = ?pipeline_handler,
                supports_multistream,
                "Found libcamera device"
            );

            // Find the underlying V4L2 device for UVC cameras so that
            // exposure/motor/color controls work via V4L2 ioctls.
            let device_info =
                v4l2_utils::find_v4l2_device_for_libcamera(&camera_id).map(|v4l2_path| {
                    v4l2_utils::build_device_info(&v4l2_path, sensor_model.as_deref())
                });

            devices.push(CameraDevice {
                name: pretty_name,
                path: camera_id,
                metadata_path: None,
                device_info,
                rotation,
                pipeline_handler,
                supports_multistream,
                sensor_model,
                camera_location,
                libcamera_version: libcamera_version.clone(),
                lens_actuator_path: None,
            });
        }

        // Discover lens actuators and associate with back camera
        let actuators = v4l2_utils::discover_lens_actuators();
        if let Some(back_cam) = devices
            .iter_mut()
            .find(|d| d.camera_location.as_deref() == Some("back"))
            && let Some((actuator_path, actuator_name)) = actuators.first()
        {
            info!(
                actuator = %actuator_path,
                actuator_name = %actuator_name,
                camera = %back_cam.name,
                "Associated lens actuator with back camera"
            );
            back_cam.lens_actuator_path = Some(actuator_path.clone());
        }

        // Sort by path for consistent ordering
        devices.sort_by(|a, b| a.path.cmp(&b.path));

        // Cache for use when capture is active
        if let Ok(mut cache) = CACHED_DEVICES.write() {
            *cache = devices.clone();
        }

        // Pre-populate format cache for all cameras while we have the CameraManager open.
        // This ensures get_formats() can return cached results when capture is active
        // (e.g., during camera switching when the old pipeline hasn't stopped yet).
        if let Ok(mut cache) = CACHED_FORMATS.write() {
            for dev in &devices {
                if let Some(cam) = mgr.get(&dev.path) {
                    let formats = query_camera_formats(&cam, dev.supports_multistream, false);
                    debug!(
                        camera = %dev.path,
                        format_count = formats.len(),
                        "Pre-cached formats"
                    );
                    cache.insert(dev.path.clone(), formats);
                }
            }
        }

        debug!(count = devices.len(), "Enumerated libcamera cameras");
        devices
    }

    fn get_formats(&self, device: &CameraDevice, video_mode: bool) -> Vec<CameraFormat> {
        info!(device_path = %device.path, video_mode, "Getting formats via libcamera-rs");

        // If a capture pipeline is active, we can't create another CameraManager.
        // Return cached results from the last successful format query for this device.
        if native::is_capture_active() {
            let cached = CACHED_FORMATS
                .read()
                .ok()
                .and_then(|cache| cache.get(&device.path).cloned())
                .unwrap_or_default();
            info!(
                count = cached.len(),
                "Capture active, returning cached formats"
            );
            return cached;
        }

        let _lock = CAMERA_MANAGER_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());

        let mgr = match CameraManager::new() {
            Ok(mgr) => mgr,
            Err(e) => {
                warn!(error = %e, "Failed to create CameraManager for format query");
                return Vec::new();
            }
        };

        let cam = match mgr.get(&device.path) {
            Some(cam) => cam,
            None => {
                warn!(path = %device.path, "Camera not found for format query");
                return Vec::new();
            }
        };

        let formats = query_camera_formats(&cam, device.supports_multistream, video_mode);

        debug!(count = formats.len(), "Enumerated formats via libcamera-rs");

        // Cache formats for this device (used when capture is active)
        if let Ok(mut cache) = CACHED_FORMATS.write() {
            cache.insert(device.path.clone(), formats.clone());
        }

        formats
    }

    fn initialize(&mut self, device: &CameraDevice, format: &CameraFormat) -> BackendResult<()> {
        info!(
            device = %device.name,
            format = %format,
            "Initializing libcamera backend (native)"
        );

        // Shutdown any existing pipeline
        if self.is_initialized() {
            self.shutdown()?;
        }

        // Create frame channel
        let (sender, receiver) = cosmic::iced::futures::channel::mpsc::channel(
            crate::constants::latency::FRAME_CHANNEL_CAPACITY,
        );

        // Determine still capture format (highest resolution)
        let still_format = self
            .get_still_format(device)
            .unwrap_or_else(|| format.clone());

        info!(
            preview = %format,
            still = %still_format,
            multistream = device.supports_multistream,
            "Multi-stream formats selected"
        );

        // Create native pipeline
        let still_requested = Arc::new(AtomicBool::new(false));
        let still_frame = Arc::new(Mutex::new(None));
        let pipeline = NativeLibcameraPipeline::new(
            &device.path,
            format,
            device.supports_multistream,
            false, // video_mode: LibcameraBackend::initialize() is not mode-aware;
            // the subscription in app/mod.rs creates the pipeline directly
            PipelineSharedState {
                frame_sender: sender,
                still_requested,
                still_frame,
                recording_sender: Arc::new(Mutex::new(None)),
            },
        )?;

        // Store state
        self.pipeline = Some(pipeline);
        *self.frame_receiver.lock().unwrap() = Some(receiver);
        self.current_device = Some(device.clone());
        self.current_format = Some(format.clone());

        info!("libcamera backend initialized with native pipeline");
        Ok(())
    }

    fn shutdown(&mut self) -> BackendResult<()> {
        info!("Shutting down libcamera backend");

        if let Some(pipeline) = self.pipeline.take() {
            pipeline.stop()?;
            // Pipeline Drop will handle thread join and hardware release
        }

        *self.frame_receiver.lock().unwrap() = None;
        self.current_device = None;
        self.current_format = None;

        info!("libcamera backend shut down");
        Ok(())
    }

    fn is_initialized(&self) -> bool {
        self.pipeline.is_some()
    }

    fn switch_camera(&mut self, device: &CameraDevice) -> BackendResult<()> {
        info!(device = %device.name, "Switching camera");

        // Shut down first to release CameraManager before querying formats
        if self.is_initialized() {
            self.shutdown()?;
        }

        let formats = self.get_formats(device, false);
        if formats.is_empty() {
            return Err(BackendError::FormatNotSupported(
                "No formats available".to_string(),
            ));
        }

        // Select a reasonable preview format (1080p or lower for performance)
        let format = formats
            .iter()
            .find(|f| f.width <= 1920 && f.height <= 1080)
            .or(formats.first())
            .cloned()
            .ok_or_else(|| BackendError::Other("No suitable format".to_string()))?;

        self.initialize(device, &format)
    }

    fn apply_format(&mut self, format: &CameraFormat) -> BackendResult<()> {
        info!(format = %format, "Applying format");

        let device = self
            .current_device
            .clone()
            .ok_or_else(|| BackendError::Other("No active device".to_string()))?;

        self.initialize(&device, format)
    }

    fn capture_photo(&self) -> BackendResult<CameraFrame> {
        debug!("Capturing photo via native libcamera pipeline");

        let pipeline = self
            .pipeline
            .as_ref()
            .ok_or_else(|| BackendError::Other("Pipeline not initialized".to_string()))?;

        // Request still capture
        pipeline.request_still_capture();

        // Wait for still frame (with timeout)
        let start = std::time::Instant::now();
        let timeout = std::time::Duration::from_secs(2);

        while start.elapsed() < timeout {
            if let Some(frame) = pipeline.get_still_frame() {
                info!(
                    width = frame.width,
                    height = frame.height,
                    format = ?frame.format,
                    "Still frame captured"
                );
                return Ok(frame);
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }

        // Timeout - fall back to preview frame
        warn!("Still capture timeout, using preview frame");
        pipeline
            .get_preview_frame()
            .ok_or_else(|| BackendError::Other("No frame available".to_string()))
    }

    fn start_recording(&mut self, output_path: PathBuf) -> BackendResult<()> {
        info!(path = %output_path.display(), "Recording via VideoRecorder (not backend)");
        Err(BackendError::Other(
            "Use VideoRecorder for recording".to_string(),
        ))
    }

    fn stop_recording(&mut self) -> BackendResult<PathBuf> {
        Err(BackendError::NoRecordingInProgress)
    }

    fn is_recording(&self) -> bool {
        false
    }

    fn get_preview_receiver(&self) -> Option<FrameReceiver> {
        self.frame_receiver.lock().unwrap().take()
    }

    fn backend_type(&self) -> CameraBackendType {
        CameraBackendType::Libcamera
    }

    fn is_available(&self) -> bool {
        Self::is_libcamera_available()
    }

    fn current_device(&self) -> Option<&CameraDevice> {
        self.current_device.as_ref()
    }

    fn current_format(&self) -> Option<&CameraFormat> {
        self.current_format.as_ref()
    }
}
