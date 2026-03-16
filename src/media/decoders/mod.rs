// SPDX-License-Identifier: GPL-3.0-only

//! Hardware and software decoder utilities
//!
//! This module provides utilities for detecting and managing video decoders,
//! particularly hardware-accelerated decoders for formats like MJPEG, H.264, etc.

mod definitions;
mod hardware;

pub use definitions::{DecoderDef, H264_DECODERS, H265_DECODERS, MJPEG_DECODERS};
pub use hardware::detect_hw_decoders;
