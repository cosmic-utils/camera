// SPDX-License-Identifier: GPL-3.0-only

//! Shared GStreamer audio-level plumbing used by the recorder and the
//! pre-recording level probe.

use std::sync::Arc;
use std::sync::Mutex;

use gstreamer as gst;
use gstreamer::prelude::*;

/// PulseAudio slave method shared by every `pulsesrc` in the project.
///
/// Pinning this to a single constant guarantees the pre-recording probe and
/// the real recorder see the audio source the same way, so the meter in
/// settings shows what the recording will record.
pub const PULSESRC_SLAVE_METHOD: &str = "skew";

/// Live audio level data shared between a GStreamer pipeline and the UI.
#[derive(Debug, Clone)]
pub struct AudioLevels {
    /// Per-input-channel peak levels in dB (before mono mix).
    pub input_peak_db: Vec<f64>,
    /// Per-input-channel RMS levels in dB (before mono mix).
    pub input_rms_db: Vec<f64>,
    /// Mono output peak level in dB (after mix).
    pub output_peak_db: f64,
    /// Mono output RMS level in dB (after mix).
    pub output_rms_db: f64,
}

impl Default for AudioLevels {
    fn default() -> Self {
        Self {
            input_peak_db: Vec::new(),
            input_rms_db: Vec::new(),
            output_peak_db: -100.0,
            output_rms_db: -100.0,
        }
    }
}

/// Thread-safe handle to live audio levels.
pub type SharedAudioLevels = Arc<Mutex<AudioLevels>>;

/// Install a bus sync handler that intercepts `level` element messages in
/// the GStreamer streaming thread and updates [`SharedAudioLevels`].
///
/// Level messages are dropped before they reach the async bus queue. All
/// other messages (Eos, Error, Warning, etc.) pass through normally, so
/// the recorder's `stop()` can use `timed_pop_filtered` without races.
pub fn install_level_sync_handler(pipeline: &gst::Pipeline, levels: &SharedAudioLevels) {
    let Some(bus) = pipeline.bus() else { return };
    let levels = Arc::clone(levels);

    bus.set_sync_handler(move |_, msg| {
        let gst::MessageView::Element(e) = msg.view() else {
            return gst::BusSyncReply::Pass;
        };
        let Some(structure) = e.structure() else {
            return gst::BusSyncReply::Pass;
        };
        if structure.name() != "level" {
            return gst::BusSyncReply::Pass;
        }

        let src_name = msg.src().map(|s| s.name().to_string()).unwrap_or_default();

        let peak_db = structure
            .get::<gst::glib::ValueArray>("peak")
            .ok()
            .map(|list| list.iter().filter_map(|v| v.get::<f64>().ok()).collect())
            .unwrap_or_default();
        let rms_db = structure
            .get::<gst::glib::ValueArray>("rms")
            .ok()
            .map(|list| list.iter().filter_map(|v| v.get::<f64>().ok()).collect())
            .unwrap_or_default();

        if let Ok(mut lock) = levels.lock() {
            if src_name == "audio-level-input" {
                lock.input_peak_db = peak_db;
                lock.input_rms_db = rms_db;
            } else if src_name == "audio-level-output" {
                lock.output_peak_db = peak_db.first().copied().unwrap_or(-100.0);
                lock.output_rms_db = rms_db.first().copied().unwrap_or(-100.0);
            }
        }

        // Drop level messages — don't clutter the bus queue.
        gst::BusSyncReply::Drop
    });
}
