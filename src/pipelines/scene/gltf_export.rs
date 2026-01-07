// SPDX-License-Identifier: GPL-3.0-only

#![cfg(target_arch = "x86_64")]

//! GLTF mesh export
//!
//! Exports depth + color data as a GLB (binary glTF) mesh with a texture.
//! Applies depth-to-RGB registration for correct UV mapping.
//! Origin is at the Kinect camera position.

use super::{CameraIntrinsics, RegistrationData, SceneCaptureConfig};
use crate::shaders::kinect_intrinsics as kinect;
use std::path::PathBuf;
use tracing::{debug, info};

/// Depth discontinuity threshold in meters (same as mesh shader)
const DEPTH_DISCONTINUITY_THRESHOLD: f32 = 0.1;

/// Export mesh as GLB file with texture (applies registration via UV coordinates)
pub async fn export_mesh_gltf(
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
    let config = config.clone();

    tokio::task::spawn_blocking(move || {
        export_gltf_sync(
            &rgb_data,
            rgb_width,
            rgb_height,
            &depth_data,
            depth_width,
            depth_height,
            &output_path,
            &config,
        )
    })
    .await
    .map_err(|e| format!("Task join error: {}", e))?
}

fn export_gltf_sync(
    rgb_data: &[u8],
    rgb_width: u32,
    rgb_height: u32,
    depth_data: &[u16],
    depth_width: u32,
    depth_height: u32,
    output_path: &PathBuf,
    config: &SceneCaptureConfig,
) -> Result<(), String> {
    // Generate mesh data (vertices, UVs, indices) using grid-based triangulation
    let (vertices, uvs, indices) = generate_mesh_data(
        rgb_width,
        rgb_height,
        depth_data,
        depth_width,
        depth_height,
        &config.intrinsics,
        config.depth_format,
        config.mirror,
        config.registration.as_ref(),
    )?;

    if vertices.is_empty() || indices.is_empty() {
        return Err("No valid mesh triangles generated".to_string());
    }

    info!(
        vertex_count = vertices.len() / 3,
        triangle_count = indices.len() / 3,
        rgb_resolution = format!("{}x{}", rgb_width, rgb_height),
        path = %output_path.display(),
        "Exporting mesh with texture"
    );

    // Encode RGB data as JPEG for embedding in GLB
    let texture_data = encode_texture_jpeg(rgb_data, rgb_width, rgb_height)?;

    // Build GLB file with texture
    build_glb_file(&vertices, &uvs, &indices, &texture_data, output_path)
}

/// Get registered RGB coordinates for a depth pixel as UV coordinates (0-1 range)
/// Returns the UV coordinates, or None if out of bounds
fn get_registered_uv_coords(
    x: u32,
    y: u32,
    depth_mm: u32,
    depth_width: u32,
    rgb_width: u32,
    rgb_height: u32,
    registration: &RegistrationData,
) -> Option<(f32, f32)> {
    let (rgb_x, rgb_y) =
        registration.get_rgb_coords(x, y, depth_mm, depth_width, rgb_width, rgb_height)?;

    // Convert to UV coordinates (0-1 range)
    // Note: V is flipped in glTF (0 at top, 1 at bottom)
    let u = rgb_x as f32 / rgb_width as f32;
    let v = rgb_y as f32 / rgb_height as f32;

    Some((u, v))
}

