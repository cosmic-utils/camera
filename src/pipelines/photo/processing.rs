// SPDX-License-Identifier: GPL-3.0-only

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
    /// Crop rectangle (x, y, width, height) - None means no cropping
    pub crop_rect: Option<(u32, u32, u32, u32)>,
    /// Zoom level (1.0 = no zoom, 2.0 = 2x zoom center crop)
    pub zoom_level: f32,
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
            crop_rect: None,
            zoom_level: 1.0,
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
            match apply_filter_gpu_rgba(&frame.data, frame_width, frame_height, config.filter_type)
                .await
            {
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

        // Step 2: Apply aspect ratio cropping if configured
        let (cropped_rgba, current_width, current_height) = if let Some((x, y, w, h)) =
            config.crop_rect
        {
            debug!(x, y, width = w, height = h, "Applying aspect ratio crop");
            let cropped = Self::crop_rgba(&filtered_rgba, frame_width, frame_height, x, y, w, h)?;
            (cropped, w, h)
        } else {
            (filtered_rgba, frame_width, frame_height)
        };

        // Step 3: Apply zoom (center crop) if zoom_level > 1.0
        let (final_rgba, final_width, final_height) = if config.zoom_level > 1.0 {
            Self::apply_zoom_crop(
                &cropped_rgba,
                current_width,
                current_height,
                config.zoom_level,
            )?
        } else {
            (cropped_rgba, current_width, current_height)
        };

        // Step 4: Convert filtered RGBA to RGB (drop alpha channel)
        let rgb_image = Self::convert_rgba_to_rgb(&final_rgba, final_width, final_height)?;

        // Step 5 & 6: Apply adjustments and sharpening (CPU-bound)
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
            width: final_width,
            height: final_height,
            image: rgb_image,
        })
    }

    /// Crop RGBA data to a rectangular region
    fn crop_rgba(
        rgba_data: &[u8],
        src_width: u32,
        src_height: u32,
        x: u32,
        y: u32,
        width: u32,
        height: u32,
    ) -> Result<Vec<u8>, String> {
        // Validate bounds
        if x + width > src_width || y + height > src_height {
            return Err(format!(
                "Crop region ({},{} {}x{}) exceeds source dimensions ({}x{})",
                x, y, width, height, src_width, src_height
            ));
        }

        let src_stride = src_width as usize * 4;
        let dst_stride = width as usize * 4;
        let mut cropped = vec![0u8; (width * height * 4) as usize];

        for row in 0..height as usize {
            let src_row_start = ((y as usize + row) * src_stride) + (x as usize * 4);
            let dst_row_start = row * dst_stride;
            cropped[dst_row_start..dst_row_start + dst_stride]
                .copy_from_slice(&rgba_data[src_row_start..src_row_start + dst_stride]);
        }

        Ok(cropped)
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

    /// Apply zoom by center-cropping the RGBA image
    ///
    /// At zoom_level 2.0, the center 50% of the image is cropped and returned.
    fn apply_zoom_crop(
        rgba_data: &[u8],
        width: u32,
        height: u32,
        zoom_level: f32,
    ) -> Result<(Vec<u8>, u32, u32), String> {
        if zoom_level <= 1.0 {
            return Ok((rgba_data.to_vec(), width, height));
        }

        // Calculate cropped dimensions
        let crop_width = (width as f32 / zoom_level).round() as u32;
        let crop_height = (height as f32 / zoom_level).round() as u32;

        // Ensure minimum size
        let crop_width = crop_width.max(1);
        let crop_height = crop_height.max(1);

        // Calculate center offset
        let offset_x = (width - crop_width) / 2;
        let offset_y = (height - crop_height) / 2;

        debug!(
            zoom_level,
            original_width = width,
            original_height = height,
            crop_width,
            crop_height,
            offset_x,
            offset_y,
            "Applying zoom crop"
        );

        // Extract the center region
        let mut cropped_data = Vec::with_capacity((crop_width * crop_height * 4) as usize);
        let bytes_per_pixel = 4;
        let src_stride = width * bytes_per_pixel;

        for y in 0..crop_height {
            let src_y = offset_y + y;
            let src_row_start = (src_y * src_stride + offset_x * bytes_per_pixel) as usize;
            let src_row_end = src_row_start + (crop_width * bytes_per_pixel) as usize;

            if src_row_end <= rgba_data.len() {
                cropped_data.extend_from_slice(&rgba_data[src_row_start..src_row_end]);
            } else {
                return Err("Zoom crop out of bounds".to_string());
            }
        }

        Ok((cropped_data, crop_width, crop_height))
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
