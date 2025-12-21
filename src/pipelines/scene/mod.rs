// SPDX-License-Identifier: GPL-3.0-only

//! Scene capture pipeline
//!
//! Captures 3D scene data including:
//! - Raw depth image (grayscale)
//! - Raw color image
//! - Preview image (rendered 3D view)
//! - Point cloud with color (LAZ format)
//! - 3D mesh with texture (GLTF format)

mod gltf_export;
mod laz_export;

pub use gltf_export::export_mesh_gltf;
pub use laz_export::export_point_cloud_las;

use crate::pipelines::photo::encoding::EncodingFormat;
use crate::shaders::depth::kinect;
use image::{GrayImage, RgbImage, RgbaImage};
use std::path::PathBuf;
use tracing::{debug, info};

/// Scene capture configuration
#[derive(Clone)]
pub struct SceneCaptureConfig {
    /// Output format for images (JPEG, PNG, DNG)
    pub image_format: EncodingFormat,
    /// Camera intrinsics for 3D reconstruction
    pub intrinsics: CameraIntrinsics,
    /// Depth format (millimeters or disparity)
    pub depth_format: crate::shaders::DepthFormat,
    /// Whether to mirror the output
    pub mirror: bool,
    /// Registration data for depth-to-RGB alignment (optional)
    pub registration: Option<RegistrationData>,
}

/// Registration data for depth-to-RGB alignment
#[derive(Clone)]
pub struct RegistrationData {
    /// Registration table: 640*480 [x_scaled, y] pairs
    pub registration_table: Vec<[i32; 2]>,
    /// Depth-to-RGB shift table: 10001 i32 values indexed by depth_mm
    pub depth_to_rgb_shift: Vec<i32>,
    /// Target offset from pad_info
    pub target_offset: u32,
    /// Scale factor for x values (typically 256)
    pub reg_x_val_scale: i32,
    /// X scale factor for high-res RGB (1.0 for 640, 2.0 for 1280)
    pub reg_scale_x: f32,
    /// Y scale factor for high-res RGB (same as X to maintain aspect ratio)
    pub reg_scale_y: f32,
    /// Y offset for high-res RGB (typically 0 for top-aligned crop)
    pub reg_y_offset: i32,
}

impl RegistrationData {
    /// Get registered RGB pixel coordinates for a depth pixel
    ///
    /// Applies the registration transform from depth space to RGB space,
    /// accounting for high-res scaling if needed.
    ///
    /// Returns None if the coordinates are out of bounds or registration data is invalid.
    pub fn get_rgb_coords(
        &self,
        x: u32,
        y: u32,
        depth_mm: u32,
        depth_width: u32,
        rgb_width: u32,
        rgb_height: u32,
    ) -> Option<(i32, i32)> {
        let reg_idx = (y * depth_width + x) as usize;
        if reg_idx >= self.registration_table.len() {
            return None;
        }

        let reg = self.registration_table[reg_idx];
        let clamped_depth_mm = depth_mm.min(10000) as usize;

        if clamped_depth_mm >= self.depth_to_rgb_shift.len() {
            return None;
        }

        let shift = self.depth_to_rgb_shift[clamped_depth_mm];

        // Calculate RGB coordinates using registration formula from libfreenect
        // Base coordinates are in 640x480 space
        let rgb_x_scaled = reg[0] + shift;
        let rgb_x_base = rgb_x_scaled / self.reg_x_val_scale;
        let rgb_y_base = reg[1] - self.target_offset as i32;

        // Scale to actual RGB resolution (for 1280x1024, scale by 2.0)
        let rgb_x = (rgb_x_base as f32 * self.reg_scale_x) as i32;
        let rgb_y = (rgb_y_base as f32 * self.reg_scale_y) as i32 + self.reg_y_offset;

        // Check bounds
        if rgb_x < 0 || rgb_x >= rgb_width as i32 || rgb_y < 0 || rgb_y >= rgb_height as i32 {
            return None;
        }

        Some((rgb_x, rgb_y))
    }
}

/// Camera intrinsics for depth-to-3D unprojection
#[derive(Clone, Copy)]
pub struct CameraIntrinsics {
    pub fx: f32,
    pub fy: f32,
    pub cx: f32,
    pub cy: f32,
    pub min_depth: f32,
    pub max_depth: f32,
}

