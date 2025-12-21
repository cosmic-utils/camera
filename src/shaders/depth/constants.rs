// SPDX-License-Identifier: GPL-3.0-only

//! Depth sensor constants - Single source of truth
//!
//! All depth range, visualization, and sensor-specific constants live here.
//! These values are used across the depth processing pipeline.

/// Kinect sensor depth range limits (millimeters)
/// Based on Xbox Kinect v1 sensor specifications
pub const DEPTH_MIN_MM: f32 = 400.0;
pub const DEPTH_MAX_MM: f32 = 4000.0;

/// Integer versions for UI display
pub const DEPTH_MIN_MM_U16: u16 = 400;
pub const DEPTH_MAX_MM_U16: u16 = 4000;

/// Invalid depth marker values
pub const DEPTH_INVALID_MM: u16 = 0;
/// Maximum valid depth value (values above this are considered invalid)
pub const DEPTH_MAX_VALID_MM: u16 = 8000;

/// Number of quantization bands for depth colormap visualization
pub const DEPTH_COLORMAP_BANDS: f32 = 32.0;
