// SPDX-License-Identifier: GPL-3.0-only

//! Capture thread implementation — owns all libcamera objects.
//!
//! All libcamera types (CameraManager, ActiveCamera, FrameBuffers) are created and
//! dropped on this single thread to avoid Send/Sync issues with libcamera's raw
//! pointers.

use super::diagnostics::{
    CAPTURE_ACTIVE, DIAGNOSTICS, DiagnosticParams, MJPEG_DECODE_TIME_US, PREVIEW_FRAME_COUNT,
    STILL_FRAME_COUNT, StreamDiag, publish_diagnostics,
};
use super::pixel_formats::{map_pixel_format, pixel_format_name};
use crate::backends::camera::types::*;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;
use tracing::{debug, error, info, warn};

/// Parameters passed to the capture thread for initialization
pub(crate) struct CaptureThreadParams {
    pub(crate) camera_id: String,
    pub(crate) preview_width: u32,
    pub(crate) preview_height: u32,
    pub(crate) supports_multistream: bool,
    pub(crate) video_mode: bool,
    pub(crate) stop_flag: Arc<AtomicBool>,
    pub(crate) latest_preview: Arc<Mutex<Option<CameraFrame>>>,
    pub(crate) latest_still: Arc<Mutex<Option<CameraFrame>>>,
    pub(crate) still_requested: Arc<AtomicBool>,
    pub(crate) preview_frame_count: Arc<AtomicU64>,
    pub(crate) still_frame_count: Arc<AtomicU64>,
    pub(crate) frame_sender: FrameSender,
    pub(crate) recording_sender: Arc<Mutex<Option<tokio::sync::mpsc::Sender<Arc<CameraFrame>>>>>,
}

/// Result of capture thread initialization (sent back to main thread)
pub(crate) struct CaptureThreadInitResult {
    pub(crate) is_multistream: bool,
    pub(crate) has_video_stream: bool,
}

/// Extract the number of bytes actually written to the first plane of a frame buffer.
/// Falls back to the full plane length if metadata is unavailable.
fn plane_bytes_used(
    buf: &impl libcamera::framebuffer::AsFrameBuffer,
    fallback_len: usize,
) -> usize {
    buf.metadata()
        .and_then(|m| m.planes().into_iter().next().map(|p| p.bytes_used as usize))
        .unwrap_or(fallback_len)
}

/// Memory-map a vec of frame buffers, returning an error with the given label on failure.
fn mmap_buffers(
    buffers: Vec<libcamera::framebuffer_allocator::FrameBuffer>,
    label: &str,
) -> Result<
    Vec<
        libcamera::framebuffer_map::MemoryMappedFrameBuffer<
            libcamera::framebuffer_allocator::FrameBuffer,
        >,
    >,
    BackendError,
> {
    buffers
        .into_iter()
        .map(|fb| {
            libcamera::framebuffer_map::MemoryMappedFrameBuffer::new(fb)
                .map_err(|e| BackendError::InitializationFailed(format!("mmap {}: {:?}", label, e)))
        })
        .collect()
}

/// Allocate buffers for an optional stream. Returns None when stream is None.
fn alloc_optional_buffers(
    alloc: &mut libcamera::framebuffer_allocator::FrameBufferAllocator,
    stream: &Option<libcamera::stream::Stream>,
    label: &str,
) -> Result<Option<Vec<libcamera::framebuffer_allocator::FrameBuffer>>, BackendError> {
    match stream {
        Some(s) => {
            let bufs = alloc.alloc(s).map_err(|e| {
                BackendError::InitializationFailed(format!("Alloc {} buffers: {}", label, e))
            })?;
            Ok(Some(bufs))
        }
        None => Ok(None),
    }
}

/// Requeue a completed request, warning on failure unless stop is requested.
fn requeue_request(
    cam: &mut libcamera::camera::ActiveCamera,
    req: libcamera::request::Request,
    stop_flag: &AtomicBool,
) {
    if let Err((_, e)) = cam.queue_request(req)
        && !stop_flag.load(Ordering::Acquire)
    {
        warn!(error = %e, "Failed to re-queue request");
    }
}

/// Read back format info from a configured stream at the given index.
/// Returns (format_name, size, mapped_pixel_format, stride).
fn read_stream_config(
    config: &libcamera::camera::CameraConfiguration,
    index: usize,
) -> (String, libcamera::geometry::Size, Option<PixelFormat>, u32) {
    let pf = config.get(index).map(|c| c.get_pixel_format());
    let name = pf.map(pixel_format_name).unwrap_or_default();
    let size = config
        .get(index)
        .map(|c| c.get_size())
        .unwrap_or(libcamera::geometry::Size::new(0, 0));
    let stride = config.get(index).map(|c| c.get_stride()).unwrap_or(0);
    let mapped = pf.and_then(map_pixel_format);
    (name, size, mapped, stride)
}

/// Spatial layout of a camera frame buffer.
struct FrameLayout {
    width: u32,
    height: u32,
    format: PixelFormat,
    stride: u32,
}

