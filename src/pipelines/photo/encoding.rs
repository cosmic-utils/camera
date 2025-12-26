// SPDX-License-Identifier: GPL-3.0-only

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
    /// DNG format (raw image data)
    Dng,
}

impl EncodingFormat {
    /// Get file extension for this format
    pub fn extension(&self) -> &'static str {
        match self {
            EncodingFormat::Jpeg => "jpg",
            EncodingFormat::Png => "png",
            EncodingFormat::Dng => "dng",
        }
    }

    /// Convert to image crate's ImageFormat (returns None for DNG)
    fn to_image_format(&self) -> Option<ImageFormat> {
        match self {
            EncodingFormat::Jpeg => Some(ImageFormat::Jpeg),
            EncodingFormat::Png => Some(ImageFormat::Png),
            EncodingFormat::Dng => None, // DNG uses separate encoding
        }
    }
}

impl From<crate::config::PhotoOutputFormat> for EncodingFormat {
    fn from(format: crate::config::PhotoOutputFormat) -> Self {
        match format {
            crate::config::PhotoOutputFormat::Jpeg => EncodingFormat::Jpeg,
            crate::config::PhotoOutputFormat::Png => EncodingFormat::Png,
            crate::config::PhotoOutputFormat::Dng => EncodingFormat::Dng,
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

/// Camera metadata for DNG encoding
#[derive(Debug, Clone, Default)]
pub struct CameraMetadata {
    /// Camera name (e.g., "Logitech C920")
    pub camera_name: Option<String>,
    /// Camera driver (e.g., "uvcvideo")
    pub camera_driver: Option<String>,
    /// Exposure time in seconds (e.g., 0.033 for 1/30s)
    pub exposure_time: Option<f64>,
    /// ISO sensitivity (e.g., 100, 400, 800)
    pub iso: Option<u32>,
    /// Gain value (camera-specific units)
    pub gain: Option<i32>,
    /// Optional 16-bit depth data for depth sensors (e.g., Kinect)
    /// When present, DNG encoder will include depth as a second IFD (SubImage)
    pub depth_data: Option<DepthDataInfo>,
}

/// Depth data information for DNG encoding
#[derive(Debug, Clone)]
pub struct DepthDataInfo {
    /// Raw 16-bit depth values
    pub values: Vec<u16>,
    /// Width of depth image
    pub width: u32,
    /// Height of depth image
    pub height: u32,
}

/// Photo encoder
pub struct PhotoEncoder {
    format: EncodingFormat,
    quality: EncodingQuality,
    camera_metadata: CameraMetadata,
}

impl PhotoEncoder {
    /// Create a new encoder with JPEG format and high quality
    pub fn new() -> Self {
        Self {
            format: EncodingFormat::Jpeg,
            quality: EncodingQuality::High,
            camera_metadata: CameraMetadata::default(),
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

    /// Set camera metadata for DNG encoding
    pub fn set_camera_metadata(&mut self, metadata: CameraMetadata) {
        self.camera_metadata = metadata;
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
        let camera_metadata = self.camera_metadata.clone();

        // Run encoding in background task (CPU-bound)
        tokio::task::spawn_blocking(move || {
            let data = match format {
                EncodingFormat::Jpeg => Self::encode_jpeg(processed.image, quality)?,
                EncodingFormat::Png => Self::encode_png(processed.image)?,
                EncodingFormat::Dng => Self::encode_dng(
                    &processed.image,
                    processed.width,
                    processed.height,
                    &camera_metadata,
                )?,
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

/// Encode 16-bit depth data as grayscale PNG (lossless)
///
/// This is used for depth sensor data (e.g., Kinect Y10B format) to preserve
/// full 16-bit precision in a standard image format.
///
/// # Arguments
/// * `depth_data` - 16-bit depth values (width * height values)
/// * `width` - Frame width in pixels
/// * `height` - Frame height in pixels
///
/// # Returns
/// * `Ok(Vec<u8>)` - PNG encoded bytes
/// * `Err(String)` - Error message
pub fn encode_depth_png(depth_data: &[u16], width: u32, height: u32) -> Result<Vec<u8>, String> {
    use image::ImageEncoder;
    use image::codecs::png::PngEncoder;

    let expected_len = (width * height) as usize;
    if depth_data.len() != expected_len {
        return Err(format!(
            "Depth data size mismatch: got {} values, expected {} for {}x{}",
            depth_data.len(),
            expected_len,
            width,
            height
        ));
    }

    // Convert u16 slice to bytes (little-endian)
    let bytes: Vec<u8> = depth_data.iter().flat_map(|&v| v.to_le_bytes()).collect();

    let mut buffer = Vec::new();
    let encoder = PngEncoder::new(&mut buffer);

    encoder
        .write_image(&bytes, width, height, image::ExtendedColorType::L16)
        .map_err(|e| format!("Depth PNG encoding failed: {}", e))?;

    debug!(
        width,
        height,
        depth_values = depth_data.len(),
        png_size = buffer.len(),
        "Encoded depth data to 16-bit PNG"
    );

    Ok(buffer)
}

/// Save depth data to PNG file
///
/// # Arguments
/// * `depth_data` - 16-bit depth values
/// * `width` - Frame width
/// * `height` - Frame height
/// * `output_dir` - Directory to save the file
///
/// # Returns
/// * `Ok(PathBuf)` - Path to saved file
/// * `Err(String)` - Error message
pub async fn save_depth_png(
    depth_data: Vec<u16>,
    width: u32,
    height: u32,
    output_dir: PathBuf,
) -> Result<PathBuf, String> {
    // Generate filename with timestamp and "depth" prefix
    let timestamp = chrono::Local::now().format("%Y%m%d_%H%M%S");
    let filename = format!("DEPTH_{}.png", timestamp);
    let filepath = output_dir.join(&filename);

    info!(path = %filepath.display(), "Saving depth image");

    let filepath_clone = filepath.clone();
    tokio::task::spawn_blocking(move || {
        let png_data = encode_depth_png(&depth_data, width, height)?;
        std::fs::write(&filepath_clone, &png_data)
            .map_err(|e| format!("Failed to save depth image: {}", e))?;
        Ok::<_, String>(())
    })
    .await
    .map_err(|e| format!("Save task error: {}", e))??;

    info!(path = %filepath.display(), "Depth image saved successfully");
    Ok(filepath)
}

impl PhotoEncoder {
    /// Encode image as DNG (Digital Negative raw format)
    ///
    /// Creates a simple linear DNG file with RGB data stored as strips.
    /// This preserves the image data in a raw-compatible format for later processing.
    fn encode_dng(
        image: &RgbImage,
        width: u32,
        height: u32,
        camera_metadata: &CameraMetadata,
    ) -> Result<Vec<u8>, String> {
        use dng::ifd::{Ifd, IfdValue, Offsets};
        use dng::tags::ifd as tiff_tags;
        use dng::{DngWriter, FileType};
        use std::io::{Cursor, Write};
        use std::sync::Arc;

        let raw_data = image.as_raw().clone();
        let raw_data_len = raw_data.len() as u32;

        // Create main IFD for the image
        let mut ifd = Ifd::default();

        // Required TIFF tags
        ifd.insert(tiff_tags::ImageWidth, IfdValue::Long(width));
        ifd.insert(tiff_tags::ImageLength, IfdValue::Long(height));
        ifd.insert(
            tiff_tags::BitsPerSample,
            IfdValue::List(vec![
                IfdValue::Short(8),
                IfdValue::Short(8),
                IfdValue::Short(8),
            ]),
        );
        ifd.insert(tiff_tags::Compression, IfdValue::Short(1)); // No compression
        ifd.insert(tiff_tags::PhotometricInterpretation, IfdValue::Short(2)); // RGB
        ifd.insert(tiff_tags::SamplesPerPixel, IfdValue::Short(3)); // RGB = 3 samples
        ifd.insert(tiff_tags::RowsPerStrip, IfdValue::Long(height)); // One strip
        ifd.insert(tiff_tags::PlanarConfiguration, IfdValue::Short(1)); // Chunky (RGBRGBRGB...)

        // Software tag with version
        let version = env!("CARGO_PKG_VERSION");
        ifd.insert(
            tiff_tags::Software,
            IfdValue::Ascii(format!("Camera v{}", version)),
        );

        // Camera make/model tags if available
        if let Some(camera_name) = &camera_metadata.camera_name {
            // Use Make tag for camera name
            ifd.insert(tiff_tags::Make, IfdValue::Ascii(camera_name.clone()));
            // Use Model tag for driver info if available, otherwise use camera name
            if let Some(driver) = &camera_metadata.camera_driver {
                ifd.insert(
                    tiff_tags::Model,
                    IfdValue::Ascii(format!("{} ({})", camera_name, driver)),
                );
            } else {
                ifd.insert(tiff_tags::Model, IfdValue::Ascii(camera_name.clone()));
            }
        }

        // Exposure metadata (EXIF tags)
        if let Some(exposure_time) = camera_metadata.exposure_time {
            // Convert to rational: e.g., 0.033333 -> 1/30
            // Use microsecond precision for the rational representation
            let numerator = (exposure_time * 1_000_000.0).round() as u32;
            let denominator = 1_000_000u32;
            // Simplify the fraction by finding GCD
            let gcd = gcd(numerator, denominator);
            ifd.insert(
                tiff_tags::ExposureTime,
                IfdValue::Rational(numerator / gcd, denominator / gcd),
            );
        }

        if let Some(iso) = camera_metadata.iso {
            ifd.insert(
                tiff_tags::ISOSpeedRatings,
                IfdValue::Short(iso.min(65535) as u16),
            );
        }

        // Store gain as a custom tag comment if available
        // (no standard EXIF tag for raw gain value)
        if let Some(gain) = camera_metadata.gain {
            // Include gain in the software/processing info
            let software_with_gain = format!("Camera v{} (Gain: {})", version, gain);
            ifd.insert(tiff_tags::Software, IfdValue::Ascii(software_with_gain));
        }

        // Create an Offsets implementation for data
        struct DataOffsets {
            data: Vec<u8>,
        }

        impl Offsets for DataOffsets {
            fn size(&self) -> u32 {
                self.data.len() as u32
            }

            fn write(&self, writer: &mut dyn Write) -> std::io::Result<()> {
                writer.write_all(&self.data)
            }
        }

        let offsets: Arc<dyn Offsets + Send + Sync> = Arc::new(DataOffsets { data: raw_data });

        // Add strip data using Offsets
        ifd.insert(tiff_tags::StripOffsets, IfdValue::Offsets(offsets));
        ifd.insert(tiff_tags::StripByteCounts, IfdValue::Long(raw_data_len));

        // Collect all IFDs
        let mut ifds = vec![ifd];

        // Add depth data as second IFD if present
        if let Some(depth_info) = &camera_metadata.depth_data {
            debug!(
                depth_width = depth_info.width,
                depth_height = depth_info.height,
                depth_values = depth_info.values.len(),
                "Adding depth data to DNG as second IFD"
            );

            let mut depth_ifd = Ifd::default();

            // Required TIFF tags for 16-bit grayscale
            depth_ifd.insert(tiff_tags::ImageWidth, IfdValue::Long(depth_info.width));
            depth_ifd.insert(tiff_tags::ImageLength, IfdValue::Long(depth_info.height));
            depth_ifd.insert(tiff_tags::BitsPerSample, IfdValue::Short(16)); // 16-bit
            depth_ifd.insert(tiff_tags::Compression, IfdValue::Short(1)); // No compression
            depth_ifd.insert(tiff_tags::PhotometricInterpretation, IfdValue::Short(1)); // BlackIsZero (grayscale)
            depth_ifd.insert(tiff_tags::SamplesPerPixel, IfdValue::Short(1)); // Grayscale = 1 sample
            depth_ifd.insert(tiff_tags::RowsPerStrip, IfdValue::Long(depth_info.height)); // One strip
            depth_ifd.insert(tiff_tags::PlanarConfiguration, IfdValue::Short(1)); // Chunky

            // Mark as depth image (NewSubfileType = 4 for depth map, but we'll use ImageDescription)
            // SubfileType 0 = full-resolution image
            depth_ifd.insert(tiff_tags::NewSubfileType, IfdValue::Long(0));

            // Add description to identify as depth data
            depth_ifd.insert(
                tiff_tags::ImageDescription,
                IfdValue::Ascii("Depth Map (16-bit)".to_string()),
            );

            // Convert u16 depth values to bytes (little-endian for TIFF)
            let depth_bytes: Vec<u8> = depth_info
                .values
                .iter()
                .flat_map(|&v| v.to_le_bytes())
                .collect();
            let depth_data_len = depth_bytes.len() as u32;

            let depth_offsets: Arc<dyn Offsets + Send + Sync> =
                Arc::new(DataOffsets { data: depth_bytes });

            depth_ifd.insert(tiff_tags::StripOffsets, IfdValue::Offsets(depth_offsets));
            depth_ifd.insert(tiff_tags::StripByteCounts, IfdValue::Long(depth_data_len));

            ifds.push(depth_ifd);
        }

        // Write the DNG file to a buffer
        let mut buffer = Vec::new();
        let cursor = Cursor::new(&mut buffer);

        DngWriter::write_dng(cursor, true, FileType::Dng, ifds)
            .map_err(|e| format!("DNG encoding failed: {:?}", e))?;

        Ok(buffer)
    }
}

impl Default for PhotoEncoder {
    fn default() -> Self {
        Self::new()
    }
}

/// Calculate greatest common divisor using Euclidean algorithm
fn gcd(mut a: u32, mut b: u32) -> u32 {
    while b != 0 {
        let t = b;
        b = a % b;
        a = t;
    }
    a.max(1) // Avoid division by zero
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_format_extensions() {
        assert_eq!(EncodingFormat::Jpeg.extension(), "jpg");
        assert_eq!(EncodingFormat::Png.extension(), "png");
        assert_eq!(EncodingFormat::Dng.extension(), "dng");
    }

    #[test]
    fn test_jpeg_quality_values() {
        assert_eq!(EncodingQuality::Low.jpeg_quality(), 60);
        assert_eq!(EncodingQuality::Medium.jpeg_quality(), 80);
        assert_eq!(EncodingQuality::High.jpeg_quality(), 92);
        assert_eq!(EncodingQuality::Maximum.jpeg_quality(), 98);
    }
}
