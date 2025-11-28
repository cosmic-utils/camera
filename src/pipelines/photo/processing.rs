// SPDX-License-Identifier: MPL-2.0

//! Async post-processing pipeline for photos
//!
//! This module handles post-processing operations on captured frames:
//! - Filter application directly on RGBA data (GPU-accelerated)
//! - RGBA to RGB conversion (drop alpha channel)
//! - Sharpening
//! - Brightness/contrast adjustments
//!
//! The pipeline is optimized to apply filters on RGBA data before RGB conversion,
//! avoiding unnecessary format conversions.

use crate::app::FilterType;
use crate::backends::camera::types::CameraFrame;
use crate::shaders::apply_filter_gpu_rgba;
use image::RgbImage;
use std::sync::Arc;
use tracing::{debug, info, warn};

/// Post-processing configuration
#[derive(Debug, Clone)]
pub struct PostProcessingConfig {
    /// Enable color correction
    pub color_correction: bool,
    /// Enable sharpening
    pub sharpening: bool,
    /// Brightness adjustment (-1.0 to 1.0, 0.0 = no change)
    pub brightness: f32,
    /// Contrast adjustment (0.0 to 2.0, 1.0 = no change)
    pub contrast: f32,
    /// Saturation adjustment (0.0 to 2.0, 1.0 = no change)
    pub saturation: f32,
    /// Filter type to apply
    pub filter_type: FilterType,
}

impl Default for PostProcessingConfig {
    fn default() -> Self {
        Self {
            color_correction: true,
            sharpening: false,
            brightness: 0.0,
            contrast: 1.0,
            saturation: 1.0,
            filter_type: FilterType::Standard,
        }
    }
}

/// Processed image data
pub struct ProcessedImage {
    pub image: RgbImage,
    pub width: u32,
    pub height: u32,
}

/// Post-processor for captured frames
pub struct PostProcessor {
    config: PostProcessingConfig,
}

impl PostProcessor {
    /// Create a new post-processor with the given configuration
    pub fn new(config: PostProcessingConfig) -> Self {
        Self { config }
    }

    /// Process a captured frame asynchronously
    ///
    /// This runs all post-processing steps using GPU acceleration where available,
    /// with software rendering fallback for systems without GPU support.
    ///
    /// # Arguments
    /// * `frame` - Raw camera frame (RGBA format)
    ///
    /// # Returns
    /// * `Ok(ProcessedImage)` - Processed RGB image
    /// * `Err(String)` - Error message
    pub async fn process(&self, frame: Arc<CameraFrame>) -> Result<ProcessedImage, String> {
        info!(
            width = frame.width,
            height = frame.height,
            "Starting post-processing"
        );

        let config = self.config.clone();
        let frame_width = frame.width;
        let frame_height = frame.height;

        // Step 1: Apply filter on RGBA data directly (more efficient - avoids RGB↔RGBA conversions)
        let filtered_rgba = if config.filter_type != FilterType::Standard {
            match apply_filter_gpu_rgba(&frame.data, frame_width, frame_height, config.filter_type).await {
                Ok(filtered_data) => {
                    debug!("Filter applied via GPU pipeline (RGBA-native)");
                    filtered_data
                }
                Err(e) => {
                    warn!(error = %e, "GPU filter failed, using unfiltered frame");
                    frame.data.to_vec()
                }
            }
        } else {
            frame.data.to_vec()
        };

        // Step 2: Convert filtered RGBA to RGB (drop alpha channel)
        let rgb_image = Self::convert_rgba_to_rgb(&filtered_rgba, frame_width, frame_height)?;

        // Step 3 & 4: Apply adjustments and sharpening (CPU-bound)
        let needs_adjustments =
            config.brightness != 0.0 || config.contrast != 1.0 || config.saturation != 1.0;
        let needs_sharpening = config.sharpening;

        let rgb_image = if needs_adjustments || needs_sharpening {
            tokio::task::spawn_blocking(move || {
                let mut image = rgb_image;

                if needs_adjustments {
                    Self::apply_adjustments(&mut image, &config);
                }

                if needs_sharpening {
                    Self::apply_sharpening(&mut image);
                }

                image
            })
            .await
            .map_err(|e| format!("Post-processing task error: {}", e))?
        } else {
            rgb_image
        };

        debug!("Post-processing complete");

        Ok(ProcessedImage {
            width: frame_width,
            height: frame_height,
            image: rgb_image,
        })
    }