/// Build a CameraFrame from copied buffer data and metadata.
fn build_camera_frame(
    data: Arc<[u8]>,
    layout: FrameLayout,
    captured_at: Instant,
    sensor_timestamp_ns: Option<u64>,
    metadata: FrameMetadata,
) -> CameraFrame {
    CameraFrame {
        width: layout.width,
        height: layout.height,
        data: FrameData::Copied(data),
        format: layout.format,
        stride: layout.stride,
        yuv_planes: None,
        captured_at,
        sensor_timestamp_ns,
        libcamera_metadata: Some(metadata),
    }
}

/// Extract buffer data from a completed request, copy it, and build a CameraFrame.
///
/// Common pattern used by video and raw buffer processing. Returns None if the
/// buffer is missing, empty, or has no plane data.
fn process_buffer(
    req: &libcamera::request::Request,
    stream: &libcamera::stream::Stream,
    size: libcamera::geometry::Size,
    config_stride: u32,
    pixel_format: PixelFormat,
    captured_at: Instant,
    metadata: &FrameMetadata,
) -> Option<CameraFrame> {
    use libcamera::framebuffer::AsFrameBuffer;
    use libcamera::framebuffer_map::MemoryMappedFrameBuffer;
    type MmapFB = MemoryMappedFrameBuffer<libcamera::framebuffer_allocator::FrameBuffer>;

    let buf = req.buffer::<MmapFB>(stream)?;
    let planes = buf.data();
    let plane_data = planes.first()?;
    let bytes_used = plane_bytes_used(buf, plane_data.len());
    if bytes_used == 0 {
        return None;
    }

    let data_slice = &plane_data[..bytes_used.min(plane_data.len())];
    let data: Arc<[u8]> = Arc::from(data_slice);

    let stride = if config_stride > 0 {
        config_stride
    } else {
        compute_stride(pixel_format, size.width, data.len(), size.height)
    };

    let sensor_timestamp_ns = buf.metadata().map(|m| m.timestamp());
    Some(build_camera_frame(
        data,
        FrameLayout {
            width: size.width,
            height: size.height,
            format: pixel_format,
            stride,
        },
        captured_at,
        sensor_timestamp_ns,
        metadata.clone(),
    ))
}

/// Main entry point for the capture thread
///
/// Creates all libcamera objects, runs the capture loop, and cleans up on exit.
/// Reports initialization success/failure via `init_tx`.
pub(crate) fn capture_thread_main(
    params: CaptureThreadParams,
    init_tx: std::sync::mpsc::SyncSender<Result<CaptureThreadInitResult, BackendError>>,
) {
    match capture_thread_setup_and_run(params, &init_tx) {
        Ok(()) => info!("Capture thread exiting normally"),
        Err(e) => {
            error!(error = %e, "Capture thread failed");
            // If init_tx hasn't been used yet, report the error
            let _ = init_tx.try_send(Err(e));
        }
    }
}

/// How often to emit per-frame diagnostic log messages (every Nth frame).
const LOG_EVERY_N_FRAMES: u64 = 30;

/// Mode-aware second stream selection
#[derive(Debug, Clone, Copy, PartialEq)]
enum SecondStream {
    None,
    Raw,
    VideoRecording,
}

/// Stream format metadata read back after configure.
struct StreamFormats {
    vf_size: libcamera::geometry::Size,
    vf_stride: u32,
    vf_pixel_format: PixelFormat,
    vf_format_name: String,
    vf_is_mjpeg: bool,
    raw_size: libcamera::geometry::Size,
    raw_stride: u32,
    raw_format_name: String,
    raw_pixel_format: Option<PixelFormat>,
    video_size: libcamera::geometry::Size,
    video_format_name: String,
    video_pixel_format: Option<PixelFormat>,
    video_stride: u32,
}

