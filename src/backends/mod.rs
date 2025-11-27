// SPDX-License-Identifier: MPL-2.0

//! Backend abstraction layer for camera and audio capture
//!
//! This module provides platform-specific backend implementations for:
//! - Camera capture via PipeWire
//! - Audio device enumeration via PipeWire
//!
//! # Architecture
//!
//! The backend layer abstracts hardware access, providing a consistent API
//! regardless of the underlying capture method:
//!
//! ```text
//! ┌─────────────────────────────────────────────┐
//! │                  App Layer                   │
//! └────────────────────┬────────────────────────┘
//!                      │
//! ┌────────────────────┴────────────────────────┐
//! │              Backend Layer                   │
//! │  ┌─────────────┐    ┌──────────────────┐   │
//! │  │    Audio    │    │     Camera       │   │
//! │  │  (PipeWire) │    │    (PipeWire)    │   │
//! │  └─────────────┘    └──────────────────┘   │
//! └─────────────────────────────────────────────┘
//! ```
//!
//! # Modules
//!
//! - [`audio`]: Audio device enumeration and selection
//! - [`camera`]: Camera backend with device enumeration and frame capture

pub mod audio;
pub mod camera;
