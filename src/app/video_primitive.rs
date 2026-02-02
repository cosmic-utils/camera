// SPDX-License-Identifier: GPL-3.0-only

//! Custom video rendering primitive with direct GPU texture updates
//!
//! This module implements iced_video_player-style optimizations:
//! - Direct GPU texture updates (no Handle recreation)
//! - RGBA textures for native RGB processing
//! - Persistent textures across frames

use crate::app::state::FilterType;
use crate::backends::camera::types::{FrameData, PixelFormat, YuvPlanes};
use cosmic::iced::Rectangle;
use cosmic::iced_wgpu::graphics::Viewport;
use cosmic::iced_wgpu::primitive::{self, Primitive as PrimitiveTrait};
use cosmic::iced_wgpu::wgpu;
use std::sync::{Arc, Mutex};

/// Video frame data for GPU upload
///
/// Supports both RGBA and YUV formats. For YUV formats, the data is converted
/// to RGBA by a GPU compute shader before rendering.
#[derive(Debug, Clone)]
pub struct VideoFrame {
    pub id: u64,
    pub width: u32,
    pub height: u32,
    /// Frame data: RGBA pixels, Y plane (NV12/I420), or packed YUYV
    pub data: FrameData,
    /// Pixel format (RGBA, NV12, I420, YUYV)
    pub format: PixelFormat,
    /// Row stride for main data (bytes per row including padding)
    pub stride: u32,
    /// Additional YUV planes (for NV12/I420 formats)
    pub yuv_planes: Option<YuvPlanes>,
}

impl VideoFrame {
    /// Get data slice for the main plane
    #[inline]
    pub fn data_slice(&self) -> &[u8] {
        &self.data
    }

    /// Get RGBA data slice (only valid for RGBA format)
    /// For YUV formats, use the YUV conversion pipeline first
    #[inline]
    pub fn rgba_data(&self) -> &[u8] {
        debug_assert!(
            self.format == PixelFormat::RGBA,
            "rgba_data() called on YUV frame"
        );
        &self.data
    }

    /// Check if this frame is in a YUV format requiring GPU conversion
    #[inline]
    pub fn is_yuv(&self) -> bool {
        self.format.is_yuv()
    }
}

/// Viewport and content fit data for Cover mode
#[repr(C)]
#[derive(Debug, Copy, Clone, bytemuck::Pod, bytemuck::Zeroable)]
struct ViewportUniform {
    /// Viewport width and height (full widget size)
    viewport_size: [f32; 2],
    /// Content fit mode: 0 = Contain, 1 = Cover
    content_fit_mode: u32,
    /// Filter mode: 0 = None, 1 = Black & White
    filter_mode: u32,
    /// Corner radius in pixels (0 = no rounding)
    corner_radius: f32,
    /// Mirror horizontally: 0 = normal, 1 = mirrored
    mirror_horizontal: u32,
    /// UV offset for scroll clipping (normalized 0-1, where visible area starts)
    uv_offset: [f32; 2],
    /// UV scale for scroll clipping (normalized, size of visible area relative to full widget)
    uv_scale: [f32; 2],
    /// Crop UV min (u_min, v_min) - normalized 0-1
    crop_uv_min: [f32; 2],
    /// Crop UV max (u_max, v_max) - normalized 0-1
    crop_uv_max: [f32; 2],
    /// Zoom level (1.0 = no zoom, 2.0 = 2x zoom, etc.)
    zoom_level: f32,
    /// Padding to maintain 16-byte alignment
    _padding: f32,
}

/// Combined frame and viewport data to reduce mutex contention
/// Single lock acquisition instead of two separate locks per frame
#[derive(Debug)]
pub struct FrameViewportData {
    pub frame: Option<VideoFrame>,
    pub viewport: (f32, f32, crate::app::video_widget::VideoContentFit),
    /// Physical widget bounds (x, y, width, height) clamped to render target
    /// Stored during prepare() and used in render() for valid viewport rect
    pub physical_bounds: Option<(f32, f32, f32, f32)>,
    /// UV offset for scroll/render-target clipping (normalized 0-1)
    pub uv_offset: (f32, f32),
    /// UV scale for scroll/render-target clipping (normalized 0-1)
    pub uv_scale: (f32, f32),
}

/// Custom primitive for video rendering
#[derive(Debug, Clone)]
pub struct VideoPrimitive {
    pub video_id: u64,
    /// Combined frame and viewport data - single mutex for both
    pub data: Arc<Mutex<FrameViewportData>>,
    /// Filter type to apply
    pub filter_type: FilterType,
    /// Corner radius in pixels (0 = no rounding)
    pub corner_radius: f32,
    /// Mirror horizontally (selfie mode)
    pub mirror_horizontal: bool,
    /// Crop UV coordinates (u_min, v_min, u_max, v_max) - None means no cropping
    pub crop_uv: Option<(f32, f32, f32, f32)>,
    /// Zoom level (1.0 = no zoom, 2.0 = 2x zoom, etc.)
    pub zoom_level: f32,
}

/// Video texture (shared across filter variations)
struct VideoTexture {
    texture: wgpu::Texture,
    view: wgpu::TextureView,
    width: u32,
    height: u32,
    /// Pointer to last uploaded frame data (for deduplication)
    /// Multiple widgets with same video_id share an Arc, so same pointer = same frame
    last_frame_ptr: usize,
}

/// Filter-specific binding (viewport buffer + bind group)
/// Created per (video_id, filter_mode) combination to allow shared texture with different filters
struct FilterBinding {
    bind_group: wgpu::BindGroup,
    viewport_buffer: wgpu::Buffer,
}

/// YUV conversion parameters uniform (must match shader struct)
#[repr(C)]
#[derive(Debug, Copy, Clone, bytemuck::Pod, bytemuck::Zeroable)]
struct YuvConvertParams {
    width: u32,
    height: u32,
    format: u32,
    y_stride: u32,
    uv_stride: u32,
    v_stride: u32,
    _pad: [u32; 2],
}

/// YUV textures for a video source (for YUV→RGBA conversion)
struct YuvTextures {
    tex_y: wgpu::Texture,
    tex_y_view: wgpu::TextureView,
    tex_uv: wgpu::Texture,
    tex_uv_view: wgpu::TextureView,
    tex_v: wgpu::Texture,
    tex_v_view: wgpu::TextureView,
    width: u32,
    height: u32,
    format: PixelFormat,
}

