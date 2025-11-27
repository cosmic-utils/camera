// SPDX-License-Identifier: MPL-2.0

//! Audio/video muxing logic
//!
//! This module handles muxing audio and video streams into a container format.

use gstreamer as gst;
use gstreamer::prelude::*;
use tracing::{debug, info};

/// Muxer configuration
pub struct MuxerConfig {
    /// Muxer element
    pub muxer: gst::Element,
    /// File sink element
    pub filesink: gst::Element,
    /// Output file path
    pub output_path: std::path::PathBuf,
}

/// Create muxer and filesink
///
/// # Arguments
/// * `muxer` - Pre-created muxer element
/// * `output_path` - Path to output file
///
/// # Returns
/// * `Ok(MuxerConfig)` - Muxer configuration
/// * `Err(String)` - Error message
pub fn create_muxer(
    muxer: gst::Element,
    output_path: std::path::PathBuf,
) -> Result<MuxerConfig, String> {
    info!(path = %output_path.display(), "Creating muxer");

    // Get muxer name for logging and specific configuration
    let muxer_name = muxer
        .factory()
        .map(|f| f.name().to_string())
        .unwrap_or_else(|| "unknown".to_string());

    // Configure muxer for proper file output (non-streamable)
    // This ensures duration and indexes are written for seekable playback
    if muxer.has_property("streamable") {
        let _ = muxer.set_property("streamable", false);
        info!(muxer = %muxer_name, "Configured muxer with streamable=false for seekable output");
    }

    // WebM-specific optimizations for proper duration writing
    if muxer_name == "webmmux" {
        // Ensure writing duration (streamable=false should handle this, but be explicit)
        debug!("WebM muxer detected - duration and cues will be written to file header/footer");
    }

    // Create filesink
    let filesink = gst::ElementFactory::make("filesink")
        .property("location", output_path.to_str().unwrap())
        .build()
        .map_err(|e| format!("Failed to create filesink: {}", e))?;

    debug!(muxer = %muxer_name, "Muxer and filesink created");

    Ok(MuxerConfig {
        muxer,
        filesink,
        output_path,
    })
}

/// Link video encoder to muxer
///
/// # Arguments
/// * `encoder` - Video encoder element (or parser if present)
/// * `muxer` - Muxer element
///
/// # Returns
/// * `Ok(())` - Success
/// * `Err(String)` - Error message
pub fn link_video_to_muxer(encoder: &gst::Element, muxer: &gst::Element) -> Result<(), String> {
    encoder
        .link(muxer)
        .map_err(|_| "Failed to link video encoder to muxer".to_string())?;

    debug!("Video encoder linked to muxer");
    Ok(())
}

/// Link audio encoder to muxer
///
/// # Arguments
/// * `encoder` - Audio encoder element
/// * `muxer` - Muxer element
///
/// # Returns
/// * `Ok(())` - Success
/// * `Err(String)` - Error message
pub fn link_audio_to_muxer(encoder: &gst::Element, muxer: &gst::Element) -> Result<(), String> {
    encoder
        .link(muxer)
        .map_err(|_| "Failed to link audio encoder to muxer".to_string())?;

    debug!("Audio encoder linked to muxer");
    Ok(())
}

/// Link muxer to filesink
///
/// # Arguments
/// * `muxer` - Muxer element
/// * `filesink` - Filesink element
///
/// # Returns
/// * `Ok(())` - Success
/// * `Err(String)` - Error message
pub fn link_muxer_to_sink(muxer: &gst::Element, filesink: &gst::Element) -> Result<(), String> {
    muxer
        .link(filesink)
        .map_err(|_| "Failed to link muxer to filesink".to_string())?;

    debug!("Muxer linked to filesink");
    Ok(())
}
