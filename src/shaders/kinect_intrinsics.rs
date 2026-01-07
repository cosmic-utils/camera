// SPDX-License-Identifier: GPL-3.0-only

//! Kinect camera intrinsics and depth coefficients
//!
//! These constants are used across the depth processing pipeline for:
//! - Point cloud rendering (unprojection from 2D to 3D)
//! - Mesh generation
//! - Scene export (LAZ, GLTF)
//!
//! Reference resolution: 640x480 (medium resolution depth mode)
//!
//! These are separated from the freedepth-specific depth processing so they
//! can be used with both the kernel driver and freedepth backends.

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
