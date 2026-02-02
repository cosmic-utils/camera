// SPDX-License-Identifier: GPL-3.0-only
//! Shared YUV conversion types and utilities
//!
//! This module contains common code used by both:
//! - `YuvConvertPipeline` (photo capture - converts to internal texture, reads back to CPU)
//! - `VideoPipeline` (preview - converts directly to render texture)
//!
//! By centralizing these definitions, we ensure consistency and reduce code duplication.

use crate::backends::camera::types::PixelFormat;
use crate::gpu::wgpu;

/// YUV conversion parameters uniform buffer layout
///
/// Must match the `ConvertParams` struct in `yuv_convert.wgsl`.
/// Used by both photo capture and preview pipelines.
#[repr(C)]
#[derive(Debug, Copy, Clone, bytemuck::Pod, bytemuck::Zeroable)]
pub struct YuvConvertParams {
    pub width: u32,
    pub height: u32,
    /// Format code: 0=RGBA (passthrough), 1=NV12, 2=I420, 3=YUYV
    pub format: u32,
    pub y_stride: u32,
    pub uv_stride: u32,
    pub v_stride: u32,
    pub _pad: [u32; 2],
}

/// Calculate texture dimensions for YUV format planes
///
/// Returns ((y_width, y_height), (uv_width, uv_height))
#[inline]
pub fn yuv_texture_dimensions(
    width: u32,
    height: u32,
    format: PixelFormat,
) -> ((u32, u32), (u32, u32)) {
    let y_dims = if format == PixelFormat::YUYV {
        // YUYV is packed: 2 pixels per texel (uploaded as RGBA8)
        (width / 2, height)
    } else {
        (width, height)
    };

    let uv_dims = match format {
        PixelFormat::NV12 | PixelFormat::I420 => (width / 2, height / 2),
        PixelFormat::YUYV => (width / 2, height),
        PixelFormat::RGBA => (1, 1), // Dummy - RGBA shouldn't use YUV path
    };

    (y_dims, uv_dims)
}

/// Get texture formats for YUV planes
///
/// Returns (y_format, uv_format)
#[inline]
pub fn yuv_texture_formats(format: PixelFormat) -> (wgpu::TextureFormat, wgpu::TextureFormat) {
    let y_format = match format {
        PixelFormat::YUYV => wgpu::TextureFormat::Rgba8Unorm, // Packed YUYV as RGBA
        _ => wgpu::TextureFormat::R8Unorm,                    // Y plane
    };

    let uv_format = match format {
        PixelFormat::NV12 => wgpu::TextureFormat::Rg8Unorm, // Interleaved UV
        _ => wgpu::TextureFormat::R8Unorm,                  // U/V planes or dummy
    };

    (y_format, uv_format)
}

/// Create the bind group layout for YUV→RGBA compute shader
///
/// Bindings:
/// - 0: tex_y (Y plane or packed YUYV)
/// - 1: tex_uv (UV plane for NV12, U plane for I420)
/// - 2: tex_v (V plane for I420)
/// - 3: output (RGBA storage texture)
/// - 4: params (uniform buffer)
pub fn create_yuv_bind_group_layout(device: &wgpu::Device) -> wgpu::BindGroupLayout {
    device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
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
    })
}

/// Create the compute pipeline for YUV→RGBA conversion
pub fn create_yuv_compute_pipeline(
    device: &wgpu::Device,
    bind_group_layout: &wgpu::BindGroupLayout,
) -> wgpu::ComputePipeline {
    let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("yuv_convert_shader"),
        source: wgpu::ShaderSource::Wgsl(include_str!("yuv_convert.wgsl").into()),
    });

    let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("yuv_convert_pipeline_layout"),
        bind_group_layouts: &[bind_group_layout],
        push_constant_ranges: &[],
    });

    device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
        label: Some("yuv_convert_compute_pipeline"),
        layout: Some(&pipeline_layout),
        module: &shader,
        entry_point: Some("main"),
        compilation_options: Default::default(),
        cache: None,
    })
}

/// Create the uniform buffer for YUV conversion parameters
pub fn create_yuv_uniform_buffer(device: &wgpu::Device) -> wgpu::Buffer {
    device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("yuv_convert_uniform_buffer"),
        size: std::mem::size_of::<YuvConvertParams>() as u64,
        usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    })
}

/// YUV texture set for a single video source
///
/// Contains the three input textures needed for YUV→RGBA conversion:
/// - Y plane (or packed YUYV data)
/// - UV plane (NV12) or U plane (I420)
/// - V plane (I420 only, dummy for other formats)
pub struct YuvTextures {
    pub tex_y: wgpu::Texture,
    pub tex_y_view: wgpu::TextureView,
    pub tex_uv: wgpu::Texture,
    pub tex_uv_view: wgpu::TextureView,
    pub tex_v: wgpu::Texture,
    pub tex_v_view: wgpu::TextureView,
    pub width: u32,
    pub height: u32,
    pub format: PixelFormat,
}

impl YuvTextures {
    /// Create YUV textures for the given dimensions and format
    pub fn new(device: &wgpu::Device, width: u32, height: u32, format: PixelFormat) -> Self {
        let ((y_width, y_height), (uv_width, uv_height)) =
            yuv_texture_dimensions(width, height, format);
        let (y_format, uv_format) = yuv_texture_formats(format);

        // Create Y texture
        let tex_y = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("yuv_tex_y"),
            size: wgpu::Extent3d {
                width: y_width,
                height: y_height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: y_format,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        let tex_y_view = tex_y.create_view(&wgpu::TextureViewDescriptor::default());

        // Create UV texture
        let tex_uv = device.create_texture(&wgpu::TextureDescriptor {
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
        });
        let tex_uv_view = tex_uv.create_view(&wgpu::TextureViewDescriptor::default());

        // Create V texture (I420 only, but always create for bind group consistency)
        let tex_v = device.create_texture(&wgpu::TextureDescriptor {
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
        });
        let tex_v_view = tex_v.create_view(&wgpu::TextureViewDescriptor::default());

        Self {
            tex_y,
            tex_y_view,
            tex_uv,
            tex_uv_view,
            tex_v,
            tex_v_view,
            width,
            height,
            format,
        }
    }

    /// Check if these textures match the given dimensions and format
    #[inline]
    pub fn matches(&self, width: u32, height: u32, format: PixelFormat) -> bool {
        self.width == width && self.height == height && self.format == format
    }
}