impl Default for CameraIntrinsics {
    fn default() -> Self {
        // Kinect defaults for 640x480 base resolution
        Self {
            fx: kinect::FX,
            fy: kinect::FY,
            cx: kinect::CX,
            cy: kinect::CY,
            min_depth: 0.4,
            max_depth: 4.0,
        }
    }
}

/// Result of scene capture
pub struct SceneCaptureResult {
    pub scene_dir: PathBuf,
    pub depth_path: PathBuf,
    pub color_path: PathBuf,
    pub preview_path: PathBuf,
    pub pointcloud_path: PathBuf,
    pub mesh_path: PathBuf,
}

/// Capture and save a complete scene
///
/// Creates a directory containing:
/// - depth.{format} - Raw depth as grayscale
/// - color.{format} - Raw color image
/// - preview.{format} - Rendered 3D preview
/// - pointcloud.las - Point cloud with color
/// - mesh.glb - 3D mesh with texture
pub async fn capture_scene(
    rgb_data: &[u8],
    rgb_width: u32,
    rgb_height: u32,
    depth_data: &[u16],
    depth_width: u32,
    depth_height: u32,
    preview_data: Option<&[u8]>,
    preview_width: u32,
    preview_height: u32,
    output_dir: PathBuf,
    config: SceneCaptureConfig,
) -> Result<SceneCaptureResult, String> {
    // Create timestamped scene directory
    let timestamp = chrono::Local::now().format("%Y%m%d_%H%M%S");
    let scene_dir = output_dir.join(format!("scene_{}", timestamp));
    tokio::fs::create_dir_all(&scene_dir)
        .await
        .map_err(|e| format!("Failed to create scene directory: {}", e))?;

    info!(scene_dir = %scene_dir.display(), "Creating scene capture");

    let ext = config.image_format.extension();

    // 1. Save depth image (grayscale)
    let depth_path = scene_dir.join(format!("depth.{}", ext));
    save_depth_image(depth_data, depth_width, depth_height, &depth_path, &config).await?;
    debug!(path = %depth_path.display(), "Saved depth image");

    // 2. Save color image
    let color_path = scene_dir.join(format!("color.{}", ext));
    save_color_image(rgb_data, rgb_width, rgb_height, &color_path, &config).await?;
    debug!(path = %color_path.display(), "Saved color image");

    // 3. Save preview image (rendered 3D view)
    let preview_path = scene_dir.join(format!("preview.{}", ext));
    if let Some(preview) = preview_data {
        save_preview_image(
            preview,
            preview_width,
            preview_height,
            &preview_path,
            &config,
        )
        .await?;
        debug!(path = %preview_path.display(), "Saved preview image");
    } else {
        // If no preview, copy the color image
        tokio::fs::copy(&color_path, &preview_path)
            .await
            .map_err(|e| format!("Failed to copy preview: {}", e))?;
    }

    // 4. Export point cloud as LAS
    let pointcloud_path = scene_dir.join("pointcloud.las");
    export_point_cloud_las(
        rgb_data,
        rgb_width,
        rgb_height,
        depth_data,
        depth_width,
        depth_height,
        &pointcloud_path,
        &config,
    )
    .await?;
    debug!(path = %pointcloud_path.display(), "Saved point cloud");

    // 5. Export mesh as GLB with vertex colors (registration applied directly)
    let mesh_path = scene_dir.join("mesh.glb");
    export_mesh_gltf(
        rgb_data,
        rgb_width,
        rgb_height,
        depth_data,
        depth_width,
        depth_height,
        &mesh_path,
        &mesh_path, // Unused - we use vertex colors now
        &config,
    )
    .await?;
    debug!(path = %mesh_path.display(), "Saved mesh");

    info!(
        scene_dir = %scene_dir.display(),
        "Scene capture complete"
    );

    Ok(SceneCaptureResult {
        scene_dir: scene_dir.clone(),
        depth_path,
        color_path,
        preview_path,
        pointcloud_path,
        mesh_path,
    })
}