/// Set up libcamera and run the capture loop.
///
/// All libcamera objects (CameraManager, Camera, ActiveCamera, buffers) must
/// live in this function's scope because they form a borrow chain that prevents
/// splitting ownership across returned structs. The three logical phases are
/// separated into clearly marked sections.
fn capture_thread_setup_and_run(
    mut params: CaptureThreadParams,
    init_tx: &std::sync::mpsc::SyncSender<Result<CaptureThreadInitResult, BackendError>>,
) -> Result<(), BackendError> {
    use libcamera::camera_manager::CameraManager;

    // Acquire global lock to prevent concurrent CameraManager creation with
    // enumerate_cameras()/get_formats(). Mark capture active BEFORE creating the
    // manager so other threads know not to try.
    let _mgr_lock = super::super::CAMERA_MANAGER_LOCK
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    CAPTURE_ACTIVE.store(true, Ordering::Release);

    // Create camera manager (we hold the lock, so no other code will try concurrently)
    let mgr = CameraManager::new().map_err(|e| {
        CAPTURE_ACTIVE.store(false, Ordering::Release);
        BackendError::InitializationFailed(format!("CameraManager::new: {}", e))
    })?;

    // Release the lock - CAPTURE_ACTIVE flag now guards against concurrent creation
    drop(_mgr_lock);

    info!(version = mgr.version(), "libcamera version");

    // Get camera by ID
    let cam = mgr.get(&params.camera_id).ok_or_else(|| {
        BackendError::DeviceNotFound(format!("Camera '{}' not found", params.camera_id))
    })?;

    // Acquire exclusive access
    let mut active_cam = cam
        .acquire()
        .map_err(|e| BackendError::InitializationFailed(format!("Camera acquire: {}", e)))?;

    let (config, is_multistream, has_video_stream, actual_second_stream) =
        configure_streams(&cam, &mut active_cam, &params)?;

    let formats = read_stream_formats(
        &config,
        is_multistream,
        has_video_stream,
        actual_second_stream,
    );

    info!(
        vf_format = %formats.vf_format_name,
        vf_size = format!("{}x{}", formats.vf_size.width, formats.vf_size.height),
        vf_stride = formats.vf_stride,
        vf_pixel_format = ?formats.vf_pixel_format,
        raw_format = %formats.raw_format_name,
        raw_size = format!("{}x{}", formats.raw_size.width, formats.raw_size.height),
        raw_pixel_format = ?formats.raw_pixel_format,
        video_format = %formats.video_format_name,
        video_size = format!("{}x{}", formats.video_size.width, formats.video_size.height),
        video_pixel_format = ?formats.video_pixel_format,
        has_video_stream,
        multistream = is_multistream,
        second_stream = ?actual_second_stream,
        "Actual configured formats (post-configure)"
    );

    // Get stream references (must be done after configure)
    let stream_vf = config
        .get(0)
        .and_then(|c| c.stream())
        .ok_or_else(|| BackendError::InitializationFailed("No viewfinder stream".to_string()))?;

    let stream_raw = if is_multistream && actual_second_stream == SecondStream::Raw {
        config.get(1).and_then(|c| c.stream())
    } else {
        None
    };

    let stream_video = if has_video_stream {
        config.get(1).and_then(|c| c.stream())
    } else {
        None
    };

    let (alloc, requests) = allocate_and_create_requests(
        &cam,
        &mut active_cam,
        &stream_vf,
        &stream_raw,
        &stream_video,
        is_multistream,
        has_video_stream,
        actual_second_stream,
    )?;

    // Subscribe to request completion via channel
    let rx = active_cam.subscribe_request_completed();

    // Start camera
    active_cam
        .start(None)
        .map_err(|e| BackendError::InitializationFailed(format!("Camera start: {}", e)))?;

    // Queue all requests
    for req in requests {
        if let Err((_, e)) = active_cam.queue_request(req) {
            error!(error = %e, "Failed to queue initial request");
        }
    }

    info!("Camera started, all requests queued");

    // Publish diagnostics
    publish_diagnostics(DiagnosticParams {
        is_multistream,
        has_video_stream,
        is_video_mode: params.video_mode,
        vf: StreamDiag {
            size: formats.vf_size,
            format_name: formats.vf_format_name.clone(),
        },
        raw: StreamDiag {
            size: formats.raw_size,
            format_name: formats.raw_format_name.clone(),
        },
        video: StreamDiag {
            size: formats.video_size,
            format_name: formats.video_format_name.clone(),
        },
    });

    // Report successful initialization
    init_tx
        .send(Ok(CaptureThreadInitResult {
            is_multistream,
            has_video_stream,
        }))
        .map_err(|_| {
            BackendError::InitializationFailed("Main thread dropped init receiver".to_string())
        })?;

    // Create MJPEG decompressor if needed
    let mut jpeg_decompressor = if formats.vf_is_mjpeg {
        let decompressor = turbojpeg::Decompressor::new()
            .map_err(|e| BackendError::Other(format!("turbojpeg init: {e}")))?;
        if let Ok(mut d) = DIAGNOSTICS.write() {
            d.mjpeg_decoder_name = Some("turbojpeg (libjpeg-turbo)".to_string());
        }
        Some(decompressor)
    } else {
        None
    };

    run_capture_loop(
        &mut active_cam,
        &rx,
        &stream_vf,
        &stream_raw,
        &stream_video,
        &formats,
        is_multistream,
        has_video_stream,
        &mut jpeg_decompressor,
        &mut params,
    );

    // Stop camera (ActiveCamera::drop also does this, but explicit is cleaner)
    info!("Capture loop ending, stopping camera");
    let _ = active_cam.stop();

    // Drop all libcamera objects in correct dependency order.
    // Previously, dropping mgr before cam/alloc caused use-after-free (SIGSEGV)
    // because those objects still referenced the CameraManager's internal state.
    drop(rx);
    drop(alloc);
    drop(config);
    drop(active_cam);
    drop(cam);
    drop(mgr);

    // Now clear the flag under the lock so that any enumerate_cameras()/get_formats()
    // call that was waiting will see CAPTURE_ACTIVE=false and proceed normally.
    {
        let _lock = super::super::CAMERA_MANAGER_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        CAPTURE_ACTIVE.store(false, Ordering::Release);
    }
    info!("CameraManager released");

    Ok(())
}

/// Generate camera configuration with stream roles, apply to hardware.
/// Falls back from VideoRecording to Raw if the camera doesn't support dual
/// processed streams.
fn configure_streams(
    cam: &libcamera::camera::Camera<'_>,
    active_cam: &mut libcamera::camera::ActiveCamera<'_>,
    params: &CaptureThreadParams,
) -> Result<
    (
        libcamera::camera::CameraConfiguration,
        bool,
        bool,
        SecondStream,
    ),
    BackendError,
