// SPDX-License-Identifier: GPL-3.0-only

//! Live recording-pipeline diagnostics and per-step counters.
//!
//! Centralises the global statics that the capture thread and the appsrc
//! pusher task increment on the hot path, and the snapshot accessors that
//! the insights drawer reads every tick. Splitting these out of
//! `recorder.rs` keeps the recorder file focused on pipeline construction.

use std::sync::RwLock;
use std::sync::atomic::{AtomicU64, Ordering};

/// Minimum elapsed seconds before computing effective FPS (avoids division by near-zero).
pub(super) const MIN_ELAPSED_FOR_FPS: f64 = 0.1;

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
/// and the appsrc pusher task. Read by the insights handler every tick.
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

/// Shared global counter set. `pub(super)` so the recorder hot path can
/// increment fields directly without going through accessor wrappers.
pub(super) static RECORDING_STATS: RecordingPipelineStats = RecordingPipelineStats {
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
pub(super) fn publish_recording_diagnostics(diag: RecordingDiagnostics) {
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
