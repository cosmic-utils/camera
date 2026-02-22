// SPDX-License-Identifier: GPL-3.0-only

//! Async photo capture pipeline
//!
//! This pipeline implements a fully asynchronous photo capture workflow:
//!
//! ```text
//! Camera Backend → Capture → Post-Processing → Encoding → Disk I/O
//!       ↓
//! Preview continues uninterrupted
//! ```
//!
//! # Pipeline Stages
//!
//! 1. **Capture**: Grab raw frame from camera backend (zero-copy)
//! 2. **Post-Processing**: Apply color correction, sharpening, etc. (async)
//! 3. **Encoding**: Convert to JPEG/PNG format (async)
//! 4. **Disk I/O**: Save to disk (async)
//!
//! # Key Features
//!
//! - **Non-blocking**: All stages run asynchronously
//! - **Preview Continuity**: Camera preview never pauses
//! - **Memory Efficient**: Uses Arc for zero-copy frame passing
//! - **Configurable**: Support for multiple output formats and quality settings

pub mod burst_mode;
pub mod capture;
pub mod encoding;
pub mod processing;

pub use encoding::{CameraMetadata, EncodingFormat, EncodingQuality, PhotoEncoder, RawBayerData};
pub use processing::{PostProcessingConfig, PostProcessor};

use crate::backends::camera::types::CameraFrame;
use std::path::PathBuf;
use std::sync::Arc;
use tracing::info;

/// Complete photo capture pipeline
///
/// Orchestrates the entire capture → process → encode → save workflow.
pub struct PhotoPipeline {
    post_processor: PostProcessor,
    encoder: PhotoEncoder,
}

impl PhotoPipeline {
    /// Create a new photo pipeline with default settings
    pub fn new() -> Self {
        Self {
            post_processor: PostProcessor::new(PostProcessingConfig::default()),
            encoder: PhotoEncoder::new(),
        }
    }

    /// Create a new photo pipeline with custom settings
    pub fn with_config(
        processing_config: PostProcessingConfig,
        encoding_format: EncodingFormat,
        encoding_quality: EncodingQuality,
    ) -> Self {
        let mut encoder = PhotoEncoder::new();
        encoder.set_format(encoding_format);
        encoder.set_quality(encoding_quality);

        Self {
            post_processor: PostProcessor::new(processing_config),
            encoder,
        }
    }

    /// Capture and save a photo asynchronously
    ///
    /// This runs the complete pipeline:
    /// 1. Captures frame from camera (already provided)
    /// 2. Post-processes the frame
    /// 3. Encodes to target format
    /// 4. Saves to disk
    ///
    /// # Arguments
    /// * `frame` - Raw camera frame (RGBA format)
    /// * `output_dir` - Directory to save the photo
    ///
    /// # Returns
    /// * `Ok(PathBuf)` - Path to saved photo
    /// * `Err(String)` - Error message
    pub async fn capture_and_save(
        &self,
        frame: Arc<CameraFrame>,
        output_dir: PathBuf,
    ) -> Result<PathBuf, String> {
        // DNG + Bayer: bypass post-processing, encode raw sensor data directly
        if self.encoder.format() == EncodingFormat::Dng && frame.format.is_bayer() {
            info!(
                width = frame.width,
                height = frame.height,
                format = ?frame.format,
                stride = frame.stride,
                "DNG raw bypass: encoding Bayer data directly"
            );

            let raw = RawBayerData {
                data: frame.data.to_vec(),
                width: frame.width,
                height: frame.height,
                stride: frame.stride,
                format: frame.format,
            };

            let encoded = self.encoder.encode_raw(raw).await?;
            let output_path = self.encoder.save(encoded, output_dir).await?;
            return Ok(output_path);
        }

        // Stage 1: Post-process (async, CPU-bound)
        let processed = self.post_processor.process(frame).await?;

        // Stage 2: Encode (async, CPU-bound)
        let encoded = self.encoder.encode(processed).await?;

        // Stage 3: Save to disk (async, I/O-bound)
        let output_path = self.encoder.save(encoded, output_dir).await?;

        Ok(output_path)
    }

    /// Capture and save with progress callback
    ///
    /// Same as `capture_and_save` but calls the provided callback at each stage.
    ///
    /// # Arguments
    /// * `frame` - Raw camera frame
    /// * `output_dir` - Directory to save the photo
    /// * `progress` - Callback for progress updates (0.0 - 1.0)
    pub async fn capture_and_save_with_progress<F>(
        &self,
        frame: Arc<CameraFrame>,
        output_dir: PathBuf,
        mut progress: F,
    ) -> Result<PathBuf, String>
    where
        F: FnMut(f32) + Send,
    {
        progress(0.0);

        // DNG + Bayer: bypass post-processing
        if self.encoder.format() == EncodingFormat::Dng && frame.format.is_bayer() {
            let raw = RawBayerData {
                data: frame.data.to_vec(),
                width: frame.width,
                height: frame.height,
                stride: frame.stride,
                format: frame.format,
            };
            progress(0.33);

            let encoded = self.encoder.encode_raw(raw).await?;
            progress(0.66);

            let output_path = self.encoder.save(encoded, output_dir).await?;
            progress(1.0);
            return Ok(output_path);
        }

        // Post-process
        let processed = self.post_processor.process(frame).await?;
        progress(0.33);

        // Encode
        let encoded = self.encoder.encode(processed).await?;
        progress(0.66);

        // Save
        let output_path = self.encoder.save(encoded, output_dir).await?;
        progress(1.0);

        Ok(output_path)
    }

    /// Update post-processing configuration
    pub fn set_processing_config(&mut self, config: PostProcessingConfig) {
        self.post_processor = PostProcessor::new(config);
    }

    /// Update encoding format
    pub fn set_encoding_format(&mut self, format: EncodingFormat) {
        self.encoder.set_format(format);
    }

    /// Update encoding quality
    pub fn set_encoding_quality(&mut self, quality: EncodingQuality) {
        self.encoder.set_quality(quality);
    }

    /// Set camera metadata for DNG encoding
    pub fn set_camera_metadata(&mut self, metadata: CameraMetadata) {
        self.encoder.set_camera_metadata(metadata);
    }
}

impl Default for PhotoPipeline {
    fn default() -> Self {
        Self::new()
    }
}