> {
    use libcamera::stream::StreamRole;

    // Mode-aware stream role selection:
    // - Video mode: ViewFinder + VideoRecording (dual processed streams for ISP-optimized encoding)
    //   Fallback:   ViewFinder + Raw (Raw keeps full sensor FoV, VF frames used for encoding)
    // - Photo mode: ViewFinder + Raw (full-res Bayer for still capture)
    // - Single:     ViewFinder only
    let (roles, second_stream) = if params.video_mode && params.supports_multistream {
        (
            vec![StreamRole::ViewFinder, StreamRole::VideoRecording],
            SecondStream::VideoRecording,
        )
    } else if params.supports_multistream {
        (
            vec![StreamRole::ViewFinder, StreamRole::Raw],
            SecondStream::Raw,
        )
    } else {
        (vec![StreamRole::ViewFinder], SecondStream::None)
    };

    info!(
        video_mode = params.video_mode,
        second_stream = ?second_stream,
        "Requesting stream roles"
    );

    let mut config = cam.generate_configuration(&roles).ok_or_else(|| {
        BackendError::InitializationFailed("Failed to generate camera configuration".to_string())
    })?;

    let mut actual_second_stream = second_stream;
    let mut is_multistream = config.len() >= 2 && params.supports_multistream;

    // Configure stream sizes
    if let Some(mut vf_cfg) = config.get_mut(0) {
        let size = libcamera::geometry::Size::new(params.preview_width, params.preview_height);
        vf_cfg.set_size(size);
        debug!(
            width = params.preview_width,
            height = params.preview_height,
            "Set viewfinder size"
        );
    }
    // For VideoRecording stream, set same resolution as ViewFinder
    if actual_second_stream == SecondStream::VideoRecording
        && let Some(mut vid_cfg) = config.get_mut(1)
    {
        vid_cfg.set_size(libcamera::geometry::Size::new(
            params.preview_width,
            params.preview_height,
        ));
    }

    let status = config.validate();
    info!(
        status = ?status,
        config = config.to_string_repr(),
        "Configuration validated"
    );

    // Apply configuration to hardware — with fallback for VideoRecording
    match active_cam.configure(&mut config) {
        Ok(()) => {}
        Err(e) if actual_second_stream == SecondStream::VideoRecording => {
            // Dual processed streams not supported (e.g. simple pipeline handler).
            // Fall back to ViewFinder + Raw — Raw forces full sensor mode (no crop/zoom)
            // and ViewFinder frames are used for video encoding via Option A.
            warn!(
                error = %e,
                "ViewFinder+VideoRecording configure failed, falling back to ViewFinder+Raw"
            );
            actual_second_stream = SecondStream::Raw;

            config = cam
                .generate_configuration(&[StreamRole::ViewFinder, StreamRole::Raw])
                .ok_or_else(|| {
                    BackendError::InitializationFailed(
                        "Failed to generate fallback configuration".to_string(),
                    )
                })?;

            if let Some(mut vf_cfg) = config.get_mut(0) {
                vf_cfg.set_size(libcamera::geometry::Size::new(
                    params.preview_width,
                    params.preview_height,
                ));
            }
            let fallback_status = config.validate();
            info!(
                status = ?fallback_status,
                config = config.to_string_repr(),
                "Fallback configuration validated"
            );

            active_cam.configure(&mut config).map_err(|e2| {
                BackendError::InitializationFailed(format!(
                    "Camera configure (fallback ViewFinder+Raw): {}",
                    e2
                ))
            })?;

            is_multistream = config.len() >= 2 && params.supports_multistream;
        }
        Err(e) => {
            return Err(BackendError::InitializationFailed(format!(
                "Camera configure: {}",
                e
            )));
        }
    }

    let has_video_stream = actual_second_stream == SecondStream::VideoRecording;
    Ok((
        config,
        is_multistream,
        has_video_stream,
        actual_second_stream,
    ))
}

/// Read back actual stream formats after hardware configure.
/// (configure may adjust stride/size beyond what validate reported)
fn read_stream_formats(
    config: &libcamera::camera::CameraConfiguration,
    is_multistream: bool,
    has_video_stream: bool,
    actual_second_stream: SecondStream,
) -> StreamFormats {
    let (vf_format_name, vf_size, vf_mapped, vf_stride) = read_stream_config(config, 0);
    let vf_is_mjpeg = config
        .get(0)
        .map(|c| c.get_pixel_format().fourcc() == libcamera::formats::MJPEG.fourcc())
        .unwrap_or(false);
    let vf_pixel_format = if vf_is_mjpeg {
        info!("Viewfinder using MJPEG — will decode to native YUV via turbojpeg");
        PixelFormat::I420
    } else {
        vf_mapped.unwrap_or_else(|| {
            warn!(format = %vf_format_name, "Unknown viewfinder pixel format, assuming ABGR");
            PixelFormat::ABGR
        })
    };

    let (raw_format_name, raw_size, raw_pixel_format, raw_stride) =
        if is_multistream && actual_second_stream == SecondStream::Raw {
            let (name, size, mapped, stride) = read_stream_config(config, 1);
            (name, size, mapped, stride)
        } else {
            (
                String::new(),
                libcamera::geometry::Size::new(0, 0),
                None,
                0u32,
            )
        };

    let (video_format_name, video_size, video_pixel_format, video_stride) = if has_video_stream {
        read_stream_config(config, 1)
    } else {
        (
            String::new(),
            libcamera::geometry::Size::new(0, 0),
            None,
            0u32,
        )
    };

    StreamFormats {
        vf_size,
        vf_stride,
        vf_pixel_format,
        vf_format_name,
        vf_is_mjpeg,
        raw_size,
        raw_stride,
        raw_format_name,
        raw_pixel_format,
        video_size,
        video_format_name,
        video_pixel_format,
        video_stride,
    }
}

