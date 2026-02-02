// SPDX-License-Identifier: GPL-3.0-only
//! GPU-accelerated YUV to RGBA conversion pipeline
//!
//! This module provides a compute shader pipeline for converting YUV video frames
//! to RGBA format on the GPU. Supported formats:
//! - NV12: Semi-planar 4:2:0 (common MJPEG decoder output)
//! - I420: Planar 4:2:0 (common software decoder output)
//! - YUYV: Packed 4:2:2 (common raw webcam format)
//!
//! The converted RGBA texture can then be used by all downstream consumers
//! (preview, filters, histogram, photo capture) without any code changes.

use crate::backends::camera::types::PixelFormat;
use crate::gpu::{self, wgpu};
use std::sync::Arc;
use tracing::{debug, info, warn};

/// Conversion parameters uniform (must match shader struct)
#[repr(C)]
#[derive(Debug, Copy, Clone, bytemuck::Pod, bytemuck::Zeroable)]
struct ConvertParams {
    width: u32,
    height: u32,
    format: u32,
    y_stride: u32,
    uv_stride: u32,
    v_stride: u32,
    _pad: [u32; 2],
}

/// Input frame data for YUV conversion
pub struct YuvFrameInput<'a> {
    pub width: u32,
    pub height: u32,
    pub format: PixelFormat,
    /// Y plane data (or packed YUYV data)
    pub y_data: &'a [u8],
    pub y_stride: u32,
    /// UV plane data (NV12: interleaved UV, I420: U plane)
    pub uv_data: Option<&'a [u8]>,
    pub uv_stride: u32,
    /// V plane data (I420 only)
    pub v_data: Option<&'a [u8]>,
    pub v_stride: u32,
}

/// GPU pipeline for YUV→RGBA conversion
pub struct YuvConvertPipeline {
    device: Arc<wgpu::Device>,
    queue: Arc<wgpu::Queue>,
    pipeline: wgpu::ComputePipeline,
    bind_group_layout: wgpu::BindGroupLayout,
    uniform_buffer: wgpu::Buffer,
    // Cached resources for current dimensions/format
    cached_width: u32,
    cached_height: u32,
    cached_format: PixelFormat,
    tex_y: Option<wgpu::Texture>,
    tex_uv: Option<wgpu::Texture>,
    tex_v: Option<wgpu::Texture>,
    output_texture: Option<wgpu::Texture>,
    output_view: Option<wgpu::TextureView>,
}