/// Custom pipeline for efficient video rendering
pub struct VideoPipeline {
    pipeline_rgba: wgpu::RenderPipeline,
    pipeline_rgb_blur: wgpu::RenderPipeline, // RGB blur for multi-pass
    bind_group_layout_rgba: wgpu::BindGroupLayout,
    bind_group_layout_rgb: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
    // Shared textures by video_id (single upload per source)
    textures: std::collections::HashMap<u64, VideoTexture>,
    // Per-filter bindings keyed by (video_id, filter_mode)
    // Allows shared texture with different filter uniforms
    bindings: std::collections::HashMap<(u64, u32), FilterBinding>,
    // Intermediate textures for multi-pass blur (recreated if size changes)
    // Using RefCell for interior mutability since render() takes &self
    blur_intermediate_1: std::cell::RefCell<Option<BlurIntermediateTexture>>,
    blur_intermediate_2: std::cell::RefCell<Option<BlurIntermediateTexture>>,
    // GPU timing tracking to detect and handle stalls
    last_upload_duration: std::cell::Cell<std::time::Duration>,
    frames_skipped: std::cell::Cell<u32>,
    // YUV→RGBA conversion compute pipeline
    yuv_compute_pipeline: Option<wgpu::ComputePipeline>,
    yuv_bind_group_layout: Option<wgpu::BindGroupLayout>,
    yuv_uniform_buffer: Option<wgpu::Buffer>,
    // YUV textures per video_id
    yuv_textures: std::collections::HashMap<u64, YuvTextures>,
}

/// Intermediate texture for multi-pass blur
struct BlurIntermediateTexture {
    view: wgpu::TextureView,
    bind_group: wgpu::BindGroup,
    viewport_buffer: wgpu::Buffer,
    width: u32,
    height: u32,
}

impl VideoPrimitive {
    pub fn new(video_id: u64) -> Self {
        use crate::app::video_widget::VideoContentFit;
        Self {
            video_id,
            data: Arc::new(Mutex::new(FrameViewportData {
                frame: None,
                viewport: (0.0, 0.0, VideoContentFit::Contain),
                physical_bounds: None,
                uv_offset: (0.0, 0.0),
                uv_scale: (1.0, 1.0),
            })),
            filter_type: FilterType::Standard,
            corner_radius: 0.0,
            mirror_horizontal: false,
            crop_uv: None,
            zoom_level: 1.0,
        }
    }

    pub fn update_frame(&self, frame: VideoFrame) {
        if let Ok(mut guard) = self.data.lock() {
            guard.frame = Some(frame);
        }
    }

    pub fn update_viewport(
        &self,
        width: f32,
        height: f32,
        content_fit: crate::app::video_widget::VideoContentFit,
    ) {
        if let Ok(mut guard) = self.data.lock() {
            guard.viewport = (width, height, content_fit);
        }
    }
}

