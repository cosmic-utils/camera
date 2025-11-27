// SPDX-License-Identifier: MPL-2.0

//! Async photo encoding pipeline
//!
//! This module handles encoding processed images to various formats:
//! - JPEG (with quality control)
//! - PNG (lossless)
//!
//! All encoding operations run asynchronously to avoid blocking.

use super::processing::ProcessedImage;
use image::{ImageFormat, RgbImage};
use std::path::PathBuf;
use tracing::{debug, info};

/// Supported encoding formats
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EncodingFormat {
    /// JPEG format (lossy compression)
    Jpeg,
    /// PNG format (lossless compression)
    Png,
}

impl EncodingFormat {
    /// Get file extension for this format
    pub fn extension(&self) -> &'static str {
        match self {
            EncodingFormat::Jpeg => "jpg",
            EncodingFormat::Png => "png",
        }
    }

    /// Convert to image crate's ImageFormat
    fn to_image_format(&self) -> ImageFormat {
        match self {
            EncodingFormat::Jpeg => ImageFormat::Jpeg,
            EncodingFormat::Png => ImageFormat::Png,
        }
    }
}

/// Encoding quality settings
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EncodingQuality {
    /// Low quality (high compression)
    Low,
    /// Medium quality (balanced)
    Medium,
    /// High quality (low compression)
    High,
    /// Maximum quality (minimal compression)
    Maximum,
}

impl EncodingQuality {
    /// Get JPEG quality value (0-100)
    pub fn jpeg_quality(&self) -> u8 {
        match self {
            EncodingQuality::Low => 60,
            EncodingQuality::Medium => 80,
            EncodingQuality::High => 92,
            EncodingQuality::Maximum => 98,
        }
    }
}

/// Encoded image data ready for saving
pub struct EncodedImage {
    pub data: Vec<u8>,
    pub format: EncodingFormat,
    pub width: u32,
    pub height: u32,
}

/// Photo encoder
pub struct PhotoEncoder {
    format: EncodingFormat,
    quality: EncodingQuality,
}

impl PhotoEncoder {
    /// Create a new encoder with JPEG format and high quality
    pub fn new() -> Self {
        Self {
            format: EncodingFormat::Jpeg,
            quality: EncodingQuality::High,
        }
    }

    /// Set encoding format
    pub fn set_format(&mut self, format: EncodingFormat) {
        self.format = format;
    }

    /// Set encoding quality (only affects JPEG)
    pub fn set_quality(&mut self, quality: EncodingQuality) {
        self.quality = quality;
    }

    /// Encode a processed image asynchronously
    ///
    /// This runs the encoding in a background task to avoid blocking.
    ///
    /// # Arguments
    /// * `processed` - Processed RGB image
    ///
    /// # Returns
    /// * `Ok(EncodedImage)` - Encoded image data
    /// * `Err(String)` - Error message
    pub async fn encode(&self, processed: ProcessedImage) -> Result<EncodedImage, String> {
        info!(
            width = processed.width,
            height = processed.height,
            format = ?self.format,
            "Starting encoding"
        );

        let format = self.format;
        let quality = self.quality;

        // Run encoding in background task (CPU-bound)
        tokio::task::spawn_blocking(move || {
            let data = match format {
                EncodingFormat::Jpeg => Self::encode_jpeg(processed.image, quality)?,
                EncodingFormat::Png => Self::encode_png(processed.image)?,
            };

            debug!(size = data.len(), "Encoding complete");

            Ok(EncodedImage {
                data,
                format,
                width: processed.width,
                height: processed.height,
            })
        })
        .await
        .map_err(|e| format!("Encoding task error: {}", e))?
    }

    /// Save encoded image to disk asynchronously
    ///
    /// Generates a timestamped filename and saves to the specified directory.
    ///
    /// # Arguments
    /// * `encoded` - Encoded image data
    /// * `output_dir` - Directory to save the photo
    ///
    /// # Returns
    /// * `Ok(PathBuf)` - Path to saved file
    /// * `Err(String)` - Error message
    pub async fn save(
        &self,
        encoded: EncodedImage,
        output_dir: PathBuf,
    ) -> Result<PathBuf, String> {
        // Generate filename with timestamp
        let timestamp = chrono::Local::now().format("%Y%m%d_%H%M%S");
        let filename = format!("IMG_{}.{}", timestamp, encoded.format.extension());
        let filepath = output_dir.join(&filename);

        info!(path = %filepath.display(), "Saving photo");

        // Write to disk in background task (I/O-bound)
        let filepath_clone = filepath.clone();
        tokio::task::spawn_blocking(move || {
            std::fs::write(&filepath_clone, &encoded.data)
                .map_err(|e| format!("Failed to save photo: {}", e))?;
            Ok::<_, String>(())
        })
        .await
        .map_err(|e| format!("Save task error: {}", e))??;

        info!(path = %filepath.display(), "Photo saved successfully");
        Ok(filepath)
    }

    /// Encode image as JPEG
    fn encode_jpeg(image: RgbImage, quality: EncodingQuality) -> Result<Vec<u8>, String> {
        let mut buffer = Vec::new();
        let mut cursor = std::io::Cursor::new(&mut buffer);

        // Create JPEG encoder with quality setting
        let mut encoder =
            image::codecs::jpeg::JpegEncoder::new_with_quality(&mut cursor, quality.jpeg_quality());

        encoder
            .encode(
                image.as_raw(),
                image.width(),
                image.height(),
                image::ExtendedColorType::Rgb8,
            )
            .map_err(|e| format!("JPEG encoding failed: {}", e))?;

        Ok(buffer)
    }

    /// Encode image as PNG
    fn encode_png(image: RgbImage) -> Result<Vec<u8>, String> {
        let mut buffer = Vec::new();

        image
            .write_to(
                &mut std::io::Cursor::new(&mut buffer),
                image::ImageFormat::Png,
            )
            .map_err(|e| format!("PNG encoding failed: {}", e))?;

        Ok(buffer)
    }
}

impl Default for PhotoEncoder {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_format_extensions() {
        assert_eq!(EncodingFormat::Jpeg.extension(), "jpg");
        assert_eq!(EncodingFormat::Png.extension(), "png");
    }

    #[test]
    fn test_jpeg_quality_values() {
        assert_eq!(EncodingQuality::Low.jpeg_quality(), 60);
        assert_eq!(EncodingQuality::Medium.jpeg_quality(), 80);
        assert_eq!(EncodingQuality::High.jpeg_quality(), 92);
        assert_eq!(EncodingQuality::Maximum.jpeg_quality(), 98);
    }
}
