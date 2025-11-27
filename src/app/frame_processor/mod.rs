// SPDX-License-Identifier: MPL-2.0

//! Frame processor module for async frame analysis
//!
//! This module provides a system for sampling camera frames at intervals
//! and running async detection tasks. Currently implements QR code detection.

pub mod tasks;
pub mod types;

pub use tasks::qr_detector;
pub(crate) use types::urlencoding_encode;
pub use types::{FrameRegion, QrAction, QrDetection, WifiSecurity};
