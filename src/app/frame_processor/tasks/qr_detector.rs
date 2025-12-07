// SPDX-License-Identifier: GPL-3.0-only

//! QR code detection task
//!
//! This module implements QR code detection using the bardecoder crate.
//! It converts camera frames to grayscale and searches for QR codes,
//! returning their positions and decoded content.

use crate::app::frame_processor::types::{FrameRegion, QrDetection};
use crate::backends::camera::types::CameraFrame;
use bardecoder::detect::{Detect, LineScan, Location};
use bardecoder::prepare::{BlockedMean, Prepare};
use image024::{ImageBuffer, Rgba};
use std::sync::Arc;
use tracing::{debug, trace, warn};

/// QR code detector
///
/// Analyzes camera frames to detect and decode QR codes.
/// Optimized for real-time processing with frame downscaling.
pub struct QrDetector {
    /// Maximum dimension for processing (frames are downscaled to this)
    max_dimension: u32,
}

impl Default for QrDetector {
    fn default() -> Self {
        Self::new()
    }
}

impl QrDetector {
    /// Create a new QR detector with default settings
    pub fn new() -> Self {
        Self {
            // Process at 640px max for better performance
            // QR codes are typically large enough to be detected at this resolution
            max_dimension: 640,
        }
    }

    /// Create a QR detector with custom max dimension
    pub fn with_max_dimension(max_dimension: u32) -> Self {
        Self { max_dimension }
    }

    /// Detect QR codes in a camera frame
    ///
    /// This is an async-friendly method that performs CPU-intensive work.
    /// The frame is converted to grayscale and optionally downscaled for
    /// faster processing.
    pub async fn detect(&self, frame: Arc<CameraFrame>) -> Vec<QrDetection> {
        let max_dim = self.max_dimension;

        // Run detection in a blocking task to avoid blocking the async runtime
        tokio::task::spawn_blocking(move || detect_sync(&frame, max_dim))
            .await
            .unwrap_or_else(|e| {
                warn!(error = %e, "QR detection task panicked");
                Vec::new()
            })
    }
}

/// Synchronous QR detection (runs in blocking task)
fn detect_sync(frame: &CameraFrame, max_dimension: u32) -> Vec<QrDetection> {
    let start = std::time::Instant::now();

    let width = frame.width;
    let height = frame.height;

    // Convert RGBA frame data to image024::RgbaImage for bardecoder
    // Also handle optional downscaling for performance
    let (rgba_image, proc_width, proc_height, scale) = if width > max_dimension
        || height > max_dimension
    {
        let scale = (width as f32 / max_dimension as f32).max(height as f32 / max_dimension as f32);
        let new_width = (width as f32 / scale) as u32;
        let new_height = (height as f32 / scale) as u32;

        let downscaled = downscale_rgba(frame, new_width, new_height);
        let img: ImageBuffer<Rgba<u8>, Vec<u8>> =
            ImageBuffer::from_raw(new_width, new_height, downscaled)
                .expect("RGBA data should match dimensions");
        (img, new_width, new_height, scale)
    } else {
        // Copy frame data without stride padding
        let rgba_data = copy_rgba_without_stride(frame);
        let img: ImageBuffer<Rgba<u8>, Vec<u8>> =
            ImageBuffer::from_raw(width, height, rgba_data)
                .expect("RGBA data should match dimensions");
        (img, width, height, 1.0)
    };

    let conversion_time = start.elapsed();
    trace!(
        proc_width,
        proc_height,
        scale,
        conversion_ms = conversion_time.as_millis(),
        "Prepared RGBA image for processing"
    );

    // Prepare image using BlockedMean (default parameters: 7x9 block size)
    // This converts RGBA to grayscale internally
    let prepare = BlockedMean::new(7, 9);
    let prepared = prepare.prepare(&rgba_image);

    // Detect QR codes to get their locations
    let detector = LineScan {};
    let locations = detector.detect(&prepared);

    let detection_time = start.elapsed() - conversion_time;
    trace!(
        count = locations.len(),
        detection_ms = detection_time.as_millis(),
        "QR detection complete"
    );

    // Use the decoder to get the decoded content
    let decoder = bardecoder::default_decoder();
    let results = decoder.decode(&rgba_image);

    // Match locations with decoded content
    // Detection and decoding should return results in the same order
    let mut detections = Vec::with_capacity(locations.len());

    for (location, result) in locations.into_iter().zip(results.into_iter()) {
        let content = match result {
            Ok(content) => content,
            Err(e) => {
                debug!(error = %e, "Failed to decode QR code");
                continue;
            }
        };

        // Extract QRLocation from the Location enum
        let qr_loc = match location {
            Location::QR(loc) => loc,
        };

        // Calculate bounding box from the three finder pattern centers
        // QRLocation provides top_left, top_right, bottom_left (centers of finder patterns)
        let top_left = &qr_loc.top_left;
        let top_right = &qr_loc.top_right;
        let bottom_left = &qr_loc.bottom_left;

        // Calculate the fourth corner (bottom_right) by vector addition
        // bottom_right = bottom_left + (top_right - top_left)
        let bottom_right_x = bottom_left.x + (top_right.x - top_left.x);
        let bottom_right_y = bottom_left.y + (top_right.y - top_left.y);

        // Calculate bounding box with some padding for the finder patterns
        // The finder patterns have a size of about 7 modules
        let padding = qr_loc.module_size * 3.5; // Half of 7 modules

        let min_x = (top_left.x.min(bottom_left.x) - padding).max(0.0) as f32;
        let max_x = (top_right.x.max(bottom_right_x) + padding).min(proc_width as f64) as f32;
        let min_y = (top_left.y.min(top_right.y) - padding).max(0.0) as f32;
        let max_y = (bottom_left.y.max(bottom_right_y) + padding).min(proc_height as f64) as f32;

        // Scale back to original frame coordinates
        let x = min_x * scale;
        let y = min_y * scale;
        let qr_width = (max_x - min_x) * scale;
        let qr_height = (max_y - min_y) * scale;

        // Convert to normalized coordinates
        let region = FrameRegion::from_pixels(
            x as u32,
            y as u32,
            qr_width as u32,
            qr_height as u32,
            width,
            height,
        );

        debug!(
            content = %content,
            x = region.x,
            y = region.y,
            width = region.width,
            height = region.height,
            "Detected QR code"
        );

        detections.push(QrDetection::new(region, content));
    }

    let total_time = start.elapsed();
    if !detections.is_empty() {
        debug!(
            count = detections.len(),
            total_ms = total_time.as_millis(),
            "QR detection found codes"
        );
    }

    detections
}

