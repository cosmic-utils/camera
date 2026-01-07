// SPDX-License-Identifier: GPL-3.0-only

//! GPU-accelerated YUV to RGBA conversion
//!
//! This module provides compute shader-based conversion of YUV 4:2:2 formats
//! (YUYV and UYVY) to RGBA. The output stays on GPU for efficient display
//! or further processing without CPU round-trips.

use crate::gpu::{self, wgpu};
use crate::shaders::compute_dispatch_size;
use crate::shaders::gpu_processor::CachedDimensions;
use std::sync::Arc;
use tracing::{debug, info};

/// YUV format variants
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum YuvFormat {
    /// YUYV format: Y0, U, Y1, V
    Yuyv = 0,
    /// UYVY format: U, Y0, V, Y1 (used by Kinect)
    Uyvy = 1,
}

/// Uniform buffer for shader parameters
#[repr(C)]
#[derive(Debug, Copy, Clone, bytemuck::Pod, bytemuck::Zeroable)]
struct YuvParams {
    width: u32,
    height: u32,
    format: u32,
    _pad: u32,
}

/// Result of YUV to RGBA conversion
pub struct YuvConvertResult {
    /// Width of output image
    pub width: u32,
    /// Height of output image
    pub height: u32,
    /// RGBA data (4 bytes per pixel) - only populated if read back to CPU
    pub rgba: Option<Vec<u8>>,
    /// GPU texture handle for zero-copy display
    pub texture: Option<Arc<wgpu::Texture>>,
}

/// GPU processor for YUV to RGBA conversion
pub struct YuvConvertProcessor {
    device: Arc<wgpu::Device>,
    queue: Arc<wgpu::Queue>,
    pipeline: wgpu::ComputePipeline,
    bind_group_layout: wgpu::BindGroupLayout,
    // Cached resources for reuse
    cached_dims: CachedDimensions,
    uniform_buffer: Option<wgpu::Buffer>,
    input_buffer: Option<wgpu::Buffer>,
    output_texture: Option<Arc<wgpu::Texture>>,
    staging_buffer: Option<wgpu::Buffer>,
}

