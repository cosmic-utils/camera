// SPDX-License-Identifier: MPL-2.0

//! Processing pipelines for photo and video capture
//!
//! This module provides async processing pipelines that handle media capture
//! without interrupting the live camera preview. All heavy operations run
//! in background tasks to maintain smooth UI performance.
//!
//! # Pipeline Architecture
//!
//! ```text
//! ┌──────────────┐     ┌───────────────────┐     ┌──────────────┐
//! │ Camera Frame │ ──▶ │  Photo Pipeline   │ ──▶ │  JPEG File   │
//! │   (NV12)     │     │  - NV12→RGB       │     │              │
//! │              │     │  - Filters        │     │              │
//! │              │     │  - Encoding       │     │              │
//! └──────────────┘     └───────────────────┘     └──────────────┘
//!
//! ┌──────────────┐     ┌───────────────────┐     ┌──────────────┐
//! │ Camera Node  │ ──▶ │  Video Pipeline   │ ──▶ │   MP4 File   │
//! │  (PipeWire)  │     │  - GStreamer      │     │              │
//! │              │     │  - HW Encoding    │     │              │
//! │              │     │  - Audio Muxing   │     │              │
//! └──────────────┘     └───────────────────┘     └──────────────┘
//! ```
//!
//! # Design Principles
//!
//! 1. **Non-blocking**: Preview never freezes during capture
//! 2. **GPU-accelerated**: NV12→RGB conversion uses compute shaders when available
//! 3. **Hardware encoding**: Video uses VA-API/NVENC when available
//! 4. **Graceful degradation**: Falls back to software when HW unavailable
//!
//! # Modules
//!
//! - [`photo`]: Async photo capture with filters and JPEG encoding
//! - [`video`]: Video recording with GStreamer and hardware acceleration

pub mod photo;
pub mod video;