/// Generate mesh data using grid-based triangulation
/// Returns vertices (positions) and UV coordinates for texture mapping
#[allow(clippy::too_many_arguments)]
fn generate_mesh_data(
    rgb_width: u32,
    rgb_height: u32,
    depth_data: &[u16],
    depth_width: u32,
    depth_height: u32,
    intrinsics: &CameraIntrinsics,
    depth_format: crate::shaders::DepthFormat,
    mirror: bool,
    registration: Option<&RegistrationData>,
) -> Result<(Vec<f32>, Vec<f32>, Vec<u32>), String> {
    // Debug logging
    if let Some(reg) = registration {
        info!(
            target_offset = reg.target_offset,
            reg_scale_x = reg.reg_scale_x,
            reg_scale_y = reg.reg_scale_y,
            "GLB export using texture with registration"
        );
    } else {
        info!("GLB export: no registration data, using simple UV mapping");
    }

    // First pass: compute depth in meters and mm, plus registered UVs
    let mut depth_meters: Vec<f32> = vec![-1.0; (depth_width * depth_height) as usize];
    let mut depth_mm_values: Vec<u32> = vec![0; (depth_width * depth_height) as usize];
    let mut vertex_uvs: Vec<[f32; 2]> = vec![[0.0, 0.0]; (depth_width * depth_height) as usize];

    for y in 0..depth_height {
        for x in 0..depth_width {
            let idx = (y * depth_width + x) as usize;
            let depth_raw = depth_data[idx];

            let (depth_m, depth_mm) = match depth_format {
                crate::shaders::DepthFormat::Millimeters => {
                    if depth_raw == 0 || depth_raw >= 10000 {
                        continue;
                    }
                    (depth_raw as f32 / 1000.0, depth_raw as u32)
                }
                crate::shaders::DepthFormat::Disparity16 => {
                    if depth_raw >= 65472 {
                        continue;
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

            if depth_m < intrinsics.min_depth || depth_m > intrinsics.max_depth {
                continue;
            }

            depth_meters[idx] = depth_m;
            depth_mm_values[idx] = depth_mm;

            // Get registered UV coordinates
            let uv = if let Some(reg) = registration {
                if let Some((u, v)) = get_registered_uv_coords(
                    x,
                    y,
                    depth_mm,
                    depth_width,
                    rgb_width,
                    rgb_height,
                    reg,
                ) {
                    [u, v]
                } else {
                    // Registration out of bounds - skip this point
                    depth_meters[idx] = -1.0;
                    continue;
                }
            } else {
                // No registration - use simple mapping
                let u = x as f32 / depth_width as f32;
                let v = y as f32 / depth_height as f32;
                [u, v]
            };

            vertex_uvs[idx] = uv;
        }
    }

    // Second pass: generate vertices and triangles
    let mut vertices: Vec<f32> = Vec::new();
    let mut uvs: Vec<f32> = Vec::new();
    let mut indices: Vec<u32> = Vec::new();
    let mut vertex_map: Vec<i32> = vec![-1; (depth_width * depth_height) as usize];

    // Generate triangles for each 2x2 quad
    for y in 1..depth_height {
        for x in 1..depth_width {
            let idx00 = ((y - 1) * depth_width + (x - 1)) as usize;
            let idx10 = ((y - 1) * depth_width + x) as usize;
            let idx01 = (y * depth_width + (x - 1)) as usize;
            let idx11 = (y * depth_width + x) as usize;

            let d00 = depth_meters[idx00];
            let d10 = depth_meters[idx10];
            let d01 = depth_meters[idx01];
            let d11 = depth_meters[idx11];

            // Skip if any depth is invalid
            if d00 < 0.0 || d10 < 0.0 || d01 < 0.0 || d11 < 0.0 {
                continue;
            }

            // Check depth discontinuity
            let max_diff = (d00 - d10)
                .abs()
                .max((d00 - d01).abs())
                .max((d11 - d10).abs())
                .max((d11 - d01).abs());

            if max_diff > DEPTH_DISCONTINUITY_THRESHOLD {
                continue;
            }

            // Helper closure to get or create vertex
            let mut get_or_create_vertex = |px: u32, py: u32| -> Option<u32> {
                let idx = (py * depth_width + px) as usize;
                let depth_m = depth_meters[idx];

                if depth_m < 0.0 {
                    return None;
                }

                if vertex_map[idx] >= 0 {
                    return Some(vertex_map[idx] as u32);
                }

                // Unproject to 3D - origin is at camera
                let mut vx = (px as f32 - intrinsics.cx) * depth_m / intrinsics.fx;
                let vy = -((py as f32 - intrinsics.cy) * depth_m / intrinsics.fy);
                let vz = depth_m;

                if mirror {
                    vx = -vx;
                }

                let vertex_idx = (vertices.len() / 3) as u32;
                vertices.push(vx);
                vertices.push(vy);
                vertices.push(vz);

                // Add UV coordinates
                let uv = vertex_uvs[idx];
                uvs.push(uv[0]);
                uvs.push(uv[1]);

                vertex_map[idx] = vertex_idx as i32;
                Some(vertex_idx)
            };

            // Get vertex indices for quad corners
            let v00 = get_or_create_vertex(x - 1, y - 1);
            let v10 = get_or_create_vertex(x, y - 1);
            let v01 = get_or_create_vertex(x - 1, y);
            let v11 = get_or_create_vertex(x, y);

            if let (Some(i00), Some(i10), Some(i01), Some(i11)) = (v00, v10, v01, v11) {
                // glTF uses counter-clockwise winding for front faces
                // Triangle 1: (00, 01, 10)
                indices.push(i00);
                indices.push(i01);
                indices.push(i10);

                // Triangle 2: (10, 01, 11)
                indices.push(i10);
                indices.push(i01);
                indices.push(i11);
            }
        }
    }

    Ok((vertices, uvs, indices))
}

/// Encode RGBA data as JPEG for embedding in GLB
fn encode_texture_jpeg(rgb_data: &[u8], width: u32, height: u32) -> Result<Vec<u8>, String> {
    use image::{ImageBuffer, Rgba};

    // Create image from RGBA data
    let img: ImageBuffer<Rgba<u8>, Vec<u8>> =
        ImageBuffer::from_raw(width, height, rgb_data.to_vec())
            .ok_or("Failed to create image buffer")?;

    // Convert to RGB and encode as JPEG
    let rgb_img = image::DynamicImage::ImageRgba8(img).into_rgb8();

    let mut jpeg_data = Vec::new();
    let mut encoder = image::codecs::jpeg::JpegEncoder::new_with_quality(&mut jpeg_data, 92);
    encoder
        .encode_image(&rgb_img)
        .map_err(|e| format!("Failed to encode JPEG: {}", e))?;

    info!(
        jpeg_size = jpeg_data.len(),
        original_size = rgb_data.len(),
        compression_ratio = format!("{:.1}x", rgb_data.len() as f32 / jpeg_data.len() as f32),
        "Encoded texture as JPEG"
    );

    Ok(jpeg_data)
}

/// Build a GLB (binary glTF) file with texture
fn build_glb_file(
    vertices: &[f32],
    uvs: &[f32],
    indices: &[u32],
    texture_data: &[u8],
    output_path: &PathBuf,
) -> Result<(), String> {
    // Calculate buffer sizes
    let vertices_bytes: Vec<u8> = vertices.iter().flat_map(|f| f.to_le_bytes()).collect();
    let uvs_bytes: Vec<u8> = uvs.iter().flat_map(|f| f.to_le_bytes()).collect();
    let indices_bytes: Vec<u8> = indices.iter().flat_map(|i| i.to_le_bytes()).collect();

    // Buffer layout: vertices | uvs | indices | texture
    let vertex_offset = 0usize;
    let vertex_len = vertices_bytes.len();
    let uv_offset = vertex_len;
    let uv_len = uvs_bytes.len();
    let index_offset = uv_offset + uv_len;
    let index_len = indices_bytes.len();
    let texture_offset = index_offset + index_len;
    let texture_len = texture_data.len();
    let total_buffer_len = texture_offset + texture_len;

    // Pad to 4-byte alignment
    let padding = (4 - (total_buffer_len % 4)) % 4;
    let padded_buffer_len = total_buffer_len + padding;

    // Calculate min/max for vertices
    let mut min_pos = [f32::MAX; 3];
    let mut max_pos = [f32::MIN; 3];
    for chunk in vertices.chunks(3) {
        min_pos[0] = min_pos[0].min(chunk[0]);
        min_pos[1] = min_pos[1].min(chunk[1]);
        min_pos[2] = min_pos[2].min(chunk[2]);
        max_pos[0] = max_pos[0].max(chunk[0]);
        max_pos[1] = max_pos[1].max(chunk[1]);
        max_pos[2] = max_pos[2].max(chunk[2]);
    }

    // Build glTF JSON with texture
    let gltf_json = serde_json::json!({
        "asset": {
            "generator": "COSMIC Camera",
            "version": "2.0"
        },
        "scene": 0,
        "scenes": [{
            "nodes": [0]
        }],
        "nodes": [{
            "mesh": 0
        }],
        "meshes": [{
            "primitives": [{
                "attributes": {
                    "POSITION": 0,
                    "TEXCOORD_0": 1
                },
                "indices": 2,
                "material": 0,
                "mode": 4
            }]
        }],
        "materials": [{
            "pbrMetallicRoughness": {
                "baseColorTexture": {
                    "index": 0
                },
                "metallicFactor": 0.0,
                "roughnessFactor": 1.0
            },
            "doubleSided": true
        }],
        "textures": [{
            "sampler": 0,
            "source": 0
        }],
        "samplers": [{
            "magFilter": 9729,  // LINEAR
            "minFilter": 9987,  // LINEAR_MIPMAP_LINEAR
            "wrapS": 33071,     // CLAMP_TO_EDGE
            "wrapT": 33071      // CLAMP_TO_EDGE
        }],
        "images": [{
            "bufferView": 3,
            "mimeType": "image/jpeg"
        }],
        "accessors": [
            {
                "bufferView": 0,
                "byteOffset": 0,
                "componentType": 5126,  // FLOAT
                "count": vertices.len() / 3,
                "type": "VEC3",
                "min": min_pos,
                "max": max_pos
            },
            {
                "bufferView": 1,
                "byteOffset": 0,
                "componentType": 5126,  // FLOAT
                "count": uvs.len() / 2,
                "type": "VEC2"
            },
            {
                "bufferView": 2,
                "byteOffset": 0,
                "componentType": 5125,  // UNSIGNED_INT
                "count": indices.len(),
                "type": "SCALAR"
            }
        ],
        "bufferViews": [
            {
                "buffer": 0,
                "byteOffset": vertex_offset,
                "byteLength": vertex_len,
                "byteStride": 12,
                "target": 34962  // ARRAY_BUFFER
            },
            {
                "buffer": 0,
                "byteOffset": uv_offset,
                "byteLength": uv_len,
                "byteStride": 8,
                "target": 34962  // ARRAY_BUFFER
            },
            {
                "buffer": 0,
                "byteOffset": index_offset,
                "byteLength": index_len,
                "target": 34963  // ELEMENT_ARRAY_BUFFER
            },
            {
                "buffer": 0,
                "byteOffset": texture_offset,
                "byteLength": texture_len
                // No target for images
            }
        ],
        "buffers": [{
            "byteLength": padded_buffer_len
        }]
    });

    // Serialize JSON
    let json_string = serde_json::to_string(&gltf_json)
        .map_err(|e| format!("Failed to serialize glTF: {}", e))?;
    let json_bytes = json_string.as_bytes();

    // Pad JSON to 4-byte alignment
    let json_padding = (4 - (json_bytes.len() % 4)) % 4;
    let padded_json_len = json_bytes.len() + json_padding;

    // Build GLB file
    let total_length = 12 + 8 + padded_json_len + 8 + padded_buffer_len;

    let mut glb_data: Vec<u8> = Vec::with_capacity(total_length);

    // GLB Header
    glb_data.extend_from_slice(b"glTF"); // Magic
    glb_data.extend_from_slice(&2u32.to_le_bytes()); // Version
    glb_data.extend_from_slice(&(total_length as u32).to_le_bytes()); // Length

    // JSON chunk
    glb_data.extend_from_slice(&(padded_json_len as u32).to_le_bytes()); // Chunk length
    glb_data.extend_from_slice(&0x4E4F534Au32.to_le_bytes()); // Chunk type "JSON"
    glb_data.extend_from_slice(json_bytes);
    glb_data.extend(std::iter::repeat_n(0x20u8, json_padding)); // Space padding

    // Binary chunk
    glb_data.extend_from_slice(&(padded_buffer_len as u32).to_le_bytes()); // Chunk length
    glb_data.extend_from_slice(&0x004E4942u32.to_le_bytes()); // Chunk type "BIN\0"
    glb_data.extend_from_slice(&vertices_bytes);
    glb_data.extend_from_slice(&uvs_bytes);
    glb_data.extend_from_slice(&indices_bytes);
    glb_data.extend_from_slice(texture_data);
    glb_data.extend(std::iter::repeat_n(0u8, padding)); // Null padding

    // Write GLB file
    std::fs::write(output_path, glb_data)
        .map_err(|e| format!("Failed to write GLB file: {}", e))?;

    debug!(path = %output_path.display(), "GLB export complete");

    Ok(())
}