/// Allocate frame buffers, memory-map them, and create capture requests.
///
/// Returns the allocator (which must outlive the requests) and the request vector.
#[allow(clippy::too_many_arguments)]
fn allocate_and_create_requests(
    cam: &libcamera::camera::Camera<'_>,
    active_cam: &mut libcamera::camera::ActiveCamera<'_>,
    stream_vf: &libcamera::stream::Stream,
    stream_raw: &Option<libcamera::stream::Stream>,
    stream_video: &Option<libcamera::stream::Stream>,
    is_multistream: bool,
    has_video_stream: bool,
    actual_second_stream: SecondStream,
) -> Result<
    (
        libcamera::framebuffer_allocator::FrameBufferAllocator,
        Vec<libcamera::request::Request>,
    ),
    BackendError,
> {
    use libcamera::framebuffer_allocator::FrameBufferAllocator;
    use libcamera::request::Request;

    let mut alloc = FrameBufferAllocator::new(cam);

    let vf_buffers = alloc.alloc(stream_vf).map_err(|e| {
        BackendError::InitializationFailed(format!("Alloc viewfinder buffers: {}", e))
    })?;

    let raw_buffers = if is_multistream && actual_second_stream == SecondStream::Raw {
        alloc_optional_buffers(&mut alloc, stream_raw, "raw")?
    } else {
        None
    };

    let video_buffers = if has_video_stream {
        alloc_optional_buffers(&mut alloc, stream_video, "video")?
    } else {
        None
    };

    info!(
        vf_buffers = vf_buffers.len(),
        raw_buffers = raw_buffers.as_ref().map(|b| b.len()).unwrap_or(0),
        video_buffers = video_buffers.as_ref().map(|b| b.len()).unwrap_or(0),
        "Allocated buffers"
    );

    // Wrap buffers in memory-mapped wrappers for data access
    let vf_mapped = mmap_buffers(vf_buffers, "vf")?;
    let raw_mapped = raw_buffers.map(|b| mmap_buffers(b, "raw")).transpose()?;
    let video_mapped = video_buffers
        .map(|b| mmap_buffers(b, "video"))
        .transpose()?;

    // Create requests with buffers
    let buf_count = vf_mapped.len();
    let mut requests: Vec<Request> = Vec::with_capacity(buf_count);
    let mut vf_iter = vf_mapped.into_iter();
    let mut raw_iter = raw_mapped.map(|v| v.into_iter());
    let mut video_iter = video_mapped.map(|v| v.into_iter());

    for i in 0..buf_count {
        let mut req = active_cam.create_request(Some(i as u64)).ok_or_else(|| {
            BackendError::InitializationFailed("Failed to create request".to_string())
        })?;

        let vf_buf = vf_iter.next().ok_or_else(|| {
            BackendError::InitializationFailed("Not enough vf buffers".to_string())
        })?;
        req.add_buffer(stream_vf, vf_buf)
            .map_err(|e| BackendError::InitializationFailed(format!("Add vf buffer: {}", e)))?;

        if let Some(ref mut raw_it) = raw_iter
            && let Some(raw_buf) = raw_it.next()
            && let Some(sr) = stream_raw
        {
            req.add_buffer(sr, raw_buf).map_err(|e| {
                BackendError::InitializationFailed(format!("Add raw buffer: {}", e))
            })?;
        }

        if let Some(ref mut vid_it) = video_iter
            && let Some(vid_buf) = vid_it.next()
            && let Some(sv) = stream_video
        {
            req.add_buffer(sv, vid_buf).map_err(|e| {
                BackendError::InitializationFailed(format!("Add video buffer: {}", e))
            })?;
        }

        requests.push(req);
    }

    info!(requests = requests.len(), "Created capture requests");
    Ok((alloc, requests))
}

