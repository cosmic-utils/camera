// SPDX-License-Identifier: GPL-3.0-only

//! LAS point cloud export
//!
//! Exports depth + color data as an uncompressed LAS point cloud file.
//! Applies depth-to-RGB registration for correct color alignment.

use super::{CameraIntrinsics, RegistrationData, SceneCaptureConfig};
use crate::shaders::depth::kinect;
use las::{Builder, Color, Point, Writer};
use std::path::PathBuf;
use tracing::{debug, info};

/// Export point cloud as LAS file with color
pub async fn export_point_cloud_las(
    rgb_data: &[u8],
    rgb_width: u32,
    rgb_height: u32,
    depth_data: &[u16],
    depth_width: u32,
    depth_height: u32,
    output_path: &PathBuf,
    config: &SceneCaptureConfig,
) -> Result<(), String> {
    let rgb_data = rgb_data.to_vec();
    let depth_data = depth_data.to_vec();
    let output_path = output_path.clone();
    let intrinsics = config.intrinsics;
    let depth_format = config.depth_format;
    let mirror = config.mirror;
    let registration = config.registration.clone();

    tokio::task::spawn_blocking(move || {
        export_las_sync(
            &rgb_data,
            rgb_width,
            rgb_height,
            &depth_data,
            depth_width,
            depth_height,
            &output_path,
            &intrinsics,
            depth_format,
            mirror,
            registration.as_ref(),
        )
    })
    .await
    .map_err(|e| format!("Task join error: {}", e))?
}

/// Get registered RGB coordinates for a depth pixel
/// Applies high-res scaling for 1280x1024 RGB mode (registration tables are built for 640x480)
fn get_registered_rgb_coords(
    x: u32,
    y: u32,
    depth_mm: u32,
    depth_width: u32,
    rgb_width: u32,
    rgb_height: u32,
    registration: &RegistrationData,
) -> Option<(u32, u32)> {
    let (rgb_x, rgb_y) =
        registration.get_rgb_coords(x, y, depth_mm, depth_width, rgb_width, rgb_height)?;
    Some((rgb_x as u32, rgb_y as u32))
}