impl PrimitiveTrait for VideoPrimitive {
    fn prepare(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        _format: wgpu::TextureFormat,
        storage: &mut primitive::Storage,
        bounds: &Rectangle,
        viewport: &Viewport,
    ) {
        use std::time::Instant;
        let prepare_start = Instant::now();

        // Get or create pipeline
        if !storage.has::<VideoPipeline>() {
            storage.store(VideoPipeline::new(device, _format));
        }

        // Calculate physical bounds from logical bounds using scale factor
        // Then clamp to render target to ensure valid viewport rect
        let scale = viewport.scale_factor() as f32;
        let render_target = viewport.physical_size();

        let raw_physical_bounds = (
            bounds.x * scale,
            bounds.y * scale,
            bounds.width * scale,
            bounds.height * scale,
        );

        // Clamp physical bounds to render target to avoid wgpu validation errors
        let clamped_x = raw_physical_bounds.0.max(0.0);
        let clamped_y = raw_physical_bounds.1.max(0.0);
        let clamped_w = ((raw_physical_bounds.0 + raw_physical_bounds.2)
            .min(render_target.width as f32)
            - clamped_x)
            .max(0.0);
        let clamped_h = ((raw_physical_bounds.1 + raw_physical_bounds.3)
            .min(render_target.height as f32)
            - clamped_y)
            .max(0.0);

        let clamped_physical_bounds = (clamped_x, clamped_y, clamped_w, clamped_h);

        // Calculate UV offset/scale to compensate for clamping
        // This ensures the visible portion maps to correct texture coordinates
        let (uv_offset, uv_scale) = if raw_physical_bounds.2 > 0.0 && raw_physical_bounds.3 > 0.0 {
            let uv_offset_x = (clamped_x - raw_physical_bounds.0) / raw_physical_bounds.2;
            let uv_offset_y = (clamped_y - raw_physical_bounds.1) / raw_physical_bounds.3;
            let uv_scale_x = clamped_w / raw_physical_bounds.2;
            let uv_scale_y = clamped_h / raw_physical_bounds.3;
            ((uv_offset_x, uv_offset_y), (uv_scale_x, uv_scale_y))
        } else {
            ((0.0, 0.0), (1.0, 1.0))
        };

        // Take frame and viewport data with brief lock, then release before GPU ops
        // Also store clamped physical bounds and UV adjustment for use in render()
        let (frame_opt, viewport_data, stored_uv_offset, stored_uv_scale) = {
            if let Ok(mut data_guard) = self.data.lock() {
                data_guard.physical_bounds = Some(clamped_physical_bounds);
                data_guard.uv_offset = uv_offset;
                data_guard.uv_scale = uv_scale;
                (
                    data_guard.frame.take(),
                    data_guard.viewport,
                    data_guard.uv_offset,
                    data_guard.uv_scale,
                )
            } else {
                return;
            }
        };
        // Mutex released here - GPU operations won't block other threads

        let lock_time = prepare_start.elapsed();

        if let Some(pipeline) = storage.get_mut::<VideoPipeline>() {
            // Upload frame if available
            if let Some(frame) = frame_opt {
                let upload_start = Instant::now();

                // For blur video (video_id == 1), ensure intermediate textures exist
                if self.video_id == 1 {
                    pipeline.ensure_intermediate_textures(
                        device,
                        frame.width,
                        frame.height,
                        _format,
                    );
                }
                pipeline.upload(device, queue, frame);

                let upload_time = upload_start.elapsed();
                if upload_time.as_millis() > 16 {
                    tracing::warn!(
                        upload_ms = upload_time.as_millis(),
                        lock_ms = lock_time.as_millis(),
                        "GPU upload took longer than frame period - causing stutter"
                    );
                }
            }

            // Update viewport uniform data (using viewport_data captured before releasing lock)
            let (width, height, content_fit) = viewport_data;

            // Get content fit mode as u32 (0 = Contain, 1 = Cover)
            use crate::app::video_widget::VideoContentFit;
            let content_fit_mode = match content_fit {
                VideoContentFit::Contain => 0,
                VideoContentFit::Cover => 1,
            };

            let filter_mode = self.filter_type.gpu_filter_code();

            // Get or create binding for this (video_id, filter_mode) combination
            // This allows sharing the source texture while having per-filter uniforms
            pipeline.get_or_create_binding(device, self.video_id, filter_mode);

            // Get texture dimensions for blur passes
            let tex_dims = pipeline
                .textures
                .get(&self.video_id)
                .map(|t| (t.width, t.height));

            // Update viewport buffer for this specific filter binding
            let binding_key = (self.video_id, filter_mode);
            if let Some(binding) = pipeline.bindings.get(&binding_key) {
                // For blur video (video_id == 1), use Contain mode for Pass 1
                // For regular video, use the requested Cover/Contain mode
                // Get crop UV values (default to full image if not set)
                let (crop_min, crop_max) = self.crop_uv.map_or(
                    ([0.0f32, 0.0], [1.0f32, 1.0]),
                    |(u_min, v_min, u_max, v_max)| ([u_min, v_min], [u_max, v_max]),
                );

                if self.video_id == 1 {
                    if let Some((tex_width, tex_height)) = tex_dims {
                        // Blur video: use Contain mode with texture dimensions for Pass 1
                        // Apply mirror in first pass since this reads from source texture
                        // Apply filter in first pass so the filter is visible during transition
                        let blur_uniform = ViewportUniform {
                            viewport_size: [tex_width as f32, tex_height as f32],
                            content_fit_mode: 0, // Contain mode - no Cover cropping in Pass 1
                            filter_mode,         // Apply filter during blur (visible in transition)
                            corner_radius: 0.0,  // No rounded corners for blur passes
                            mirror_horizontal: if self.mirror_horizontal { 1 } else { 0 },
                            uv_offset: [0.0, 0.0],
                            uv_scale: [1.0, 1.0],
                            crop_uv_min: crop_min,
                            crop_uv_max: crop_max,
                            zoom_level: 1.0, // No zoom for blur passes
                            _padding: 0.0,
                        };
                        queue.write_buffer(
                            &binding.viewport_buffer,
                            0,
                            bytemuck::cast_slice(&[blur_uniform]),
                        );
                    }
                } else {
                    // Regular video: use requested mode with UV adjustment for clipping
                    let uniform_data = ViewportUniform {
                        viewport_size: [width, height],
                        content_fit_mode,
                        filter_mode,
                        corner_radius: self.corner_radius,
                        mirror_horizontal: if self.mirror_horizontal { 1 } else { 0 },
                        uv_offset: [stored_uv_offset.0, stored_uv_offset.1],
                        uv_scale: [stored_uv_scale.0, stored_uv_scale.1],
                        crop_uv_min: crop_min,
                        crop_uv_max: crop_max,
                        zoom_level: self.zoom_level,
                        _padding: 0.0,
                    };
                    queue.write_buffer(
                        &binding.viewport_buffer,
                        0,
                        bytemuck::cast_slice(&[uniform_data]),
                    );
                }

                // Update intermediate texture viewport buffers for blur passes
                // intermediate_1: Contain mode (no cropping) for pass 2
                // intermediate_2: Cover mode with screen viewport for final pass 3
                if let Some(intermediate_1) = pipeline.blur_intermediate_1.borrow().as_ref() {
                    let intermediate_uniform = ViewportUniform {
                        viewport_size: [intermediate_1.width as f32, intermediate_1.height as f32],
                        content_fit_mode: 0, // Contain mode - no Cover cropping in intermediate pass
                        filter_mode: 0,      // No filter during intermediate pass
                        corner_radius: 0.0,  // No rounded corners for intermediate passes
                        mirror_horizontal: 0, // No mirror for intermediate passes
                        uv_offset: [0.0, 0.0],
                        uv_scale: [1.0, 1.0],
                        crop_uv_min: [0.0, 0.0], // No crop for intermediate
                        crop_uv_max: [1.0, 1.0],
                        zoom_level: 1.0, // No zoom for intermediate passes
                        _padding: 0.0,
                    };
                    queue.write_buffer(
                        &intermediate_1.viewport_buffer,
                        0,
                        bytemuck::cast_slice(&[intermediate_uniform]),
                    );
                }
                if let Some(intermediate_2) = pipeline.blur_intermediate_2.borrow().as_ref() {
                    // Use screen viewport dimensions and Cover mode for final pass to screen
                    // Mirror is already applied in pass 1, don't apply again
                    let final_pass_uniform = ViewportUniform {
                        viewport_size: [width, height],
                        content_fit_mode,
                        filter_mode: 0,       // No filter during blur
                        corner_radius: 0.0,   // No rounded corners for blur
                        mirror_horizontal: 0, // Already mirrored in pass 1
                        uv_offset: [0.0, 0.0],
                        uv_scale: [1.0, 1.0],
                        crop_uv_min: [0.0, 0.0], // No crop for final blur pass
                        crop_uv_max: [1.0, 1.0],
                        zoom_level: 1.0, // No zoom for blur
                        _padding: 0.0,
                    };
                    queue.write_buffer(
                        &intermediate_2.viewport_buffer,
                        0,
                        bytemuck::cast_slice(&[final_pass_uniform]),
                    );
                }
            }
        }
    }

    fn render(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        storage: &primitive::Storage,
        target: &wgpu::TextureView,
        clip_bounds: &Rectangle<u32>,
    ) {
        // Convert filter_type to filter_mode for binding lookup
        let filter_mode = self.filter_type.gpu_filter_code();

        // Use stored physical bounds for viewport (prevents distortion in scrollable contexts)
        // Fall back to clip_bounds if physical_bounds not available
        let widget_bounds = self
            .data
            .lock()
            .ok()
            .and_then(|guard| guard.physical_bounds)
            .unwrap_or((
                clip_bounds.x as f32,
                clip_bounds.y as f32,
                clip_bounds.width as f32,
                clip_bounds.height as f32,
            ));

        if let Some(pipeline) = storage.get::<VideoPipeline>() {
            pipeline.render(
                self.video_id,
                filter_mode,
                encoder,
                target,
                clip_bounds,
                widget_bounds,
            );
        }
    }
}

