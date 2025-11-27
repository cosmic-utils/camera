// SPDX-License-Identifier: MPL-2.0

//! GPU-accelerated NV12 to RGB conversion for photo capture
//!
//! This module provides efficient NV12→RGB conversion using the GPU compute pipeline,
//! with an optimized CPU fallback when GPU is unavailable.

use crate::backends::camera::types::CameraFrame;
use cosmic::iced_wgpu::wgpu;
use image::RgbImage;
use std::sync::{Arc, OnceLock};
use tracing::{debug, info};

/// GPU converter instance (lazily initialized)
static GPU_CONVERTER: OnceLock<Option<Arc<GpuConverter>>> = OnceLock::new();

/// Convert NV12 frame to RGB using GPU acceleration (if available) or optimized CPU fallback
///
/// This function automatically selects the best conversion method:
/// 1. Try GPU compute shader (fastest)
/// 2. Fall back to optimized CPU conversion if GPU fails
pub async fn convert_nv12_to_rgb(frame: Arc<CameraFrame>) -> Result<RgbImage, String> {
    info!(
        width = frame.width,
        height = frame.height,
        "Converting NV12 to RGB for photo capture"
    );

    // Try GPU conversion first
    match try_gpu_convert(frame.clone()).await {
        Ok(rgb) => {
            debug!("GPU conversion successful");
            return Ok(rgb);
        }
        Err(e) => {
            debug!(error = %e, "GPU conversion failed, falling back to CPU");
        }
    }

    // Fall back to optimized CPU conversion
    tokio::task::spawn_blocking(move || convert_nv12_to_rgb_cpu(&frame))
        .await
        .map_err(|e| format!("CPU conversion task error: {}", e))?
}

/// Attempt GPU-accelerated conversion
async fn try_gpu_convert(frame: Arc<CameraFrame>) -> Result<RgbImage, String> {
    // Get or initialize GPU converter (done once at first call)
    let converter = GPU_CONVERTER.get_or_init(|| GpuConverter::new().ok().map(Arc::new));

    if let Some(converter) = converter {
        converter.convert(frame).await
    } else {
        Err("GPU converter not initialized".to_string())
    }
}

/// GPU-based NV12 to RGB converter using wgpu compute shader
struct GpuConverter {
    device: wgpu::Device,
    queue: wgpu::Queue,
    compute_pipeline: wgpu::ComputePipeline,
    bind_group_layout: wgpu::BindGroupLayout,
}

