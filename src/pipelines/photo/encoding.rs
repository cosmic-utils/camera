// SPDX-License-Identifier: GPL-3.0-only

//! Async photo encoding pipeline
//!
//! This module handles encoding processed images to various formats:
//! - JPEG (with quality control)
//! - PNG (lossless)
//!
//! All encoding operations run asynchronously to avoid blocking.

use super::processing::ProcessedImage;
use crate::backends::camera::types::PixelFormat;
use image::RgbImage;
use std::path::PathBuf;
use tracing::{debug, error, info};

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

/// Raw Bayer data for DNG encoding (bypasses post-processing)
pub struct RawBayerData {
    /// Raw packed sensor data (e.g., CSI2P 10-bit)
    pub data: Vec<u8>,
    pub width: u32,
    pub height: u32,
    /// Row stride in bytes (may include alignment padding)
    pub stride: u32,
    /// Bayer pixel format
    pub format: PixelFormat,
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

    /// Get the current encoding format
    pub fn format(&self) -> EncodingFormat {
        self.format
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

    /// Encode raw Bayer data directly as DNG (bypasses post-processing)
    ///
    /// This writes the raw sensor data into a CFA-pattern DNG file with proper
    /// metadata tags. The data is unpacked from CSI2P 10-bit to 16-bit values.
    pub async fn encode_raw(&self, raw: RawBayerData) -> Result<EncodedImage, String> {
        info!(
            width = raw.width,
            height = raw.height,
            stride = raw.stride,
            format = ?raw.format,
            "Encoding raw Bayer DNG"
        );

        let camera_metadata = self.camera_metadata.clone();
        let width = raw.width;
        let height = raw.height;

        tokio::task::spawn_blocking(move || {
            let data = Self::encode_dng_raw(&raw, &camera_metadata)?;
            debug!(size = data.len(), "Raw DNG encoding complete");
            Ok(EncodedImage {
                data,
                format: EncodingFormat::Dng,
                width,
                height,
            })
        })
        .await
        .map_err(|e| format!("Raw DNG encoding task error: {}", e))?
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
        debug!(
            output_dir = %output_dir.display(),
            format = ?encoded.format,
            size_bytes = encoded.data.len(),
            "Preparing to save photo"
        );

        // Ensure output directory exists
        if let Err(e) = tokio::fs::create_dir_all(&output_dir).await {
            error!(
                output_dir = %output_dir.display(),
                error = %e,
                "Failed to create output directory - check filesystem permissions and path validity"
            );
            return Err(format!(
                "Failed to create output directory '{}': {}",
                output_dir.display(),
                e
            ));
        }

        // Generate filename with timestamp
        let timestamp = chrono::Local::now().format("%Y%m%d_%H%M%S");
        let filename = format!("IMG_{}.{}", timestamp, encoded.format.extension());
        let filepath = output_dir.join(&filename);

        info!(path = %filepath.display(), "Saving photo");

        // Write to disk in background task (I/O-bound)
        let filepath_clone = filepath.clone();
        let filepath_for_error = filepath.clone();
        let write_result =
            tokio::task::spawn_blocking(move || std::fs::write(&filepath_clone, &encoded.data))
                .await;

        match write_result {
            Ok(Ok(())) => {
                info!(path = %filepath.display(), "Photo saved successfully");
                Ok(filepath)
            }
            Ok(Err(io_err)) => {
                error!(
                    path = %filepath_for_error.display(),
                    error = %io_err,
                    "Failed to write photo to disk - check disk space and permissions"
                );
                Err(format!(
                    "Failed to save photo to '{}': {}",
                    filepath_for_error.display(),
                    io_err
                ))
            }
            Err(join_err) => {
                error!(
                    path = %filepath_for_error.display(),
                    error = %join_err,
                    "Save task panicked or was cancelled"
                );
                Err(format!("Save task error: {}", join_err))
            }
        }
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
        use dng::ifd::{IfdValue, Offsets};
        use dng::tags::ifd as tiff_tags;
        use dng::{DngWriter, FileType};
        use std::io::Cursor;
        use std::sync::Arc;

        let raw_data = image.as_raw().clone();
        let raw_data_len = raw_data.len() as u32;

        let version = env!("CARGO_PKG_VERSION");
        let mut ifd = set_common_dng_tags(width, height, camera_metadata, version);

        // RGB-specific tags
        ifd.insert(
            tiff_tags::BitsPerSample,
            IfdValue::List(vec![
                IfdValue::Short(8),
                IfdValue::Short(8),
                IfdValue::Short(8),
            ]),
        );
        ifd.insert(tiff_tags::PhotometricInterpretation, IfdValue::Short(2)); // RGB
        ifd.insert(tiff_tags::SamplesPerPixel, IfdValue::Short(3)); // RGB = 3 samples

        // Strip data
        let offsets: Arc<dyn Offsets + Send + Sync> = Arc::new(DngOffsets { data: raw_data });
        ifd.insert(tiff_tags::StripOffsets, IfdValue::Offsets(offsets));
        ifd.insert(tiff_tags::StripByteCounts, IfdValue::Long(raw_data_len));

        // Write the DNG file to a buffer
        let mut buffer = Vec::new();
        let cursor = Cursor::new(&mut buffer);

        DngWriter::write_dng(cursor, true, FileType::Dng, vec![ifd])
            .map_err(|e| format!("DNG encoding failed: {:?}", e))?;

        Ok(buffer)
    }

    /// Encode raw Bayer sensor data as a CFA-pattern DNG
    ///
    /// Unpacks CSI2P 10-bit packed data to 16-bit values and writes a proper
    /// DNG file with CFA metadata. This preserves the original sensor data
    /// for later raw processing in tools like RawTherapee or darktable.
    fn encode_dng_raw(
        raw: &RawBayerData,
        camera_metadata: &CameraMetadata,
    ) -> Result<Vec<u8>, String> {
        use dng::ifd::{IfdValue, Offsets};
        use dng::tags::ifd as tiff_tags;
        use dng::{DngWriter, FileType};
        use std::io::Cursor;
        use std::sync::Arc;

        let width = raw.width;
        let height = raw.height;
        let stride = raw.stride;

        // Determine bit depth from stride vs width ratio
        let min_stride_10 = (width * 5).div_ceil(4);
        let min_stride_12 = (width * 3).div_ceil(2);
        let is_packed = stride > width;
        let bit_depth: u32 = if is_packed {
            if stride >= min_stride_10 && stride < min_stride_12 {
                10
            } else if stride >= min_stride_12 {
                12
            } else {
                8
            }
        } else {
            8
        };

        info!(
            width,
            height, stride, bit_depth, is_packed, "Unpacking raw Bayer for DNG"
        );

        // Unpack to 16-bit values
        let pixel_data_16 = if bit_depth == 10 && is_packed {
            unpack_csi2p_10bit_to_16bit(&raw.data, width, height, stride)
        } else {
            // 8-bit: promote each byte to 16-bit (shift left by 8 for full range)
            let mut out = Vec::with_capacity((width * height * 2) as usize);
            for row in 0..height {
                let row_start = (row * stride) as usize;
                for col in 0..width {
                    let val = raw.data[row_start + col as usize] as u16;
                    out.extend_from_slice(&val.to_le_bytes());
                }
            }
            out
        };

        let raw_data_len = pixel_data_16.len() as u32;
        let white_level: u32 = if bit_depth == 10 {
            1023
        } else if bit_depth == 12 {
            4095
        } else {
            255
        };

        // Get CFA pattern bytes: 0=R, 1=G, 2=B
        let cfa_pattern: Vec<u8> = match raw.format {
            PixelFormat::BayerRGGB => vec![0, 1, 1, 2], // R G / G B
            PixelFormat::BayerBGGR => vec![2, 1, 1, 0], // B G / G R
            PixelFormat::BayerGRBG => vec![1, 0, 2, 1], // G R / B G
            PixelFormat::BayerGBRG => vec![1, 2, 0, 1], // G B / R G
            _ => return Err(format!("Not a Bayer format: {:?}", raw.format)),
        };

        let version = env!("CARGO_PKG_VERSION");
        let mut ifd = set_common_dng_tags(width, height, camera_metadata, version);

        // DNG version 1.4.0.0
        ifd.insert(
            tiff_tags::DNGVersion,
            IfdValue::List(vec![
                IfdValue::Byte(1),
                IfdValue::Byte(4),
                IfdValue::Byte(0),
                IfdValue::Byte(0),
            ]),
        );
        ifd.insert(
            tiff_tags::DNGBackwardVersion,
            IfdValue::List(vec![
                IfdValue::Byte(1),
                IfdValue::Byte(1),
                IfdValue::Byte(0),
                IfdValue::Byte(0),
            ]),
        );

        // CFA-specific tags
        ifd.insert(tiff_tags::BitsPerSample, IfdValue::Short(16));
        ifd.insert(tiff_tags::PhotometricInterpretation, IfdValue::Short(32803)); // CFA
        ifd.insert(tiff_tags::SamplesPerPixel, IfdValue::Short(1));

        // CFA pattern
        ifd.insert(
            tiff_tags::CFARepeatPatternDim,
            IfdValue::List(vec![IfdValue::Short(2), IfdValue::Short(2)]),
        );
        ifd.insert(
            tiff_tags::CFAPattern,
            IfdValue::List(cfa_pattern.iter().map(|&b| IfdValue::Byte(b)).collect()),
        );
        ifd.insert(
            tiff_tags::CFAPlaneColor,
            IfdValue::List(vec![
                IfdValue::Byte(0),
                IfdValue::Byte(1),
                IfdValue::Byte(2),
            ]),
        );
        ifd.insert(tiff_tags::CFALayout, IfdValue::Short(1)); // Rectangular

        // Black/White levels
        ifd.insert(tiff_tags::BlackLevel, IfdValue::Long(0));
        ifd.insert(tiff_tags::WhiteLevel, IfdValue::Long(white_level));

        // Strip data
        let offsets: Arc<dyn Offsets + Send + Sync> = Arc::new(DngOffsets {
            data: pixel_data_16,
        });
        ifd.insert(tiff_tags::StripOffsets, IfdValue::Offsets(offsets));
        ifd.insert(tiff_tags::StripByteCounts, IfdValue::Long(raw_data_len));

        let mut buffer = Vec::new();
        let cursor = Cursor::new(&mut buffer);
        DngWriter::write_dng(cursor, true, FileType::Dng, vec![ifd])
            .map_err(|e| format!("Raw DNG encoding failed: {:?}", e))?;

        info!(size = buffer.len(), "Raw CFA DNG written");
        Ok(buffer)
    }
}

impl Default for PhotoEncoder {
    fn default() -> Self {
        Self::new()
    }
}

/// Strip data offsets for DNG encoding (used for both RGB and raw CFA data)
struct DngOffsets {
    data: Vec<u8>,
}

impl dng::ifd::Offsets for DngOffsets {
    fn size(&self) -> u32 {
        self.data.len() as u32
    }

