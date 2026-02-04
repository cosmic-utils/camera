// SPDX-License-Identifier: GPL-3.0-only
//! GPU-accelerated format conversion pipelines
//!
//! This module provides specialized compute shader pipelines for converting various
//! video formats to RGBA. Each format has its own optimized shader without branching.
//!
//! **Supported formats:**
//! - NV12/NV21: Semi-planar 4:2:0
//! - I420: Planar 4:2:0
//! - YUYV/UYVY/YVYU/VYUY: Packed 4:2:2
//! - Gray8: 8-bit grayscale
//! - RGBA: Passthrough (no conversion needed)

use crate::backends::camera::types::PixelFormat;
use crate::gpu::{self, wgpu};
use std::collections::HashMap;
use std::sync::Arc;
use tracing::{debug, info, warn};

/// Simple conversion parameters (width and height only)
#[repr(C)]
#[derive(Debug, Copy, Clone, bytemuck::Pod, bytemuck::Zeroable)]
struct ConvertParams {
    width: u32,
    height: u32,
    _pad0: u32,
    _pad1: u32,
}

/// Input frame data for conversion
pub struct GpuFrameInput<'a> {
    pub width: u32,
    pub height: u32,
    pub format: PixelFormat,
    /// Y plane data (or packed data for YUYV variants)
    pub y_data: &'a [u8],
    pub y_stride: u32,
    /// UV plane data (NV12: interleaved UV, I420: U plane)
    pub uv_data: Option<&'a [u8]>,
    pub uv_stride: u32,
    /// V plane data (I420 only)
    pub v_data: Option<&'a [u8]>,
    pub v_stride: u32,
}

/// Format-specific pipeline resources
struct FormatPipeline {
    pipeline: wgpu::ComputePipeline,
    bind_group_layout: wgpu::BindGroupLayout,
}

/// GPU pipeline for format conversion with specialized shaders
pub struct GpuConvertPipeline {
    device: Arc<wgpu::Device>,
    queue: Arc<wgpu::Queue>,
    /// Format-specific pipelines (lazily created)
    pipelines: HashMap<PixelFormat, FormatPipeline>,
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

impl GpuConvertPipeline {
    /// Create a new conversion pipeline
    pub async fn new() -> Result<Self, String> {
        info!("Initializing format conversion pipelines");

        let (device, queue, gpu_info) =
            gpu::create_low_priority_compute_device("yuv_convert_pipeline").await?;

        info!(
            adapter_name = %gpu_info.adapter_name,
            adapter_backend = ?gpu_info.backend,
            low_priority = gpu_info.low_priority_enabled,
            "GPU device created for format conversion"
        );

        // Create uniform buffer (shared across all pipelines)
        let uniform_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("convert_uniform_buffer"),
            size: std::mem::size_of::<ConvertParams>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        Ok(Self {
            device,
            queue,
            pipelines: HashMap::new(),
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

    /// Get or create the pipeline for a specific format
    fn get_or_create_pipeline(&mut self, format: PixelFormat) -> &FormatPipeline {
        if !self.pipelines.contains_key(&format) {
            let pipeline = self.create_pipeline_for_format(format);
            self.pipelines.insert(format, pipeline);
        }
        self.pipelines.get(&format).unwrap()
    }

    /// Create a format-specific pipeline
    fn create_pipeline_for_format(&self, format: PixelFormat) -> FormatPipeline {
        debug!(?format, "Creating specialized pipeline");

        match format {
            PixelFormat::NV12 => self.create_nv12_pipeline(),
            PixelFormat::NV21 => self.create_nv21_pipeline(),
            PixelFormat::I420 => self.create_i420_pipeline(),
            PixelFormat::YUYV => {
                self.create_packed_pipeline(include_str!("convert_yuyv.wgsl"), "yuyv")
            }
            PixelFormat::UYVY => {
                self.create_packed_pipeline(include_str!("convert_uyvy.wgsl"), "uyvy")
            }
            PixelFormat::YVYU => {
                self.create_packed_pipeline(include_str!("convert_yvyu.wgsl"), "yvyu")
            }
            PixelFormat::VYUY => {
                self.create_packed_pipeline(include_str!("convert_vyuy.wgsl"), "vyuy")
            }
            PixelFormat::Gray8 => self.create_gray8_pipeline(),
            PixelFormat::RGBA | PixelFormat::RGB24 => {
                // RGBA doesn't need conversion, but create a dummy pipeline for API consistency
                self.create_nv12_pipeline() // Fallback, shouldn't be used
            }
        }
    }

    /// Create NV12 pipeline (Y + interleaved UV)
    fn create_nv12_pipeline(&self) -> FormatPipeline {
        let shader = self
            .device
            .create_shader_module(wgpu::ShaderModuleDescriptor {
                label: Some("convert_nv12_shader"),
                source: wgpu::ShaderSource::Wgsl(include_str!("convert_nv12.wgsl").into()),
            });

        let bind_group_layout =
            self.device
                .create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                    label: Some("nv12_bind_group_layout"),
                    entries: &[
                        // tex_y: Y plane (R8)
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
                        // tex_uv: UV plane (RG8)
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
                        // output: RGBA storage texture
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
                        // params: uniform buffer
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

        let pipeline_layout = self
            .device
            .create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("nv12_pipeline_layout"),
                bind_group_layouts: &[&bind_group_layout],
                push_constant_ranges: &[],
            });

        let pipeline = self
            .device
            .create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
                label: Some("nv12_pipeline"),
                layout: Some(&pipeline_layout),
                module: &shader,
                entry_point: "main",
                compilation_options: Default::default(),
                cache: None,
            });

        FormatPipeline {
            pipeline,
            bind_group_layout,
        }
    }

