// SPDX-License-Identifier: GPL-3.0-only

//! Pipeline diagnostics statics and accessors for the insights handler.

use std::sync::RwLock;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

/// Whether a capture thread currently holds a CameraManager.
/// libcamera only allows one CameraManager at a time, so enumeration/format
/// queries must not create a new one while this is true.
pub(crate) static CAPTURE_ACTIVE: AtomicBool = AtomicBool::new(false);

/// Pipeline diagnostics accessible by the insights handler.
///
/// String/Option fields are behind a RwLock for consistent snapshots.
/// High-frequency counters use AtomicU64 to avoid lock contention on every frame.
#[derive(Default)]
pub(crate) struct PipelineDiagnostics {
    pub(crate) pipeline_string: Option<String>,
    pub(crate) is_multistream: bool,
    pub(crate) preview_stream_info: Option<(String, String)>,
    pub(crate) capture_stream_info: Option<(String, String)>,
    pub(crate) mjpeg_decoder_name: Option<String>,
    pub(crate) mjpeg_decoded_format: Option<String>,
    pub(crate) preview_role: Option<String>,
    pub(crate) capture_role: Option<String>,
}

/// Per-frame counters updated without holding the RwLock.
pub(crate) static PREVIEW_FRAME_COUNT: AtomicU64 = AtomicU64::new(0);
pub(crate) static STILL_FRAME_COUNT: AtomicU64 = AtomicU64::new(0);
pub(crate) static MJPEG_DECODE_TIME_US: AtomicU64 = AtomicU64::new(0);

pub(crate) static DIAGNOSTICS: RwLock<PipelineDiagnostics> = RwLock::new(PipelineDiagnostics {
    pipeline_string: None,
    is_multistream: false,
    preview_stream_info: None,
    capture_stream_info: None,
    mjpeg_decoder_name: None,
    mjpeg_decoded_format: None,
    preview_role: None,
    capture_role: None,
});

/// Check if a capture thread currently holds a CameraManager.
/// When true, no other code should create a CameraManager.
pub(crate) fn is_capture_active() -> bool {
    CAPTURE_ACTIVE.load(Ordering::Acquire)
}

// Accessors for insights handler (crate-internal)
pub(crate) fn get_pipeline_string() -> Option<String> {
    DIAGNOSTICS.read().ok()?.pipeline_string.clone()
}

pub(crate) fn get_is_multistream() -> bool {
    DIAGNOSTICS
        .read()
        .ok()
        .map(|d| d.is_multistream)
        .unwrap_or(false)
}

pub(crate) fn get_preview_stream_info() -> Option<(String, String, String, u64)> {
    let d = DIAGNOSTICS.read().ok()?;
    let info = d.preview_stream_info.clone()?;
    let role = d.preview_role.clone().unwrap_or_default();
    let count = PREVIEW_FRAME_COUNT.load(Ordering::Relaxed);
    Some((info.0, info.1, role, count))
}

pub(crate) fn get_mjpeg_decoder() -> Option<String> {
    DIAGNOSTICS.read().ok()?.mjpeg_decoder_name.clone()
}

pub(crate) fn get_mjpeg_decode_time_us() -> u64 {
    MJPEG_DECODE_TIME_US.load(Ordering::Relaxed)
}

pub(crate) fn get_mjpeg_decoded_format() -> Option<String> {
    DIAGNOSTICS.read().ok()?.mjpeg_decoded_format.clone()
}

pub(crate) fn get_capture_stream_info() -> Option<(String, String, String, u64)> {
    let d = DIAGNOSTICS.read().ok()?;
    let info = d.capture_stream_info.clone()?;
    let role = d.capture_role.clone().unwrap_or_default();
    let count = STILL_FRAME_COUNT.load(Ordering::Relaxed);
    Some((info.0, info.1, role, count))
}

/// Clear all global pipeline diagnostics (called on shutdown)
pub(crate) fn clear_global_diagnostics() {
    if let Ok(mut d) = DIAGNOSTICS.write() {
        *d = PipelineDiagnostics::default();
    }
    PREVIEW_FRAME_COUNT.store(0, Ordering::Relaxed);
    STILL_FRAME_COUNT.store(0, Ordering::Relaxed);
    MJPEG_DECODE_TIME_US.store(0, Ordering::Relaxed);
}

pub(crate) struct StreamDiag {
    pub(crate) size: libcamera::geometry::Size,
    pub(crate) format_name: String,
}

pub(crate) struct DiagnosticParams {
    pub(crate) is_multistream: bool,
    pub(crate) has_video_stream: bool,
    pub(crate) is_video_mode: bool,
    pub(crate) vf: StreamDiag,
    pub(crate) raw: StreamDiag,
    pub(crate) video: StreamDiag,
}

pub(crate) fn publish_diagnostics(p: DiagnosticParams) {
    let Ok(mut d) = DIAGNOSTICS.write() else {
        return;
    };

    let mut desc = format!(
        "libcamera ViewFinder {}x{} {}",
        p.vf.size.width, p.vf.size.height, p.vf.format_name
    );
    if p.has_video_stream {
        desc.push_str(&format!(
            " + VideoRecording {}x{} {}",
            p.video.size.width, p.video.size.height, p.video.format_name
        ));
    } else if p.is_multistream {
        desc.push_str(&format!(
            " + Raw {}x{} {}",
            p.raw.size.width, p.raw.size.height, p.raw.format_name
        ));
    }
    if p.is_video_mode && !p.has_video_stream {
        desc.push_str(" (video fallback: VF→encoder)");
    }
    d.pipeline_string = Some(desc);

    d.is_multistream = p.is_multistream || p.has_video_stream;

    // When MJPEG: initial label is "MJPEG"; updated to actual decoded
    // format (e.g. "I422 (MJPEG)") after first frame is decoded.
    d.preview_stream_info = Some((
        format!("{}x{}", p.vf.size.width, p.vf.size.height),
        p.vf.format_name.clone(),
    ));

    if p.has_video_stream {
        d.capture_stream_info = Some((
            format!("{}x{}", p.video.size.width, p.video.size.height),
            p.video.format_name,
        ));
        d.preview_role = Some("View-finder".to_string());
        d.capture_role = Some("Video-recording".to_string());
    } else if p.is_multistream {
        d.capture_stream_info = Some((
            format!("{}x{}", p.raw.size.width, p.raw.size.height),
            p.raw.format_name,
        ));
        d.preview_role = Some("View-finder".to_string());
        d.capture_role = Some("Raw".to_string());
    } else {
        d.preview_role = Some("View-finder (single)".to_string());
        d.capture_role = None;
    }
}
