// SPDX-License-Identifier: MPL-2.0

//! GStreamer encoder detection
//!
//! This module provides functionality to detect available video and audio
//! encoders in the GStreamer installation.

use gstreamer as gst;
use tracing::{debug, info};

/// Check if a specific GStreamer element is available
pub fn is_element_available(element_name: &str) -> bool {
    gst::init().ok();
    gst::ElementFactory::make(element_name).build().is_ok()
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
