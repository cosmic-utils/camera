// SPDX-License-Identifier: MPL-2.0

//! Async post-processing pipeline for photos
//!
//! This module handles post-processing operations on captured frames:
//! - Color space conversion (NV12 → RGB)
//! - Color correction
//! - Sharpening
//! - Brightness/contrast adjustments

use crate::app::FilterType;
use crate::backends::camera::types::CameraFrame;
use image::RgbImage;
use std::sync::Arc;
use tracing::{debug, info};

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
    /// This runs all post-processing steps in a background task to avoid
    /// blocking the main thread or preview stream.
    ///
    /// # Arguments
    /// * `frame` - Raw camera frame (NV12 format)
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

        // Step 1: Convert NV12 to RGB using optimized converter
        let mut rgb_image = crate::media::convert_nv12_to_rgb(frame).await?;

        // Run remaining processing in a background task (CPU-bound)
        tokio::task::spawn_blocking(move || {
            // Step 2: Apply filter based on filter_type
            Self::apply_filter(&mut rgb_image, config.filter_type);

            // Step 3: Apply adjustments if configured
            if config.brightness != 0.0 || config.contrast != 1.0 || config.saturation != 1.0 {
                Self::apply_adjustments(&mut rgb_image, &config);
            }

            // Step 4: Apply sharpening if enabled
            if config.sharpening {
                Self::apply_sharpening(&mut rgb_image);
            }

            debug!("Post-processing complete");

            Ok(ProcessedImage {
                width: frame_width,
                height: frame_height,
                image: rgb_image,
            })
        })
        .await
        .map_err(|e| format!("Post-processing task error: {}", e))?
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

    /// Apply filter based on filter type
    ///
    /// Applies the same filters as the GPU shader for consistent results.
    fn apply_filter(image: &mut RgbImage, filter_type: FilterType) {
        match filter_type {
            FilterType::Standard => {
                // No filter - keep original colors
            }
            FilterType::Mono => {
                // Black & white using BT.601 luminance formula
                for pixel in image.pixels_mut() {
                    let r = pixel[0] as f32;
                    let g = pixel[1] as f32;
                    let b = pixel[2] as f32;
                    let luminance = (0.299 * r + 0.587 * g + 0.114 * b) as u8;
                    pixel[0] = luminance;
                    pixel[1] = luminance;
                    pixel[2] = luminance;
                }
            }
            FilterType::Sepia => {
                // Warm brownish vintage tone
                for pixel in image.pixels_mut() {
                    let r = pixel[0] as f32;
                    let g = pixel[1] as f32;
                    let b = pixel[2] as f32;
                    let luminance = 0.299 * r + 0.587 * g + 0.114 * b;
                    pixel[0] = ((luminance * 1.2 + 0.1 * 255.0).clamp(0.0, 255.0)) as u8;
                    pixel[1] = ((luminance * 0.9 + 0.05 * 255.0).clamp(0.0, 255.0)) as u8;
                    pixel[2] = (luminance * 0.7).clamp(0.0, 255.0) as u8;
                }
            }
            FilterType::Noir => {
                // High contrast black & white
                for pixel in image.pixels_mut() {
                    let r = pixel[0] as f32;
                    let g = pixel[1] as f32;
                    let b = pixel[2] as f32;
                    let luminance = 0.299 * r + 0.587 * g + 0.114 * b;
                    let adjusted = ((luminance / 255.0 - 0.5) * 2.0 + 0.5) * 255.0;
                    let noir_val = adjusted.clamp(0.0, 255.0) as u8;
                    pixel[0] = noir_val;
                    pixel[1] = noir_val;
                    pixel[2] = noir_val;
                }
            }
            FilterType::Vivid => {
                // Boosted saturation and contrast for punchy colors
                for pixel in image.pixels_mut() {
                    let r = pixel[0] as f32;
                    let g = pixel[1] as f32;
                    let b = pixel[2] as f32;
                    let luminance = 0.299 * r + 0.587 * g + 0.114 * b;
                    // Boost saturation by 1.4x
                    let mut new_r = (luminance + (r - luminance) * 1.4).clamp(0.0, 255.0);
                    let mut new_g = (luminance + (g - luminance) * 1.4).clamp(0.0, 255.0);
                    let mut new_b = (luminance + (b - luminance) * 1.4).clamp(0.0, 255.0);
                    // Apply slight contrast boost
                    new_r = ((new_r / 255.0 - 0.5) * 1.15 + 0.5).clamp(0.0, 1.0) * 255.0;
                    new_g = ((new_g / 255.0 - 0.5) * 1.15 + 0.5).clamp(0.0, 1.0) * 255.0;
                    new_b = ((new_b / 255.0 - 0.5) * 1.15 + 0.5).clamp(0.0, 1.0) * 255.0;
                    pixel[0] = new_r as u8;
                    pixel[1] = new_g as u8;
                    pixel[2] = new_b as u8;
                }
            }
            FilterType::Cool => {
                // Blue color temperature shift
                for pixel in image.pixels_mut() {
                    pixel[0] = ((pixel[0] as f32 * 0.9).clamp(0.0, 255.0)) as u8;
                    pixel[1] = ((pixel[1] as f32 * 0.95).clamp(0.0, 255.0)) as u8;
                    pixel[2] = ((pixel[2] as f32 * 1.1).clamp(0.0, 255.0)) as u8;
                }
            }
            FilterType::Warm => {
                // Orange/amber color temperature
                for pixel in image.pixels_mut() {
                    pixel[0] = ((pixel[0] as f32 * 1.1).clamp(0.0, 255.0)) as u8;
                    // Green stays the same
                    pixel[2] = ((pixel[2] as f32 * 0.85).clamp(0.0, 255.0)) as u8;
                }
            }
            FilterType::Fade => {
                // Lifted blacks with muted colors for vintage look
                for pixel in image.pixels_mut() {
                    let r = pixel[0] as f32;
                    let g = pixel[1] as f32;
                    let b = pixel[2] as f32;
                    // Lift blacks
                    let mut new_r = (r * 0.85 + 0.1 * 255.0).clamp(0.0, 255.0);
                    let mut new_g = (g * 0.85 + 0.1 * 255.0).clamp(0.0, 255.0);
                    let mut new_b = (b * 0.85 + 0.1 * 255.0).clamp(0.0, 255.0);
                    // Reduce saturation
                    let luminance = 0.299 * new_r + 0.587 * new_g + 0.114 * new_b;
                    new_r = (luminance + (new_r - luminance) * 0.7).clamp(0.0, 255.0);
                    new_g = (luminance + (new_g - luminance) * 0.7).clamp(0.0, 255.0);
                    new_b = (luminance + (new_b - luminance) * 0.7).clamp(0.0, 255.0);
                    pixel[0] = new_r as u8;
                    pixel[1] = new_g as u8;
                    pixel[2] = new_b as u8;
                }
            }
            FilterType::Duotone => {
                // Two-color gradient mapping (dark blue to golden yellow)
                for pixel in image.pixels_mut() {
                    let r = pixel[0] as f32 / 255.0;
                    let g = pixel[1] as f32 / 255.0;
                    let b = pixel[2] as f32 / 255.0;
                    let luminance = 0.299 * r + 0.587 * g + 0.114 * b;
                    // Dark: (0.1, 0.1, 0.4), Light: (1.0, 0.9, 0.5)
                    let new_r = 0.1 + luminance * (1.0 - 0.1);
                    let new_g = 0.1 + luminance * (0.9 - 0.1);
                    let new_b = 0.4 + luminance * (0.5 - 0.4);
                    pixel[0] = (new_r * 255.0).clamp(0.0, 255.0) as u8;
                    pixel[1] = (new_g * 255.0).clamp(0.0, 255.0) as u8;
                    pixel[2] = (new_b * 255.0).clamp(0.0, 255.0) as u8;
                }
            }
            FilterType::Vignette => {
                // Darkened edges
                let (width, height) = image.dimensions();
                let center_x = width as f32 / 2.0;
                let center_y = height as f32 / 2.0;
                let max_dist = (center_x * center_x + center_y * center_y).sqrt();
                for y in 0..height {
                    for x in 0..width {
                        let pixel = image.get_pixel_mut(x, y);
                        let dx = x as f32 - center_x;
                        let dy = y as f32 - center_y;
                        let dist = (dx * dx + dy * dy).sqrt() / max_dist;
                        // smoothstep(0.3, 0.9, dist)
                        let t = ((dist - 0.3) / (0.9 - 0.3)).clamp(0.0, 1.0);
                        let vignette = 1.0 - (t * t * (3.0 - 2.0 * t));
                        pixel[0] = (pixel[0] as f32 * vignette) as u8;
                        pixel[1] = (pixel[1] as f32 * vignette) as u8;
                        pixel[2] = (pixel[2] as f32 * vignette) as u8;
                    }
                }
            }
            FilterType::Negative => {
                // Inverted colors
                for pixel in image.pixels_mut() {
                    pixel[0] = 255 - pixel[0];
                    pixel[1] = 255 - pixel[1];
                    pixel[2] = 255 - pixel[2];
                }
            }
            FilterType::Posterize => {
                // Reduced color levels
                let levels = 4.0;
                for pixel in image.pixels_mut() {
                    let r = pixel[0] as f32 / 255.0;
                    let g = pixel[1] as f32 / 255.0;
                    let b = pixel[2] as f32 / 255.0;
                    pixel[0] = ((r * levels).floor() / levels * 255.0) as u8;
                    pixel[1] = ((g * levels).floor() / levels * 255.0) as u8;
                    pixel[2] = ((b * levels).floor() / levels * 255.0) as u8;
                }
            }
            FilterType::Solarize => {
                // Partially inverted tones
                let threshold = 128;
                for pixel in image.pixels_mut() {
                    if pixel[0] > threshold {
                        pixel[0] = 255 - pixel[0];
                    }
                    if pixel[1] > threshold {
                        pixel[1] = 255 - pixel[1];
                    }
                    if pixel[2] > threshold {
                        pixel[2] = 255 - pixel[2];
                    }
                }
            }
            FilterType::ChromaticAberration => {
                // RGB channel split (scales with image resolution)
                let (width, height) = image.dimensions();
                let original = image.clone();
                // Scale offset as percentage of width (0.4%)
                let offset = (width as f32 * 0.004) as i32;
                for y in 0..height {
                    for x in 0..width {
                        let pixel = image.get_pixel_mut(x, y);
                        // Red from right
                        let rx = (x as i32 + offset).clamp(0, width as i32 - 1) as u32;
                        pixel[0] = original.get_pixel(rx, y)[0];
                        // Blue from left
                        let bx = (x as i32 - offset).clamp(0, width as i32 - 1) as u32;
                        pixel[2] = original.get_pixel(bx, y)[2];
                    }
                }
            }
            FilterType::Pencil => {
                // Pencil sketch drawing effect
                let (width, height) = image.dimensions();
                let original = image.clone();
                for y in 1..height - 1 {
                    for x in 1..width - 1 {
                        // Sobel edge detection
                        let get_lum = |px: u32, py: u32| -> f32 {
                            let p = original.get_pixel(px, py);
                            0.299 * p[0] as f32 + 0.587 * p[1] as f32 + 0.114 * p[2] as f32
                        };
                        let tl = get_lum(x - 1, y - 1);
                        let tm = get_lum(x, y - 1);
                        let tr = get_lum(x + 1, y - 1);
                        let ml = get_lum(x - 1, y);
                        let mr = get_lum(x + 1, y);
                        let bl = get_lum(x - 1, y + 1);
                        let bm = get_lum(x, y + 1);
                        let br = get_lum(x + 1, y + 1);
                        let gx = -tl - 2.0 * ml - bl + tr + 2.0 * mr + br;
                        let gy = -tl - 2.0 * tm - tr + bl + 2.0 * bm + br;
                        let edge = (gx * gx + gy * gy).sqrt() / 255.0;

                        // Pencil lines on white
                        let pencil = 1.0 - edge * 2.0;
                        // Paper texture using simple hash
                        use std::collections::hash_map::DefaultHasher;
                        use std::hash::{Hash, Hasher};
                        let mut hasher = DefaultHasher::new();
                        (x, y).hash(&mut hasher);
                        let hash = hasher.finish();
                        let noise = ((hash % 1000) as f32 / 1000.0) * 0.05;
                        let paper = 0.95 + noise;
                        let final_val = (pencil * paper).clamp(0.0, 1.0);
                        let gray = (final_val * 255.0) as u8;
                        let pixel = image.get_pixel_mut(x, y);
                        pixel[0] = gray;
                        pixel[1] = gray;
                        pixel[2] = gray;
                    }
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
