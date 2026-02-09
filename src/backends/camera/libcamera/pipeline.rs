// SPDX-License-Identifier: GPL-3.0-only

//! libcamera streaming pipeline
//!
//! Manages request cycling, buffer allocation, and frame delivery for the libcamera backend.

use crate::backends::camera::types::*;
use libcamera::{
    camera_manager::CameraManager,
    framebuffer::AsFrameBuffer,
    framebuffer_allocator::{FrameBuffer, FrameBufferAllocator},
    framebuffer_map::MemoryMappedFrameBuffer,
    geometry::Size,
    pixel_format::PixelFormat as LibcameraPixelFormat,
    request::{Request, ReuseFlag},
    stream::StreamRole,
};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};
use tracing::{debug, error, info, trace, warn};

/// Number of buffers to allocate for streaming
const NUM_BUFFERS: usize = 4;

/// Pipeline state
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PipelineState {
    Stopped,
    Running,
    Stopping,
}

/// libcamera streaming pipeline
///
/// Manages the request/buffer cycling loop for continuous frame capture.
pub struct LibcameraPipeline {
    /// Camera ID
    camera_id: String,
    /// Pipeline state
    state: Arc<Mutex<PipelineState>>,
    /// Thread handle for the streaming loop
    thread_handle: Option<JoinHandle<()>>,
    /// Channel to send stop signal
    stop_tx: Option<Sender<()>>,
    /// Latest captured frame (for photo capture)
    latest_frame: Arc<Mutex<Option<CameraFrame>>>,
    /// Frame width
    width: u32,
    /// Frame height
    height: u32,
}

impl LibcameraPipeline {
    /// Create a new libcamera pipeline
    ///
    /// Note: CameraManager is created on-demand in the streaming thread because
    /// it's not Send+Sync. We just store the camera_id here.
    pub fn new(
        camera_id: &str,
        format: &CameraFormat,
        _frame_sender: FrameSender,
    ) -> BackendResult<Self> {
        info!(camera_id, ?format, "Creating libcamera pipeline");

        // Verify camera exists by creating a temporary CameraManager
        let manager = CameraManager::new()
            .map_err(|e| BackendError::InitializationFailed(format!("CameraManager: {e:?}")))?;
        let cameras = manager.cameras();
        let _cam = cameras
            .iter()
            .find(|c| c.id() == camera_id)
            .ok_or_else(|| BackendError::DeviceNotFound(camera_id.to_string()))?;

        Ok(Self {
            camera_id: camera_id.to_string(),
            state: Arc::new(Mutex::new(PipelineState::Stopped)),
            thread_handle: None,
            stop_tx: None,
            latest_frame: Arc::new(Mutex::new(None)),
            width: format.width,
            height: format.height,
        })
    }

    /// Start the streaming pipeline
    pub fn start(&mut self) -> BackendResult<()> {
        info!("Starting libcamera pipeline");

        {
            let mut state = self.state.lock().unwrap();
            if *state == PipelineState::Running {
                return Ok(());
            }
            *state = PipelineState::Running;
        }

        let (stop_tx, stop_rx) = mpsc::channel();
        self.stop_tx = Some(stop_tx);

        let camera_id = self.camera_id.clone();
        let state = Arc::clone(&self.state);
        let latest_frame = Arc::clone(&self.latest_frame);
        let width = self.width;
        let height = self.height;

        let handle = thread::spawn(move || {
            if let Err(e) = run_streaming_loop(camera_id, width, height, state.clone(), latest_frame, stop_rx) {
                error!(?e, "Streaming loop error");
            }
            let mut s = state.lock().unwrap();
            *s = PipelineState::Stopped;
        });

        self.thread_handle = Some(handle);
        info!("libcamera pipeline started");
        Ok(())
    }

    /// Stop the streaming pipeline
    pub fn stop(mut self) -> BackendResult<()> {
        info!("Stopping libcamera pipeline");

        {
            let mut state = self.state.lock().unwrap();
            *state = PipelineState::Stopping;
        }

        // Signal the thread to stop
        if let Some(tx) = self.stop_tx.take() {
            let _ = tx.send(());
        }

        // Wait for thread to finish
        if let Some(handle) = self.thread_handle.take() {
            let _ = handle.join();
        }

        info!("libcamera pipeline stopped");
        Ok(())
    }

    /// Capture a single frame
    pub fn capture_frame(&self) -> BackendResult<CameraFrame> {
        let frame = self.latest_frame.lock().unwrap();
        frame
            .clone()
            .ok_or_else(|| BackendError::Other("No frame available".to_string()))
    }

