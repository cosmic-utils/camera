// SPDX-License-Identifier: MPL-2.0

//! GPU-accelerated filter processing for virtual camera
//!
//! This module renders filtered video frames using wgpu and reads back the result
//! for output to the virtual camera. The pipeline is:
//!
//! 1. Upload NV12 textures to GPU
//! 2. Apply filter shader (NV12 → filtered RGB)
//! 3. Convert RGB back to NV12 via compute shader
//! 4. Read back NV12 buffer for PipeWire output

use crate::app::FilterType;
use crate::backends::camera::types::{BackendError, BackendResult, CameraFrame, PixelFormat};
use cosmic::iced_wgpu::wgpu;
use tracing::{debug, info};

/// Uniform data for the filter shader
#[repr(C)]
#[derive(Debug, Copy, Clone, bytemuck::Pod, bytemuck::Zeroable)]
struct FilterUniform {
    /// Width and height of the frame
    frame_size: [f32; 2],
    /// Filter mode (0-14)
    filter_mode: u32,
    /// Padding for alignment
    _padding: u32,
}

/// GPU filter renderer for virtual camera output
pub struct GpuFilterRenderer {
    device: wgpu::Device,
    queue: wgpu::Queue,
    // NV12 input textures
    texture_y: Option<wgpu::Texture>,
    texture_uv: Option<wgpu::Texture>,
    // RGB render target for filter output
    rgb_target: Option<wgpu::Texture>,
    rgb_target_view: Option<wgpu::TextureView>,
    // NV12 output buffer for readback
    output_buffer_y: Option<wgpu::Buffer>,
    output_buffer_uv: Option<wgpu::Buffer>,
    staging_buffer_y: Option<wgpu::Buffer>,
    staging_buffer_uv: Option<wgpu::Buffer>,
    // Pipelines
    filter_pipeline: wgpu::RenderPipeline,
    rgb_to_nv12_pipeline: wgpu::ComputePipeline,
    // Bind group layouts
    filter_bind_group_layout: wgpu::BindGroupLayout,
    rgb_to_nv12_bind_group_layout: wgpu::BindGroupLayout,
    // Sampler
    sampler: wgpu::Sampler,
    // Current dimensions
    width: u32,
    height: u32,
    // Filter uniform buffer
    uniform_buffer: wgpu::Buffer,
}