    fn write(&self, writer: &mut dyn std::io::Write) -> std::io::Result<()> {
        writer.write_all(&self.data)
    }
}

/// Set TIFF/EXIF tags common to both RGB and raw CFA DNG files
///
/// Sets: ImageWidth, ImageLength, Compression, RowsPerStrip, PlanarConfiguration,
/// Software (with optional gain info), Make/Model, ExposureTime, and ISOSpeedRatings.
fn set_common_dng_tags(
    width: u32,
    height: u32,
    camera_metadata: &CameraMetadata,
    version: &str,
) -> dng::ifd::Ifd {
    use dng::ifd::{Ifd, IfdValue};
    use dng::tags::ifd as tiff_tags;

    let mut ifd = Ifd::default();

    // Required TIFF tags
    ifd.insert(tiff_tags::ImageWidth, IfdValue::Long(width));
    ifd.insert(tiff_tags::ImageLength, IfdValue::Long(height));
    ifd.insert(tiff_tags::Compression, IfdValue::Short(1)); // No compression
    ifd.insert(tiff_tags::RowsPerStrip, IfdValue::Long(height)); // One strip
    ifd.insert(tiff_tags::PlanarConfiguration, IfdValue::Short(1)); // Chunky

    // Software tag: include gain if available
    let software = match camera_metadata.gain {
        Some(gain) => format!("Camera v{} (Gain: {})", version, gain),
        None => format!("Camera v{}", version),
    };
    ifd.insert(tiff_tags::Software, IfdValue::Ascii(software));

    // Camera make/model tags
    if let Some(camera_name) = &camera_metadata.camera_name {
        ifd.insert(tiff_tags::Make, IfdValue::Ascii(camera_name.clone()));
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
        let g = gcd(numerator, denominator);
        ifd.insert(
            tiff_tags::ExposureTime,
            IfdValue::Rational(numerator / g, denominator / g),
        );
    }

    if let Some(iso) = camera_metadata.iso {
        ifd.insert(
            tiff_tags::ISOSpeedRatings,
            IfdValue::Short(iso.min(65535) as u16),
        );
    }

    ifd
}

/// Unpack CSI-2 10-bit packed Bayer data to 16-bit little-endian values
///
/// CSI2P packing: every 5 bytes contain 4 pixels.
/// Bytes 0-3 are high 8 bits of pixels 0-3.
/// Byte 4 contains low 2 bits: [p0_lo:2 | p1_lo:2 | p2_lo:2 | p3_lo:2]
fn unpack_csi2p_10bit_to_16bit(packed: &[u8], width: u32, height: u32, stride: u32) -> Vec<u8> {
    let mut out = Vec::with_capacity((width * height * 2) as usize);
    let groups_per_row = width / 4;

    for row in 0..height {
        let row_start = (row * stride) as usize;
        for group in 0..groups_per_row {
            let base = row_start + (group * 5) as usize;
            if base + 4 >= packed.len() {
                // Pad remaining pixels with zeros
                for _ in 0..(width - group * 4) {
                    out.extend_from_slice(&0u16.to_le_bytes());
                }
                break;
            }
            let lo = packed[base + 4];
            for i in 0..4u8 {
                let hi8 = packed[base + i as usize] as u16;
                let lo2 = ((lo >> (i * 2)) & 0x03) as u16;
                let val = (hi8 << 2) | lo2;
                out.extend_from_slice(&val.to_le_bytes());
            }
        }
        // Handle remaining pixels if width is not divisible by 4
        let remaining = width % 4;
        if remaining > 0 {
            let base = row_start + (groups_per_row * 5) as usize;
            if base + remaining as usize <= packed.len() {
                let lo = if base + 4 < packed.len() {
                    packed[base + 4]
                } else {
                    0
                };
                for i in 0..remaining {
                    let hi8 = packed[base + i as usize] as u16;
                    let lo2 = ((lo >> (i as u8 * 2)) & 0x03) as u16;
                    let val = (hi8 << 2) | lo2;
                    out.extend_from_slice(&val.to_le_bytes());
                }
            }
        }
    }

    out
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