    /// Check if pipeline is running
    pub fn is_running(&self) -> bool {
        *self.state.lock().unwrap() == PipelineState::Running
    }
}

/// Run the streaming loop in a separate thread
fn run_streaming_loop(
    camera_id: String,
    width: u32,
    height: u32,
    state: Arc<Mutex<PipelineState>>,
    latest_frame: Arc<Mutex<Option<CameraFrame>>>,
    stop_rx: Receiver<()>,
) -> BackendResult<()> {
    debug!(camera_id, "Starting streaming loop");

    // Create a new CameraManager for this thread
    let manager = CameraManager::new()
        .map_err(|e| BackendError::InitializationFailed(format!("CameraManager: {e:?}")))?;

    let cameras = manager.cameras();
    let cam = cameras
        .iter()
        .find(|c| c.id() == camera_id)
        .ok_or_else(|| BackendError::DeviceNotFound(camera_id.clone()))?;

    let mut cam = cam
        .acquire()
        .map_err(|e| BackendError::InitializationFailed(format!("Camera acquire: {e:?}")))?;

    // Configure for viewfinder (preview) role
    let mut cfgs = cam
        .generate_configuration(&[StreamRole::ViewFinder])
        .ok_or_else(|| BackendError::InitializationFailed("Generate config failed".to_string()))?;

    // Try to set the requested resolution
    if let Some(mut stream_cfg) = cfgs.get_mut(0) {
        stream_cfg.set_size(Size { width, height });

        // Prefer NV12 or YUYV for preview (common formats)
        // Try to find a suitable format
        let stream_formats = stream_cfg.formats();
        let pixel_fmts = stream_formats.pixel_formats();
        let preferred_formats = [
            "NV12", "YUYV", "YUY2", "MJPG", "RGB888", "BGR888",
        ];

        'outer: for pref in preferred_formats {
            for i in 0..pixel_fmts.len() {
                if let Some(fmt) = pixel_fmts.get(i)
                    && format!("{:?}", fmt).contains(pref)
                {
                    stream_cfg.set_pixel_format(fmt);
                    break 'outer;
                }
            }
        }
    }

    let validation = cfgs.validate();
    debug!(?validation, "Configuration validation result");

    cam.configure(&mut cfgs)
        .map_err(|e| BackendError::InitializationFailed(format!("Configure: {e:?}")))?;

    let stream_cfg = cfgs.get(0)
        .ok_or_else(|| BackendError::InitializationFailed("No stream config".to_string()))?;
    let stream = stream_cfg.stream()
        .ok_or_else(|| BackendError::InitializationFailed("No stream".to_string()))?;

    let actual_width = stream_cfg.get_size().width;
    let actual_height = stream_cfg.get_size().height;
    let pixel_format = stream_cfg.get_pixel_format();

    info!(
        actual_width, actual_height,
        format = ?pixel_format,
        "Stream configured"
    );

    // Allocate frame buffers
    let mut alloc = FrameBufferAllocator::new(&cam);
    let buffers = alloc
        .alloc(&stream)
        .map_err(|e| BackendError::InitializationFailed(format!("Buffer alloc: {e:?}")))?;

    debug!(count = buffers.len(), "Allocated buffers");

    // Memory-map buffers for CPU access
    let buffers: Vec<MemoryMappedFrameBuffer<FrameBuffer>> = buffers
        .into_iter()
        .filter_map(|buf| MemoryMappedFrameBuffer::new(buf).ok())
        .collect();

    if buffers.is_empty() {
        return Err(BackendError::InitializationFailed("No mapped buffers".to_string()));
    }

    // Create requests with buffers
    let mut requests: Vec<Request> = buffers
        .into_iter()
        .enumerate()
        .filter_map(|(i, buf)| {
            let mut req = cam.create_request(Some(i as u64))?;
            req.add_buffer(&stream, buf).ok()?;
            Some(req)
        })
        .collect();

    if requests.is_empty() {
        return Err(BackendError::InitializationFailed("No requests created".to_string()));
    }

    // Set up completion callback
    let (req_tx, req_rx) = mpsc::channel();
    cam.on_request_completed(move |req| {
        let _ = req_tx.send(req);
    });

    // Start the camera
    cam.start(None)
        .map_err(|e| BackendError::InitializationFailed(format!("Camera start: {e:?}")))?;

    // Queue all requests
    for req in requests.drain(..) {
        cam.queue_request(req)
            .map_err(|(_, e)| BackendError::Other(format!("Queue request: {e:?}")))?;
    }

    info!("Streaming started, processing frames...");

    let mut frame_count = 0u64;
    let start_time = Instant::now();

    // Main streaming loop
    loop {
        // Check for stop signal
        if stop_rx.try_recv().is_ok() {
            debug!("Stop signal received");
            break;
        }

        // Check state
        {
            let s = state.lock().unwrap();
            if *s != PipelineState::Running {
                break;
            }
        }

        // Wait for completed request with timeout
        match req_rx.recv_timeout(Duration::from_millis(100)) {
            Ok(mut req) => {
                frame_count += 1;
                let frame_start = Instant::now();

                // Get the framebuffer
                if let Some(framebuffer) = req.buffer::<MemoryMappedFrameBuffer<FrameBuffer>>(&stream) {
                    // Extract frame data
                    let planes = framebuffer.data();
                    if let Some(plane_data) = planes.first() {
                        // Get actual bytes used from metadata
                        let bytes_used = framebuffer
                            .metadata()
                            .and_then(|m| m.planes().get(0))
                            .map(|p| p.bytes_used as usize)
                            .unwrap_or(plane_data.len());

                        // Copy frame data
                        let data: Vec<u8> = plane_data[..bytes_used].to_vec();

                        // Determine pixel format
                        let format = determine_pixel_format(&pixel_format);

                        // Calculate stride (simplified - assumes packed format)
                        let stride = actual_width * format.bytes_per_pixel() as u32;

                        let frame = CameraFrame {
                            width: actual_width,
                            height: actual_height,
                            data: FrameData::Copied(Arc::from(data.into_boxed_slice())),
                            format,
                            stride: stride as u32,
                            yuv_planes: None, // TODO: Handle multi-plane formats
                            captured_at: frame_start,
                            libcamera_metadata: None, // TODO: Extract from request metadata
                        };

                        // Store as latest frame
                        {
                            let mut latest = latest_frame.lock().unwrap();
                            *latest = Some(frame);
                        }

                        if frame_count.is_multiple_of(30) {
                            let elapsed = start_time.elapsed().as_secs_f64();
                            let fps = frame_count as f64 / elapsed;
                            trace!(frame_count, fps = format!("{:.1}", fps), "Frame captured");
                        }
                    }
                }

                // Reuse the request
                req.reuse(ReuseFlag::REUSE_BUFFERS);
                if let Err((_, e)) = cam.queue_request(req) {
                    warn!(?e, "Failed to requeue request");
                }
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {
                // Normal timeout, continue loop
                continue;
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                warn!("Request channel disconnected");
                break;
            }
        }
    }

    // Stop camera
    let _ = cam.stop();

    let elapsed = start_time.elapsed().as_secs_f64();
    let fps = frame_count as f64 / elapsed;
    info!(
        frame_count,
        elapsed_secs = format!("{:.1}", elapsed),
        avg_fps = format!("{:.1}", fps),
        "Streaming loop ended"
    );

    Ok(())
}

