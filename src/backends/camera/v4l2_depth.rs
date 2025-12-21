// SPDX-License-Identifier: GPL-3.0-only

//! Direct V4L2 depth sensor capture for Y10B format
//!
//! GStreamer doesn't support Y10B (10-bit packed grayscale) format,
//! so we use the v4l crate to capture raw bytes directly from the
//! Kinect depth sensor and process them with WGPU shaders.

use super::types::*;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::Instant;
use tracing::{debug, error, info, warn};
use v4l::buffer::Type;
use v4l::io::traits::CaptureStream;
use v4l::prelude::*;
use v4l::video::Capture;

// Import depth format constants from freedepth
use freedepth::y10b_packed_size;

/// Raw depth frame from Y10B capture
#[derive(Debug, Clone)]
pub struct DepthFrame {
    /// Frame width in pixels
    pub width: u32,
    /// Frame height in pixels
    pub height: u32,
    /// Raw Y10B packed data (4 pixels = 5 bytes)
    pub raw_data: Arc<[u8]>,
    /// Frame sequence number
    pub sequence: u32,
    /// Timestamp when frame was captured
    pub captured_at: Instant,
}

/// Depth frame sender type
pub type DepthFrameSender = cosmic::iced::futures::channel::mpsc::Sender<DepthFrame>;

/// Depth frame receiver type
pub type DepthFrameReceiver = cosmic::iced::futures::channel::mpsc::Receiver<DepthFrame>;

/// Y10B depth sensor pipeline using direct V4L2 capture
pub struct V4l2DepthPipeline {
    device_path: String,
    width: u32,
    height: u32,
    running: Arc<AtomicBool>,
    thread_handle: Option<std::thread::JoinHandle<()>>,
}

impl V4l2DepthPipeline {
    /// Create a new V4L2 depth capture pipeline
    ///
    /// # Arguments
    /// * `device` - Camera device (must have a valid V4L2 path in device_info)
    /// * `format` - Camera format (must be Y10B pixel format)
    /// * `depth_sender` - Channel to send captured depth frames
    pub fn new(
        device: &CameraDevice,
        format: &CameraFormat,
        depth_sender: DepthFrameSender,
    ) -> BackendResult<Self> {
        // Get the V4L2 device path
        let v4l2_path = device
            .device_info
            .as_ref()
            .map(|info| info.real_path.clone())
            .or_else(|| {
                // Try to extract from path if it's a v4l2: prefixed path
                if device.path.starts_with("v4l2:") {
                    Some(
                        device
                            .path
                            .strip_prefix("v4l2:")
                            .unwrap_or(&device.path)
                            .to_string(),
                    )
                } else if device.path.starts_with("/dev/video") {
                    Some(device.path.clone())
                } else {
                    None
                }
            })
            .ok_or_else(|| {
                BackendError::DeviceNotFound(
                    "No V4L2 device path available for depth sensor".to_string(),
                )
            })?;

        info!(
            device_path = %v4l2_path,
            width = format.width,
            height = format.height,
            "Creating V4L2 depth capture pipeline for Y10B format"
        );

        let width = format.width;
        let height = format.height;
        let running = Arc::new(AtomicBool::new(true));
        let running_clone = running.clone();

        // Spawn capture thread
        let device_path_clone = v4l2_path.clone();
        let thread_handle = std::thread::spawn(move || {
            if let Err(e) = capture_loop(
                &device_path_clone,
                width,
                height,
                depth_sender,
                running_clone,
            ) {
                error!(error = %e, "Depth capture loop failed");
            }
        });

        Ok(Self {
            device_path: v4l2_path,
            width,
            height,
            running,
            thread_handle: Some(thread_handle),
        })
    }

    /// Stop the depth capture pipeline
    pub fn stop(mut self) -> BackendResult<()> {
        info!("Stopping V4L2 depth pipeline");
        self.running.store(false, Ordering::SeqCst);

        if let Some(handle) = self.thread_handle.take() {
            match handle.join() {
                Ok(_) => info!("Depth capture thread stopped"),
                Err(_) => warn!("Depth capture thread panicked"),
            }
        }

        Ok(())
    }
}

impl Drop for V4l2DepthPipeline {
    fn drop(&mut self) {
        info!("Dropping V4L2 depth pipeline");
        self.running.store(false, Ordering::SeqCst);
        // Don't wait for thread in drop - it may already be finished
    }
}

