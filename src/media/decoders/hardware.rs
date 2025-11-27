// SPDX-License-Identifier: MPL-2.0

//! Hardware decoder detection for video formats

use tracing::{debug, info};

/// Detect available hardware decoders for JPEG/MJPEG
///
/// Returns list of TRUE hardware decoders (VA-API, NVDEC, V4L2).
/// Does NOT include software decoders like avdec_mjpeg or jpegdec.
///
/// # Returns
/// Vector of decoder element names that are available on this system.
pub fn detect_hw_decoders() -> Vec<&'static str> {
    debug!("Detecting available hardware decoders");
    let mut available = Vec::new();

    // List of TRUE hardware decoder candidates (NOT software decoders like avdec_mjpeg)
    let hw_candidates = vec![
        ("vaapijpegdec", "VA-API JPEG decoder"), // Intel/AMD VA-API hardware
        ("nvjpegdec", "NVIDIA JPEG decoder"),    // NVIDIA NVDEC hardware
        ("v4l2jpegdec", "V4L2 JPEG decoder"),    // Hardware V4L2
    ];

    for (decoder, desc) in hw_candidates {
        // Try to create the element to see if it exists
        if gstreamer::ElementFactory::make(decoder).build().is_ok() {
            info!("✓ {} available - HARDWARE ACCELERATION SUPPORTED", desc);
            available.push(decoder);
        } else {
            debug!("✗ {} not available", desc);
        }
    }

    if available.is_empty() {
        info!("No hardware decoders available, will use software decoder");
    } else {
        info!("Found {} hardware decoder(s)", available.len());
    }

    available
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_detect_hw_decoders() {
        // Initialize GStreamer for testing
        let _ = gstreamer::init();

        // Just ensure it doesn't crash
        let decoders = detect_hw_decoders();

        // We can't assert specific decoders since it depends on the system
        // but we can check the return type is correct
        assert!(decoders.iter().all(|s| !s.is_empty()));
    }
}