/// Run the capture loop — receive completed requests, process viewfinder/video/raw
/// buffers, send frames to consumers. Returns when `stop_flag` is set or the
/// request channel disconnects.
#[allow(clippy::too_many_arguments)]
fn run_capture_loop(
    active_cam: &mut libcamera::camera::ActiveCamera<'_>,
    rx: &std::sync::mpsc::Receiver<libcamera::request::Request>,
    stream_vf: &libcamera::stream::Stream,
    stream_raw: &Option<libcamera::stream::Stream>,
    stream_video: &Option<libcamera::stream::Stream>,
    formats: &StreamFormats,
    is_multistream: bool,
    has_video_stream: bool,
    jpeg_decompressor: &mut Option<turbojpeg::Decompressor>,
    params: &mut CaptureThreadParams,
) {
    use libcamera::framebuffer::AsFrameBuffer;
    use libcamera::framebuffer_map::MemoryMappedFrameBuffer;
    use libcamera::request::{RequestStatus, ReuseFlag};

    info!("Entering capture loop");

    // Reusable buffer for MJPEG→YUV decompression.
    // Avoids allocating + zeroing several MB per frame.
    let mut jpeg_yuv_buf: Vec<u8> = Vec::new();

    type MmapFB = MemoryMappedFrameBuffer<libcamera::framebuffer_allocator::FrameBuffer>;

    while !params.stop_flag.load(Ordering::Acquire) {
        // Wait for completed request with timeout
        let mut req = match rx.recv_timeout(std::time::Duration::from_millis(100)) {
            Ok(req) => req,
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => continue,
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                info!("Request channel disconnected, stopping capture loop");
                break;
            }
        };

        if req.status() != RequestStatus::Complete {
            debug!(status = ?req.status(), "Skipping non-complete request");
            req.reuse(ReuseFlag::REUSE_BUFFERS);
            requeue_request(active_cam, req, &params.stop_flag);
            continue;
        }

        let captured_at = Instant::now();
        let frame_num = params.preview_frame_count.fetch_add(1, Ordering::Relaxed);

        // Extract per-frame metadata
        let metadata = extract_metadata(&req);

        // Process viewfinder buffer
        if let Some(vf_buf) = req.buffer::<MmapFB>(stream_vf) {
            // Extract kernel buffer timestamp (CLOCK_BOOTTIME ns) for video recording PTS
            let sensor_timestamp_ns = vf_buf.metadata().map(|m| m.timestamp());
            let planes = vf_buf.data();
            if let Some(plane_data) = planes.first() {
                let bytes_used = plane_bytes_used(vf_buf, plane_data.len());

                let expected_size = formats.vf_stride as usize * formats.vf_size.height as usize;
                if frame_num == 0 {
                    info!(
                        bytes_used,
                        plane_len = plane_data.len(),
                        expected_size,
                        vf_stride = formats.vf_stride,
                        width = formats.vf_size.width,
                        height = formats.vf_size.height,
                        "First frame buffer diagnostics"
                    );
                }

                // Skip frames with no data (e.g. first frame from some UVC cameras)
                if bytes_used == 0 {
                    req.reuse(ReuseFlag::REUSE_BUFFERS);
                    requeue_request(active_cam, req, &params.stop_flag);
                    continue;
                }

                let data_slice = &plane_data[..bytes_used.min(plane_data.len())];

                // Decode MJPEG or copy raw data from mmap BEFORE req.reuse()
                let frame = if let Some(decompressor) = jpeg_decompressor {
                    let decode_start = Instant::now();
                    let decode_result = decode_mjpeg_frame(
                        decompressor,
                        data_slice,
                        captured_at,
                        sensor_timestamp_ns,
                        &metadata,
                        frame_num,
                        &mut jpeg_yuv_buf,
                    );
                    MJPEG_DECODE_TIME_US
                        .store(decode_start.elapsed().as_micros() as u64, Ordering::Relaxed);
                    match decode_result {
                        Some(f) => f,
                        None => {
                            // Decode failed, skip this frame
                            req.reuse(ReuseFlag::REUSE_BUFFERS);
                            requeue_request(active_cam, req, &params.stop_flag);
                            continue;
                        }
                    }
                } else {
                    let data: Arc<[u8]> = Arc::from(data_slice);

                    let stride = if formats.vf_stride > 0 {
                        formats.vf_stride
                    } else {
                        compute_stride(
                            formats.vf_pixel_format,
                            formats.vf_size.width,
                            data.len(),
                            formats.vf_size.height,
                        )
                    };

                    build_camera_frame(
                        data,
                        FrameLayout {
                            width: formats.vf_size.width,
                            height: formats.vf_size.height,
                            format: formats.vf_pixel_format,
                            stride,
                        },
                        captured_at,
                        sensor_timestamp_ns,
                        metadata.clone(),
                    )
                };

                dispatch_viewfinder_frame(
                    frame,
                    frame_num,
                    params,
                    is_multistream,
                    has_video_stream,
                );
            }
        } else if frame_num.is_multiple_of(LOG_EVERY_N_FRAMES) {
            warn!("No viewfinder buffer in completed request");
        }

        // Process VideoRecording buffer (only when recording is active)
        if has_video_stream && let Some(sv) = stream_video {
            let has_recorder = params
                .recording_sender
                .lock()
                .ok()
                .as_ref()
                .map(|g| g.is_some())
                .unwrap_or(false);

            if has_recorder {
                let vid_pf = formats.video_pixel_format.unwrap_or(PixelFormat::NV12);
                if let Some(video_frame) = process_buffer(
                    &req,
                    sv,
                    formats.video_size,
                    formats.video_stride,
                    vid_pf,
                    captured_at,
                    &metadata,
                ) && let Ok(guard) = params.recording_sender.lock()
                    && let Some(ref tx) = *guard
                {
                    let _ = tx.try_send(Arc::new(video_frame));
                }
            }
        }

        // Process raw buffer only when still capture is requested (photo mode multistream only)
        if !has_video_stream
            && is_multistream
            && params.still_requested.load(Ordering::Relaxed)
            && let Some(sr) = stream_raw
        {
            let raw_pf = formats.raw_pixel_format.unwrap_or(PixelFormat::BayerRGGB);
            if let Some(still_frame) = process_buffer(
                &req,
                sr,
                formats.raw_size,
                formats.raw_stride,
                raw_pf,
                captured_at,
                &metadata,
            ) {
                let still_count = params.still_frame_count.fetch_add(1, Ordering::Relaxed);
                info!(
                    frame = still_count,
                    width = still_frame.width,
                    height = still_frame.height,
                    format = ?still_frame.format,
                    "Raw still frame captured"
                );

                if let Ok(mut still) = params.latest_still.lock() {
                    *still = Some(still_frame);
                }
                params.still_requested.store(false, Ordering::Relaxed);
            }
        }

        // Update global frame counts for insights (lock-free)
        PREVIEW_FRAME_COUNT.store(
            params.preview_frame_count.load(Ordering::Relaxed),
            Ordering::Relaxed,
        );
        STILL_FRAME_COUNT.store(
            params.still_frame_count.load(Ordering::Relaxed),
            Ordering::Relaxed,
        );

        // Reuse request and re-queue
        req.reuse(ReuseFlag::REUSE_BUFFERS);
        requeue_request(active_cam, req, &params.stop_flag);
    }
}

