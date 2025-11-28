// SPDX-License-Identifier: MPL-2.0

//! Photo capture from camera backend
//!
//! This module handles capturing a single frame from the camera backend
//! in a non-blocking way that doesn't interrupt the preview stream.

use crate::backends::camera::CameraBackendManager;
use crate::backends::camera::types::CameraFrame;
use std::sync::Arc;
use tracing::{debug, error, info};

/// Photo capture handler
///
/// Responsible for grabbing a single frame from the camera backend
/// without interrupting the preview stream.
pub struct PhotoCapture;

impl PhotoCapture {
    /// Capture a photo from the camera backend
    ///
    /// This pulls a single frame from the camera. The frame data is immediately
    /// copied to avoid locking the camera backend.
    ///
    /// # Arguments
    /// * `backend` - Camera backend manager
    ///
    /// # Returns
    /// * `Ok(Arc<CameraFrame>)` - Captured frame (zero-copy via Arc)
    /// * `Err(String)` - Error message
    pub async fn capture_from_backend(
        backend: &CameraBackendManager,
    ) -> Result<Arc<CameraFrame>, String> {
        info!("Capturing photo from camera backend");

        // Capture frame from backend (this should be fast and non-blocking)
        let frame = backend
            .capture_photo()
            .map_err(|e| format!("Failed to capture photo: {}", e))?;

        debug!(
            width = frame.width,
            height = frame.height,
            format = ?frame.format,
            "Frame captured from backend"
        );

        // Wrap in Arc for zero-copy passing through pipeline
        Ok(Arc::new(frame))
    }

    /// Capture a photo from the current frame (fallback method)
    ///
    /// This is used when the backend doesn't support direct photo capture.
    /// It simply wraps the provided frame.
    ///
    /// # Arguments
    /// * `frame` - Current preview frame
    ///
    /// # Returns
    /// * `Ok(Arc<CameraFrame>)` - Frame wrapped in Arc
    pub fn capture_from_frame(frame: CameraFrame) -> Result<Arc<CameraFrame>, String> {
        debug!(
            width = frame.width,
            height = frame.height,
            "Using current preview frame for photo"
        );

        Ok(Arc::new(frame))
    }

    /// Capture with automatic fallback
    ///
    /// Tries to capture from backend first, falls back to using current frame.
    ///
    /// # Arguments
    /// * `backend` - Camera backend manager (optional)
    /// * `current_frame` - Current preview frame (fallback)
    ///
    /// # Returns
    /// * `Ok(Arc<CameraFrame>)` - Captured frame
    /// * `Err(String)` - Error message
    pub async fn capture_with_fallback(
        backend: Option<&CameraBackendManager>,
        current_frame: Option<CameraFrame>,
    ) -> Result<Arc<CameraFrame>, String> {
        // Try backend first
        if let Some(backend) = backend {
            match Self::capture_from_backend(backend).await {
                Ok(frame) => return Ok(frame),
                Err(e) => {
                    error!(error = %e, "Backend capture failed, trying fallback");
                }
            }
        }

        // Fallback to current frame
        if let Some(frame) = current_frame {
            return Self::capture_from_frame(frame);
        }

        Err("No frame available for capture".to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backends::camera::types::PixelFormat;

    #[test]
    fn test_capture_from_frame() {
        let frame = CameraFrame {
            width: 1920,
            height: 1080,
            data: Arc::from(vec![0u8; 1920 * 1080 * 4]), // RGBA size (4 bytes per pixel)
            format: PixelFormat::RGBA,
            stride: 1920 * 4, // RGBA stride
            captured_at: std::time::Instant::now(),
        };

        let captured = PhotoCapture::capture_from_frame(frame).unwrap();
        assert_eq!(captured.width, 1920);
        assert_eq!(captured.height, 1080);
    }
}
