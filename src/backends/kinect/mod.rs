// SPDX-License-Identifier: GPL-3.0-only

//! Kinect support via freedepth
//!
//! This module re-exports types from freedepth for Kinect sensor control.
//! Depth/video streaming is handled via V4L2 (kernel driver), while freedepth
//! handles the control features that V4L2 doesn't expose:
//!
//! - Motor/tilt control (-27° to +27°)
//! - LED control (off, green, red, yellow, blinking patterns)
//! - Accelerometer readout
//! - Device calibration data for accurate depth conversion
//!
//! ## Usage Pattern
//!
//! 1. Use freedepth to detect Kinect devices and fetch calibration
//! 2. Use V4L2 (`/dev/video*`) for depth/video streaming via GStreamer or v4l crate
//! 3. Use freedepth's `DepthToMm` converter to transform raw 11-bit depth to mm
//! 4. Use freedepth for motor/LED control during capture
//!
//! ## Example
//!
//! ```ignore
//! use camera::backends::kinect::{Context, Led};
//!
//! // Initialize and fetch calibration
//! let ctx = Context::new()?;
//! let mut device = ctx.open_device(0)?;
//! device.fetch_calibration()?;
//!
//! // Get depth converter
//! let converter = device.depth_to_mm().unwrap();
//!
//! // Start V4L2 streaming separately (via GStreamer pipeline)
//! // For each raw depth value from V4L2:
//! let raw_depth: u16 = 800;
//! let mm = converter.convert(raw_depth);
//! ```

// Re-export everything from freedepth
pub use freedepth::*;