/// Dispatch a completed viewfinder frame to all consumers:
/// 1. Store as latest preview (for still capture fallback)
/// 2. Handle single-stream still capture
/// 3. Send to recording (Option A: VF→encoder when no video stream)
/// 4. Send to UI preview channel
fn dispatch_viewfinder_frame(
    frame: CameraFrame,
    frame_num: u64,
    params: &mut CaptureThreadParams,
    is_multistream: bool,
    has_video_stream: bool,
) {
    // Store latest preview frame
    if let Ok(mut latest) = params.latest_preview.lock() {
        *latest = Some(frame.clone());
    }

    // In single-stream mode, also handle still capture from preview
    if !is_multistream && params.still_requested.load(Ordering::Relaxed) {
        info!(
            frame = frame_num,
            width = frame.width,
            height = frame.height,
            "Still frame captured from preview stream"
        );
        if let Ok(mut still) = params.latest_still.lock() {
            *still = Some(frame.clone());
        }
        params.still_requested.store(false, Ordering::Relaxed);
    }

    // Option A: ViewFinder → recording (when no dedicated video stream).
    // Send to recording FIRST for lowest encoder latency, then preview.
    if !has_video_stream
        && let Ok(guard) = params.recording_sender.lock()
        && let Some(ref tx) = *guard
    {
        let _ = tx.try_send(Arc::new(frame.clone()));
    }

    // Send to UI (preview)
    if let Err(e) = params.frame_sender.try_send(frame)
        && frame_num.is_multiple_of(LOG_EVERY_N_FRAMES)
    {
        debug!(error = ?e, "Preview frame dropped (channel full)");
    }
}

/// Extract per-frame metadata from a completed request
fn extract_metadata(req: &libcamera::request::Request) -> FrameMetadata {
    use libcamera::controls;
    let meta = req.metadata();

    FrameMetadata {
        exposure_time: meta
            .get::<controls::ExposureTime>()
            .ok()
            .map(|v| v.0 as u64),
        analogue_gain: meta.get::<controls::AnalogueGain>().ok().map(|v| v.0),
        digital_gain: meta.get::<controls::DigitalGain>().ok().map(|v| v.0),
        colour_temperature: meta
            .get::<controls::ColourTemperature>()
            .ok()
            .map(|v| v.0 as u32),
        sequence: Some(req.sequence()),
        lens_position: meta.get::<controls::LensPosition>().ok().map(|v| v.0),
        sensor_timestamp: None,
        af_state: None,
        ae_state: None,
        awb_state: None,
        colour_gains: meta.get::<controls::ColourGains>().ok().map(|v| v.0),
        colour_correction_matrix: meta
            .get::<controls::ColourCorrectionMatrix>()
            .ok()
            .map(|v| v.0),
        black_level: meta.get::<controls::SensorBlackLevels>().ok().map(|v| {
            // SensorBlackLevels are 16-bit [R, Gr, Gb, B]; average and normalize
            let levels = v.0;
            let avg =
                (levels[0] as f32 + levels[1] as f32 + levels[2] as f32 + levels[3] as f32) / 4.0;
            avg / 65535.0
        }),
        lux: meta.get::<controls::Lux>().ok().map(|v| v.0),
        focus_fom: meta.get::<controls::FocusFoM>().ok().map(|v| v.0),
    }
}