#[allow(clippy::too_many_arguments)]
fn export_las_sync(
    rgb_data: &[u8],
    rgb_width: u32,
    rgb_height: u32,
    depth_data: &[u16],
    depth_width: u32,
    depth_height: u32,
    output_path: &PathBuf,
    intrinsics: &CameraIntrinsics,
    depth_format: crate::shaders::DepthFormat,
    mirror: bool,
    registration: Option<&RegistrationData>,
) -> Result<(), String> {
    // Debug logging to compare with shader values
    if let Some(reg) = registration {
        let center_idx = 240 * 640 + 320;
        info!(
            target_offset = reg.target_offset,
            reg_x_val_scale = reg.reg_x_val_scale,
            table_len = reg.registration_table.len(),
            shift_len = reg.depth_to_rgb_shift.len(),
            center_reg_x = reg
                .registration_table
                .get(center_idx)
                .map(|v| v[0])
                .unwrap_or(-1),
            center_reg_y = reg
                .registration_table
                .get(center_idx)
                .map(|v| v[1])
                .unwrap_or(-1),
            shift_1000mm = reg.depth_to_rgb_shift.get(1000).copied().unwrap_or(-1),
            "LAS export registration data"
        );
    } else {
        info!("LAS export: no registration data available");
    }

    // Collect valid 3D points with color
    let mut points: Vec<(f64, f64, f64, u16, u16, u16)> = Vec::new();

    for y in 0..depth_height {
        for x in 0..depth_width {
            let depth_idx = (y * depth_width + x) as usize;
            let depth_raw = depth_data[depth_idx];

            // Convert to meters based on depth format
            let (depth_m, depth_mm) = match depth_format {
                crate::shaders::DepthFormat::Millimeters => {
                    if depth_raw == 0 || depth_raw >= 10000 {
                        continue; // Invalid depth
                    }
                    (depth_raw as f32 / 1000.0, depth_raw as u32)
                }
                crate::shaders::DepthFormat::Disparity16 => {
                    if depth_raw >= 65472 {
                        continue; // Invalid depth marker
                    }
                    let raw = (depth_raw >> 6) as f32;
                    let denom = raw * kinect::DEPTH_COEFF_A + kinect::DEPTH_COEFF_B;
                    if denom <= 0.01 {
                        continue;
                    }
                    let dm = 1.0 / denom;
                    (dm, (dm * 1000.0) as u32)
                }
            };

            // Check depth range
            if depth_m < intrinsics.min_depth || depth_m > intrinsics.max_depth {
                continue;
            }

            // Get RGB color with registration (same as shader)
            let (rgb_x, rgb_y) = if let Some(reg) = registration {
                if let Some(coords) = get_registered_rgb_coords(
                    x,
                    y,
                    depth_mm,
                    depth_width,
                    rgb_width,
                    rgb_height,
                    reg,
                ) {
                    coords
                } else {
                    // Registration out of bounds - skip this point
                    continue;
                }
            } else {
                // No registration - use simple mapping
                let rx = if rgb_width != depth_width {
                    ((x as f32 * rgb_width as f32 / depth_width as f32) as u32).min(rgb_width - 1)
                } else {
                    x
                };
                let ry = if rgb_height != depth_height {
                    ((y as f32 * rgb_height as f32 / depth_height as f32) as u32)
                        .min(rgb_height - 1)
                } else {
                    y
                };
                (rx, ry)
            };

            let rgb_idx = ((rgb_y * rgb_width + rgb_x) * 4) as usize;
            let r = rgb_data.get(rgb_idx).copied().unwrap_or(128) as u16 * 256;
            let g = rgb_data.get(rgb_idx + 1).copied().unwrap_or(128) as u16 * 256;
            let b = rgb_data.get(rgb_idx + 2).copied().unwrap_or(128) as u16 * 256;

            // Unproject to 3D
            let mut px = ((x as f32 - intrinsics.cx) * depth_m / intrinsics.fx) as f64;
            let py = -((y as f32 - intrinsics.cy) * depth_m / intrinsics.fy) as f64;
            let pz = depth_m as f64;

            if mirror {
                px = -px;
            }

            points.push((px, py, pz, r, g, b));
        }
    }

    if points.is_empty() {
        return Err("No valid depth points to export".to_string());
    }

    info!(
        point_count = points.len(),
        path = %output_path.display(),
        "Exporting point cloud"
    );

    // Calculate bounds for LAS header
    let (min_x, max_x) = points
        .iter()
        .map(|p| p.0)
        .fold((f64::MAX, f64::MIN), |(min, max), x| {
            (min.min(x), max.max(x))
        });
    let (min_y, max_y) = points
        .iter()
        .map(|p| p.1)
        .fold((f64::MAX, f64::MIN), |(min, max), y| {
            (min.min(y), max.max(y))
        });
    let (min_z, max_z) = points
        .iter()
        .map(|p| p.2)
        .fold((f64::MAX, f64::MIN), |(min, max), z| {
            (min.min(z), max.max(z))
        });

    // Build LAS header
    let mut builder = Builder::from((1, 4)); // LAS 1.4
    builder.point_format.has_color = true;
    builder.point_format.is_compressed = false; // Uncompressed LAS

    // Set transforms for coordinate precision
    let scale = 0.001; // 1mm precision
    builder.transforms = las::Vector {
        x: las::Transform {
            scale,
            offset: (min_x + max_x) / 2.0,
        },
        y: las::Transform {
            scale,
            offset: (min_y + max_y) / 2.0,
        },
        z: las::Transform {
            scale,
            offset: (min_z + max_z) / 2.0,
        },
    };

    let header = builder
        .into_header()
        .map_err(|e| format!("Failed to build LAS header: {}", e))?;

    // Create writer
    let mut writer = Writer::from_path(output_path, header)
        .map_err(|e| format!("Failed to create LAS writer: {}", e))?;

    // Write points
    for (px, py, pz, r, g, b) in points {
        let mut point = Point::default();
        point.x = px;
        point.y = py;
        point.z = pz;
        point.color = Some(Color::new(r, g, b));

        writer
            .write_point(point)
            .map_err(|e| format!("Failed to write point: {}", e))?;
    }

    writer
        .close()
        .map_err(|e| format!("Failed to close LAS file: {}", e))?;

    debug!(
        path = %output_path.display(),
        "LAS export complete"
    );

    Ok(())
}
