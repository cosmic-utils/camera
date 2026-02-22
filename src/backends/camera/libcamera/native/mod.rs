// SPDX-License-Identifier: GPL-3.0-only

//! Native libcamera pipeline using libcamera-rs bindings
//!
//! Replaces the GStreamer-based multi-stream pipeline with direct libcamera access,
//! providing:
//! - Direct buffer lifecycle control (no GStreamer overhead)
//! - Per-frame metadata (exposure, gain, colour temperature)
//! - Direct camera property access (model, location, rotation)
//!
//! # Architecture
//!
//! All libcamera objects (CameraManager, ActiveCamera, FrameBuffers) live on a
//! dedicated capture thread. This avoids Send issues with libcamera's raw pointers
//! and keeps the entire pipeline lifecycle on one thread.
//!
//! ```text
//! ┌──────────────────────┐         ┌───────────────────────┐
//! │  NativeLibcamera     │         │   Capture Thread      │
//! │  Pipeline (main)     │         │                       │
//! │                      │  init   │  CameraManager        │
//! │  stop_flag ──────────┼────────►│  ActiveCamera         │
//! │  still_requested ────┼────────►│  FrameBuffers         │
//! │  latest_preview ◄────┼─────────│  Request loop         │
//! │  latest_still   ◄────┼─────────│                       │
//! │  frame_sender   ◄────┼─────────│  (all libcamera ops)  │
//! └──────────────────────┘         └───────────────────────┘
//! ```

mod capture_thread;
pub(crate) mod diagnostics;
pub(crate) mod pixel_formats;

pub(crate) use diagnostics::is_capture_active;

use crate::backends::camera::types::*;
use capture_thread::{CaptureThreadInitResult, CaptureThreadParams, capture_thread_main};
use diagnostics::clear_global_diagnostics;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use tracing::{debug, error, info};

/// Shared communication handles passed into the native pipeline
pub(crate) struct PipelineSharedState {
    pub(crate) frame_sender: FrameSender,
    pub(crate) still_requested: Arc<AtomicBool>,
    pub(crate) still_frame: Arc<Mutex<Option<CameraFrame>>>,
    pub(crate) recording_sender: Arc<Mutex<Option<tokio::sync::mpsc::Sender<Arc<CameraFrame>>>>>,
}

/// Native libcamera pipeline using direct libcamera-rs bindings
///
/// All libcamera objects live on a dedicated capture thread.
/// The main thread communicates via atomic flags and mutexed shared state.
pub(crate) struct NativeLibcameraPipeline {
    /// Capture processing thread (owns all libcamera objects)
    capture_thread: Option<JoinHandle<()>>,
    /// Stop flag for capture thread
    stop_flag: Arc<AtomicBool>,
    /// Still capture requested flag
    still_capture_requested: Arc<AtomicBool>,
    /// Latest preview frame
    latest_preview: Arc<Mutex<Option<CameraFrame>>>,
    /// Latest still frame (full resolution)
    latest_still: Arc<Mutex<Option<CameraFrame>>>,
    /// Preview frame counter
    preview_frame_count: Arc<AtomicU64>,
    /// Still frame counter
    still_frame_count: Arc<AtomicU64>,
    /// Dynamically-settable sender for direct recording path (bypasses UI thread).
    /// Set to Some(tx) when recording starts, None when recording stops.
    /// Kept alive here so the Arc is not dropped while the capture thread holds a clone.
    _recording_sender: Arc<Mutex<Option<tokio::sync::mpsc::Sender<Arc<CameraFrame>>>>>,
}

