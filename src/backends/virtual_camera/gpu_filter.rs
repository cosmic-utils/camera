// SPDX-License-Identifier: GPL-3.0-only

//! GPU-accelerated filter processing for virtual camera
//!
//! This module applies filters directly on RGBA textures using compute shaders:
//!
//! 1. Upload RGBA frame as texture
//! 2. Apply filter compute shader in RGBA space
//! 3. Read back filtered RGBA buffer for PipeWire output

use crate::app::FilterType;
use crate::backends::camera::types::{BackendError, BackendResult, CameraFrame, PixelFormat};
use crate::gpu::{self, wgpu};
use std::sync::Arc;
use tracing::{debug, info};

/// Uniform data for the filter shader
#[repr(C)]
#[derive(Debug, Copy, Clone, bytemuck::Pod, bytemuck::Zeroable)]
struct FilterParams {
    width: u32,
    height: u32,
    filter_mode: u32,
    _padding: u32,
}

/// GPU filter renderer for virtual camera output
///
/// Applies filters directly on RGBA textures for maximum simplicity and efficiency.
pub struct GpuFilterRenderer {
    device: Arc<wgpu::Device>,
    queue: Arc<wgpu::Queue>,
    // RGBA input texture
    texture_rgba: Option<wgpu::Texture>,
    // RGBA output buffer (storage buffer for compute shader output)
    output_buffer: Option<wgpu::Buffer>,
    // Staging buffer for CPU readback
    staging_buffer: Option<wgpu::Buffer>,
    // Compute pipeline
    pipeline: wgpu::ComputePipeline,
    // Bind group layout
    bind_group_layout: wgpu::BindGroupLayout,
    // Sampler
    sampler: wgpu::Sampler,
    // Uniform buffer
    uniform_buffer: wgpu::Buffer,
    // Current dimensions
    width: u32,
    height: u32,
}