impl YuvConvertPipeline {
    /// Create a new YUV conversion pipeline
    ///
    /// Uses low-priority GPU queue to avoid starving UI rendering.
    pub async fn new() -> Result<Self, String> {
        info!("Initializing YUV→RGBA conversion pipeline");

        // Create device with low-priority queue
        let (device, queue, gpu_info) =
            gpu::create_low_priority_compute_device("yuv_convert_pipeline").await?;

        info!(
            adapter_name = %gpu_info.adapter_name,
            adapter_backend = ?gpu_info.backend,
            low_priority = gpu_info.low_priority_enabled,
            "GPU device created for YUV conversion"
        );

        // Create shader module
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("yuv_convert_shader"),
            source: wgpu::ShaderSource::Wgsl(include_str!("yuv_convert.wgsl").into()),
        });

        // Create bind group layout
        // Bindings: tex_y, tex_uv, tex_v, output, params
        let bind_group_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("yuv_convert_bind_group_layout"),
            entries: &[
                // tex_y: Y plane or packed YUYV
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
                // tex_uv: UV plane (NV12) or U plane (I420)
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
                // tex_v: V plane (I420 only)
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: false },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                // output: RGBA storage texture
                wgpu::BindGroupLayoutEntry {
                    binding: 3,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::StorageTexture {
                        access: wgpu::StorageTextureAccess::WriteOnly,
                        format: wgpu::TextureFormat::Rgba8Unorm,
                        view_dimension: wgpu::TextureViewDimension::D2,
                    },
                    count: None,
                },
                // params: uniform buffer
                wgpu::BindGroupLayoutEntry {
                    binding: 4,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
            ],
        });

        // Create pipeline layout
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("yuv_convert_pipeline_layout"),
            bind_group_layouts: &[&bind_group_layout],
            push_constant_ranges: &[],
        });

        // Create compute pipeline
        let pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some("yuv_convert_pipeline"),
            layout: Some(&pipeline_layout),
            module: &shader,
            entry_point: Some("main"),
            compilation_options: Default::default(),
            cache: None,
        });

        // Create uniform buffer
        let uniform_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("yuv_convert_uniform_buffer"),
            size: std::mem::size_of::<ConvertParams>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        Ok(Self {
            device,
            queue,
            pipeline,
            bind_group_layout,
            uniform_buffer,
            cached_width: 0,
            cached_height: 0,
            cached_format: PixelFormat::RGBA,
            tex_y: None,
            tex_uv: None,
            tex_v: None,
            output_texture: None,
            output_view: None,
        })
    }

    /// Ensure textures are allocated for the given dimensions and format
    fn ensure_resources(&mut self, width: u32, height: u32, format: PixelFormat) {
        if self.cached_width == width
            && self.cached_height == height
            && self.cached_format == format
        {
            return;
        }

        debug!(
            width,
            height,
            ?format,
            "Allocating YUV conversion resources"
        );

        // Calculate texture dimensions based on format
        let (y_width, y_height) = (width, height);
        let (uv_width, uv_height) = match format {
            PixelFormat::NV12 | PixelFormat::I420 => (width / 2, height / 2),
            PixelFormat::YUYV => (width / 2, height), // Packed: 2 pixels per texel
            PixelFormat::RGBA => (width, height),
        };

        // Y plane texture format
        let y_format = match format {
            PixelFormat::YUYV => wgpu::TextureFormat::Rgba8Unorm, // Packed YUYV as RGBA
            _ => wgpu::TextureFormat::R8Unorm,                    // Y plane
        };

        // UV plane texture format
        let uv_format = match format {
            PixelFormat::NV12 => wgpu::TextureFormat::Rg8Unorm, // Interleaved UV
            PixelFormat::I420 => wgpu::TextureFormat::R8Unorm,  // U plane only
            _ => wgpu::TextureFormat::R8Unorm,                  // Dummy
        };

        // Create Y texture
        self.tex_y = Some(self.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("yuv_tex_y"),
            size: wgpu::Extent3d {
                width: if format == PixelFormat::YUYV {
                    y_width / 2
                } else {
                    y_width
                },
                height: y_height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: y_format,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        }));

        // Create UV texture
        self.tex_uv = Some(self.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("yuv_tex_uv"),
            size: wgpu::Extent3d {
                width: uv_width.max(1),
                height: uv_height.max(1),
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: uv_format,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        }));

        // Create V texture (I420 only)
        self.tex_v = Some(self.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("yuv_tex_v"),
            size: wgpu::Extent3d {
                width: uv_width.max(1),
                height: uv_height.max(1),
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::R8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        }));

        // Create output RGBA texture
        let output = self.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("yuv_output_rgba"),
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
                | wgpu::TextureUsages::TEXTURE_BINDING
                | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });

        self.output_view = Some(output.create_view(&wgpu::TextureViewDescriptor::default()));
        self.output_texture = Some(output);

        self.cached_width = width;
        self.cached_height = height;
        self.cached_format = format;
    }

    /// Convert YUV frame to RGBA
    ///
    /// Returns the output RGBA texture that can be used for rendering.
    /// The texture remains valid until the next convert call.
    pub fn convert(&mut self, input: &YuvFrameInput) -> Result<&wgpu::Texture, String> {
        let start = std::time::Instant::now();

        self.ensure_resources(input.width, input.height, input.format);

        let tex_y = self.tex_y.as_ref().ok_or("Y texture not allocated")?;
        let tex_uv = self.tex_uv.as_ref().ok_or("UV texture not allocated")?;
        let tex_v = self.tex_v.as_ref().ok_or("V texture not allocated")?;
        let output = self
            .output_texture
            .as_ref()
            .ok_or("Output texture not allocated")?;
        let output_view = self
            .output_view
            .as_ref()
            .ok_or("Output view not allocated")?;

        // Upload Y plane (or packed YUYV)
        let y_tex_width = if input.format == PixelFormat::YUYV {
            input.width / 2
        } else {
            input.width
        };

        self.queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: tex_y,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            input.y_data,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(input.y_stride),
                rows_per_image: Some(input.height),
            },
            wgpu::Extent3d {
                width: y_tex_width,
                height: input.height,
                depth_or_array_layers: 1,
            },
        );

        // Upload UV plane (if present)
        if let Some(uv_data) = input.uv_data {
            let (uv_width, uv_height) = (input.width / 2, input.height / 2);

            self.queue.write_texture(
                wgpu::TexelCopyTextureInfo {
                    texture: tex_uv,
                    mip_level: 0,
                    origin: wgpu::Origin3d::ZERO,
                    aspect: wgpu::TextureAspect::All,
                },
                uv_data,
                wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(input.uv_stride),
                    rows_per_image: Some(uv_height),
                },
                wgpu::Extent3d {
                    width: uv_width,
                    height: uv_height,
                    depth_or_array_layers: 1,
                },
            );
        }

        // Upload V plane (I420 only)
        if let Some(v_data) = input.v_data {
            let (v_width, v_height) = (input.width / 2, input.height / 2);

            self.queue.write_texture(
                wgpu::TexelCopyTextureInfo {
                    texture: tex_v,
                    mip_level: 0,
                    origin: wgpu::Origin3d::ZERO,
                    aspect: wgpu::TextureAspect::All,
                },
                v_data,
                wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(input.v_stride),
                    rows_per_image: Some(v_height),
                },
                wgpu::Extent3d {
                    width: v_width,
                    height: v_height,
                    depth_or_array_layers: 1,
                },
            );
        }

        // Update uniform buffer
        let params = ConvertParams {
            width: input.width,
            height: input.height,
            format: input.format.gpu_format_code(),
            y_stride: input.y_stride,
            uv_stride: input.uv_stride,
            v_stride: input.v_stride,
            _pad: [0; 2],
        };
        self.queue
            .write_buffer(&self.uniform_buffer, 0, bytemuck::bytes_of(&params));

        // Create texture views
        let y_view = tex_y.create_view(&wgpu::TextureViewDescriptor::default());
        let uv_view = tex_uv.create_view(&wgpu::TextureViewDescriptor::default());
        let v_view = tex_v.create_view(&wgpu::TextureViewDescriptor::default());

        // Create bind group
        let bind_group = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("yuv_convert_bind_group"),
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
                    resource: wgpu::BindingResource::TextureView(&v_view),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: wgpu::BindingResource::TextureView(output_view),
                },
                wgpu::BindGroupEntry {
                    binding: 4,
                    resource: self.uniform_buffer.as_entire_binding(),
                },
            ],
        });

        // Create and submit command buffer
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("yuv_convert_encoder"),
            });

        {
            let mut compute_pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some("yuv_convert_compute_pass"),
                timestamp_writes: None,
            });

            compute_pass.set_pipeline(&self.pipeline);
            compute_pass.set_bind_group(0, Some(&bind_group), &[]);

            // Dispatch workgroups (16x16 threads per workgroup)
            let workgroups_x = (input.width + 15) / 16;
            let workgroups_y = (input.height + 15) / 16;
            compute_pass.dispatch_workgroups(workgroups_x, workgroups_y, 1);
        }

        self.queue.submit(std::iter::once(encoder.finish()));

        let elapsed = start.elapsed();
        if elapsed.as_millis() > 2 {
            debug!(
                elapsed_ms = format!("{:.2}", elapsed.as_micros() as f64 / 1000.0),
                width = input.width,
                height = input.height,
                format = ?input.format,
                "YUV→RGBA GPU conversion"
            );
        }

        Ok(output)
    }

    /// Read back the converted RGBA data to CPU memory
    ///
    /// This is an expensive operation and should only be used when CPU access
    /// is required (e.g., photo capture, virtual camera output).
    pub async fn read_rgba_to_cpu(&self, width: u32, height: u32) -> Result<Vec<u8>, String> {
        let output = self
            .output_texture
            .as_ref()
            .ok_or("Output texture not allocated")?;

        let padded_bytes_per_row = (width * 4 + 255) & !255; // Align to 256 bytes

        // Create staging buffer
        let staging_buffer = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("yuv_convert_staging"),
            size: (padded_bytes_per_row * height) as u64,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });

        // Copy texture to buffer
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("yuv_readback_encoder"),
            });

        encoder.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo {
                texture: output,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::TexelCopyBufferInfo {
                buffer: &staging_buffer,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(padded_bytes_per_row),
                    rows_per_image: Some(height),
                },
            },
            wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
        );

        self.queue.submit(std::iter::once(encoder.finish()));

        // Map and read
        let buffer_slice = staging_buffer.slice(..);
        let (sender, receiver) = futures::channel::oneshot::channel();
        buffer_slice.map_async(wgpu::MapMode::Read, move |result| {
            let _ = sender.send(result);
        });

        let _ = self.device.poll(wgpu::PollType::wait_indefinitely());

        receiver
            .await
            .map_err(|_| "Failed to receive buffer mapping result")?
            .map_err(|e| format!("Failed to map buffer: {:?}", e))?;

        // Read data, removing row padding if necessary
        let data = buffer_slice.get_mapped_range();
        let mut output = Vec::with_capacity((width * height * 4) as usize);

        if padded_bytes_per_row == width * 4 {
            output.extend_from_slice(&data[..(width * height * 4) as usize]);
        } else {
            // Remove padding
            for row in 0..height {
                let start = (row * padded_bytes_per_row) as usize;
                let end = start + (width * 4) as usize;
                output.extend_from_slice(&data[start..end]);
            }
        }

        drop(data);
        staging_buffer.unmap();

        Ok(output)
    }

    /// Get a reference to the output texture (for use in rendering pipelines)
    pub fn output_texture(&self) -> Option<&wgpu::Texture> {
        self.output_texture.as_ref()
    }

    /// Get the device (for sharing with other pipelines)
    pub fn device(&self) -> &Arc<wgpu::Device> {
        &self.device
    }

    /// Get the queue (for sharing with other pipelines)
    pub fn queue(&self) -> &Arc<wgpu::Queue> {
        &self.queue
    }
}

/// Cached global YUV convert pipeline instance
static YUV_CONVERT_PIPELINE: std::sync::OnceLock<tokio::sync::Mutex<Option<YuvConvertPipeline>>> =
    std::sync::OnceLock::new();

/// Get or create the shared YUV convert pipeline instance
pub async fn get_yuv_convert_pipeline()
-> Result<tokio::sync::MutexGuard<'static, Option<YuvConvertPipeline>>, String> {
    let lock = YUV_CONVERT_PIPELINE.get_or_init(|| tokio::sync::Mutex::new(None));
    let mut guard = lock.lock().await;

    if guard.is_none() {
        match YuvConvertPipeline::new().await {
            Ok(pipeline) => {
                *guard = Some(pipeline);
            }
            Err(e) => {
                warn!("Failed to initialize YUV convert pipeline: {}", e);
                return Err(e);
            }
        }
    }

    Ok(guard)
}
