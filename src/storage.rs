// SPDX-License-Identifier: MPL-2.0

//! Storage utilities for managing photo and video files

use std::path::PathBuf;
use std::sync::Arc;
use tracing::debug;

/// Load latest thumbnail for gallery button
///
/// Scans the photos directory for JPEG and PNG files, finds the most recent one,
/// and loads it as both an image handle and RGBA data for custom rendering.
/// Returns (Handle, RGBA bytes wrapped in Arc, width, height)
pub async fn load_latest_thumbnail(
    photos_dir: PathBuf,
) -> Option<(cosmic::widget::image::Handle, Arc<Vec<u8>>, u32, u32)> {
    // Get list of photo files (using blocking std::fs)
    let photos_dir_clone = photos_dir.clone();
    let mut entries = tokio::task::spawn_blocking(move || {
        let mut files = Vec::new();
        if let Ok(entries) = std::fs::read_dir(&photos_dir_clone) {
            for entry in entries.flatten() {
                let path = entry.path();
                if let Some(ext) = path.extension() {
                    let ext_str = ext.to_string_lossy();
                    if ext_str.eq_ignore_ascii_case("jpg") || ext_str.eq_ignore_ascii_case("png") {
                        files.push(entry);
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
    entries.sort_by_key(|e| {
        e.metadata()
            .ok()
            .and_then(|m| m.modified().ok())
            .map(|t| std::cmp::Reverse(t))
    });

    let latest_path = entries.first()?.path();

    debug!(path = ?latest_path, "Loading latest thumbnail");

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