impl GpuConverter {
    /// Initialize GPU converter
    fn new() -> Result<Self, String> {
        // Create wgpu instance
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
            backends: wgpu::Backends::all(),
            ..Default::default()
        });

        // Request adapter
        let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::HighPerformance,
            compatible_surface: None,
            force_fallback_adapter: false,
        }))
        .ok_or_else(|| "Failed to find suitable GPU adapter".to_string())?;

        // Request device and queue
        let (device, queue) = pollster::block_on(adapter.request_device(
            &wgpu::DeviceDescriptor {
                label: Some("NV12 Converter Device"),
                required_features: wgpu::Features::empty(),
                required_limits: wgpu::Limits::default(),
                memory_hints: Default::default(),
            },
            None,
        ))
        .map_err(|e| format!("Failed to create device: {}", e))?;

        // Load compute shader
        let shader_source = include_str!("nv12_compute.wgsl");
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("NV12 Compute Shader"),
            source: wgpu::ShaderSource::Wgsl(shader_source.into()),
        });

        // Create bind group layout
        let bind_group_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("NV12 Converter Bind Group Layout"),
            entries: &[
                // Y texture (input)
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: false },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                // UV texture (input)
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: false },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                // Output texture (storage)
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::StorageTexture {
                        access: wgpu::StorageTextureAccess::WriteOnly,
                        format: wgpu::TextureFormat::Rgba8Unorm,
                        view_dimension: wgpu::TextureViewDimension::D2,
                    },
                    count: None,
                },
            ],
        });

        // Create pipeline layout
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("NV12 Converter Pipeline Layout"),
            bind_group_layouts: &[&bind_group_layout],
            push_constant_ranges: &[],
        });

        // Create compute pipeline
        let compute_pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some("NV12 Converter Pipeline"),
            layout: Some(&pipeline_layout),
            module: &shader,
            entry_point: "main",
            compilation_options: Default::default(),
            cache: None,
        });

        Ok(Self {
            device,
            queue,
            compute_pipeline,
            bind_group_layout,
        })
    }

    /// Convert NV12 frame to RGB using GPU compute shader
    async fn convert(&self, frame: Arc<CameraFrame>) -> Result<RgbImage, String> {
        let width = frame.width;
        let height = frame.height;

        // Create Y texture (R8 format)
        let y_texture = self.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("Y Plane Texture"),
            size: wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::R8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });

        // Create UV texture (RG8 format, half resolution)
        let uv_texture = self.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("UV Plane Texture"),
            size: wgpu::Extent3d {
                width: width / 2,
                height: height / 2,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rg8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });

        // Create output texture (RGBA8)
        let output_texture = self.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("RGB Output Texture"),
            size: wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::STORAGE_BINDING | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });

        // Upload Y plane data
        let y_plane_size = frame.stride_y as usize * height as usize;
        let y_data = &frame.data[..frame.offset_uv.min(y_plane_size)];

        self.queue.write_texture(
            wgpu::ImageCopyTexture {
                texture: &y_texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            y_data,
            wgpu::ImageDataLayout {
                offset: 0,
                bytes_per_row: Some(frame.stride_y),
                rows_per_image: None,
            },
            wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
        );

        // Upload UV plane data
        let uv_data = &frame.data[frame.offset_uv..];
        self.queue.write_texture(
            wgpu::ImageCopyTexture {
                texture: &uv_texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            uv_data,
            wgpu::ImageDataLayout {
                offset: 0,
                bytes_per_row: Some(frame.stride_uv),
                rows_per_image: None,
            },
            wgpu::Extent3d {
                width: width / 2,
                height: height / 2,
                depth_or_array_layers: 1,
            },
        );

        // Create texture views
        let y_view = y_texture.create_view(&wgpu::TextureViewDescriptor::default());
        let uv_view = uv_texture.create_view(&wgpu::TextureViewDescriptor::default());
        let output_view = output_texture.create_view(&wgpu::TextureViewDescriptor::default());

        // Create bind group
        let bind_group = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("NV12 Converter Bind Group"),
            layout: &self.bind_group_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&y_view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(&uv_view),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::TextureView(&output_view),
                },
            ],
        });

        // Create command encoder
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("NV12 Converter Encoder"),
            });

        // Dispatch compute shader
        {
            let mut compute_pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some("NV12 Conversion Pass"),
                timestamp_writes: None,
            });
            compute_pass.set_pipeline(&self.compute_pipeline);
            compute_pass.set_bind_group(0, &bind_group, &[]);

            // Dispatch workgroups: ceil(width/8) x ceil(height/8)
            let workgroup_count_x = (width + 7) / 8;
            let workgroup_count_y = (height + 7) / 8;
            compute_pass.dispatch_workgroups(workgroup_count_x, workgroup_count_y, 1);
        }

        // Create staging buffer for readback
        let bytes_per_pixel = 4; // RGBA8
        let unpadded_bytes_per_row = width * bytes_per_pixel;
        let align = wgpu::COPY_BYTES_PER_ROW_ALIGNMENT;
        let padded_bytes_per_row = (unpadded_bytes_per_row + align - 1) / align * align;
        let buffer_size = (padded_bytes_per_row * height) as u64;

        let staging_buffer = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Staging Buffer"),
            size: buffer_size,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });

        // Copy output texture to staging buffer
        encoder.copy_texture_to_buffer(
            wgpu::ImageCopyTexture {
                texture: &output_texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::ImageCopyBuffer {
                buffer: &staging_buffer,
                layout: wgpu::ImageDataLayout {
                    offset: 0,
                    bytes_per_row: Some(padded_bytes_per_row),
                    rows_per_image: None,
                },
            },
            wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
        );

        // Submit commands
        self.queue.submit(Some(encoder.finish()));

        // Map buffer and read back data
        let buffer_slice = staging_buffer.slice(..);
        let (tx, rx) = futures::channel::oneshot::channel();
        buffer_slice.map_async(wgpu::MapMode::Read, move |result| {
            let _ = tx.send(result);
        });

        // Poll device until mapping is complete
        self.device.poll(wgpu::Maintain::Wait);

        rx.await
            .map_err(|e| format!("Buffer mapping channel error: {}", e))?
            .map_err(|e| format!("Buffer mapping failed: {}", e))?;

        // Read RGBA data and convert to RGB
        let data = buffer_slice.get_mapped_range();
        let mut rgb_data = Vec::with_capacity((width * height * 3) as usize);

        for row in 0..height {
            let row_start = (row * padded_bytes_per_row) as usize;
            let row_data = &data[row_start..row_start + (width * 4) as usize];

            // Convert RGBA to RGB (drop alpha channel)
            for pixel in row_data.chunks_exact(4) {
                rgb_data.push(pixel[0]); // R
                rgb_data.push(pixel[1]); // G
                rgb_data.push(pixel[2]); // B
            }
        }

        drop(data);
        staging_buffer.unmap();

        RgbImage::from_raw(width, height, rgb_data)
            .ok_or_else(|| "Failed to create RGB image from GPU buffer".to_string())
    }
}