    /// Create NV21 pipeline (Y + interleaved VU)
    fn create_nv21_pipeline(&self) -> FormatPipeline {
        let shader = self
            .device
            .create_shader_module(wgpu::ShaderModuleDescriptor {
                label: Some("convert_nv21_shader"),
                source: wgpu::ShaderSource::Wgsl(include_str!("convert_nv21.wgsl").into()),
            });

        // Same layout as NV12
        let bind_group_layout =
            self.device
                .create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                    label: Some("nv21_bind_group_layout"),
                    entries: &[
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

        let pipeline_layout = self
            .device
            .create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("nv21_pipeline_layout"),
                bind_group_layouts: &[&bind_group_layout],
                push_constant_ranges: &[],
            });

        let pipeline = self
            .device
            .create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
                label: Some("nv21_pipeline"),
                layout: Some(&pipeline_layout),
                module: &shader,
                entry_point: "main",
                compilation_options: Default::default(),
                cache: None,
            });

        FormatPipeline {
            pipeline,
            bind_group_layout,
        }
    }

    /// Create I420 pipeline (Y + U + V separate planes)
    fn create_i420_pipeline(&self) -> FormatPipeline {
        let shader = self
            .device
            .create_shader_module(wgpu::ShaderModuleDescriptor {
                label: Some("convert_i420_shader"),
                source: wgpu::ShaderSource::Wgsl(include_str!("convert_i420.wgsl").into()),
            });

        let bind_group_layout =
            self.device
                .create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                    label: Some("i420_bind_group_layout"),
                    entries: &[
                        // tex_y: Y plane (R8)
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
                        // tex_u: U plane (R8)
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
                        // tex_v: V plane (R8)
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

        let pipeline_layout = self
            .device
            .create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("i420_pipeline_layout"),
                bind_group_layouts: &[&bind_group_layout],
                push_constant_ranges: &[],
            });

        let pipeline = self
            .device
            .create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
                label: Some("i420_pipeline"),
                layout: Some(&pipeline_layout),
                module: &shader,
                entry_point: "main",
                compilation_options: Default::default(),
                cache: None,
            });

        FormatPipeline {
            pipeline,
            bind_group_layout,
        }
    }

    /// Create packed 4:2:2 pipeline (YUYV, UYVY, YVYU, VYUY)
    fn create_packed_pipeline(&self, shader_source: &str, name: &str) -> FormatPipeline {
        let shader = self
            .device
            .create_shader_module(wgpu::ShaderModuleDescriptor {
                label: Some(&format!("convert_{}_shader", name)),
                source: wgpu::ShaderSource::Wgsl(shader_source.into()),
            });

        let bind_group_layout =
            self.device
                .create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                    label: Some(&format!("{}_bind_group_layout", name)),
                    entries: &[
                        // tex_packed: Packed data as RGBA8
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
                        // output: RGBA storage texture
                        wgpu::BindGroupLayoutEntry {
                            binding: 1,
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
                            binding: 2,
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

        let pipeline_layout = self
            .device
            .create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some(&format!("{}_pipeline_layout", name)),
                bind_group_layouts: &[&bind_group_layout],
                push_constant_ranges: &[],
            });

        let pipeline = self
            .device
            .create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
                label: Some(&format!("{}_pipeline", name)),
                layout: Some(&pipeline_layout),
                module: &shader,
                entry_point: "main",
                compilation_options: Default::default(),
                cache: None,
            });

        FormatPipeline {
            pipeline,
            bind_group_layout,
        }
    }

    /// Create Gray8 pipeline
    fn create_gray8_pipeline(&self) -> FormatPipeline {
        let shader = self
            .device
            .create_shader_module(wgpu::ShaderModuleDescriptor {
                label: Some("convert_gray8_shader"),
                source: wgpu::ShaderSource::Wgsl(include_str!("convert_gray8.wgsl").into()),
            });

        let bind_group_layout =
            self.device
                .create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                    label: Some("gray8_bind_group_layout"),
                    entries: &[
                        // tex_gray: Grayscale (R8)
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
                        // output: RGBA storage texture
                        wgpu::BindGroupLayoutEntry {
                            binding: 1,
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
                            binding: 2,
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

        let pipeline_layout = self
            .device
            .create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("gray8_pipeline_layout"),
                bind_group_layouts: &[&bind_group_layout],
                push_constant_ranges: &[],
            });

        let pipeline = self
            .device
            .create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
                label: Some("gray8_pipeline"),
                layout: Some(&pipeline_layout),
                module: &shader,
                entry_point: "main",
                compilation_options: Default::default(),
                cache: None,
            });

        FormatPipeline {
            pipeline,
            bind_group_layout,
        }
    }

    /// Ensure textures are allocated for the given dimensions and format
    fn ensure_resources(&mut self, width: u32, height: u32, format: PixelFormat) {
        if self.cached_width == width
            && self.cached_height == height
            && self.cached_format == format
        {
            return;
        }

        debug!(width, height, ?format, "Allocating conversion resources");

        // Calculate texture dimensions based on format
        let (uv_width, uv_height) = match format {
            PixelFormat::NV12 | PixelFormat::NV21 | PixelFormat::I420 => (width / 2, height / 2),
            PixelFormat::YUYV | PixelFormat::UYVY | PixelFormat::YVYU | PixelFormat::VYUY => {
                (width / 2, height)
            }
            PixelFormat::Gray8 | PixelFormat::RGBA | PixelFormat::RGB24 => (1, 1),
        };

        // Y plane texture format and dimensions
        let (y_format, y_width) = match format {
            PixelFormat::YUYV | PixelFormat::UYVY | PixelFormat::YVYU | PixelFormat::VYUY => {
                (wgpu::TextureFormat::Rgba8Unorm, width / 2)
            }
            PixelFormat::RGBA | PixelFormat::RGB24 => (wgpu::TextureFormat::Rgba8Unorm, width),
            _ => (wgpu::TextureFormat::R8Unorm, width),
        };

        // UV plane texture format
        let uv_format = match format {
            PixelFormat::NV12 | PixelFormat::NV21 => wgpu::TextureFormat::Rg8Unorm,
            _ => wgpu::TextureFormat::R8Unorm,
        };

        // Create Y texture
        self.tex_y = Some(self.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("convert_tex_y"),
            size: wgpu::Extent3d {
                width: y_width,
                height,
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
            label: Some("convert_tex_uv"),
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
            label: Some("convert_tex_v"),
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
            label: Some("convert_output_rgba"),
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

    /// Convert frame to RGBA using format-specific shader
    pub fn convert(&mut self, input: &GpuFrameInput) -> Result<&wgpu::Texture, String> {
        let start = std::time::Instant::now();

        // RGBA doesn't need conversion
        if input.format == PixelFormat::RGBA {
            return Err("RGBA format doesn't need conversion".to_string());
        }

        // Ensure resources and get/create pipeline first (may require &mut self)
        self.ensure_resources(input.width, input.height, input.format);
        let _ = self.get_or_create_pipeline(input.format);

        // Now work with immutable references
        let tex_y = self.tex_y.as_ref().ok_or("Y texture not allocated")?;
        let tex_uv = self.tex_uv.as_ref().ok_or("UV texture not allocated")?;
        let tex_v = self.tex_v.as_ref().ok_or("V texture not allocated")?;
        let output_view = self
            .output_view
            .as_ref()
            .ok_or("Output view not allocated")?;

        // Upload textures based on format
        self.upload_textures(input, tex_y, tex_uv, tex_v)?;

        // Update uniform buffer
        let params = ConvertParams {
            width: input.width,
            height: input.height,
            _pad0: 0,
            _pad1: 0,
        };
        self.queue
            .write_buffer(&self.uniform_buffer, 0, bytemuck::bytes_of(&params));

        // Get the pipeline (it's guaranteed to exist now)
        let format_pipeline = self
            .pipelines
            .get(&input.format)
            .ok_or("Pipeline not found after creation")?;

        // Create texture views
        let y_view = tex_y.create_view(&wgpu::TextureViewDescriptor::default());
        let uv_view = tex_uv.create_view(&wgpu::TextureViewDescriptor::default());
        let v_view = tex_v.create_view(&wgpu::TextureViewDescriptor::default());

        // Create bind group based on format
        let bind_group = self.create_bind_group(
            input.format,
            &format_pipeline.bind_group_layout,
            &y_view,
            &uv_view,
            &v_view,
            output_view,
        );

        // Dispatch compute shader
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("convert_encoder"),
            });

        {
            let mut compute_pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some("convert_compute_pass"),
                timestamp_writes: None,
            });

            compute_pass.set_pipeline(&format_pipeline.pipeline);
            compute_pass.set_bind_group(0, &bind_group, &[]);

            let workgroups_x = input.width.div_ceil(16);
            let workgroups_y = input.height.div_ceil(16);
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
                "Format conversion"
            );
        }

        self.output_texture
            .as_ref()
            .ok_or("Output texture not allocated".to_string())
    }

    /// Upload textures based on format
    fn upload_textures(
        &self,
        input: &GpuFrameInput,
        tex_y: &wgpu::Texture,
        tex_uv: &wgpu::Texture,
        tex_v: &wgpu::Texture,
    ) -> Result<(), String> {
        match input.format {
            // Packed 4:2:2 formats
            PixelFormat::YUYV | PixelFormat::UYVY | PixelFormat::YVYU | PixelFormat::VYUY => {
                self.queue.write_texture(
                    wgpu::ImageCopyTexture {
                        texture: tex_y,
                        mip_level: 0,
                        origin: wgpu::Origin3d::ZERO,
                        aspect: wgpu::TextureAspect::All,
                    },
                    input.y_data,
                    wgpu::ImageDataLayout {
                        offset: 0,
                        bytes_per_row: Some(input.y_stride),
                        rows_per_image: Some(input.height),
                    },
                    wgpu::Extent3d {
                        width: input.width / 2,
                        height: input.height,
                        depth_or_array_layers: 1,
                    },
                );
            }

            // NV12/NV21: Y plane + UV plane
            PixelFormat::NV12 | PixelFormat::NV21 => {
                // Y plane
                self.queue.write_texture(
                    wgpu::ImageCopyTexture {
                        texture: tex_y,
                        mip_level: 0,
                        origin: wgpu::Origin3d::ZERO,
                        aspect: wgpu::TextureAspect::All,
                    },
                    input.y_data,
                    wgpu::ImageDataLayout {
                        offset: 0,
                        bytes_per_row: Some(input.y_stride),
                        rows_per_image: Some(input.height),
                    },
                    wgpu::Extent3d {
                        width: input.width,
                        height: input.height,
                        depth_or_array_layers: 1,
                    },
                );

                // UV plane
                if let Some(uv_data) = input.uv_data {
                    let uv_height = input.height / 2;
                    self.queue.write_texture(
                        wgpu::ImageCopyTexture {
                            texture: tex_uv,
                            mip_level: 0,
                            origin: wgpu::Origin3d::ZERO,
                            aspect: wgpu::TextureAspect::All,
                        },
                        uv_data,
                        wgpu::ImageDataLayout {
                            offset: 0,
                            bytes_per_row: Some(input.uv_stride),
                            rows_per_image: Some(uv_height),
                        },
                        wgpu::Extent3d {
                            width: input.width / 2,
                            height: uv_height,
                            depth_or_array_layers: 1,
                        },
                    );
                }
            }

            // I420: Y + U + V planes
            PixelFormat::I420 => {
                // Y plane
                self.queue.write_texture(
                    wgpu::ImageCopyTexture {
                        texture: tex_y,
                        mip_level: 0,
                        origin: wgpu::Origin3d::ZERO,
                        aspect: wgpu::TextureAspect::All,
                    },
                    input.y_data,
                    wgpu::ImageDataLayout {
                        offset: 0,
                        bytes_per_row: Some(input.y_stride),
                        rows_per_image: Some(input.height),
                    },
                    wgpu::Extent3d {
                        width: input.width,
                        height: input.height,
                        depth_or_array_layers: 1,
                    },
                );

                // U plane
                if let Some(uv_data) = input.uv_data {
                    let uv_height = input.height / 2;
                    self.queue.write_texture(
                        wgpu::ImageCopyTexture {
                            texture: tex_uv,
                            mip_level: 0,
                            origin: wgpu::Origin3d::ZERO,
                            aspect: wgpu::TextureAspect::All,
                        },
                        uv_data,
                        wgpu::ImageDataLayout {
                            offset: 0,
                            bytes_per_row: Some(input.uv_stride),
                            rows_per_image: Some(uv_height),
                        },
                        wgpu::Extent3d {
                            width: input.width / 2,
                            height: uv_height,
                            depth_or_array_layers: 1,
                        },
                    );
                }

                // V plane
                if let Some(v_data) = input.v_data {
                    let v_height = input.height / 2;
                    self.queue.write_texture(
                        wgpu::ImageCopyTexture {
                            texture: tex_v,
                            mip_level: 0,
                            origin: wgpu::Origin3d::ZERO,
                            aspect: wgpu::TextureAspect::All,
                        },
                        v_data,
                        wgpu::ImageDataLayout {
                            offset: 0,
                            bytes_per_row: Some(input.v_stride),
                            rows_per_image: Some(v_height),
                        },
                        wgpu::Extent3d {
                            width: input.width / 2,
                            height: v_height,
                            depth_or_array_layers: 1,
                        },
                    );
                }
            }

            // Gray8: single channel
            PixelFormat::Gray8 => {
                self.queue.write_texture(
                    wgpu::ImageCopyTexture {
                        texture: tex_y,
                        mip_level: 0,
                        origin: wgpu::Origin3d::ZERO,
                        aspect: wgpu::TextureAspect::All,
                    },
                    input.y_data,
                    wgpu::ImageDataLayout {
                        offset: 0,
                        bytes_per_row: Some(input.y_stride),
                        rows_per_image: Some(input.height),
                    },
                    wgpu::Extent3d {
                        width: input.width,
                        height: input.height,
                        depth_or_array_layers: 1,
                    },
                );
            }

            _ => {
                return Err(format!("Unsupported format: {:?}", input.format));
            }
        }

        Ok(())
    }

    /// Create bind group for format-specific pipeline
    fn create_bind_group(
        &self,
        format: PixelFormat,
        layout: &wgpu::BindGroupLayout,
        y_view: &wgpu::TextureView,
        uv_view: &wgpu::TextureView,
        v_view: &wgpu::TextureView,
        output_view: &wgpu::TextureView,
    ) -> wgpu::BindGroup {
        match format {
            // NV12/NV21: tex_y, tex_uv, output, params
            PixelFormat::NV12 | PixelFormat::NV21 => {
                self.device.create_bind_group(&wgpu::BindGroupDescriptor {
                    label: Some("nv12_bind_group"),
                    layout,
                    entries: &[
                        wgpu::BindGroupEntry {
                            binding: 0,
                            resource: wgpu::BindingResource::TextureView(y_view),
                        },
                        wgpu::BindGroupEntry {
                            binding: 1,
                            resource: wgpu::BindingResource::TextureView(uv_view),
                        },
                        wgpu::BindGroupEntry {
                            binding: 2,
                            resource: wgpu::BindingResource::TextureView(output_view),
                        },
                        wgpu::BindGroupEntry {
                            binding: 3,
                            resource: self.uniform_buffer.as_entire_binding(),
                        },
                    ],
                })
            }

            // I420: tex_y, tex_u, tex_v, output, params
            PixelFormat::I420 => self.device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("i420_bind_group"),
                layout,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: wgpu::BindingResource::TextureView(y_view),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: wgpu::BindingResource::TextureView(uv_view),
                    },
                    wgpu::BindGroupEntry {
                        binding: 2,
                        resource: wgpu::BindingResource::TextureView(v_view),
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
            }),

            // Packed formats and Gray8: tex_packed/tex_gray, output, params
            PixelFormat::YUYV
            | PixelFormat::UYVY
            | PixelFormat::YVYU
            | PixelFormat::VYUY
            | PixelFormat::Gray8 => self.device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("packed_bind_group"),
                layout,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: wgpu::BindingResource::TextureView(y_view),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: wgpu::BindingResource::TextureView(output_view),
                    },
                    wgpu::BindGroupEntry {
                        binding: 2,
                        resource: self.uniform_buffer.as_entire_binding(),
                    },
                ],
            }),

            _ => {
                // Fallback
                self.device.create_bind_group(&wgpu::BindGroupDescriptor {
                    label: Some("fallback_bind_group"),
                    layout,
                    entries: &[
                        wgpu::BindGroupEntry {
                            binding: 0,
                            resource: wgpu::BindingResource::TextureView(y_view),
                        },
                        wgpu::BindGroupEntry {
                            binding: 1,
                            resource: wgpu::BindingResource::TextureView(uv_view),
                        },
                        wgpu::BindGroupEntry {
                            binding: 2,
                            resource: wgpu::BindingResource::TextureView(output_view),
                        },
                        wgpu::BindGroupEntry {
                            binding: 3,
                            resource: self.uniform_buffer.as_entire_binding(),
                        },
                    ],
                })
            }
        }
    }

    /// Read back the converted RGBA data to CPU memory
    pub async fn read_rgba_to_cpu(&self, width: u32, height: u32) -> Result<Vec<u8>, String> {
        let output = self
            .output_texture
            .as_ref()
            .ok_or("Output texture not allocated")?;

        let padded_bytes_per_row = (width * 4 + 255) & !255;

        let staging_buffer = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("convert_staging"),
            size: (padded_bytes_per_row * height) as u64,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });

        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("readback_encoder"),
            });

        encoder.copy_texture_to_buffer(
            wgpu::ImageCopyTexture {
                texture: output,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::ImageCopyBuffer {
                buffer: &staging_buffer,
                layout: wgpu::ImageDataLayout {
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

        let buffer_slice = staging_buffer.slice(..);
        let (sender, receiver) = futures::channel::oneshot::channel();
        buffer_slice.map_async(wgpu::MapMode::Read, move |result| {
            let _ = sender.send(result);
        });

        let _ = self.device.poll(wgpu::Maintain::Wait);

        receiver
            .await
            .map_err(|_| "Failed to receive buffer mapping result")?
            .map_err(|e| format!("Failed to map buffer: {:?}", e))?;

        let data = buffer_slice.get_mapped_range();
        let mut output_data = Vec::with_capacity((width * height * 4) as usize);

        if padded_bytes_per_row == width * 4 {
            output_data.extend_from_slice(&data[..(width * height * 4) as usize]);
        } else {
            for row in 0..height {
                let start = (row * padded_bytes_per_row) as usize;
                let end = start + (width * 4) as usize;
                output_data.extend_from_slice(&data[start..end]);
            }
        }

        drop(data);
        staging_buffer.unmap();

        Ok(output_data)
    }

    pub fn output_texture(&self) -> Option<&wgpu::Texture> {
        self.output_texture.as_ref()
    }

    pub fn device(&self) -> &Arc<wgpu::Device> {
        &self.device
    }

    pub fn queue(&self) -> &Arc<wgpu::Queue> {
        &self.queue
    }
}

/// Cached global pipeline instance
static GPU_CONVERT_PIPELINE: std::sync::OnceLock<tokio::sync::Mutex<Option<GpuConvertPipeline>>> =
    std::sync::OnceLock::new();

/// Get or create the shared pipeline instance
pub async fn get_gpu_convert_pipeline()
-> Result<tokio::sync::MutexGuard<'static, Option<GpuConvertPipeline>>, String> {
    let lock = GPU_CONVERT_PIPELINE.get_or_init(|| tokio::sync::Mutex::new(None));
    let mut guard = lock.lock().await;

    if guard.is_none() {
        match GpuConvertPipeline::new().await {
            Ok(pipeline) => {
                *guard = Some(pipeline);
            }
            Err(e) => {
                warn!("Failed to initialize convert pipeline: {}", e);
                return Err(e);
            }
        }
    }

    Ok(guard)
}
