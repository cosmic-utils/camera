// SPDX-License-Identifier: GPL-3.0-only

#![cfg(all(target_arch = "x86_64", feature = "freedepth"))]

//! GPU-accelerated depth processing for Y10B format
//!
//! This module provides GPU-based unpacking of Y10B depth sensor data (Kinect)
//! into viewable RGBA preview and lossless 16-bit depth values.

mod constants;
mod processor;
mod visualization;

pub use constants::*;
pub use visualization::{depth_mm_to_rgba, rgb_to_rgba};

/// Kinect camera intrinsics - re-exported from kinect_intrinsics module
pub use crate::shaders::kinect_intrinsics as kinect;

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