/// Optimized CPU-based NV12 to RGB conversion
///
/// This uses an optimized algorithm with better cache locality.
/// Used as fallback when GPU is unavailable.
fn convert_nv12_to_rgb_cpu(frame: &CameraFrame) -> Result<RgbImage, String> {
    let width = frame.width as usize;
    let height = frame.height as usize;

    let y_stride = frame.stride_y as usize;
    let uv_stride = frame.stride_uv as usize;
    let uv_offset = frame.offset_uv;

    let y_plane = &frame.data[..uv_offset];
    let uv_plane = &frame.data[uv_offset..];

    // Pre-allocate output buffer
    let mut rgb_data = vec![0u8; width * height * 3];

    // Process two rows at a time for better cache locality
    for y_idx in (0..height).step_by(2) {
        let uv_row = y_idx / 2;

        // Process first Y row
        process_row(
            &y_plane,
            &uv_plane,
            &mut rgb_data,
            y_idx,
            uv_row,
            width,
            y_stride,
            uv_stride,
        );

        // Process second Y row (if exists)
        if y_idx + 1 < height {
            process_row(
                &y_plane,
                &uv_plane,
                &mut rgb_data,
                y_idx + 1,
                uv_row,
                width,
                y_stride,
                uv_stride,
            );
        }
    }

    image::RgbImage::from_raw(width as u32, height as u32, rgb_data)
        .ok_or_else(|| "Failed to create RGB image from buffer".to_string())
}

#[inline]
fn process_row(
    y_plane: &[u8],
    uv_plane: &[u8],
    rgb_data: &mut [u8],
    y_idx: usize,
    uv_row: usize,
    width: usize,
    y_stride: usize,
    uv_stride: usize,
) {
    let y_row_start = y_idx * y_stride;
    let uv_row_start = uv_row * uv_stride;
    let rgb_row_start = y_idx * width * 3;

    // Process pixels in pairs
    for x_idx in (0..width).step_by(2) {
        let y_offset = y_row_start + x_idx;
        let uv_col = (x_idx / 2) * 2;
        let uv_offset = uv_row_start + uv_col;

        // Read UV values once for two pixels
        let u = uv_plane[uv_offset] as i32 - 128;
        let v = uv_plane[uv_offset + 1] as i32 - 128;

        // Pre-compute color contributions
        let r_v = (179 * v) >> 7;
        let g_u = (44 * u) >> 7;
        let g_v = (91 * v) >> 7;
        let b_u = (227 * u) >> 7;

        // Process first pixel
        let y1 = ((y_plane[y_offset] as i32 - 16) * 149) >> 7;
        let rgb_offset = rgb_row_start + x_idx * 3;
        rgb_data[rgb_offset] = (y1 + r_v).clamp(0, 255) as u8;
        rgb_data[rgb_offset + 1] = (y1 - g_u - g_v).clamp(0, 255) as u8;
        rgb_data[rgb_offset + 2] = (y1 + b_u).clamp(0, 255) as u8;

        // Process second pixel
        if x_idx + 1 < width {
            let y2 = ((y_plane[y_offset + 1] as i32 - 16) * 149) >> 7;
            let rgb_offset2 = rgb_row_start + (x_idx + 1) * 3;
            rgb_data[rgb_offset2] = (y2 + r_v).clamp(0, 255) as u8;
            rgb_data[rgb_offset2 + 1] = (y2 - g_u - g_v).clamp(0, 255) as u8;
            rgb_data[rgb_offset2 + 2] = (y2 + b_u).clamp(0, 255) as u8;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backends::camera::types::PixelFormat;

    #[tokio::test]
    async fn test_basic_conversion() {
        let width = 16;
        let height = 16;
        let y_size = width * height;
        let uv_size = (width / 2) * (height / 2) * 2;

        let mut data = vec![0u8; y_size + uv_size];

        // Fill with mid-gray and neutral UV
        for i in 0..y_size {
            data[i] = 128;
        }
        for i in y_size..(y_size + uv_size) {
            data[i] = 128;
        }

        let frame = CameraFrame {
            width: width as u32,
            height: height as u32,
            data: Arc::from(data.as_slice()),
            format: PixelFormat::NV12,
            stride_y: width as u32,
            stride_uv: width as u32,
            offset_uv: y_size,
            captured_at: std::time::Instant::now(),
        };

        let result = convert_nv12_to_rgb(Arc::new(frame)).await;
        assert!(result.is_ok());

        let rgb_image = result.unwrap();
        assert_eq!(rgb_image.width(), width as u32);
        assert_eq!(rgb_image.height(), height as u32);
    }
}