impl NativeLibcameraPipeline {
    /// Create and start a new native libcamera pipeline
    ///
    /// # Arguments
    /// * `camera_id` - libcamera camera ID (from enumeration)
    /// * `preview_format` - Format for preview stream (typically 1080p or lower)
    /// * `supports_multistream` - Whether camera supports dual-stream capture
    /// * `video_mode` - Whether to configure for video recording
    /// * `shared` - Shared communication handles (frame sender, still capture, recording)
    pub(crate) fn new(
        camera_id: &str,
        preview_format: &CameraFormat,
        supports_multistream: bool,
        video_mode: bool,
        shared: PipelineSharedState,
    ) -> BackendResult<Self> {
        info!(
            camera = camera_id,
            preview = %preview_format,
            multistream = supports_multistream,
            "Creating native libcamera pipeline"
        );

        // Shared state
        let stop_flag = Arc::new(AtomicBool::new(false));
        let latest_preview = Arc::new(Mutex::new(None));
        let preview_frame_count = Arc::new(AtomicU64::new(0));
        let still_frame_count = Arc::new(AtomicU64::new(0));

        // Channel for thread to report initialization result
        let (init_tx, init_rx) =
            std::sync::mpsc::sync_channel::<Result<CaptureThreadInitResult, BackendError>>(1);

        let params = CaptureThreadParams {
            camera_id: camera_id.to_string(),
            preview_width: preview_format.width,
            preview_height: preview_format.height,
            supports_multistream,
            video_mode,
            stop_flag: Arc::clone(&stop_flag),
            latest_preview: Arc::clone(&latest_preview),
            latest_still: Arc::clone(&shared.still_frame),
            still_requested: Arc::clone(&shared.still_requested),
            preview_frame_count: Arc::clone(&preview_frame_count),
            still_frame_count: Arc::clone(&still_frame_count),
            frame_sender: shared.frame_sender,
            recording_sender: Arc::clone(&shared.recording_sender),
        };

        // Spawn capture thread - it owns all libcamera objects
        let capture_thread = std::thread::Builder::new()
            .name("libcamera-capture".to_string())
            .spawn(move || {
                capture_thread_main(params, init_tx);
            })
            .map_err(|e| {
                BackendError::InitializationFailed(format!("Spawn capture thread: {}", e))
            })?;

        // Wait for initialization result from capture thread
        let init_result = init_rx.recv().map_err(|_| {
            BackendError::InitializationFailed(
                "Capture thread died during initialization".to_string(),
            )
        })??;

        info!(
            multistream = init_result.is_multistream,
            has_video_stream = init_result.has_video_stream,
            "Native libcamera pipeline started"
        );

        Ok(Self {
            capture_thread: Some(capture_thread),
            stop_flag,
            still_capture_requested: shared.still_requested,
            latest_preview,
            latest_still: shared.still_frame,
            preview_frame_count,
            still_frame_count,
            _recording_sender: shared.recording_sender,
        })
    }

    /// Request a still capture (full resolution)
    pub(crate) fn request_still_capture(&self) {
        debug!("Still capture requested");
        self.still_capture_requested.store(true, Ordering::Relaxed);
    }

    /// Get the latest still frame (if available)
    pub(crate) fn get_still_frame(&self) -> Option<CameraFrame> {
        self.latest_still
            .lock()
            .ok()
            .and_then(|mut guard| guard.take())
    }

    /// Get the latest preview frame
    pub(crate) fn get_preview_frame(&self) -> Option<CameraFrame> {
        self.latest_preview
            .lock()
            .ok()
            .and_then(|guard| guard.clone())
    }

    /// Get frame counts for debugging
    fn frame_counts(&self) -> (u64, u64) {
        (
            self.preview_frame_count.load(Ordering::Relaxed),
            self.still_frame_count.load(Ordering::Relaxed),
        )
    }

    /// Stop the pipeline
    pub(crate) fn stop(&self) -> BackendResult<()> {
        info!("Stopping native libcamera pipeline");
        clear_global_diagnostics();
        self.stop_flag.store(true, Ordering::Release);
        Ok(())
    }
}

impl Drop for NativeLibcameraPipeline {
    fn drop(&mut self) {
        clear_global_diagnostics();
        self.stop_flag.store(true, Ordering::Release);

        // Wait for capture thread to finish (it will stop camera and drop all libcamera objects)
        if let Some(thread) = self.capture_thread.take()
            && let Err(e) = thread.join()
        {
            error!("Capture thread panicked: {:?}", e);
        }

        // Delay for hardware release - the "simple" pipeline handler needs time
        // to fully release V4L2 resources before a new pipeline can start.
        // 200ms was insufficient and caused "Operation timed out" on rapid switching.
        std::thread::sleep(std::time::Duration::from_millis(500));

        let (preview_count, still_count) = self.frame_counts();
        info!(
            preview_frames = preview_count,
            still_frames = still_count,
            "Native libcamera pipeline dropped"
        );
    }
}
