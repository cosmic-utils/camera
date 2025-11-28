// SPDX-License-Identifier: MPL-2.0

//! Camera preview module
//!
//! This module handles the camera preview display widget.
//! The actual video rendering is delegated to the video_widget module
//! which uses GPU-accelerated RGBA rendering with filter support.

pub mod widget;

// Re-export for convenience
