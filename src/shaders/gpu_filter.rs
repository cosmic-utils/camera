// SPDX-License-Identifier: MPL-2.0
//! GPU-accelerated filter pipeline for images
//!
//! This module provides a unified GPU filter pipeline used by both photo capture
//! and virtual camera for consistent filter application.
//! It uses wgpu with software rendering fallback for systems without GPU support.

use crate::app::FilterType;
use cosmic::iced_wgpu::wgpu;
use tracing::{debug, info, warn};

/// Filter parameters uniform
#[repr(C)]
#[derive(Debug, Copy, Clone, bytemuck::Pod, bytemuck::Zeroable)]
struct FilterParams {
    width: u32,
    height: u32,
    filter_mode: u32,
    _padding: u32,
}

/// GPU filter pipeline for images
pub struct GpuFilterPipeline {
    device: wgpu::Device,
    queue: wgpu::Queue,
    pipeline: wgpu::ComputePipeline,
    bind_group_layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
    uniform_buffer: wgpu::Buffer,
    // Cached resources for current dimensions
    cached_width: u32,
    cached_height: u32,
    input_texture: Option<wgpu::Texture>,
    output_buffer: Option<wgpu::Buffer>,
    staging_buffer: Option<wgpu::Buffer>,
}

impl GpuFilterPipeline {
    /// Create a new GPU filter pipeline
    ///
    /// This will attempt to use hardware GPU acceleration, falling back to
    /// software rendering (llvmpipe/SwiftShader) if no GPU is available.
    pub async fn new() -> Result<Self, String> {
        info!("Initializing GPU filter pipeline");

        // Request adapter - will use software rendering if no hardware GPU
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
            backends: wgpu::Backends::all(),
            ..Default::default()
        });

        let adapter = instance
            .request_adapter(&wgpu::RequestAdapterOptions {
                power_preference: wgpu::PowerPreference::HighPerformance,
                compatible_surface: None,
                force_fallback_adapter: false,
            })
            .await
            .ok_or("Failed to find suitable GPU adapter")?;

        let adapter_info = adapter.get_info();
        info!(
            adapter_name = %adapter_info.name,
            adapter_backend = ?adapter_info.backend,
            "Selected GPU adapter for filter pipeline"
        );

        // Request device
        let (device, queue) = adapter
            .request_device(
                &wgpu::DeviceDescriptor {
                    label: Some("filter_pipeline_gpu"),
                    required_features: wgpu::Features::empty(),
                    required_limits: wgpu::Limits::default(),
                    memory_hints: Default::default(),
                },
                None,
            )
            .await
            .map_err(|e| format!("Failed to get GPU device: {}", e))?;

        // Create shader with shared filter functions
        let shader_source = format!(
            "{}\n{}",
            super::FILTER_FUNCTIONS,
            include_str!("filter_compute.wgsl")
        );
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("filter_compute_shader"),
            source: wgpu::ShaderSource::Wgsl(shader_source.into()),
        });

        // Create bind group layout
        let bind_group_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("filter_bind_group_layout"),
            entries: &[
                // Input texture
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
                // Output storage buffer
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
            label: Some("filter_pipeline_layout"),
            bind_group_layouts: &[&bind_group_layout],
            push_constant_ranges: &[],
        });

        // Create compute pipeline
        let pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some("filter_pipeline"),
            layout: Some(&pipeline_layout),
            module: &shader,
            entry_point: "main",
            compilation_options: Default::default(),
            cache: None,
        });

        // Create sampler
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("filter_sampler"),
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            address_mode_w: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::FilterMode::Nearest,
            ..Default::default()
        });

        // Create uniform buffer
        let uniform_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("filter_uniform_buffer"),
            size: std::mem::size_of::<FilterParams>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        Ok(Self {
            device,
            queue,
            pipeline,
            bind_group_layout,
            sampler,
            uniform_buffer,
            cached_width: 0,
            cached_height: 0,
            input_texture: None,
            output_buffer: None,
            staging_buffer: None,
        })
    }

    /// Ensure resources are allocated for the given dimensions
    fn ensure_resources(&mut self, width: u32, height: u32) {
        if self.cached_width == width && self.cached_height == height {
            return;
        }

        debug!(width, height, "Allocating filter pipeline resources");

        let buffer_size = (width * height * 4) as u64;

        // Create input texture
        self.input_texture = Some(self.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("filter_input_texture"),
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

        // Create output storage buffer
        self.output_buffer = Some(self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("filter_output_buffer"),
            size: buffer_size,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
            mapped_at_creation: false,
        }));

        // Create staging buffer for CPU readback
        self.staging_buffer = Some(self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("filter_staging_buffer"),
            size: buffer_size,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        }));

        self.cached_width = width;
        self.cached_height = height;
    }

    /// Apply a filter to RGBA data
    ///
    /// Takes RGBA pixel data (width * height * 4 bytes) and returns filtered RGBA data.
    /// This runs on the GPU with software rendering fallback.
    pub async fn apply_filter_rgba(
        &mut self,
        rgba_data: &[u8],
        width: u32,
        height: u32,
        filter: FilterType,
    ) -> Result<Vec<u8>, String> {
        if filter == FilterType::Standard {
            // No filter needed, return as-is
            return Ok(rgba_data.to_vec());
        }

        self.ensure_resources(width, height);

        let input_texture = self
            .input_texture
            .as_ref()
            .ok_or("Input texture not allocated")?;
        let output_buffer = self
            .output_buffer
            .as_ref()
            .ok_or("Output buffer not allocated")?;
        let staging_buffer = self
            .staging_buffer
            .as_ref()
            .ok_or("Staging buffer not allocated")?;

        // Upload RGBA data to input texture
        self.queue.write_texture(
            wgpu::ImageCopyTexture {
                texture: input_texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            rgba_data,
            wgpu::ImageDataLayout {
                offset: 0,
                bytes_per_row: Some(width * 4),
                rows_per_image: Some(height),
            },
            wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
        );

        // Update uniform buffer
        let params = FilterParams {
            width,
            height,
            filter_mode: filter as u32,
            _padding: 0,
        };
        self.queue
            .write_buffer(&self.uniform_buffer, 0, bytemuck::bytes_of(&params));

        // Create bind group
        let input_view = input_texture.create_view(&wgpu::TextureViewDescriptor::default());

        let bind_group = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("filter_bind_group"),
            layout: &self.bind_group_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&input_view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: output_buffer.as_entire_binding(),
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

        // Create and submit command buffer
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("filter_encoder"),
            });

        {
            let mut compute_pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some("filter_compute_pass"),
                timestamp_writes: None,
            });

            compute_pass.set_pipeline(&self.pipeline);
            compute_pass.set_bind_group(0, &bind_group, &[]);

            // Dispatch workgroups (16x16 threads per workgroup)
            let workgroups_x = (width + 15) / 16;
            let workgroups_y = (height + 15) / 16;
            compute_pass.dispatch_workgroups(workgroups_x, workgroups_y, 1);
        }

        // Copy output buffer to staging buffer
        let buffer_size = (width * height * 4) as u64;
        encoder.copy_buffer_to_buffer(output_buffer, 0, staging_buffer, 0, buffer_size);

        self.queue.submit(std::iter::once(encoder.finish()));

        // Map staging buffer and read back result
        let buffer_slice = staging_buffer.slice(..);
        let (sender, receiver) = futures::channel::oneshot::channel();
        buffer_slice.map_async(wgpu::MapMode::Read, move |result| {
            let _ = sender.send(result);
        });

        self.device.poll(wgpu::Maintain::Wait);

        receiver
            .await
            .map_err(|_| "Failed to receive buffer mapping result")?
            .map_err(|e| format!("Failed to map buffer: {:?}", e))?;

        // Read RGBA data directly from buffer
        let data = buffer_slice.get_mapped_range();
        let output = data.to_vec();

        drop(data);
        staging_buffer.unmap();

        Ok(output)
    }
}

