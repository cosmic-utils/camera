// SPDX-License-Identifier: MPL-2.0

//! COSMIC Camera - A camera application for the COSMIC desktop environment
//!
//! This library provides the core functionality for the COSMIC Camera application,
//! including camera capture, video recording, and photo processing.
//!
//! # Architecture
//!
//! The crate is organized into several modules:
//!
//! - [`app`]: Main application logic and UI
//! - [`backends`]: Camera and audio backend abstraction
//! - [`media`]: Media encoding, decoding, and color conversion
//! - [`pipelines`]: Photo and video capture pipelines
//! - [`config`]: User configuration handling
//! - [`storage`]: File storage and thumbnail management
//!
//! # Example
//!
//! ```ignore
//! // This is a GUI application, typically run via:
//! // cosmic-camera
//! ```

pub mod app;
pub mod backends;
pub mod bug_report;
pub mod config;
pub mod constants;
pub mod errors;
pub mod i18n;
pub mod media;
pub mod network_manager;
pub mod pipelines;
pub mod shaders;
pub mod storage;

// Re-export commonly used types
pub use app::frame_processor::{QrAction, QrDetection};
pub use app::{AppModel, CameraMode, FilterType, Message};
pub use config::Config;
pub use constants::BitratePreset;