/// Save depth data as grayscale image
async fn save_depth_image(
    depth_data: &[u16],
    width: u32,
    height: u32,
    path: &PathBuf,
    config: &SceneCaptureConfig,
) -> Result<(), String> {
    let depth_data = depth_data.to_vec();
    let path = path.clone();
    let format = config.image_format;

    tokio::task::spawn_blocking(move || {
        // Convert 16-bit depth to 8-bit grayscale for visualization
        // Normalize to 0-255 range based on valid depth range
        let mut gray_data = Vec::with_capacity((width * height) as usize);

        for &d in &depth_data {
            let normalized = if d == 0 || d >= 10000 {
                0u8 // Invalid depth = black
            } else {
                // Normalize depth to 0-255 (closer = brighter)
                let depth_m = d as f32 / 1000.0;
                let normalized = 1.0 - (depth_m - 0.4) / (4.0 - 0.4);
                (normalized.clamp(0.0, 1.0) * 255.0) as u8
            };
            gray_data.push(normalized);
        }

        let gray_image = GrayImage::from_raw(width, height, gray_data)
            .ok_or("Failed to create grayscale image")?;

        match format {
            EncodingFormat::Jpeg => {
                let rgb_image: RgbImage = image::DynamicImage::ImageLuma8(gray_image).into_rgb8();
                rgb_image
                    .save(&path)
                    .map_err(|e| format!("Failed to save depth JPEG: {}", e))
            }
            EncodingFormat::Png => gray_image
                .save(&path)
                .map_err(|e| format!("Failed to save depth PNG: {}", e)),
            EncodingFormat::Dng => {
                // For DNG, save as 16-bit PNG instead (DNG is for RGB)
                let path = path.with_extension("png");
                // Save raw 16-bit depth
                let img = image::ImageBuffer::<image::Luma<u16>, Vec<u16>>::from_raw(
                    width, height, depth_data,
                )
                .ok_or("Failed to create 16-bit depth image")?;
                img.save(&path)
                    .map_err(|e| format!("Failed to save depth 16-bit PNG: {}", e))
            }
        }
    })
    .await
    .map_err(|e| format!("Task join error: {}", e))?
}

/// Save color image
async fn save_color_image(
    rgb_data: &[u8],
    width: u32,
    height: u32,
    path: &PathBuf,
    config: &SceneCaptureConfig,
) -> Result<(), String> {
    let rgb_data = rgb_data.to_vec();
    let path = path.clone();
    let format = config.image_format;

    tokio::task::spawn_blocking(move || {
        // Convert RGBA to RGB
        let rgb_only: Vec<u8> = rgb_data.chunks(4).flat_map(|c| &c[0..3]).copied().collect();

        let rgb_image = RgbImage::from_raw(width, height, rgb_only)
            .ok_or("Failed to create RGB image from color data")?;

        match format {
            EncodingFormat::Jpeg => {
                let mut buf = Vec::new();
                let mut encoder = image::codecs::jpeg::JpegEncoder::new_with_quality(&mut buf, 92);
                encoder
                    .encode_image(&rgb_image)
                    .map_err(|e| format!("Failed to encode JPEG: {}", e))?;
                std::fs::write(&path, buf).map_err(|e| format!("Failed to write JPEG: {}", e))
            }
            EncodingFormat::Png | EncodingFormat::Dng => {
                // Use PNG for both PNG and DNG (DNG is complex for simple RGB)
                rgb_image
                    .save(&path)
                    .map_err(|e| format!("Failed to save PNG: {}", e))
            }
        }
    })
    .await
    .map_err(|e| format!("Task join error: {}", e))?
}

/// Save preview image (rendered 3D view)
async fn save_preview_image(
    preview_data: &[u8],
    width: u32,
    height: u32,
    path: &PathBuf,
    config: &SceneCaptureConfig,
) -> Result<(), String> {
    let preview_data = preview_data.to_vec();
    let path = path.clone();
    let format = config.image_format;

    tokio::task::spawn_blocking(move || {
        // Preview data is RGBA from the shader
        let rgba_image = RgbaImage::from_raw(width, height, preview_data)
            .ok_or("Failed to create RGBA image from preview data")?;

        // Convert to RGB
        let rgb_image: RgbImage = image::DynamicImage::ImageRgba8(rgba_image).into_rgb8();

        match format {
            EncodingFormat::Jpeg => {
                let mut buf = Vec::new();
                let mut encoder = image::codecs::jpeg::JpegEncoder::new_with_quality(&mut buf, 92);
                encoder
                    .encode_image(&rgb_image)
                    .map_err(|e| format!("Failed to encode preview JPEG: {}", e))?;
                std::fs::write(&path, buf)
                    .map_err(|e| format!("Failed to write preview JPEG: {}", e))
            }
            EncodingFormat::Png | EncodingFormat::Dng => rgb_image
                .save(&path)
                .map_err(|e| format!("Failed to save preview PNG: {}", e)),
        }
    })
    .await
    .map_err(|e| format!("Task join error: {}", e))?
}