/// Cached GPU filter pipeline instance
static GPU_FILTER_PIPELINE: std::sync::OnceLock<tokio::sync::Mutex<Option<GpuFilterPipeline>>> =
    std::sync::OnceLock::new();

/// Get or create the shared GPU filter pipeline instance
pub async fn get_gpu_filter_pipeline() -> Result<tokio::sync::MutexGuard<'static, Option<GpuFilterPipeline>>, String> {
    let lock = GPU_FILTER_PIPELINE.get_or_init(|| tokio::sync::Mutex::new(None));
    let mut guard = lock.lock().await;

    if guard.is_none() {
        match GpuFilterPipeline::new().await {
            Ok(pipeline) => {
                *guard = Some(pipeline);
            }
            Err(e) => {
                warn!("Failed to initialize GPU filter pipeline: {}", e);
                return Err(e);
            }
        }
    }

    Ok(guard)
}

/// Apply a filter to RGBA data using the shared GPU pipeline
///
/// This is the main entry point for applying filters. It uses GPU acceleration
/// with software rendering fallback. Takes RGBA input and returns RGBA output.
pub async fn apply_filter_gpu_rgba(
    rgba_data: &[u8],
    width: u32,
    height: u32,
    filter: FilterType,
) -> Result<Vec<u8>, String> {
    let mut guard = get_gpu_filter_pipeline().await?;
    let pipeline = guard
        .as_mut()
        .ok_or("GPU filter pipeline not initialized")?;

    pipeline.apply_filter_rgba(rgba_data, width, height, filter).await
}
