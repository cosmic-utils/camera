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

/// Debayer conversion parameters (80 bytes, std140-compatible)
#[repr(C)]
#[derive(Debug, Copy, Clone, bytemuck::Pod, bytemuck::Zeroable)]
struct DebayerParams {
    width: u32,
    height: u32,
    pattern: u32,        // 0=RGGB, 1=BGGR, 2=GRBG, 3=GBRG
    use_isp_colour: u32, // 1 = apply gains+CCM, 0 = raw output
    colour_gain_r: f32,
    colour_gain_b: f32,
    black_level: f32,
    _pad0: u32,
    ccm_row0: [f32; 4], // xyz used, w=pad
    ccm_row1: [f32; 4], // xyz used, w=pad
    ccm_row2: [f32; 4], // xyz used, w=pad
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
    /// ISP white balance gains [R, B] (Bayer only)
    pub colour_gains: Option<[f32; 2]>,
    /// 3x3 colour correction matrix (row-major, Bayer only)
    pub colour_correction_matrix: Option<[[f32; 3]; 3]>,
    /// Sensor black level normalized to 0..1 (Bayer only)
    pub black_level: Option<f32>,
}

/// Binding type specification for pipeline creation
#[derive(Clone, Copy)]
enum BindingSpec {
    Texture,
    StorageTexture,
    Uniform,
}