/// Decode a MJPEG frame to native planar YUV via turbojpeg
///
/// turbojpeg's `decompress_to_yuv` produces the native subsampling:
/// - 4:2:0 JPEG → I420 (Sub2x2)
/// - 4:2:2 JPEG → I422 (Sub2x1)
/// - 4:4:4 JPEG → I444 (None)
/// - etc.
///
/// The GPU shader handles all subsampling types via `textureDimensions()`.
///
/// `yuv_buf` is a reusable buffer to avoid allocating on every frame.
/// Returns `None` if decoding fails (caller should skip the frame).
fn decode_mjpeg_frame(
    decompressor: &mut turbojpeg::Decompressor,
    jpeg_data: &[u8],
    captured_at: Instant,
    sensor_timestamp_ns: Option<u64>,
    metadata: &FrameMetadata,
    frame_num: u64,
    yuv_buf: &mut Vec<u8>,
) -> Option<CameraFrame> {
    // Read JPEG header to get dimensions and subsampling
    let header = match decompressor.read_header(jpeg_data) {
        Ok(h) => h,
        Err(e) => {
            if frame_num.is_multiple_of(LOG_EVERY_N_FRAMES) {
                warn!(error = %e, "MJPEG header read failed, skipping frame");
            }
            return None;
        }
    };

    let width = header.width;
    let height = header.height;

    // Compute YUV buffer size for this subsampling
    let align = 1; // no row padding
    let buf_len = match turbojpeg::yuv_pixels_len(width, align, height, header.subsamp) {
        Ok(len) => len,
        Err(e) => {
            warn!(error = %e, subsamp = ?header.subsamp, "Failed to compute YUV buffer size");
            return None;
        }
    };

    // Resize reusable buffer (free if same size, which it will be after first frame)
    yuv_buf.resize(buf_len, 0);

    let yuv_image = turbojpeg::YuvImage {
        pixels: &mut yuv_buf[..],
        width,
        align,
        height,
        subsamp: header.subsamp,
    };

    if let Err(e) = decompressor.decompress_to_yuv(jpeg_data, yuv_image) {
        if frame_num.is_multiple_of(LOG_EVERY_N_FRAMES) {
            warn!(error = %e, subsamp = ?header.subsamp, "MJPEG decompress to YUV failed, skipping frame");
        }
        return None;
    }

    // Compute UV plane dimensions based on subsampling
    let (uv_w, uv_h) = match header.subsamp {
        turbojpeg::Subsamp::None => (width, height), // 4:4:4
        turbojpeg::Subsamp::Sub2x1 => (width.div_ceil(2), height), // 4:2:2
        turbojpeg::Subsamp::Sub1x2 => (width, height.div_ceil(2)), // 4:4:0
        turbojpeg::Subsamp::Gray => (0, 0),          // Grayscale
        _ => (width.div_ceil(2), height.div_ceil(2)), // 4:2:0
    };

    // Determine standard YUV format name from subsampling
    let format_name = match header.subsamp {
        turbojpeg::Subsamp::Sub2x2 => "I420",
        turbojpeg::Subsamp::Sub2x1 => "I422",
        turbojpeg::Subsamp::None => "I444",
        turbojpeg::Subsamp::Sub1x2 => "I440",
        turbojpeg::Subsamp::Gray => "Y800",
        _ => "YUV",
    };

    // Update decoded format and stream info on first frame
    if frame_num == 0 {
        info!(
            subsamp = ?header.subsamp,
            format_name,
            jpeg_size = jpeg_data.len(),
            yuv_size = buf_len,
            width,
            height,
            uv_w,
            uv_h,
            "First MJPEG frame decoded to {format_name} via turbojpeg"
        );

        if let Ok(mut d) = DIAGNOSTICS.write() {
            d.mjpeg_decoded_format = Some(format_name.to_string());
            // Update stream info pixel format to show actual decoded format
            if let Some(ref mut info) = d.preview_stream_info {
                info.1 = format!("{} (MJPEG)", format_name);
            }
        }
    }

    let y_size = width * height;
    let uv_size = uv_w * uv_h;

    let yuv_planes = YuvPlanes {
        y_offset: 0,
        y_size,
        uv_offset: y_size,
        uv_size,
        uv_stride: uv_w as u32,
        v_offset: y_size + uv_size,
        v_size: uv_size,
        v_stride: uv_w as u32,
        uv_width: uv_w as u32,
        uv_height: uv_h as u32,
    };

    // Copy into Arc<[u8]> for shared ownership across preview/still/channel.
    // The Vec buffer itself is reused across frames (no alloc after first frame).
    let data: Arc<[u8]> = Arc::from(&yuv_buf[..]);

    Some(CameraFrame {
        width: width as u32,
        height: height as u32,
        data: FrameData::Copied(data),
        format: PixelFormat::I420,
        stride: width as u32,
        yuv_planes: Some(yuv_planes),
        captured_at,
        sensor_timestamp_ns,
        libcamera_metadata: Some(metadata.clone()),
    })
}

/// Compute stride from pixel format, width, and buffer size
fn compute_stride(pf: PixelFormat, width: u32, buffer_size: usize, height: u32) -> u32 {
    if height == 0 {
        return width;
    }

    // For Bayer formats, derive stride from buffer size (may include padding)
    if pf.is_bayer() {
        return (buffer_size as u32) / height;
    }

    // For standard formats, compute from format's bytes per pixel
    let bpp = pf.bytes_per_pixel();
    let computed = (width as f32 * bpp) as u32;

    // Cross-check with buffer: if buffer is larger, use actual stride
    let buffer_stride = (buffer_size as u32) / height;

    // Use the larger of computed and buffer-derived stride
    // (buffer may have alignment padding)
    computed.max(buffer_stride)
}
