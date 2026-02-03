// SPDX-License-Identifier: GPL-3.0-only

//! Storage utilities for managing photo and video files

use crate::constants::file_formats;
use std::path::PathBuf;
use std::sync::Arc;
use tracing::{debug, warn};

/// Load latest thumbnail for gallery button
///
/// Scans both photo and video directories for files, finds the most recent one,
/// and loads it as both an image handle and RGBA data for custom rendering.
/// For videos, extracts the first frame as a thumbnail.
/// Returns (Handle, RGBA bytes wrapped in Arc, width, height)
pub async fn load_latest_thumbnail(
    photos_dir: PathBuf,
    videos_dir: PathBuf,
) -> Option<(cosmic::widget::image::Handle, Arc<Vec<u8>>, u32, u32)> {
    // Get list of photo and video files from both directories (using blocking std::fs)
    let mut entries = tokio::task::spawn_blocking(move || {
        let mut files: Vec<(PathBuf, std::time::SystemTime)> = Vec::new();

        // Scan photos directory for image files
        if let Ok(entries) = std::fs::read_dir(&photos_dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if let Some(ext) = path.extension() {
                    let ext_str = ext.to_string_lossy().to_lowercase();
                    if file_formats::is_image_extension(&ext_str) {
                        if let Ok(metadata) = entry.metadata() {
                            if let Ok(modified) = metadata.modified() {
                                files.push((path, modified));
                            }
                        }
                    }
                }
            }
        }

        // Scan videos directory for video files
        if let Ok(entries) = std::fs::read_dir(&videos_dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if let Some(ext) = path.extension() {
                    let ext_str = ext.to_string_lossy().to_lowercase();
                    if file_formats::is_video_extension(&ext_str) {
                        if let Ok(metadata) = entry.metadata() {
                            if let Ok(modified) = metadata.modified() {
                                files.push((path, modified));
                            }
                        }
                    }
                }
            }
        }

        files
    })
    .await
    .ok()?;

    if entries.is_empty() {
        return None;
    }

    // Sort by modification time (newest first)
    entries.sort_by(|a, b| b.1.cmp(&a.1));

    let latest_path = entries.first()?.0.clone();
    let extension = latest_path
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_lowercase())
        .unwrap_or_default();

    debug!(path = ?latest_path, "Loading latest thumbnail");

    // Check if it's a video file
    if file_formats::is_video_extension(&extension) {
        return load_video_thumbnail(latest_path).await;
    }

    // Load image bytes
    let bytes = tokio::fs::read(&latest_path).await.ok()?;
    let bytes_clone = bytes.clone();

    // Decode image to RGBA in blocking task
    let (rgba_data, width, height) = tokio::task::spawn_blocking(move || {
        use image::GenericImageView;

        let img = image::load_from_memory(&bytes_clone).ok()?;
        let rgba = img.to_rgba8();
        let (width, height) = img.dimensions();

        Some((rgba.into_raw(), width, height))
    })
    .await
    .ok()??;

    let handle = cosmic::widget::image::Handle::from_bytes(bytes);

    Some((handle, Arc::new(rgba_data), width, height))
}

/// Load a thumbnail from a video file by extracting the first frame
async fn load_video_thumbnail(
    video_path: PathBuf,
) -> Option<(cosmic::widget::image::Handle, Arc<Vec<u8>>, u32, u32)> {
    debug!(path = ?video_path, "Extracting thumbnail from video");

    // Extract first frame from video in blocking task (uses GStreamer)
    let result = tokio::task::spawn_blocking(move || {
        use crate::backends::virtual_camera::load_preview_frame;

        match load_preview_frame(&video_path) {
            Ok(frame) => {
                let width = frame.width;
                let height = frame.height;
                let rgba_data: Vec<u8> = frame.data.to_vec();

                // Encode as PNG for the Handle
                let png_bytes = encode_rgba_to_png(&rgba_data, width, height)?;

                Some((png_bytes, rgba_data, width, height))
            }
            Err(e) => {
                warn!(error = ?e, "Failed to extract video thumbnail");
                None
            }
        }
    })
    .await
    .ok()??;

    let (png_bytes, rgba_data, width, height) = result;
    let handle = cosmic::widget::image::Handle::from_bytes(png_bytes);

    Some((handle, Arc::new(rgba_data), width, height))
}

/// Encode RGBA data to PNG bytes
fn encode_rgba_to_png(rgba_data: &[u8], width: u32, height: u32) -> Option<Vec<u8>> {
    use image::{ImageBuffer, Rgba};

    let img: ImageBuffer<Rgba<u8>, _> = ImageBuffer::from_raw(width, height, rgba_data.to_vec())?;

    let mut png_bytes = Vec::new();
    let mut cursor = std::io::Cursor::new(&mut png_bytes);

    img.write_to(&mut cursor, image::ImageFormat::Png).ok()?;

    Some(png_bytes)
}