/// Main capture loop running in a separate thread
fn capture_loop(
    device_path: &str,
    width: u32,
    height: u32,
    mut frame_sender: DepthFrameSender,
    running: Arc<AtomicBool>,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    static FRAME_COUNTER: AtomicU64 = AtomicU64::new(0);

    info!(
        device_path,
        width, height, "Opening V4L2 device for Y10B capture"
    );

    // Open the device
    let mut dev = Device::with_path(device_path)
        .map_err(|e| format!("Failed to open V4L2 device {}: {}", device_path, e))?;

    // Query current format to see what the device supports
    let current_format = dev
        .format()
        .map_err(|e| format!("Failed to query format: {}", e))?;
    info!(
        current_width = current_format.width,
        current_height = current_format.height,
        fourcc = ?current_format.fourcc,
        "Current device format"
    );

    // Create Y10B FourCC: 'Y' '1' '0' 'B' = 0x42303159
    let y10b_fourcc = v4l::FourCC::new(b"Y10B");
    info!(?y10b_fourcc, "Creating Y10B format");

    // Set format to Y10B with specified dimensions
    let mut format = dev
        .format()
        .map_err(|e| format!("Failed to get format: {}", e))?;
    format.width = width;
    format.height = height;
    format.fourcc = y10b_fourcc;

    // Try to set the format
    match dev.set_format(&format) {
        Ok(f) => {
            info!(
                width = f.width,
                height = f.height,
                fourcc = ?f.fourcc,
                "Set V4L2 format"
            );
            // Verify we got Y10B
            if f.fourcc != y10b_fourcc {
                warn!(
                    expected = ?y10b_fourcc,
                    got = ?f.fourcc,
                    "Device did not accept Y10B format, depth capture may not work correctly"
                );
            }
        }
        Err(e) => {
            warn!(error = %e, "Could not set format, using current device format");
        }
    }

    // Calculate expected frame size for Y10B (10 bits per pixel, packed)
    let expected_size = y10b_packed_size(width, height);
    info!(expected_size, "Expected Y10B frame size");

    // Create memory-mapped stream with 4 buffers
    let mut stream = MmapStream::with_buffers(&mut dev, Type::VideoCapture, 4)
        .map_err(|e| format!("Failed to create buffer stream: {}", e))?;

    info!("V4L2 depth capture stream started");

    // Capture loop
    while running.load(Ordering::SeqCst) {
        let frame_start = Instant::now();

        match stream.next() {
            Ok((buf, meta)) => {
                let frame_num = FRAME_COUNTER.fetch_add(1, Ordering::Relaxed);

                // Validate buffer size
                if buf.len() != expected_size {
                    if frame_num % 30 == 0 {
                        warn!(
                            frame = frame_num,
                            got = buf.len(),
                            expected = expected_size,
                            "Unexpected buffer size"
                        );
                    }
                }

                // Create depth frame
                let depth_frame = DepthFrame {
                    width,
                    height,
                    raw_data: Arc::from(buf),
                    sequence: meta.sequence,
                    captured_at: frame_start,
                };

                // Send frame (non-blocking)
                match frame_sender.try_send(depth_frame) {
                    Ok(_) => {
                        if frame_num % 60 == 0 {
                            let elapsed = frame_start.elapsed();
                            debug!(
                                frame = frame_num,
                                sequence = meta.sequence,
                                size = buf.len(),
                                elapsed_us = elapsed.as_micros(),
                                "Depth frame captured"
                            );
                        }
                    }
                    Err(e) => {
                        if frame_num % 30 == 0 {
                            debug!(frame = frame_num, error = ?e, "Depth frame dropped (channel full)");
                        }
                    }
                }
            }
            Err(e) => {
                warn!(error = %e, "Failed to capture depth frame");
                // Brief sleep before retry
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
        }
    }

    info!("V4L2 depth capture loop ended");
    Ok(())
}

/// Unpack Y10B data to 16-bit depth values
///
/// This function is for CPU fallback - prefer GPU shader for real-time processing.
/// Uses freedepth's unpacking function and shifts to use full 16-bit range.
pub fn unpack_y10b_to_u16(raw_data: &[u8], width: u32, height: u32) -> Vec<u16> {
    // Use freedepth's unpacking function
    let raw_10bit = freedepth::unpack_10bit_ir(raw_data, width, height);

    // Shift left by 6 to use full 16-bit range (10-bit -> 16-bit)
    raw_10bit.into_iter().map(|v| v << 6).collect()
}

/// Convert 16-bit depth values to 8-bit grayscale for preview
pub fn depth_to_grayscale(depth_values: &[u16]) -> Vec<u8> {
    depth_values
        .iter()
        .map(|&d| (d >> 8) as u8) // Take upper 8 bits
        .collect()
}

/// Convert 16-bit depth values to RGBA for display
pub fn depth_to_rgba(depth_values: &[u16]) -> Vec<u8> {
    let mut rgba = Vec::with_capacity(depth_values.len() * 4);
    for &depth in depth_values {
        let gray = (depth >> 8) as u8;
        rgba.push(gray); // R
        rgba.push(gray); // G
        rgba.push(gray); // B
        rgba.push(255); // A
    }
    rgba
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_y10b_unpacking() {
        // Test data: 4 pixels with known values
        // P0 = 1023 (all 1s), P1 = 512, P2 = 256, P3 = 0
        // P0 = 0b11_1111_1111 = 1023
        // P1 = 0b10_0000_0000 = 512
        // P2 = 0b01_0000_0000 = 256
        // P3 = 0b00_0000_0000 = 0
        //
        // Byte 0: P0[9:2] = 0b1111_1111 = 255
        // Byte 1: P0[1:0]|P1[9:4] = 0b11|10_0000 = 0b1110_0000 = 224
        // Byte 2: P1[3:0]|P2[9:6] = 0b0000|0100 = 0b0000_0100 = 4
        // Byte 3: P2[5:0]|P3[9:8] = 0b00_0000|00 = 0b0000_0000 = 0
        // Byte 4: P3[7:0] = 0b0000_0000 = 0
        let raw_data = vec![255u8, 224, 4, 0, 0];

        let depth = unpack_y10b_to_u16(&raw_data, 2, 2);

        // Values are shifted left by 6 to use full 16-bit range
        assert_eq!(depth[0], 1023 << 6);
        assert_eq!(depth[1], 512 << 6);
        assert_eq!(depth[2], 256 << 6);
        assert_eq!(depth[3], 0);
    }

    #[test]
    fn test_depth_to_rgba() {
        let depth = vec![0xFFFF, 0x8000, 0x0000];
        let rgba = depth_to_rgba(&depth);

        assert_eq!(rgba.len(), 12);
        // First pixel (max depth)
        assert_eq!(rgba[0], 255); // R
        assert_eq!(rgba[1], 255); // G
        assert_eq!(rgba[2], 255); // B
        assert_eq!(rgba[3], 255); // A
        // Second pixel (mid depth)
        assert_eq!(rgba[4], 128); // R
        // Third pixel (min depth)
        assert_eq!(rgba[8], 0); // R
    }
}
