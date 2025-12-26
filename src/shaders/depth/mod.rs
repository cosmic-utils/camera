// SPDX-License-Identifier: GPL-3.0-only

//! GPU-accelerated depth processing for Y10B format
//!
//! This module provides GPU-based unpacking of Y10B depth sensor data (Kinect)
//! into viewable RGBA preview and lossless 16-bit depth values.

mod constants;
mod processor;
mod visualization;

pub use constants::*;
pub use visualization::{depth_mm_to_rgba, rgb_to_rgba};

/// Kinect camera intrinsics and depth coefficients
///
/// These constants are used across the depth processing pipeline for:
/// - Point cloud rendering (unprojection from 2D to 3D)
/// - Mesh generation
/// - Scene export (LAZ, GLTF)
///
/// Reference resolution: 640x480 (medium resolution depth mode)
pub mod kinect {
    /// Focal length X (pixels) at 640x480 base resolution
    pub const FX: f32 = 594.21;
    /// Focal length Y (pixels) at 640x480 base resolution
    pub const FY: f32 = 591.04;
    /// Principal point X (pixels) at 640x480 base resolution
    pub const CX: f32 = 339.5;
    /// Principal point Y (pixels) at 640x480 base resolution
    pub const CY: f32 = 242.7;

    /// Disparity-to-depth coefficient A
    /// Used in formula: depth_m = 1.0 / (raw * DEPTH_COEFF_A + DEPTH_COEFF_B)
    pub const DEPTH_COEFF_A: f32 = -0.0030711;
    /// Disparity-to-depth coefficient B
    /// Used in formula: depth_m = 1.0 / (raw * DEPTH_COEFF_A + DEPTH_COEFF_B)
    pub const DEPTH_COEFF_B: f32 = 3.3309495;

    /// Base width for intrinsics calculation
    pub const BASE_WIDTH: f32 = 640.0;
    /// Base height for intrinsics calculation
    pub const BASE_HEIGHT: f32 = 480.0;
}

pub use processor::{DepthProcessor, unpack_y10b_gpu};

/// Y10B shader source
pub const Y10B_UNPACK_SHADER: &str = include_str!("y10b_unpack.wgsl");

use std::sync::atomic::{AtomicBool, Ordering};

/// Global depth visualization settings
/// These can be updated from the UI thread and read from the processing thread
static DEPTH_COLORMAP_ENABLED: AtomicBool = AtomicBool::new(false);
static DEPTH_ONLY_MODE: AtomicBool = AtomicBool::new(false);
static DEPTH_GRAYSCALE_MODE: AtomicBool = AtomicBool::new(false);

/// Set whether the depth colormap should be enabled
pub fn set_depth_colormap_enabled(enabled: bool) {
    DEPTH_COLORMAP_ENABLED.store(enabled, Ordering::Relaxed);
}

/// Get whether the depth colormap is enabled
pub fn is_depth_colormap_enabled() -> bool {
    DEPTH_COLORMAP_ENABLED.load(Ordering::Relaxed)
}

/// Set whether depth-only mode is enabled (pure colormap without blending)
pub fn set_depth_only_mode(enabled: bool) {
    DEPTH_ONLY_MODE.store(enabled, Ordering::Relaxed);
}

/// Get whether depth-only mode is enabled
pub fn is_depth_only_mode() -> bool {
    DEPTH_ONLY_MODE.load(Ordering::Relaxed)
}

/// Set whether grayscale depth mode is enabled (grayscale instead of colormap)
pub fn set_depth_grayscale_mode(enabled: bool) {
    DEPTH_GRAYSCALE_MODE.store(enabled, Ordering::Relaxed);
}

/// Get whether grayscale depth mode is enabled
pub fn is_depth_grayscale_mode() -> bool {
    DEPTH_GRAYSCALE_MODE.load(Ordering::Relaxed)
}