impl VideoPipeline {
    pub fn new(device: &wgpu::Device, format: wgpu::TextureFormat) -> Self {
        // ===== Video Pipeline =====
        // Shader for video rendering with shared filter functions
        let shader_source = format!(
            "{}\n{}",
            crate::shaders::FILTER_FUNCTIONS,
            include_str!("video_shader.wgsl")
        );
        let shader_rgba = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("camera video shader"),
            source: wgpu::ShaderSource::Wgsl(shader_source.into()),
        });

        // Bind group layout for video texture, sampler, and viewport
        let bind_group_layout_rgba =
            device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("camera video bind group layout"),
                entries: &[
                    // RGBA texture
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
                    // Sampler
                    wgpu::BindGroupLayoutEntry {
                        binding: 1,
                        visibility: wgpu::ShaderStages::FRAGMENT,
                        ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                        count: None,
                    },
                    // Viewport uniform
                    wgpu::BindGroupLayoutEntry {
                        binding: 2,
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

        let pipeline_layout_rgba = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("camera video pipeline layout"),
            bind_group_layouts: &[&bind_group_layout_rgba],
            push_constant_ranges: &[],
        });

        let pipeline_rgba = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("camera video pipeline"),
            layout: Some(&pipeline_layout_rgba),
            vertex: wgpu::VertexState {
                module: &shader_rgba,
                entry_point: "vs_main",
                buffers: &[],
                compilation_options: Default::default(),
            },
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: None,
            multisample: wgpu::MultisampleState {
                count: 1,
                mask: !0,
                alpha_to_coverage_enabled: false,
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader_rgba,
                entry_point: "fs_main",
                targets: &[Some(wgpu::ColorTargetState {
                    format,
                    blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
                compilation_options: Default::default(),
            }),
            multiview: None,
            cache: None,
        });

        // ===== Blur Pipeline (for multi-pass blur) =====
        let shader_blur_source = include_str!("video_shader_blur.wgsl");
        let shader_rgb_blur = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("camera blur shader"),
            source: wgpu::ShaderSource::Wgsl(shader_blur_source.into()),
        });

        // Bind group layout for blur texture, sampler, and viewport
        let bind_group_layout_rgb =
            device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("camera blur bind group layout"),
                entries: &[
                    // RGB texture
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
                    // Sampler
                    wgpu::BindGroupLayoutEntry {
                        binding: 1,
                        visibility: wgpu::ShaderStages::FRAGMENT,
                        ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                        count: None,
                    },
                    // Viewport uniform
                    wgpu::BindGroupLayoutEntry {
                        binding: 2,
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

        let pipeline_layout_rgb = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("camera blur pipeline layout"),
            bind_group_layouts: &[&bind_group_layout_rgb],
            push_constant_ranges: &[],
        });

        let pipeline_rgb_blur = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("camera blur pipeline"),
            layout: Some(&pipeline_layout_rgb),
            vertex: wgpu::VertexState {
                module: &shader_rgb_blur,
                entry_point: "vs_main",
                buffers: &[],
                compilation_options: Default::default(),
            },
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: None,
            multisample: wgpu::MultisampleState {
                count: 1,
                mask: !0,
                alpha_to_coverage_enabled: false,
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader_rgb_blur,
                entry_point: "fs_main",
                targets: &[Some(wgpu::ColorTargetState {
                    format,
                    blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
                compilation_options: Default::default(),
            }),
            multiview: None,
            cache: None,
        });

        // Shared sampler for all pipelines
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("camera video sampler"),
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            address_mode_w: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::FilterMode::Nearest,
            ..Default::default()
        });

        // ===== YUV→RGBA Conversion Compute Pipeline =====
        let yuv_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("yuv_convert_shader"),
            source: wgpu::ShaderSource::Wgsl(include_str!("../shaders/yuv_convert.wgsl").into()),
        });

        let yuv_bind_group_layout =
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
            });

        let yuv_pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("yuv_convert_pipeline_layout"),
            bind_group_layouts: &[&yuv_bind_group_layout],
            push_constant_ranges: &[],
        });

        let yuv_compute_pipeline =
            device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
                label: Some("yuv_convert_compute_pipeline"),
                layout: Some(&yuv_pipeline_layout),
                module: &yuv_shader,
                entry_point: "main",
                compilation_options: Default::default(),
                cache: None,
            });

        let yuv_uniform_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("yuv_convert_uniform_buffer"),
            size: std::mem::size_of::<YuvConvertParams>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        Self {
            pipeline_rgba,
            pipeline_rgb_blur,
            bind_group_layout_rgba,
            bind_group_layout_rgb,
            sampler,
            textures: std::collections::HashMap::new(),
            bindings: std::collections::HashMap::new(),
            blur_intermediate_1: std::cell::RefCell::new(None),
            blur_intermediate_2: std::cell::RefCell::new(None),
            last_upload_duration: std::cell::Cell::new(std::time::Duration::ZERO),
            frames_skipped: std::cell::Cell::new(0),
            yuv_compute_pipeline: Some(yuv_compute_pipeline),
            yuv_bind_group_layout: Some(yuv_bind_group_layout),
            yuv_uniform_buffer: Some(yuv_uniform_buffer),
            yuv_textures: std::collections::HashMap::new(),
        }
    }

    /// Upload frame data directly to GPU textures (texture only, bindings created separately)
    fn upload(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, frame: VideoFrame) {
        use std::time::Instant;

        if frame.width == 0 || frame.height == 0 {
            return;
        }

        // Skip frame if GPU is behind (last upload took > 32ms = 2 frame periods at 60fps)
        // This prevents the GPU command queue from backing up and causing UI hangs
        let last_duration = self.last_upload_duration.get();
        if last_duration.as_millis() > 32 {
            let skipped = self.frames_skipped.get() + 1;
            self.frames_skipped.set(skipped);
            if skipped % 10 == 1 {
                tracing::warn!(
                    skipped_count = skipped,
                    last_upload_ms = last_duration.as_millis(),
                    "Skipping frame - GPU behind, preventing UI hang"
                );
            }
            // Reset timing to allow next frame through
            self.last_upload_duration.set(std::time::Duration::ZERO);
            return;
        }

        let upload_start = Instant::now();

        // Get data pointer for deduplication (all filter picker widgets share the same Arc)
        let frame_data_ptr = frame.data.as_ptr() as usize;

        // Check if texture exists and needs resizing
        let needs_creation = match self.textures.get(&frame.id) {
            Some(tex) => tex.width != frame.width || tex.height != frame.height,
            None => true,
        };

        // Check if this exact frame was already uploaded (same Arc pointer)
        // This prevents 15 redundant uploads when filter picker widgets share the same frame
        if !needs_creation {
            if let Some(tex) = self.textures.get(&frame.id) {
                if tex.last_frame_ptr == frame_data_ptr {
                    // Same frame data already uploaded, skip
                    return;
                }
            }
        }

        // Create or resize texture if needed (invalidates all bindings for this video_id)
        if needs_creation {
            let create_start = Instant::now();
            let new_tex = self.create_texture(device, frame.width, frame.height);
            self.textures.insert(frame.id, new_tex);
            // Remove all bindings for this video_id since texture changed
            self.bindings.retain(|(vid, _), _| *vid != frame.id);
            let create_time = create_start.elapsed();
            if create_time.as_millis() > 5 {
                tracing::warn!(
                    create_ms = create_time.as_millis(),
                    width = frame.width,
                    height = frame.height,
                    "Texture creation took significant time - may cause stutter"
                );
            }
        }

        // Handle YUV or RGBA upload
        let gpu_copy_start = Instant::now();

        if frame.is_yuv() {
            // YUV path: Update last frame pointer, then do YUV conversion
            {
                let tex = self
                    .textures
                    .get_mut(&frame.id)
                    .expect("Texture should exist");
                tex.last_frame_ptr = frame_data_ptr;
            }
            // Now self.textures borrow is released, we can call upload_yuv_and_convert
            self.upload_yuv_and_convert(device, queue, &frame);
        } else {
            // Direct RGBA texture upload (CPU to GPU copy)
            let tex = self
                .textures
                .get_mut(&frame.id)
                .expect("Texture should exist");
            tex.last_frame_ptr = frame_data_ptr;

            queue.write_texture(
                wgpu::ImageCopyTexture {
                    texture: &tex.texture,
                    mip_level: 0,
                    origin: wgpu::Origin3d::ZERO,
                    aspect: wgpu::TextureAspect::All,
                },
                frame.rgba_data(),
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
        }
        let gpu_copy_time = gpu_copy_start.elapsed();

        // Track upload duration for frame skipping decisions
        let upload_duration = upload_start.elapsed();
        self.last_upload_duration.set(upload_duration);

        // Log GPU upload performance periodically (every ~30 frames based on frame.id)
        if frame.id % 30 == 0 {
            let size_bytes = frame.data_slice().len();
            tracing::debug!(
                gpu_upload_ms = format!("{:.2}", gpu_copy_time.as_micros() as f64 / 1000.0),
                total_prepare_ms = format!("{:.2}", upload_duration.as_micros() as f64 / 1000.0),
                width = frame.width,
                height = frame.height,
                size_mb = format!("{:.1}", size_bytes as f64 / 1_000_000.0),
                format = ?frame.format,
                "GPU texture upload"
            );
        }

        // Reset skip counter on successful upload
        if self.frames_skipped.get() > 0 {
            tracing::info!(
                frames_recovered = self.frames_skipped.get(),
                "GPU caught up, resuming normal frame rate"
            );
            self.frames_skipped.set(0);
        }
    }

    /// Create a texture for a video source (shared across filter variations)
    /// Includes STORAGE_BINDING usage for YUV→RGBA compute shader output
    fn create_texture(&self, device: &wgpu::Device, width: u32, height: u32) -> VideoTexture {
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("camera RGBA texture"),
            size: wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            // Include STORAGE_BINDING for YUV→RGBA compute shader output
            usage: wgpu::TextureUsages::TEXTURE_BINDING
                | wgpu::TextureUsages::COPY_DST
                | wgpu::TextureUsages::STORAGE_BINDING,
            view_formats: &[],
        });

        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());

        VideoTexture {
            texture,
            view,
            width,
            height,
            last_frame_ptr: 0, // Will be set on first upload
        }
    }

    /// Upload YUV frame data and convert to RGBA using GPU compute shader
    ///
    /// This method:
    /// 1. Uploads YUV plane data to GPU textures
    /// 2. Runs compute shader to convert YUV→RGBA
    /// 3. Outputs directly to the RGBA texture used for rendering
    ///
    /// All processing stays on GPU - no CPU round-trip between YUV conversion and rendering.
    fn upload_yuv_and_convert(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        frame: &VideoFrame,
    ) {
        use std::time::Instant;
        let convert_start = Instant::now();

        // Ensure YUV textures exist
        self.ensure_yuv_textures(device, frame.id, frame.width, frame.height, frame.format);

        // Get output texture for compute shader
        let output_texture = match self.textures.get(&frame.id) {
            Some(tex) => &tex.texture,
            None => {
                tracing::error!("Output texture not found for YUV conversion");
                return;
            }
        };

        let yuv_textures = match self.yuv_textures.get(&frame.id) {
            Some(t) => t,
            None => {
                tracing::error!("YUV textures not found after ensure_yuv_textures");
                return;
            }
        };

        // Get the full buffer data (zero-copy from GStreamer)
        let buffer_data = frame.data_slice();

        // Upload planes using offsets (zero-copy: we slice from the mapped buffer)
        match frame.format {
            PixelFormat::YUYV => {
                // YUYV: packed as RGBA8 where each texel is [Y0, U, Y1, V]
                // Texture width is frame.width / 2 (2 pixels per texel)
                let packed_width = frame.width / 2;
                queue.write_texture(
                    wgpu::ImageCopyTexture {
                        texture: &yuv_textures.tex_y,
                        mip_level: 0,
                        origin: wgpu::Origin3d::ZERO,
                        aspect: wgpu::TextureAspect::All,
                    },
                    buffer_data,
                    wgpu::ImageDataLayout {
                        offset: 0,
                        bytes_per_row: Some(frame.stride),
                        rows_per_image: None,
                    },
                    wgpu::Extent3d {
                        width: packed_width,
                        height: frame.height,
                        depth_or_array_layers: 1,
                    },
                );
            }
            PixelFormat::NV12 => {
                // NV12: Use offsets to slice Y and UV planes from buffer
                if let Some(ref yuv_planes) = frame.yuv_planes {
                    let uv_width = frame.width / 2;
                    let uv_height = frame.height / 2;

                    // Y plane: full resolution, R8 format
                    let y_end = yuv_planes.y_offset + yuv_planes.y_size;
                    queue.write_texture(
                        wgpu::ImageCopyTexture {
                            texture: &yuv_textures.tex_y,
                            mip_level: 0,
                            origin: wgpu::Origin3d::ZERO,
                            aspect: wgpu::TextureAspect::All,
                        },
                        &buffer_data[yuv_planes.y_offset..y_end],
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

                    // UV plane: interleaved UV as RG8
                    let uv_end = yuv_planes.uv_offset + yuv_planes.uv_size;
                    queue.write_texture(
                        wgpu::ImageCopyTexture {
                            texture: &yuv_textures.tex_uv,
                            mip_level: 0,
                            origin: wgpu::Origin3d::ZERO,
                            aspect: wgpu::TextureAspect::All,
                        },
                        &buffer_data[yuv_planes.uv_offset..uv_end],
                        wgpu::ImageDataLayout {
                            offset: 0,
                            bytes_per_row: Some(yuv_planes.uv_stride),
                            rows_per_image: None,
                        },
                        wgpu::Extent3d {
                            width: uv_width,
                            height: uv_height,
                            depth_or_array_layers: 1,
                        },
                    );
                }
            }
            PixelFormat::I420 => {
                // I420: Use offsets to slice Y, U, V planes from buffer
                if let Some(ref yuv_planes) = frame.yuv_planes {
                    let uv_width = frame.width / 2;
                    let uv_height = frame.height / 2;

                    // Y plane: full resolution, R8 format
                    let y_end = yuv_planes.y_offset + yuv_planes.y_size;
                    queue.write_texture(
                        wgpu::ImageCopyTexture {
                            texture: &yuv_textures.tex_y,
                            mip_level: 0,
                            origin: wgpu::Origin3d::ZERO,
                            aspect: wgpu::TextureAspect::All,
                        },
                        &buffer_data[yuv_planes.y_offset..y_end],
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

                    // U plane: R8 format
                    let u_end = yuv_planes.uv_offset + yuv_planes.uv_size;
                    queue.write_texture(
                        wgpu::ImageCopyTexture {
                            texture: &yuv_textures.tex_uv,
                            mip_level: 0,
                            origin: wgpu::Origin3d::ZERO,
                            aspect: wgpu::TextureAspect::All,
                        },
                        &buffer_data[yuv_planes.uv_offset..u_end],
                        wgpu::ImageDataLayout {
                            offset: 0,
                            bytes_per_row: Some(yuv_planes.uv_stride),
                            rows_per_image: None,
                        },
                        wgpu::Extent3d {
                            width: uv_width,
                            height: uv_height,
                            depth_or_array_layers: 1,
                        },
                    );

                    // V plane: R8 format
                    if yuv_planes.v_size > 0 {
                        let v_end = yuv_planes.v_offset + yuv_planes.v_size;
                        queue.write_texture(
                            wgpu::ImageCopyTexture {
                                texture: &yuv_textures.tex_v,
                                mip_level: 0,
                                origin: wgpu::Origin3d::ZERO,
                                aspect: wgpu::TextureAspect::All,
                            },
                            &buffer_data[yuv_planes.v_offset..v_end],
                            wgpu::ImageDataLayout {
                                offset: 0,
                                bytes_per_row: Some(yuv_planes.v_stride),
                                rows_per_image: None,
                            },
                            wgpu::Extent3d {
                                width: uv_width,
                                height: uv_height,
                                depth_or_array_layers: 1,
                            },
                        );
                    }
                }
            }
            PixelFormat::RGBA => {
                // Should not reach here - RGBA is handled by direct upload path
                tracing::warn!("upload_yuv_and_convert called for RGBA frame");
                return;
            }
        }

        // Update uniform buffer with conversion parameters
        let format_code = match frame.format {
            PixelFormat::RGBA => 0u32,
            PixelFormat::NV12 => 1u32,
            PixelFormat::I420 => 2u32,
            PixelFormat::YUYV => 3u32,
        };

        let params = YuvConvertParams {
            width: frame.width,
            height: frame.height,
            format: format_code,
            y_stride: frame.stride,
            uv_stride: frame.yuv_planes.as_ref().map(|p| p.uv_stride).unwrap_or(0),
            v_stride: frame.yuv_planes.as_ref().map(|p| p.v_stride).unwrap_or(0),
            _pad: [0, 0],
        };

        if let Some(ref uniform_buffer) = self.yuv_uniform_buffer {
            queue.write_buffer(uniform_buffer, 0, bytemuck::cast_slice(&[params]));
        }

        // Create output texture view for storage binding
        let output_view = output_texture.create_view(&wgpu::TextureViewDescriptor::default());

        // Create bind group for compute shader
        let bind_group_layout = match &self.yuv_bind_group_layout {
            Some(layout) => layout,
            None => {
                tracing::error!("YUV bind group layout not initialized");
                return;
            }
        };

        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("yuv_convert_bind_group"),
            layout: bind_group_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&yuv_textures.tex_y_view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(&yuv_textures.tex_uv_view),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::TextureView(&yuv_textures.tex_v_view),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: wgpu::BindingResource::TextureView(&output_view),
                },
                wgpu::BindGroupEntry {
                    binding: 4,
                    resource: self
                        .yuv_uniform_buffer
                        .as_ref()
                        .unwrap()
                        .as_entire_binding(),
                },
            ],
        });

        // Dispatch compute shader
        let compute_pipeline = match &self.yuv_compute_pipeline {
            Some(pipeline) => pipeline,
            None => {
                tracing::error!("YUV compute pipeline not initialized");
                return;
            }
        };

        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("yuv_convert_encoder"),
        });

        {
            let mut compute_pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some("yuv_convert_pass"),
                timestamp_writes: None,
            });

            compute_pass.set_pipeline(compute_pipeline);
            compute_pass.set_bind_group(0, &bind_group, &[]);

            // Dispatch: workgroup size is 16x16, so divide and round up
            let workgroup_x = (frame.width + 15) / 16;
            let workgroup_y = (frame.height + 15) / 16;
            compute_pass.dispatch_workgroups(workgroup_x, workgroup_y, 1);
        }

        queue.submit(std::iter::once(encoder.finish()));

        let convert_time = convert_start.elapsed();
        if frame.id % 60 == 0 {
            tracing::debug!(
                format = ?frame.format,
                width = frame.width,
                height = frame.height,
                convert_us = convert_time.as_micros(),
                "YUV→RGBA GPU conversion"
            );
        }
    }

    /// Create or update YUV textures for a video source
    fn ensure_yuv_textures(
        &mut self,
        device: &wgpu::Device,
        video_id: u64,
        width: u32,
        height: u32,
        format: PixelFormat,
    ) {
        // Check if textures exist and match dimensions/format
        if let Some(yuv) = self.yuv_textures.get(&video_id) {
            if yuv.width == width && yuv.height == height && yuv.format == format {
                return;
            }
        }

        // Calculate texture dimensions based on format
        let (y_width, y_height) = (width, height);
        let (uv_width, uv_height) = match format {
            PixelFormat::NV12 | PixelFormat::I420 => (width / 2, height / 2),
            PixelFormat::YUYV => (width / 2, height), // Packed: 2 pixels per texel
            PixelFormat::RGBA => (1, 1),              // Dummy
        };

        // Y plane texture format
        let y_format = match format {
            PixelFormat::YUYV => wgpu::TextureFormat::Rgba8Unorm, // Packed YUYV as RGBA
            _ => wgpu::TextureFormat::R8Unorm,                    // Y plane
        };

        // UV plane texture format
        let uv_format = match format {
            PixelFormat::NV12 => wgpu::TextureFormat::Rg8Unorm, // Interleaved UV
            _ => wgpu::TextureFormat::R8Unorm,                  // U/V planes
        };

        // Create Y texture
        let tex_y = device.create_texture(&wgpu::TextureDescriptor {
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

        self.yuv_textures.insert(
            video_id,
            YuvTextures {
                tex_y,
                tex_y_view,
                tex_uv,
                tex_uv_view,
                tex_v,
                tex_v_view,
                width,
                height,
                format,
            },
        );

        tracing::debug!(
            video_id,
            width,
            height,
            ?format,
            "Created YUV textures for GPU conversion"
        );
    }

    /// Get or create a filter-specific binding for a video
    /// Creates a unique binding per (video_id, filter_mode) combination
    /// This allows sharing the source texture while having different filter uniforms
    fn get_or_create_binding(
        &mut self,
        device: &wgpu::Device,
        video_id: u64,
        filter_mode: u32,
    ) -> Option<&FilterBinding> {
        let key = (video_id, filter_mode);

        // Check if binding already exists
        if self.bindings.contains_key(&key) {
            return self.bindings.get(&key);
        }

        // Need to create new binding - get the texture first
        let tex = self.textures.get(&video_id)?;

        // Create viewport buffer for this filter
        let viewport_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("camera filter viewport buffer"),
            size: std::mem::size_of::<ViewportUniform>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("camera filter bind group"),
            layout: &self.bind_group_layout_rgba,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&tex.view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(&self.sampler),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: viewport_buffer.as_entire_binding(),
                },
            ],
        });

        self.bindings.insert(
            key,
            FilterBinding {
                bind_group,
                viewport_buffer,
            },
        );

        self.bindings.get(&key)
    }

    /// Create or update intermediate textures for multi-pass blur
    fn ensure_intermediate_textures(
        &self,
        device: &wgpu::Device,
        width: u32,
        height: u32,
        format: wgpu::TextureFormat,
    ) {
        // Check if we need to recreate intermediate textures
        let needs_recreation = {
            let intermediate_1 = self.blur_intermediate_1.borrow();
            match intermediate_1.as_ref() {
                Some(intermediate) => intermediate.width != width || intermediate.height != height,
                None => true,
            }
        };

        if needs_recreation {
            // Create intermediate texture 1
            let texture_1 = device.create_texture(&wgpu::TextureDescriptor {
                label: Some("camera blur intermediate 1"),
                size: wgpu::Extent3d {
                    width,
                    height,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format,
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT
                    | wgpu::TextureUsages::TEXTURE_BINDING,
                view_formats: &[],
            });

            let view_1 = texture_1.create_view(&wgpu::TextureViewDescriptor::default());

            // Create viewport buffer for intermediate texture 1
            let viewport_buffer_1 = device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("camera blur intermediate 1 viewport buffer"),
                size: std::mem::size_of::<ViewportUniform>() as u64,
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });

            let bind_group_1 = device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("camera blur intermediate 1 bind group"),
                layout: &self.bind_group_layout_rgb,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: wgpu::BindingResource::TextureView(&view_1),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: wgpu::BindingResource::Sampler(&self.sampler),
                    },
                    wgpu::BindGroupEntry {
                        binding: 2,
                        resource: viewport_buffer_1.as_entire_binding(),
                    },
                ],
            });

            self.blur_intermediate_1
                .replace(Some(BlurIntermediateTexture {
                    view: view_1,
                    bind_group: bind_group_1,
                    viewport_buffer: viewport_buffer_1,
                    width,
                    height,
                }));

            // Create intermediate texture 2
            let texture_2 = device.create_texture(&wgpu::TextureDescriptor {
                label: Some("camera blur intermediate 2"),
                size: wgpu::Extent3d {
                    width,
                    height,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format,
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT
                    | wgpu::TextureUsages::TEXTURE_BINDING,
                view_formats: &[],
            });

            let view_2 = texture_2.create_view(&wgpu::TextureViewDescriptor::default());

            // Create viewport buffer for intermediate texture 2
            let viewport_buffer_2 = device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("camera blur intermediate 2 viewport buffer"),
                size: std::mem::size_of::<ViewportUniform>() as u64,
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });

            let bind_group_2 = device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("camera blur intermediate 2 bind group"),
                layout: &self.bind_group_layout_rgb,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: wgpu::BindingResource::TextureView(&view_2),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: wgpu::BindingResource::Sampler(&self.sampler),
                    },
                    wgpu::BindGroupEntry {
                        binding: 2,
                        resource: viewport_buffer_2.as_entire_binding(),
                    },
                ],
            });

            self.blur_intermediate_2
                .replace(Some(BlurIntermediateTexture {
                    view: view_2,
                    bind_group: bind_group_2,
                    viewport_buffer: viewport_buffer_2,
                    width,
                    height,
                }));
        }
    }

    /// Render the video primitive.
    ///
    /// # Arguments
    /// * `video_id` - Unique identifier for the video source
    /// * `filter_mode` - Filter to apply (0 = none, 1+ = various filters)
    /// * `encoder` - GPU command encoder
    /// * `target` - Render target texture view
    /// * `clip_bounds` - Clipped bounds for scissor rect (visible portion after scroll clipping)
    /// * `widget_bounds` - Full widget bounds for viewport (x, y, width, height)
    pub fn render(
        &self,
        video_id: u64,
        filter_mode: u32,
        encoder: &mut wgpu::CommandEncoder,
        target: &wgpu::TextureView,
        clip_bounds: &Rectangle<u32>,
        widget_bounds: (f32, f32, f32, f32),
    ) {
        // Look up binding for this (video_id, filter_mode) combination
        let binding_key = (video_id, filter_mode);
        if let Some(binding) = self.bindings.get(&binding_key) {
            // Skip rendering if clip bounds are empty
            if clip_bounds.width == 0 || clip_bounds.height == 0 {
                return;
            }

            // Video ID 1 is used for blurred transition frames with 3-pass blur
            if video_id == 1 {
                // 3-PASS BLUR for transition frames
                let intermediate_1_opt = self.blur_intermediate_1.borrow();
                let intermediate_2_opt = self.blur_intermediate_2.borrow();

                if intermediate_1_opt.is_none() || intermediate_2_opt.is_none() {
                    // Fallback to single-pass if intermediates aren't ready
                    let mut render_pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                        label: Some("camera video render pass fallback"),
                        color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                            view: target,
                            resolve_target: None,
                            ops: wgpu::Operations {
                                load: wgpu::LoadOp::Load,
                                store: wgpu::StoreOp::Store,
                            },
                        })],
                        depth_stencil_attachment: None,
                        timestamp_writes: None,
                        occlusion_query_set: None,
                    });

                    // Use full widget bounds for viewport (prevents distortion in scrollables)
                    render_pass.set_viewport(
                        widget_bounds.0,
                        widget_bounds.1,
                        widget_bounds.2,
                        widget_bounds.3,
                        0.0,
                        1.0,
                    );

                    // Use clip bounds for scissor (clips to visible portion)
                    render_pass.set_scissor_rect(
                        clip_bounds.x,
                        clip_bounds.y,
                        clip_bounds.width,
                        clip_bounds.height,
                    );

                    render_pass.set_pipeline(&self.pipeline_rgb_blur);
                    render_pass.set_bind_group(0, &binding.bind_group, &[]);
                    render_pass.draw(0..6, 0..1);
                    return;
                }

                let intermediate_1 = intermediate_1_opt.as_ref().unwrap();
                let intermediate_2 = intermediate_2_opt.as_ref().unwrap();

                // Pass 1: RGBA blur to intermediate texture 1
                {
                    let mut render_pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                        label: Some("camera blur pass 1"),
                        color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                            view: &intermediate_1.view,
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

                    render_pass.set_pipeline(&self.pipeline_rgb_blur);
                    render_pass.set_bind_group(0, &binding.bind_group, &[]);
                    render_pass.draw(0..6, 0..1);
                }

                // Pass 2: RGB blur from intermediate 1 to intermediate 2
                {
                    let mut render_pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                        label: Some("camera blur pass 2"),
                        color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                            view: &intermediate_2.view,
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

                    render_pass.set_pipeline(&self.pipeline_rgb_blur);
                    render_pass.set_bind_group(0, &intermediate_1.bind_group, &[]);
                    render_pass.draw(0..6, 0..1);
                }

                // Pass 3: RGB blur from intermediate 2 to final target
                {
                    let mut render_pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                        label: Some("camera blur pass 3"),
                        color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                            view: target,
                            resolve_target: None,
                            ops: wgpu::Operations {
                                load: wgpu::LoadOp::Load,
                                store: wgpu::StoreOp::Store,
                            },
                        })],
                        depth_stencil_attachment: None,
                        timestamp_writes: None,
                        occlusion_query_set: None,
                    });

                    // Use full widget bounds for viewport (prevents distortion in scrollables)
                    render_pass.set_viewport(
                        widget_bounds.0,
                        widget_bounds.1,
                        widget_bounds.2,
                        widget_bounds.3,
                        0.0,
                        1.0,
                    );

                    // Use clip bounds for scissor (clips to visible portion)
                    render_pass.set_scissor_rect(
                        clip_bounds.x,
                        clip_bounds.y,
                        clip_bounds.width,
                        clip_bounds.height,
                    );

                    render_pass.set_pipeline(&self.pipeline_rgb_blur);
                    render_pass.set_bind_group(0, &intermediate_2.bind_group, &[]);
                    render_pass.draw(0..6, 0..1);
                }
            } else {
                // Single-pass RGBA rendering for live preview
                let mut render_pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some("camera video render pass"),
                    color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                        view: target,
                        resolve_target: None,
                        ops: wgpu::Operations {
                            load: wgpu::LoadOp::Load,
                            store: wgpu::StoreOp::Store,
                        },
                    })],
                    depth_stencil_attachment: None,
                    timestamp_writes: None,
                    occlusion_query_set: None,
                });

                // Use full widget bounds for viewport (prevents distortion in scrollables)
                render_pass.set_viewport(
                    widget_bounds.0,
                    widget_bounds.1,
                    widget_bounds.2,
                    widget_bounds.3,
                    0.0,
                    1.0,
                );

                // Use clip bounds for scissor (clips to visible portion)
                render_pass.set_scissor_rect(
                    clip_bounds.x,
                    clip_bounds.y,
                    clip_bounds.width,
                    clip_bounds.height,
                );

                render_pass.set_pipeline(&self.pipeline_rgba);
                render_pass.set_bind_group(0, &binding.bind_group, &[]);
                render_pass.draw(0..6, 0..1);
            }
        }
    }
}
