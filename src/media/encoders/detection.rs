// SPDX-License-Identifier: GPL-3.0-only

//! GStreamer encoder detection
//!
//! This module provides functionality to detect available video and audio
//! encoders in the GStreamer installation.

use gstreamer as gst;
use gstreamer::prelude::*;
use tracing::{debug, info, warn};

/// Check if a specific GStreamer element is available
pub fn is_element_available(element_name: &str) -> bool {
    gst::init().ok();
    gst::ElementFactory::find(element_name).is_some()
}

/// Detect all available video encoders
///
/// Returns a list of available encoder names in no particular order.
pub fn detect_video_encoders() -> Vec<String> {
    gst::init().ok();

    let encoders = [
        // Hardware AV1
        "vaapiavcenc",
        "nvav1enc",
        // Hardware HEVC/H.265
        "vaapih265enc",
        "nvh265enc",
        "v4l2h265enc",
        // Hardware H.264
        "vaapih264enc",
        "nvh264enc",
        "v4l2h264enc",
        // Software HEVC/H.265
        "x265enc",
        // Software H.264
        "x264enc",
        "openh264enc",
    ];

    let mut available = Vec::new();

    for encoder in &encoders {
        if is_element_available(encoder) {
            debug!("Video encoder available: {}", encoder);
            available.push(encoder.to_string());
        }
    }

    info!("Detected {} video encoders", available.len());
    available
}

/// Detect all available audio encoders
///
/// Returns a list of available encoder names in no particular order.
pub fn detect_audio_encoders() -> Vec<String> {
    gst::init().ok();

    let encoders = [
        // Opus (preferred)
        "opusenc",
        // AAC (fallback)
        "avenc_aac",
        "faac",
        "voaacenc",
    ];

    let mut available = Vec::new();

    for encoder in &encoders {
        if is_element_available(encoder) {
            debug!("Audio encoder available: {}", encoder);
            available.push(encoder.to_string());
        }
    }

    info!("Detected {} audio encoders", available.len());
    available
}

/// Test whether a video encoder actually works by building a short pipeline
/// with `videotestsrc` and checking for negotiation or encoding errors.
///
/// Returns `true` if the encoder produced valid output, `false` if it failed.
fn probe_single_encoder(encoder_name: &str) -> bool {
    let _ = gst::init();

    // Determine the codec's parser from the encoder name
    let parser = if encoder_name.contains("h265") || encoder_name.contains("hevc") {
        "h265parse"
    } else if encoder_name.contains("h264")
        || encoder_name.contains("openh264")
        || encoder_name.contains("x264")
    {
        "h264parse"
    } else if encoder_name.contains("av1") {
        "av1parse"
    } else {
        // Unknown codec, skip probing
        return true;
    };

    let pipeline_desc = format!(
        "videotestsrc num-buffers=3 ! video/x-raw,format=NV12,width=320,height=240,framerate=10/1 \
         ! videoconvert ! {encoder} ! {parser} ! fakesink",
        encoder = encoder_name,
        parser = parser,
    );

    let pipeline = match gst::parse::launch(&pipeline_desc) {
        Ok(p) => p,
        Err(_) => return false,
    };

    let bus = match pipeline.bus() {
        Some(b) => b,
        None => return false,
    };

    if pipeline.set_state(gst::State::Playing).is_err() {
        let _ = pipeline.set_state(gst::State::Null);
        return false;
    }

    // Wait up to 5 seconds for EOS (success) or Error (broken)
    let ok = loop {
        match bus.timed_pop(gst::ClockTime::from_seconds(5)) {
            Some(msg) => match msg.view() {
                gst::MessageView::Eos(_) => break true,
                gst::MessageView::Error(_) => break false,
                _ => continue,
            },
            None => break false, // timeout
        }
    };

    let _ = pipeline.set_state(gst::State::Null);
    ok
}

/// Probe all given video encoder names and return the names of the broken ones.
///
/// Hardware encoders are tested sequentially (they may share a single V4L2 or
/// VA-API device that doesn't allow concurrent access). Software encoders are
/// tested in parallel for speed.
pub fn probe_broken_encoders(encoder_names: &[String]) -> Vec<String> {
    let _ = gst::init();

    let mut broken = Vec::new();

    // V4L2 encoders share CMA/IOMMU resources with the camera ISP on
    // embedded SoCs — probing them while the camera is active can crash
    // the capture pipeline.  Mark them as broken unconditionally so they
    // are removed from the settings UI.  The runtime fallback in
    // recorder.rs handles V4L2 → software encoder substitution if one
    // is ever selected.
    for name in encoder_names.iter().filter(|n| n.starts_with("v4l2")) {
        warn!(encoder = %name, "V4L2 encoder cannot be probed safely — marking as broken");
        broken.push(name.to_string());
    }

    let (hw, sw): (Vec<&String>, Vec<&String>) = encoder_names
        .iter()
        .filter(|name| !name.starts_with("v4l2"))
        .partition(|name| {
            name.starts_with("vaapi")
                || name.starts_with("va")
                || name.starts_with("nv")
                || name.starts_with("qsv")
                || name.starts_with("amf")
        });

    // Test hardware encoders sequentially
    for name in &hw {
        if !probe_single_encoder(name) {
            warn!(encoder = %name, "Hardware encoder failed probe — marking as broken");
            broken.push(name.to_string());
        } else {
            info!(encoder = %name, "Hardware encoder probe OK");
        }
    }

    // Test software encoders in parallel
    let sw_results: Vec<(String, bool)> = std::thread::scope(|s| {
        let handles: Vec<_> = sw
            .iter()
            .map(|name| {
                let name = name.to_string();
                s.spawn(move || {
                    let ok = probe_single_encoder(&name);
                    (name, ok)
                })
            })
            .collect();
        handles.into_iter().map(|h| h.join().unwrap()).collect()
    });

    for (name, ok) in sw_results {
        if !ok {
            warn!(encoder = %name, "Software encoder failed probe — marking as broken");
            broken.push(name);
        } else {
            info!(encoder = %name, "Software encoder probe OK");
        }
    }

    broken
}

/// Log all available encoders (for debugging)
pub fn log_available_encoders() {
    info!("=== GStreamer Encoder Detection ===");

    info!("Video encoders:");
    for encoder in detect_video_encoders() {
        info!("  ✓ {}", encoder);
    }

    info!("Audio encoders:");
    for encoder in detect_audio_encoders() {
        info!("  ✓ {}", encoder);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_detection_runs() {
        // Just ensure detection doesn't panic
        let _ = detect_video_encoders();
        let _ = detect_audio_encoders();
    }
}