/// Determine our PixelFormat from libcamera's pixel format
fn determine_pixel_format(fmt: &LibcameraPixelFormat) -> PixelFormat {
    let fmt_str = format!("{:?}", fmt);

    if fmt_str.contains("NV12") {
        PixelFormat::NV12
    } else if fmt_str.contains("NV21") {
        PixelFormat::NV21
    } else if fmt_str.contains("YUYV") || fmt_str.contains("YUY2") {
        PixelFormat::YUYV
    } else if fmt_str.contains("UYVY") {
        PixelFormat::UYVY
    } else if fmt_str.contains("I420") || fmt_str.contains("YU12") {
        PixelFormat::I420
    } else if fmt_str.contains("RGB") || fmt_str.contains("BGR") {
        if fmt_str.contains("A") || fmt_str.contains("X") {
            PixelFormat::RGBA
        } else {
            PixelFormat::RGB24
        }
    } else if fmt_str.contains("GREY") || fmt_str.contains("GRAY") {
        PixelFormat::Gray8
    } else {
        // Default to RGBA for unknown formats
        // The GPU shader will handle conversion
        PixelFormat::RGBA
    }
}

impl Drop for LibcameraPipeline {
    fn drop(&mut self) {
        // Signal stop if still running
        if let Some(tx) = self.stop_tx.take() {
            let _ = tx.send(());
        }
        // Wait for thread
        if let Some(handle) = self.thread_handle.take() {
            let _ = handle.join();
        }
    }
}