impl YuvConvertProcessor {
    /// Create a new YUV converter with GPU acceleration
    pub async fn new() -> Result<Self, String> {
        // Create low-priority GPU device for compute operations
        let (device, queue, info) = gpu::create_low_priority_compute_device("YUV Convert").await?;

        info!(
            adapter_name = %info.adapter_name,
            low_priority = info.low_priority_enabled,
            "GPU device created for YUV conversion"
        );

        // Load shader
        let shader_source = include_str!("yuv_to_rgba.wgsl");
        let shader_module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("YUV to RGBA Shader"),
            source: wgpu::ShaderSource::Wgsl(shader_source.into()),
        });

        // Create bind group layout
        let bind_group_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("YUV Convert Bind Group Layout"),
            entries: &[
                // Params uniform
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                // Input YUV buffer
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Storage { read_only: true },
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                // Output RGBA texture
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
            label: Some("YUV Convert Pipeline Layout"),
            bind_group_layouts: &[&bind_group_layout],
            push_constant_ranges: &[],
        });

        // Create compute pipeline
        let pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some("YUV to RGBA Pipeline"),
            layout: Some(&pipeline_layout),
            module: &shader_module,
            entry_point: Some("main"),
            compilation_options: Default::default(),
            cache: None,
        });

        Ok(Self {
            device,
            queue,
            pipeline,
            bind_group_layout,
            cached_dims: CachedDimensions::default(),
            uniform_buffer: None,
            input_buffer: None,
            output_texture: None,
            staging_buffer: None,
        })
    }

    /// Ensure resources are allocated for the given dimensions
    fn ensure_resources(&mut self, width: u32, height: u32) {
        if !self.cached_dims.needs_update(width, height) {
            return;
        }

        debug!(width, height, "Allocating YUV convert resources");

        // Create uniform buffer
        self.uniform_buffer = Some(self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("YUV Params Buffer"),
            size: std::mem::size_of::<YuvParams>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        }));

        // Input buffer: YUV 4:2:2 = 2 bytes per pixel
        let input_size = (width * height * 2) as u64;
        self.input_buffer = Some(self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("YUV Input Buffer"),
            size: input_size,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        }));

        // Output texture
        let texture = self.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("RGBA Output Texture"),
            size: wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::STORAGE_BINDING
                | wgpu::TextureUsages::COPY_SRC
                | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });
        self.output_texture = Some(Arc::new(texture));

        // Staging buffer for reading back to CPU (optional)
        let output_size = (width * height * 4) as u64;
        // Align to 256 bytes for COPY_DST
        let aligned_size = (output_size + 255) & !255;
        self.staging_buffer = Some(self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("RGBA Staging Buffer"),
            size: aligned_size,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        }));

        self.cached_dims.update(width, height);
    }

    /// Convert YUV data to RGBA on GPU
    ///
    /// # Arguments
    /// * `yuv_data` - Raw YUV 4:2:2 data (2 bytes per pixel)
    /// * `width` - Image width in pixels
    /// * `height` - Image height in pixels
    /// * `format` - YUV format (YUYV or UYVY)
    /// * `read_back` - If true, read RGBA data back to CPU; if false, keep on GPU only
    ///
    /// # Returns
    /// Result containing the converted RGBA data and/or GPU texture handle
    pub async fn convert(
        &mut self,
        yuv_data: &[u8],
        width: u32,
        height: u32,
        format: YuvFormat,
        read_back: bool,
    ) -> Result<YuvConvertResult, String> {
        // Validate input size
        let expected_size = (width * height * 2) as usize;
        if yuv_data.len() < expected_size {
            return Err(format!(
                "YUV data too small: {} bytes, expected {}",
                yuv_data.len(),
                expected_size
            ));
        }

        // Ensure resources are allocated
        self.ensure_resources(width, height);

        let uniform_buffer = self.uniform_buffer.as_ref().unwrap();
        let input_buffer = self.input_buffer.as_ref().unwrap();
        let output_texture = self.output_texture.as_ref().unwrap();

        // Update uniform buffer
        let params = YuvParams {
            width,
            height,
            format: format as u32,
            _pad: 0,
        };
        self.queue
            .write_buffer(uniform_buffer, 0, bytemuck::bytes_of(&params));

        // Upload YUV data
        self.queue
            .write_buffer(input_buffer, 0, &yuv_data[..expected_size]);

        // Create texture view
        let output_view = output_texture.create_view(&wgpu::TextureViewDescriptor::default());

        // Create bind group
        let bind_group = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("YUV Convert Bind Group"),
            layout: &self.bind_group_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: uniform_buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: input_buffer.as_entire_binding(),
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
                label: Some("YUV Convert Encoder"),
            });

        // Dispatch compute shader
        // Each workgroup processes 16x16 macro-pixels (32x16 actual pixels)
        let workgroups_x = compute_dispatch_size(width / 2, 16);
        let workgroups_y = compute_dispatch_size(height, 16);

        {
            let mut compute_pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some("YUV to RGBA Pass"),
                timestamp_writes: None,
            });
            compute_pass.set_pipeline(&self.pipeline);
            compute_pass.set_bind_group(0, &bind_group, &[]);
            compute_pass.dispatch_workgroups(workgroups_x, workgroups_y, 1);
        }

        // Optionally copy to staging buffer for CPU readback
        if read_back {
            let staging_buffer = self.staging_buffer.as_ref().unwrap();
            encoder.copy_texture_to_buffer(
                wgpu::TexelCopyTextureInfo {
                    texture: output_texture,
                    mip_level: 0,
                    origin: wgpu::Origin3d::ZERO,
                    aspect: wgpu::TextureAspect::All,
                },
                wgpu::TexelCopyBufferInfo {
                    buffer: staging_buffer,
                    layout: wgpu::TexelCopyBufferLayout {
                        offset: 0,
                        bytes_per_row: Some(width * 4),
                        rows_per_image: Some(height),
                    },
                },
                wgpu::Extent3d {
                    width,
                    height,
                    depth_or_array_layers: 1,
                },
            );
        }

        // Submit commands
        self.queue.submit(std::iter::once(encoder.finish()));

        // Read back if requested
        let rgba = if read_back {
            let staging_buffer = self.staging_buffer.as_ref().unwrap();
            let rgba_data =
                crate::shaders::gpu_processor::read_buffer_async(&self.device, staging_buffer)
                    .await?;
            Some(rgba_data)
        } else {
            None
        };

        Ok(YuvConvertResult {
            width,
            height,
            rgba,
            texture: Some(Arc::clone(output_texture)),
        })
    }

    /// Get the GPU device for sharing with other GPU operations
    pub fn device(&self) -> &Arc<wgpu::Device> {
        &self.device
    }

    /// Get the GPU queue for sharing with other GPU operations
    pub fn queue(&self) -> &Arc<wgpu::Queue> {
        &self.queue
    }

    /// Get the current output texture (if any)
    pub fn output_texture(&self) -> Option<&Arc<wgpu::Texture>> {
        self.output_texture.as_ref()
    }
}

// Global shared processor instance using the standard macro
crate::gpu_processor_singleton!(YuvConvertProcessor, GPU_YUV_PROCESSOR, get_yuv_processor);

/// Convert YUV to RGBA using GPU acceleration
///
/// This is the main public API for GPU-accelerated YUV conversion.
///
/// # Arguments
/// * `yuv_data` - Raw YUV 4:2:2 data (2 bytes per pixel)
/// * `width` - Image width in pixels
/// * `height` - Image height in pixels
/// * `format` - YUV format (YUYV or UYVY)
///
/// # Returns
/// RGBA data (4 bytes per pixel) as a Vec<u8>
pub async fn convert_yuv_to_rgba_gpu(
    yuv_data: &[u8],
    width: u32,
    height: u32,
    format: YuvFormat,
) -> Result<Vec<u8>, String> {
    let mut guard = get_yuv_processor().await?;
    let processor = guard.as_mut().ok_or("YUV GPU processor not available")?;

    let result = processor
        .convert(yuv_data, width, height, format, true)
        .await?;
    result
        .rgba
        .ok_or_else(|| "No RGBA data returned".to_string())
}

/// Convert YUV to RGBA on GPU without CPU readback
///
/// Returns the GPU texture handle for zero-copy display or further GPU processing.
///
/// # Arguments
/// * `yuv_data` - Raw YUV 4:2:2 data (2 bytes per pixel)
/// * `width` - Image width in pixels
/// * `height` - Image height in pixels
/// * `format` - YUV format (YUYV or UYVY)
///
/// # Returns
/// YuvConvertResult with GPU texture handle
pub async fn convert_yuv_to_rgba_gpu_texture(
    yuv_data: &[u8],
    width: u32,
    height: u32,
    format: YuvFormat,
) -> Result<YuvConvertResult, String> {
    let mut guard = get_yuv_processor().await?;
    let processor = guard.as_mut().ok_or("YUV GPU processor not available")?;

    processor
        .convert(yuv_data, width, height, format, false)
        .await
}