impl GpuFilterRenderer {
    /// Create a new GPU filter renderer
    pub async fn new() -> BackendResult<Self> {
        info!("Initializing GPU filter renderer");

        // Request adapter
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
            .ok_or_else(|| {
                BackendError::InitializationFailed(
                    "Failed to find suitable GPU adapter".to_string(),
                )
            })?;

        info!(adapter = ?adapter.get_info(), "Selected GPU adapter");

        // Request device
        let (device, queue) = adapter
            .request_device(
                &wgpu::DeviceDescriptor {
                    label: Some("virtual_camera_gpu"),
                    required_features: wgpu::Features::empty(),
                    required_limits: wgpu::Limits::default(),
                    memory_hints: Default::default(),
                },
                None,
            )
            .await
            .map_err(|e| {
                BackendError::InitializationFailed(format!("Failed to get GPU device: {}", e))
            })?;

        // Create filter shader
        let filter_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("virtual_camera_filter_shader"),
            source: wgpu::ShaderSource::Wgsl(include_str!("filter_shader.wgsl").into()),
        });

        // Create RGB to NV12 compute shader
        let rgb_to_nv12_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("virtual_camera_rgb_to_nv12_shader"),
            source: wgpu::ShaderSource::Wgsl(include_str!("rgb_to_nv12.wgsl").into()),
        });

        // Filter bind group layout
        let filter_bind_group_layout =
            device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("virtual_camera_filter_bind_group_layout"),
                entries: &[
                    // Y texture
                    wgpu::BindGroupLayoutEntry {
                        binding: 0,
                        visibility: wgpu::ShaderStages::FRAGMENT,
                        ty: wgpu::BindingType::Texture {
                            sample_type: wgpu::TextureSampleType::Float { filterable: true },
                            view_dimension: wgpu::TextureViewDimension::D2,
                            multisampled: false,
                        },
                        count: None,
                    },
                    // UV texture
                    wgpu::BindGroupLayoutEntry {
                        binding: 1,
                        visibility: wgpu::ShaderStages::FRAGMENT,
                        ty: wgpu::BindingType::Texture {
                            sample_type: wgpu::TextureSampleType::Float { filterable: true },
                            view_dimension: wgpu::TextureViewDimension::D2,
                            multisampled: false,
                        },
                        count: None,
                    },
                    // Sampler
                    wgpu::BindGroupLayoutEntry {
                        binding: 2,
                        visibility: wgpu::ShaderStages::FRAGMENT,
                        ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                        count: None,
                    },
                    // Uniform buffer
                    wgpu::BindGroupLayoutEntry {
                        binding: 3,
                        visibility: wgpu::ShaderStages::FRAGMENT,
                        ty: wgpu::BindingType::Buffer {
                            ty: wgpu::BufferBindingType::Uniform,
                            has_dynamic_offset: false,
                            min_binding_size: None,
                        },
                        count: None,
                    },
                ],
            });

        // RGB to NV12 bind group layout
        let rgb_to_nv12_bind_group_layout =
            device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("virtual_camera_rgb_to_nv12_bind_group_layout"),
                entries: &[
                    // RGB input texture
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
                    // Y output buffer
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
                    // UV output buffer
                    wgpu::BindGroupLayoutEntry {
                        binding: 2,
                        visibility: wgpu::ShaderStages::COMPUTE,
                        ty: wgpu::BindingType::Buffer {
                            ty: wgpu::BufferBindingType::Storage { read_only: false },
                            has_dynamic_offset: false,
                            min_binding_size: None,
                        },
                        count: None,
                    },
                    // Dimensions uniform
                    wgpu::BindGroupLayoutEntry {
                        binding: 3,
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

        // Create filter pipeline
        let filter_pipeline_layout =
            device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("virtual_camera_filter_pipeline_layout"),
                bind_group_layouts: &[&filter_bind_group_layout],
                push_constant_ranges: &[],
            });

        let filter_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("virtual_camera_filter_pipeline"),
            layout: Some(&filter_pipeline_layout),
            vertex: wgpu::VertexState {
                module: &filter_shader,
                entry_point: "vs_main",
                buffers: &[],
                compilation_options: Default::default(),
            },
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            fragment: Some(wgpu::FragmentState {
                module: &filter_shader,
                entry_point: "fs_main",
                targets: &[Some(wgpu::ColorTargetState {
                    format: wgpu::TextureFormat::Rgba8Unorm,
                    blend: None,
                    write_mask: wgpu::ColorWrites::ALL,
                })],
                compilation_options: Default::default(),
            }),
            multiview: None,
            cache: None,
        });

        // Create RGB to NV12 compute pipeline
        let rgb_to_nv12_pipeline_layout =
            device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("virtual_camera_rgb_to_nv12_pipeline_layout"),
                bind_group_layouts: &[&rgb_to_nv12_bind_group_layout],
                push_constant_ranges: &[],
            });

        let rgb_to_nv12_pipeline =
            device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
                label: Some("virtual_camera_rgb_to_nv12_pipeline"),
                layout: Some(&rgb_to_nv12_pipeline_layout),
                module: &rgb_to_nv12_shader,
                entry_point: "main",
                compilation_options: Default::default(),
                cache: None,
            });

        // Create sampler
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("virtual_camera_sampler"),
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });

        // Create uniform buffer
        let uniform_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("virtual_camera_uniform_buffer"),
            size: std::mem::size_of::<FilterUniform>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        info!("GPU filter renderer initialized successfully");

        Ok(Self {
            device,
            queue,
            texture_y: None,
            texture_uv: None,
            rgb_target: None,
            rgb_target_view: None,
            output_buffer_y: None,
            output_buffer_uv: None,
            staging_buffer_y: None,
            staging_buffer_uv: None,
            filter_pipeline,
            rgb_to_nv12_pipeline,
            filter_bind_group_layout,
            rgb_to_nv12_bind_group_layout,
            sampler,
            width: 0,
            height: 0,
            uniform_buffer,
        })
    }

    /// Ensure textures and buffers are allocated for the given dimensions
    fn ensure_resources(&mut self, width: u32, height: u32) {
        if self.width == width && self.height == height {
            return;
        }

        debug!(width, height, "Allocating GPU resources for virtual camera");

        self.width = width;
        self.height = height;

        // Create Y texture
        self.texture_y = Some(self.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("virtual_camera_y_texture"),
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
        }));

        // Create UV texture
        self.texture_uv = Some(self.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("virtual_camera_uv_texture"),
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
        }));

        // Create RGB render target
        let rgb_target = self.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("virtual_camera_rgb_target"),
            size: wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });
        self.rgb_target_view =
            Some(rgb_target.create_view(&wgpu::TextureViewDescriptor::default()));
        self.rgb_target = Some(rgb_target);

        // Calculate buffer sizes
        let y_size = (width * height) as u64;
        let uv_size = (width * height / 2) as u64;

        // Create output storage buffers
        self.output_buffer_y = Some(self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("virtual_camera_output_y"),
            size: y_size,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
            mapped_at_creation: false,
        }));

        self.output_buffer_uv = Some(self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("virtual_camera_output_uv"),
            size: uv_size,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
            mapped_at_creation: false,
        }));

        // Create staging buffers for readback
        self.staging_buffer_y = Some(self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("virtual_camera_staging_y"),
            size: y_size,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        }));

        self.staging_buffer_uv = Some(self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("virtual_camera_staging_uv"),
            size: uv_size,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        }));
    }

    /// Apply filter to frame and return NV12 output
    pub fn apply_filter(
        &mut self,
        frame: &CameraFrame,
        filter: FilterType,
    ) -> BackendResult<Vec<u8>> {
        if frame.format != PixelFormat::NV12 {
            return Err(BackendError::FormatNotSupported(
                "Only NV12 input is supported".into(),
            ));
        }

        self.ensure_resources(frame.width, frame.height);

        // Upload input textures
        let texture_y = self.texture_y.as_ref().unwrap();
        let texture_uv = self.texture_uv.as_ref().unwrap();

        // Upload Y plane
        self.queue.write_texture(
            wgpu::ImageCopyTexture {
                texture: texture_y,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            &frame.data[..frame.offset_uv],
            wgpu::ImageDataLayout {
                offset: 0,
                bytes_per_row: Some(frame.stride_y),
                rows_per_image: None,
            },
            wgpu::Extent3d {
                width: frame.width,
                height: frame.height,
                depth_or_array_layers: 1,
            },
        );

        // Upload UV plane
        self.queue.write_texture(
            wgpu::ImageCopyTexture {
                texture: texture_uv,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            &frame.data[frame.offset_uv..],
            wgpu::ImageDataLayout {
                offset: 0,
                bytes_per_row: Some(frame.stride_uv),
                rows_per_image: None,
            },
            wgpu::Extent3d {
                width: frame.width / 2,
                height: frame.height / 2,
                depth_or_array_layers: 1,
            },
        );

        // Update uniform buffer
        let filter_mode = match filter {
            FilterType::Standard => 0,
            FilterType::Mono => 1,
            FilterType::Sepia => 2,
            FilterType::Noir => 3,
            FilterType::Vivid => 4,
            FilterType::Cool => 5,
            FilterType::Warm => 6,
            FilterType::Fade => 7,
            FilterType::Duotone => 8,
            FilterType::Vignette => 9,
            FilterType::Negative => 10,
            FilterType::Posterize => 11,
            FilterType::Solarize => 12,
            FilterType::ChromaticAberration => 13,
            FilterType::Pencil => 14,
        };

        let uniform = FilterUniform {
            frame_size: [frame.width as f32, frame.height as f32],
            filter_mode,
            _padding: 0,
        };
        self.queue
            .write_buffer(&self.uniform_buffer, 0, bytemuck::cast_slice(&[uniform]));

        // Create bind groups
        let view_y = texture_y.create_view(&wgpu::TextureViewDescriptor::default());
        let view_uv = texture_uv.create_view(&wgpu::TextureViewDescriptor::default());

        let filter_bind_group = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("virtual_camera_filter_bind_group"),
            layout: &self.filter_bind_group_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&view_y),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(&view_uv),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::Sampler(&self.sampler),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: self.uniform_buffer.as_entire_binding(),
                },
            ],
        });

        // Create RGB to NV12 bind group
        let rgb_to_nv12_bind_group = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("virtual_camera_rgb_to_nv12_bind_group"),
            layout: &self.rgb_to_nv12_bind_group_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(
                        self.rgb_target_view.as_ref().unwrap(),
                    ),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: self.output_buffer_y.as_ref().unwrap().as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: self.output_buffer_uv.as_ref().unwrap().as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: self.uniform_buffer.as_entire_binding(),
                },
            ],
        });

        // Create command encoder
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("virtual_camera_encoder"),
            });

        // Pass 1: Apply filter (NV12 → RGB)
        {
            let mut render_pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("virtual_camera_filter_pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: self.rgb_target_view.as_ref().unwrap(),
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
            });

            render_pass.set_pipeline(&self.filter_pipeline);
            render_pass.set_bind_group(0, &filter_bind_group, &[]);
            render_pass.draw(0..6, 0..1);
        }

        // Pass 2: Convert RGB → NV12
        {
            let mut compute_pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some("virtual_camera_rgb_to_nv12_pass"),
                timestamp_writes: None,
            });

            compute_pass.set_pipeline(&self.rgb_to_nv12_pipeline);
            compute_pass.set_bind_group(0, &rgb_to_nv12_bind_group, &[]);

            // Dispatch workgroups (8x8 threads per workgroup)
            let workgroups_x = (frame.width + 7) / 8;
            let workgroups_y = (frame.height + 7) / 8;
            compute_pass.dispatch_workgroups(workgroups_x, workgroups_y, 1);
        }

        // Copy to staging buffers
        let y_size = (frame.width * frame.height) as u64;
        let uv_size = (frame.width * frame.height / 2) as u64;

        encoder.copy_buffer_to_buffer(
            self.output_buffer_y.as_ref().unwrap(),
            0,
            self.staging_buffer_y.as_ref().unwrap(),
            0,
            y_size,
        );

        encoder.copy_buffer_to_buffer(
            self.output_buffer_uv.as_ref().unwrap(),
            0,
            self.staging_buffer_uv.as_ref().unwrap(),
            0,
            uv_size,
        );

        // Submit commands
        self.queue.submit(std::iter::once(encoder.finish()));

        // Read back results
        let staging_y = self.staging_buffer_y.as_ref().unwrap();
        let staging_uv = self.staging_buffer_uv.as_ref().unwrap();

        let y_slice = staging_y.slice(..);
        let uv_slice = staging_uv.slice(..);

        let (tx_y, rx_y) = std::sync::mpsc::channel();
        let (tx_uv, rx_uv) = std::sync::mpsc::channel();

        y_slice.map_async(wgpu::MapMode::Read, move |result| {
            tx_y.send(result).unwrap();
        });
        uv_slice.map_async(wgpu::MapMode::Read, move |result| {
            tx_uv.send(result).unwrap();
        });

        let _ = self.device.poll(wgpu::MaintainBase::Wait);

        rx_y.recv()
            .map_err(|e| BackendError::Other(format!("Failed to map Y buffer: {}", e)))?
            .map_err(|e| BackendError::Other(format!("Y buffer map error: {:?}", e)))?;
        rx_uv
            .recv()
            .map_err(|e| BackendError::Other(format!("Failed to map UV buffer: {}", e)))?
            .map_err(|e| BackendError::Other(format!("UV buffer map error: {:?}", e)))?;

        // Copy data
        let y_data = y_slice.get_mapped_range();
        let uv_data = uv_slice.get_mapped_range();

        let mut output = Vec::with_capacity(y_size as usize + uv_size as usize);
        output.extend_from_slice(&y_data);
        output.extend_from_slice(&uv_data);

        // Unmap buffers
        drop(y_data);
        drop(uv_data);
        staging_y.unmap();
        staging_uv.unmap();

        Ok(output)
    }
}