impl GpuFilterRenderer {
    /// Create a new GPU filter renderer
    pub async fn new() -> BackendResult<Self> {
        info!("Initializing GPU filter renderer (RGBA mode)");

        // Create device with low-priority queue to avoid starving UI rendering
        let (device, queue, gpu_info) =
            gpu::create_low_priority_compute_device("virtual_camera_gpu")
                .await
                .map_err(BackendError::InitializationFailed)?;

        info!(
            name = %gpu_info.adapter_name,
            backend = ?gpu_info.backend,
            low_priority = gpu_info.low_priority_enabled,
            "GPU device created for virtual camera filter"
        );

        // Create shader with shared filter functions
        let shader_source = format!(
            "{}\n{}",
            include_str!("../../shaders/filters.wgsl"),
            include_str!("../../shaders/filter_compute.wgsl")
        );
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("vcam_filter_shader"),
            source: wgpu::ShaderSource::Wgsl(shader_source.into()),
        });

        // Create bind group layout
        let bind_group_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("vcam_filter_bind_group_layout"),
            entries: &[
                // Input RGBA texture
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                // Output RGBA buffer
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Storage { read_only: false },
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                // Uniform buffer
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                // Sampler
                wgpu::BindGroupLayoutEntry {
                    binding: 3,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });

        // Create pipeline layout
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("vcam_filter_pipeline_layout"),
            bind_group_layouts: &[&bind_group_layout],
            push_constant_ranges: &[],
        });

        // Create compute pipeline
        let pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some("vcam_filter_pipeline"),
            layout: Some(&pipeline_layout),
            module: &shader,
            entry_point: "main",
            compilation_options: Default::default(),
            cache: None,
        });

        // Create sampler
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("vcam_filter_sampler"),
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            address_mode_w: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });

        // Create uniform buffer
        let uniform_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("vcam_filter_uniform"),
            size: std::mem::size_of::<FilterParams>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        Ok(Self {
            device,
            queue,
            texture_rgba: None,
            output_buffer: None,
            staging_buffer: None,
            pipeline,
            bind_group_layout,
            sampler,
            uniform_buffer,
            width: 0,
            height: 0,
        })
    }

    /// Ensure GPU resources are allocated for the given dimensions
    fn ensure_resources(&mut self, width: u32, height: u32) {
        if self.width == width && self.height == height {
            return;
        }

        debug!(width, height, "Allocating RGBA filter resources");

        let rgba_size = (width * height * 4) as u64;

        // Create RGBA texture
        self.texture_rgba = Some(self.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("input_rgba_texture"),
            size: wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        }));

        // Output buffer (storage buffer, one u32 per pixel)
        self.output_buffer = Some(self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("output_rgba_buffer"),
            size: rgba_size,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
            mapped_at_creation: false,
        }));

        // Staging buffer for readback
        self.staging_buffer = Some(self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("staging_rgba_buffer"),
            size: rgba_size,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        }));

        self.width = width;
        self.height = height;
    }

    /// Apply filter to frame and return RGBA output
    pub fn apply_filter(
        &mut self,
        frame: &CameraFrame,
        filter: FilterType,
    ) -> BackendResult<Vec<u8>> {
        if frame.format != PixelFormat::RGBA {
            return Err(BackendError::FormatNotSupported(
                "Only RGBA input is supported".into(),
            ));
        }

        // For standard filter, just copy the data (handle stride)
        if filter == FilterType::Standard {
            return self.passthrough_frame(frame);
        }

        self.ensure_resources(frame.width, frame.height);

        let texture_rgba = self.texture_rgba.as_ref().unwrap();

        // Upload RGBA data
        self.queue.write_texture(
            wgpu::ImageCopyTexture {
                texture: texture_rgba,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            &frame.data,
            wgpu::ImageDataLayout {
                offset: 0,
                bytes_per_row: Some(frame.stride),
                rows_per_image: None,
            },
            wgpu::Extent3d {
                width: frame.width,
                height: frame.height,
                depth_or_array_layers: 1,
            },
        );

        // Create texture view
        let view = texture_rgba.create_view(&wgpu::TextureViewDescriptor::default());

        // Update uniform buffer
        let params = FilterParams {
            width: frame.width,
            height: frame.height,
            filter_mode: filter as u32,
            _padding: 0,
        };
        self.queue
            .write_buffer(&self.uniform_buffer, 0, bytemuck::bytes_of(&params));

        let bind_group = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("vcam_filter_bind_group"),
            layout: &self.bind_group_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: self.output_buffer.as_ref().unwrap().as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: self.uniform_buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: wgpu::BindingResource::Sampler(&self.sampler),
                },
            ],
        });

        // Create command encoder
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("vcam_filter_encoder"),
            });

        // Run compute pass
        {
            let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some("apply_filter_pass"),
                timestamp_writes: None,
            });
            pass.set_pipeline(&self.pipeline);
            pass.set_bind_group(0, &bind_group, &[]);
            let workgroups_x = frame.width.div_ceil(16);
            let workgroups_y = frame.height.div_ceil(16);
            pass.dispatch_workgroups(workgroups_x, workgroups_y, 1);
        }

        // Copy to staging buffer
        let buffer_size = (frame.width * frame.height * 4) as u64;
        encoder.copy_buffer_to_buffer(
            self.output_buffer.as_ref().unwrap(),
            0,
            self.staging_buffer.as_ref().unwrap(),
            0,
            buffer_size,
        );

        // Submit commands
        self.queue.submit(std::iter::once(encoder.finish()));

        // Read back results
        let staging = self.staging_buffer.as_ref().unwrap();
        let slice = staging.slice(..);

        let (tx, rx) = std::sync::mpsc::channel();
        slice.map_async(wgpu::MapMode::Read, move |result| {
            let _ = tx.send(result);
        });

        let _ = self.device.poll(wgpu::Maintain::Wait);

        rx.recv()
            .map_err(|e| BackendError::Other(format!("Failed to map buffer: {}", e)))?
            .map_err(|e| BackendError::Other(format!("Buffer map error: {:?}", e)))?;

        let data = slice.get_mapped_range();
        let output = data.to_vec();

        drop(data);
        staging.unmap();

        Ok(output)
    }

    /// Pass through frame without filtering (handle stride)
    fn passthrough_frame(&self, frame: &CameraFrame) -> BackendResult<Vec<u8>> {
        let width = frame.width as usize;
        let height = frame.height as usize;
        let stride = frame.stride as usize;
        let row_bytes = width * 4;

        // If stride matches, return data directly
        if stride == row_bytes {
            return Ok(frame.data.to_vec());
        }

        // Handle stride padding
        let mut output = vec![0u8; row_bytes * height];
        for y in 0..height {
            let src_start = y * stride;
            let dst_start = y * row_bytes;
            output[dst_start..dst_start + row_bytes]
                .copy_from_slice(&frame.data[src_start..src_start + row_bytes]);
        }

        Ok(output)
    }
}