/// Copy RGBA frame data without stride padding
fn copy_rgba_without_stride(frame: &CameraFrame) -> Vec<u8> {
    let width = frame.width as usize;
    let height = frame.height as usize;
    let stride = frame.stride as usize;

    let mut result = Vec::with_capacity(width * height * 4);

    for y in 0..height {
        let row_start = y * stride;
        let row_end = row_start + width * 4;
        if row_end <= frame.data.len() {
            result.extend_from_slice(&frame.data[row_start..row_end]);
        }
    }

    result
}

/// Downscale RGBA frame using bilinear interpolation
fn downscale_rgba(frame: &CameraFrame, dst_width: u32, dst_height: u32) -> Vec<u8> {
    let src_width = frame.width as usize;
    let src_height = frame.height as usize;
    let stride = frame.stride as usize;

    let mut result = Vec::with_capacity((dst_width * dst_height * 4) as usize);

    let x_ratio = src_width as f32 / dst_width as f32;
    let y_ratio = src_height as f32 / dst_height as f32;

    for y in 0..dst_height {
        for x in 0..dst_width {
            let src_x = x as f32 * x_ratio;
            let src_y = y as f32 * y_ratio;

            let x0 = src_x as usize;
            let y0 = src_y as usize;
            let x1 = (x0 + 1).min(src_width - 1);
            let y1 = (y0 + 1).min(src_height - 1);

            let x_frac = src_x - x0 as f32;
            let y_frac = src_y - y0 as f32;

            // Calculate pixel offsets accounting for stride
            let get_pixel = |px: usize, py: usize, channel: usize| -> f32 {
                let offset = py * stride + px * 4 + channel;
                frame.data.get(offset).copied().unwrap_or(0) as f32
            };

            // Bilinear interpolation for each channel (RGBA)
            for channel in 0..4 {
                let p00 = get_pixel(x0, y0, channel);
                let p01 = get_pixel(x1, y0, channel);
                let p10 = get_pixel(x0, y1, channel);
                let p11 = get_pixel(x1, y1, channel);

                let value = p00 * (1.0 - x_frac) * (1.0 - y_frac)
                    + p01 * x_frac * (1.0 - y_frac)
                    + p10 * (1.0 - x_frac) * y_frac
                    + p11 * x_frac * y_frac;

                result.push(value as u8);
            }
        }
    }

    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backends::camera::types::PixelFormat;

    #[test]
    fn test_copy_rgba_without_stride() {
        // Create a simple 2x2 RGBA frame with extra stride padding
        let data: Vec<u8> = vec![
            255, 0, 0, 255, // Red pixel
            0, 255, 0, 255, // Green pixel
            0, 0,           // stride padding
            0, 0, 255, 255, // Blue pixel
            255, 255, 255, 255, // White pixel
            0, 0,           // stride padding
        ];

        let frame = CameraFrame {
            width: 2,
            height: 2,
            data: Arc::from(data.as_slice()),
            format: PixelFormat::RGBA,
            stride: 10, // 2 pixels * 4 bytes + 2 padding = 10 bytes per row
            captured_at: std::time::Instant::now(),
        };

        let result = copy_rgba_without_stride(&frame);
        assert_eq!(result.len(), 16); // 2x2 pixels * 4 channels = 16 bytes

        // Check that stride padding was removed
        assert_eq!(&result[0..4], &[255, 0, 0, 255]); // Red
        assert_eq!(&result[4..8], &[0, 255, 0, 255]); // Green
        assert_eq!(&result[8..12], &[0, 0, 255, 255]); // Blue
        assert_eq!(&result[12..16], &[255, 255, 255, 255]); // White
    }

    #[test]
    fn test_downscale_rgba() {
        // 4x2 RGBA image with gradient in red channel
        let data: Vec<u8> = vec![
            // Row 0
            0, 0, 0, 255,     // pixel (0,0) - black
            85, 0, 0, 255,    // pixel (1,0) - dark red
            170, 0, 0, 255,   // pixel (2,0) - medium red
            255, 0, 0, 255,   // pixel (3,0) - bright red
            // Row 1
            0, 0, 0, 255,
            85, 0, 0, 255,
            170, 0, 0, 255,
            255, 0, 0, 255,
        ];

        let frame = CameraFrame {
            width: 4,
            height: 2,
            data: Arc::from(data.as_slice()),
            format: PixelFormat::RGBA,
            stride: 16, // 4 pixels * 4 bytes = 16 bytes per row
            captured_at: std::time::Instant::now(),
        };

        let result = downscale_rgba(&frame, 2, 1);
        assert_eq!(result.len(), 8); // 2x1 pixels * 4 channels = 8 bytes

        // First pixel samples around (0,0), second around (2,0)
        assert!(result[0] < 100); // Near start of gradient (red channel)
        assert!(result[4] > 150); // Near end of gradient (red channel)
    }
}
