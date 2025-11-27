// SPDX-License-Identifier: MPL-2.0

//! QR code detection task
//!
//! This module implements QR code detection using the rqrr crate.
//! It converts camera frames to grayscale and searches for QR codes,
//! returning their positions and decoded content.

use crate::app::frame_processor::types::{FrameRegion, QrDetection};
use crate::backends::camera::types::{CameraFrame, PixelFormat};
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

    // Convert frame to grayscale
    let (gray_data, width, height) = match frame.format {
        PixelFormat::NV12 => convert_nv12_to_gray(frame),
        PixelFormat::RGBA => convert_rgba_to_gray(frame),
    };

    let conversion_time = start.elapsed();
    trace!(
        width,
        height,
        conversion_ms = conversion_time.as_millis(),
        "Converted frame to grayscale"
    );

    // Downscale if needed for faster processing
    let (gray_data, proc_width, proc_height, scale) = if width > max_dimension
        || height > max_dimension
    {
        let scale = (width as f32 / max_dimension as f32).max(height as f32 / max_dimension as f32);
        let new_width = (width as f32 / scale) as u32;
        let new_height = (height as f32 / scale) as u32;

        let downscaled = downscale_gray(&gray_data, width, height, new_width, new_height);
        (downscaled, new_width, new_height, scale)
    } else {
        (gray_data, width, height, 1.0)
    };

    let downscale_time = start.elapsed() - conversion_time;
    trace!(
        proc_width,
        proc_height,
        scale,
        downscale_ms = downscale_time.as_millis(),
        "Downscaled for processing"
    );

    // Create grayscale image for rqrr
    let mut img = match rqrr::PreparedImage::prepare_from_greyscale(
        proc_width as usize,
        proc_height as usize,
        |x, y| gray_data[y * proc_width as usize + x],
    ) {
        img => img,
    };

    // Detect QR codes
    let grids = img.detect_grids();

    let detection_time = start.elapsed() - conversion_time - downscale_time;
    trace!(
        count = grids.len(),
        detection_ms = detection_time.as_millis(),
        "QR detection complete"
    );

    // Decode and convert to our format
    let mut detections = Vec::with_capacity(grids.len());

    for grid in grids {
        // Decode the QR content
        let content = match grid.decode() {
            Ok((_, content)) => content,
            Err(e) => {
                debug!(error = %e, "Failed to decode QR code");
                continue;
            }
        };

        // Get bounding box from grid bounds
        let bounds = grid.bounds;

        // Find min/max coordinates of the QR code corners (Point uses i32)
        let min_x = bounds.iter().map(|p| p.x).min().unwrap_or(0) as f32;
        let max_x = bounds.iter().map(|p| p.x).max().unwrap_or(0) as f32;
        let min_y = bounds.iter().map(|p| p.y).min().unwrap_or(0) as f32;
        let max_y = bounds.iter().map(|p| p.y).max().unwrap_or(0) as f32;

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

/// Convert NV12 frame to grayscale (Y plane only)
fn convert_nv12_to_gray(frame: &CameraFrame) -> (Vec<u8>, u32, u32) {
    let width = frame.width;
    let height = frame.height;
    let stride = frame.stride_y as usize;

    // Y plane is the grayscale data - just need to copy with proper stride
    let mut gray = Vec::with_capacity((width * height) as usize);

    for y in 0..height as usize {
        let row_start = y * stride;
        let row_end = row_start + width as usize;
        if row_end <= frame.data.len() {
            gray.extend_from_slice(&frame.data[row_start..row_end]);
        }
    }

    (gray, width, height)
}

/// Convert RGBA frame to grayscale
fn convert_rgba_to_gray(frame: &CameraFrame) -> (Vec<u8>, u32, u32) {
    let width = frame.width;
    let height = frame.height;
    let pixels = (width * height) as usize;

    let mut gray = Vec::with_capacity(pixels);

    for i in 0..pixels {
        let offset = i * 4;
        if offset + 2 < frame.data.len() {
            let r = frame.data[offset] as u32;
            let g = frame.data[offset + 1] as u32;
            let b = frame.data[offset + 2] as u32;
            // Standard luminance formula: 0.299*R + 0.587*G + 0.114*B
            let gray_val = ((r * 77 + g * 150 + b * 29) >> 8) as u8;
            gray.push(gray_val);
        }
    }

    (gray, width, height)
}

/// Downscale grayscale image using bilinear interpolation
fn downscale_gray(
    data: &[u8],
    src_width: u32,
    src_height: u32,
    dst_width: u32,
    dst_height: u32,
) -> Vec<u8> {
    let mut result = Vec::with_capacity((dst_width * dst_height) as usize);

    let x_ratio = src_width as f32 / dst_width as f32;
    let y_ratio = src_height as f32 / dst_height as f32;

    for y in 0..dst_height {
        for x in 0..dst_width {
            let src_x = x as f32 * x_ratio;
            let src_y = y as f32 * y_ratio;

            let x0 = src_x as u32;
            let y0 = src_y as u32;
            let x1 = (x0 + 1).min(src_width - 1);
            let y1 = (y0 + 1).min(src_height - 1);

            let x_frac = src_x - x0 as f32;
            let y_frac = src_y - y0 as f32;

            let idx00 = (y0 * src_width + x0) as usize;
            let idx01 = (y0 * src_width + x1) as usize;
            let idx10 = (y1 * src_width + x0) as usize;
            let idx11 = (y1 * src_width + x1) as usize;

            let p00 = data.get(idx00).copied().unwrap_or(0) as f32;
            let p01 = data.get(idx01).copied().unwrap_or(0) as f32;
            let p10 = data.get(idx10).copied().unwrap_or(0) as f32;
            let p11 = data.get(idx11).copied().unwrap_or(0) as f32;

            let value = p00 * (1.0 - x_frac) * (1.0 - y_frac)
                + p01 * x_frac * (1.0 - y_frac)
                + p10 * (1.0 - x_frac) * y_frac
                + p11 * x_frac * y_frac;

            result.push(value as u8);
        }
    }

    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_rgba_to_gray() {
        // Create a simple 2x2 RGBA frame
        let data: Vec<u8> = vec![
            255, 0, 0, 255, // Red pixel
            0, 255, 0, 255, // Green pixel
            0, 0, 255, 255, // Blue pixel
            255, 255, 255, 255, // White pixel
        ];

        let frame = CameraFrame {
            width: 2,
            height: 2,
            data: Arc::from(data.as_slice()),
            format: PixelFormat::RGBA,
            stride_y: 8,
            stride_uv: 0,
            offset_uv: 0,
            captured_at: std::time::Instant::now(),
        };

        let (gray, w, h) = convert_rgba_to_gray(&frame);
        assert_eq!(w, 2);
        assert_eq!(h, 2);
        assert_eq!(gray.len(), 4);

        // Check gray values (approximate due to integer math)
        assert!(gray[0] > 70 && gray[0] < 80); // Red -> ~77
        assert!(gray[1] > 145 && gray[1] < 155); // Green -> ~150
        assert!(gray[2] > 25 && gray[2] < 35); // Blue -> ~29
        assert!(gray[3] > 250); // White -> ~255
    }

    #[test]
    fn test_downscale() {
        // 4x2 uniform gradient
        let data: Vec<u8> = vec![0, 100, 200, 255, 0, 100, 200, 255];

        let result = downscale_gray(&data, 4, 2, 2, 1);
        assert_eq!(result.len(), 2);

        // First pixel samples around (0,0), second around (2,0)
        assert!(result[0] < 100); // Near start of gradient
        assert!(result[1] > 150); // Near end of gradient
    }
}