impl BindingSpec {
    fn to_layout_entry(self, binding: u32) -> wgpu::BindGroupLayoutEntry {
        wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::COMPUTE,
            ty: match self {
                BindingSpec::Texture => wgpu::BindingType::Texture {
                    sample_type: wgpu::TextureSampleType::Float { filterable: false },
                    view_dimension: wgpu::TextureViewDimension::D2,
                    multisampled: false,
                },
                BindingSpec::StorageTexture => wgpu::BindingType::StorageTexture {
                    access: wgpu::StorageTextureAccess::WriteOnly,
                    format: wgpu::TextureFormat::Rgba8Unorm,
                    view_dimension: wgpu::TextureViewDimension::D2,
                },
                BindingSpec::Uniform => wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
            },
            count: None,
        }
    }
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

        // Create uniform buffer (shared across all pipelines, sized for largest params struct)
        let uniform_size =
            std::mem::size_of::<ConvertParams>().max(std::mem::size_of::<DebayerParams>());
        let uniform_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("convert_uniform_buffer"),
            size: uniform_size as u64,
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
            PixelFormat::NV12 => self.create_pipeline(
                include_str!("convert_nv12.wgsl"),
                "nv12",
                &Self::BIND_LAYOUT_TWO_TEXTURE,
            ),
            PixelFormat::NV21 => self.create_pipeline(
                include_str!("convert_nv21.wgsl"),
                "nv21",
                &Self::BIND_LAYOUT_TWO_TEXTURE,
            ),
            PixelFormat::I420 => self.create_pipeline(
                include_str!("convert_i420.wgsl"),
                "i420",
                &Self::BIND_LAYOUT_THREE_TEXTURE,
            ),
            PixelFormat::YUYV => self.create_pipeline(
                include_str!("convert_yuyv.wgsl"),
                "yuyv",
                &Self::BIND_LAYOUT_ONE_TEXTURE,
            ),
            PixelFormat::UYVY => self.create_pipeline(
                include_str!("convert_uyvy.wgsl"),
                "uyvy",
                &Self::BIND_LAYOUT_ONE_TEXTURE,
            ),
            PixelFormat::YVYU => self.create_pipeline(
                include_str!("convert_yvyu.wgsl"),
                "yvyu",
                &Self::BIND_LAYOUT_ONE_TEXTURE,
            ),
            PixelFormat::VYUY => self.create_pipeline(
                include_str!("convert_vyuy.wgsl"),
                "vyuy",
                &Self::BIND_LAYOUT_ONE_TEXTURE,
            ),
            PixelFormat::Gray8 => self.create_pipeline(
                include_str!("convert_gray8.wgsl"),
                "gray8",
                &Self::BIND_LAYOUT_ONE_TEXTURE,
            ),
            PixelFormat::RGBA | PixelFormat::RGB24 | PixelFormat::ABGR | PixelFormat::BGRA => {
                // Fallback for formats that don't need GPU conversion
                self.create_pipeline(
                    include_str!("convert_nv12.wgsl"),
                    "fallback",
                    &Self::BIND_LAYOUT_TWO_TEXTURE,
                )
            }
            PixelFormat::BayerRGGB
            | PixelFormat::BayerBGGR
            | PixelFormat::BayerGRBG
            | PixelFormat::BayerGBRG => self.create_pipeline(
                include_str!("debayer.wgsl"),
                "debayer",
                &Self::BIND_LAYOUT_ONE_TEXTURE,
            ),
        }
    }

    // Bind group layout specifications (binding indices, not full entries)
    // One texture + output + params (packed formats, gray8, debayer)
    const BIND_LAYOUT_ONE_TEXTURE: [(u32, BindingSpec); 3] = [
        (0, BindingSpec::Texture),
        (1, BindingSpec::StorageTexture),
        (2, BindingSpec::Uniform),
    ];

    // Two textures + output + params (NV12, NV21)
    const BIND_LAYOUT_TWO_TEXTURE: [(u32, BindingSpec); 4] = [
        (0, BindingSpec::Texture),
        (1, BindingSpec::Texture),
        (2, BindingSpec::StorageTexture),
        (3, BindingSpec::Uniform),
    ];

    // Three textures + output + params (I420)
    const BIND_LAYOUT_THREE_TEXTURE: [(u32, BindingSpec); 5] = [
        (0, BindingSpec::Texture),
        (1, BindingSpec::Texture),
        (2, BindingSpec::Texture),
        (3, BindingSpec::StorageTexture),
        (4, BindingSpec::Uniform),
    ];

    /// Create a compute pipeline with the given shader and bind group layout spec
    fn create_pipeline<const N: usize>(
        &self,
        shader_source: &str,
        name: &str,
        layout_spec: &[(u32, BindingSpec); N],
    ) -> FormatPipeline {
        let shader = self
            .device
            .create_shader_module(wgpu::ShaderModuleDescriptor {
                label: Some(&format!("{}_shader", name)),
                source: wgpu::ShaderSource::Wgsl(shader_source.into()),
            });

        let entries: Vec<wgpu::BindGroupLayoutEntry> = layout_spec
            .iter()
            .map(|(binding, spec)| spec.to_layout_entry(*binding))
            .collect();

        let bind_group_layout =
            self.device
                .create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                    label: Some(&format!("{}_bind_group_layout", name)),
                    entries: &entries,
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
            PixelFormat::Gray8
            | PixelFormat::RGBA
            | PixelFormat::RGB24
            | PixelFormat::ABGR
            | PixelFormat::BGRA
            | PixelFormat::BayerRGGB
            | PixelFormat::BayerBGGR
            | PixelFormat::BayerGRBG
            | PixelFormat::BayerGBRG => (1, 1),
        };

        // Y plane texture format and dimensions
        let (y_format, y_width) = match format {
            PixelFormat::YUYV | PixelFormat::UYVY | PixelFormat::YVYU | PixelFormat::VYUY => {
                (wgpu::TextureFormat::Rgba8Unorm, width / 2)
            }
            PixelFormat::RGBA | PixelFormat::RGB24 | PixelFormat::ABGR | PixelFormat::BGRA => {
                (wgpu::TextureFormat::Rgba8Unorm, width)
            }
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

        // Update uniform buffer (different params struct for Bayer formats)
        if input.format.is_bayer() {
            let identity: [[f32; 3]; 3] = [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]];
            // Apply ISP colour when gains are available. Black level subtraction
            // is required before gains to avoid amplifying the sensor's DC offset
            // differently per channel (which causes colour casts).
            let (use_isp, gain_r, gain_b, bl, ccm) =
                match (input.colour_gains, input.colour_correction_matrix) {
                    (Some(gains), Some(matrix)) => (
                        1u32,
                        gains[0],
                        gains[1],
                        input.black_level.unwrap_or(0.0),
                        matrix,
                    ),
                    (Some(gains), None) => (
                        1u32,
                        gains[0],
                        gains[1],
                        input.black_level.unwrap_or(0.0),
                        identity,
                    ),
                    _ => (0u32, 1.0, 1.0, 0.0, identity),
                };
            let params = DebayerParams {
                width: input.width,
                height: input.height,
                pattern: input.format.bayer_pattern_code().unwrap_or(0),
                use_isp_colour: use_isp,
                colour_gain_r: gain_r,
                colour_gain_b: gain_b,
                black_level: bl,
                _pad0: 0,
                ccm_row0: [ccm[0][0], ccm[0][1], ccm[0][2], 0.0],
                ccm_row1: [ccm[1][0], ccm[1][1], ccm[1][2], 0.0],
                ccm_row2: [ccm[2][0], ccm[2][1], ccm[2][2], 0.0],
            };
            self.queue
                .write_buffer(&self.uniform_buffer, 0, bytemuck::bytes_of(&params));
        } else {
            let params = ConvertParams {
                width: input.width,
                height: input.height,
                _pad0: 0,
                _pad1: 0,
            };
            self.queue
                .write_buffer(&self.uniform_buffer, 0, bytemuck::bytes_of(&params));
        }

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

    /// Write a single plane to a GPU texture.
    fn write_plane(
        &self,
        texture: &wgpu::Texture,
        data: &[u8],
        bytes_per_row: u32,
        width: u32,
        height: u32,
    ) {
        self.queue.write_texture(
            wgpu::ImageCopyTexture {
                texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            data,
            wgpu::ImageDataLayout {
                offset: 0,
                bytes_per_row: Some(bytes_per_row),
                rows_per_image: Some(height),
            },
            wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
        );
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
                self.write_plane(
                    tex_y,
                    input.y_data,
                    input.y_stride,
                    input.width / 2,
                    input.height,
                );
            }

            // NV12/NV21: Y plane + UV plane
            PixelFormat::NV12 | PixelFormat::NV21 => {
                self.write_plane(
                    tex_y,
                    input.y_data,
                    input.y_stride,
                    input.width,
                    input.height,
                );
                if let Some(uv_data) = input.uv_data {
                    let uv_height = input.height / 2;
                    self.write_plane(tex_uv, uv_data, input.uv_stride, input.width / 2, uv_height);
                }
            }

            // I420: Y + U + V planes
            PixelFormat::I420 => {
                self.write_plane(
                    tex_y,
                    input.y_data,
                    input.y_stride,
                    input.width,
                    input.height,
                );
                if let Some(uv_data) = input.uv_data {
                    let uv_height = input.height / 2;
                    self.write_plane(tex_uv, uv_data, input.uv_stride, input.width / 2, uv_height);
                }
                if let Some(v_data) = input.v_data {
                    let v_height = input.height / 2;
                    self.write_plane(tex_v, v_data, input.v_stride, input.width / 2, v_height);
                }
            }

            // Gray8: single channel
            PixelFormat::Gray8 => {
                self.write_plane(
                    tex_y,
                    input.y_data,
                    input.y_stride,
                    input.width,
                    input.height,
                );
            }

            // Bayer formats: raw sensor data, single channel
            // CSI-2 packed formats (10/12/14-bit) need unpacking first
            PixelFormat::BayerRGGB
            | PixelFormat::BayerBGGR
            | PixelFormat::BayerGRBG
            | PixelFormat::BayerGBRG => {
                let (upload_data, upload_stride) = if input.y_stride > input.width {
                    let unpacked = unpack_bayer_packed_to_8bit(
                        input.y_data,
                        input.width,
                        input.height,
                        input.y_stride,
                    );
                    debug!(
                        width = input.width,
                        height = input.height,
                        packed_stride = input.y_stride,
                        unpacked_len = unpacked.len(),
                        "Unpacked CSI2P Bayer data to 8-bit"
                    );
                    (Some(unpacked), input.width)
                } else {
                    (None, input.y_stride)
                };

                let data = upload_data.as_deref().unwrap_or(input.y_data);
                self.write_plane(tex_y, data, upload_stride, input.width, input.height);
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

            // Packed formats, Gray8, and Bayer: tex_packed/tex_gray/tex_bayer, output, params
            PixelFormat::YUYV
            | PixelFormat::UYVY
            | PixelFormat::YVYU
            | PixelFormat::VYUY
            | PixelFormat::Gray8
            | PixelFormat::BayerRGGB
            | PixelFormat::BayerBGGR
            | PixelFormat::BayerGRBG
            | PixelFormat::BayerGBRG => self.device.create_bind_group(&wgpu::BindGroupDescriptor {
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

/// Unpack CSI-2 packed Bayer data to 8-bit per pixel (one byte per pixel).
///
/// CSI-2 packed formats store pixel data in groups with high bits first,
/// followed by packed low bits:
/// - 10-bit: 4 pixels in 5 bytes (bytes 0-3 = high 8 bits, byte 4 = low 2 bits)
/// - 12-bit: 2 pixels in 3 bytes (bytes 0-1 = high 8 bits, byte 2 = low 4 bits)
/// - 14-bit: 4 pixels in 7 bytes (bytes 0-3 = high 8 bits, bytes 4-6 = low 6 bits)
///
/// This function extracts the high 8 bits of each pixel. The packed_stride
/// is the actual bytes per row from the buffer (may include alignment padding).
/// Extract high 8 bits from CSI-2 packed pixel groups.
///
/// Copies the first `pixels_per_group` bytes from each `bytes_per_group`-byte
/// group in the source row. The packed metadata (low bits) at the end of each
/// group is discarded.
fn unpack_groups(
    packed: &[u8],
    output: &mut [u8],
    width: u32,
    height: u32,
    packed_stride: u32,
    pixels_per_group: usize,
    bytes_per_group: usize,
) {
    let num_groups = width as usize / pixels_per_group;
    let remaining = width as usize % pixels_per_group;

    for y in 0..height as usize {
        let row_start = y * packed_stride as usize;
        let out_start = y * width as usize;

        for g in 0..num_groups {
            let base = row_start + g * bytes_per_group;
            let out_base = out_start + g * pixels_per_group;
            output[out_base..out_base + pixels_per_group]
                .copy_from_slice(&packed[base..base + pixels_per_group]);
        }

        if remaining > 0 {
            let base = row_start + num_groups * bytes_per_group;
            let out_base = out_start + num_groups * pixels_per_group;
            output[out_base..out_base + remaining].copy_from_slice(&packed[base..base + remaining]);
        }
    }
}

fn unpack_bayer_packed_to_8bit(
    packed: &[u8],
    width: u32,
    height: u32,
    packed_stride: u32,
) -> Vec<u8> {
    // Detect bit depth from stride range (strides may include alignment padding)
    let min_stride_10 = (width * 5).div_ceil(4);
    let min_stride_12 = (width * 3).div_ceil(2);
    let min_stride_14 = (width * 7).div_ceil(4);

    let mut output = vec![0u8; (width * height) as usize];

    if packed_stride >= min_stride_10 && packed_stride < min_stride_12 {
        // CSI2P 10-bit: 4 pixels in 5 bytes
        unpack_groups(packed, &mut output, width, height, packed_stride, 4, 5);
    } else if packed_stride >= min_stride_12 && packed_stride < min_stride_14 {
        // CSI2P 12-bit: 2 pixels in 3 bytes
        unpack_groups(packed, &mut output, width, height, packed_stride, 2, 3);
    } else if packed_stride >= min_stride_14 && packed_stride < width * 2 {
        // CSI2P 14-bit: 4 pixels in 7 bytes
        unpack_groups(packed, &mut output, width, height, packed_stride, 4, 7);
    } else {
        // Unknown packing or 16-bit - copy high bytes row by row
        warn!(
            packed_stride,
            width, "Unknown Bayer packing, copying raw bytes"
        );
        for y in 0..height as usize {
            let src = y * packed_stride as usize;
            let dst = y * width as usize;
            let copy_len = (width as usize).min(packed_stride as usize);
            output[dst..dst + copy_len].copy_from_slice(&packed[src..src + copy_len]);
        }
    }

    output
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