    /// Convert RGBA data to RGB image (drop alpha channel)
    fn convert_rgba_to_rgb(rgba_data: &[u8], width: u32, height: u32) -> Result<RgbImage, String> {
        let expected_size = (width * height * 4) as usize;
        if rgba_data.len() < expected_size {
            return Err(format!(
                "RGBA data too small: expected {}, got {}",
                expected_size,
                rgba_data.len()
            ));
        }

        let rgb_data: Vec<u8> = rgba_data
            .chunks(4)
            .take((width * height) as usize)
            .flat_map(|rgba| [rgba[0], rgba[1], rgba[2]])
            .collect();

        RgbImage::from_raw(width, height, rgb_data)
            .ok_or_else(|| "Failed to create RGB image from converted data".to_string())
    }

    /// Apply brightness, contrast, and saturation adjustments
    fn apply_adjustments(image: &mut RgbImage, config: &PostProcessingConfig) {
        for pixel in image.pixels_mut() {
            let r = pixel[0] as f32;
            let g = pixel[1] as f32;
            let b = pixel[2] as f32;

            // Apply brightness
            let r = r + config.brightness * 255.0;
            let g = g + config.brightness * 255.0;
            let b = b + config.brightness * 255.0;

            // Apply contrast
            let r = ((r - 128.0) * config.contrast + 128.0).clamp(0.0, 255.0);
            let g = ((g - 128.0) * config.contrast + 128.0).clamp(0.0, 255.0);
            let b = ((b - 128.0) * config.contrast + 128.0).clamp(0.0, 255.0);

            // Apply saturation
            if config.saturation != 1.0 {
                let gray = 0.299 * r + 0.587 * g + 0.114 * b;
                let r = (gray + (r - gray) * config.saturation).clamp(0.0, 255.0);
                let g = (gray + (g - gray) * config.saturation).clamp(0.0, 255.0);
                let b = (gray + (b - gray) * config.saturation).clamp(0.0, 255.0);

                pixel[0] = r as u8;
                pixel[1] = g as u8;
                pixel[2] = b as u8;
            } else {
                pixel[0] = r as u8;
                pixel[1] = g as u8;
                pixel[2] = b as u8;
            }
        }
    }

    /// Apply unsharp mask sharpening
    ///
    /// This is a simple 3x3 kernel sharpening filter.
    fn apply_sharpening(image: &mut RgbImage) {
        // Simple sharpen kernel: [ [0, -1, 0], [-1, 5, -1], [0, -1, 0] ]
        let (width, height) = image.dimensions();
        let original = image.clone();

        for y in 1..height - 1 {
            for x in 1..width - 1 {
                for c in 0..3 {
                    let center = original.get_pixel(x, y)[c] as i32 * 5;
                    let top = original.get_pixel(x, y - 1)[c] as i32;
                    let bottom = original.get_pixel(x, y + 1)[c] as i32;
                    let left = original.get_pixel(x - 1, y)[c] as i32;
                    let right = original.get_pixel(x + 1, y)[c] as i32;

                    let value = (center - top - bottom - left - right).clamp(0, 255) as u8;
                    image.get_pixel_mut(x, y)[c] = value;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_config() {
        let config = PostProcessingConfig::default();
        assert!(config.color_correction);
        assert!(!config.sharpening);
        assert_eq!(config.brightness, 0.0);
        assert_eq!(config.contrast, 1.0);
        assert_eq!(config.saturation, 1.0);
    }
}
