// SPDX-License-Identifier: GPL-3.0-only

//! Custom primitive for rendering gallery thumbnails with rounded corners

use std::sync::Arc;

use cosmic::iced::Rectangle;
use cosmic::iced_core::image::Id as ImageId;
use cosmic::iced_wgpu::graphics::Viewport;
use cosmic::iced_wgpu::primitive::{self, Primitive as PrimitiveTrait};
use cosmic::iced_wgpu::wgpu;

/// Custom primitive for gallery thumbnail with rounded corners
#[derive(Debug, Clone)]
pub struct GalleryPrimitive {
    pub image_handle: cosmic::widget::image::Handle,
    /// RGBA data wrapped in Arc for cheap cloning (avoids copying image buffer every frame)
    pub rgba_data: Arc<Vec<u8>>,
    pub width: u32,
    pub height: u32,
    pub corner_radius: f32,
}

impl GalleryPrimitive {
    pub fn new(
        image_handle: cosmic::widget::image::Handle,
        rgba_data: Arc<Vec<u8>>,
        width: u32,
        height: u32,
        corner_radius: f32,
    ) -> Self {
        Self {
            image_handle,
            rgba_data,
            width,
            height,
            corner_radius,
        }
    }
}

/// Pipeline for rendering gallery thumbnails
pub struct GalleryPipeline {
    pipeline: wgpu::RenderPipeline,
    bind_group_layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
    texture_format: wgpu::TextureFormat,
    // Cache for uploaded textures
    texture_cache: std::collections::HashMap<ImageId, GalleryTexture>,
}

struct GalleryTexture {
    bind_group: wgpu::BindGroup,
    viewport_buffer: wgpu::Buffer,
}

impl PrimitiveTrait for GalleryPrimitive {
    fn prepare(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        format: wgpu::TextureFormat,
        storage: &mut primitive::Storage,
        bounds: &Rectangle,
        _viewport: &Viewport,
    ) {
        // Get or create pipeline
        if !storage.has::<GalleryPipeline>() {
            storage.store(GalleryPipeline::new(device, format));
        }

        // Upload image texture if needed and update viewport buffer
        if let Some(pipeline) = storage.get_mut::<GalleryPipeline>() {
            pipeline.upload_image(
                device,
                queue,
                &self.image_handle,
                &self.rgba_data,
                self.width,
                self.height,
            );

            // Update viewport size and corner radius for this image
            pipeline.update_viewport(
                queue,
                &self.image_handle,
                bounds.width,
                bounds.height,
                self.corner_radius,
            );
        }
    }

    fn render(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        storage: &primitive::Storage,
        target: &wgpu::TextureView,
        clip_bounds: &Rectangle<u32>,
    ) {
        if let Some(pipeline) = storage.get::<GalleryPipeline>() {
            pipeline.render(encoder, target, clip_bounds, &self.image_handle);
        }
    }
}

impl GalleryPipeline {
    pub fn new(device: &wgpu::Device, format: wgpu::TextureFormat) -> Self {
        // Load shader
        let shader_source = include_str!("gallery_rounded_shader.wgsl");
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("gallery rounded shader"),
            source: wgpu::ShaderSource::Wgsl(shader_source.into()),
        });

        // Create bind group layout
        let bind_group_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("gallery bind group layout"),
            entries: &[
                // Texture
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
                // Viewport size uniform
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

        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("gallery pipeline layout"),
            bind_group_layouts: &[&bind_group_layout],
            push_constant_ranges: &[],
        });

        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("gallery pipeline"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
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
                module: &shader,
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

        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("gallery sampler"),
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            address_mode_w: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::FilterMode::Nearest,
            ..Default::default()
        });

        Self {
            pipeline,
            bind_group_layout,
            sampler,
            texture_format: format,
            texture_cache: std::collections::HashMap::new(),
        }
    }

    fn upload_image(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        image_handle: &cosmic::widget::image::Handle,
        rgba_data: &[u8],
        width: u32,
        height: u32,
    ) {
        // Get a unique ID for this image
        let image_id = image_handle.id();

        // Check if already uploaded
        if self.texture_cache.contains_key(&image_id) {
            return;
        }

        // Clear stale textures - since there's only one gallery thumbnail,
        // remove any old textures before uploading the new one
        self.texture_cache.clear();

        // Use the provided RGBA data directly
        if rgba_data.is_empty() {
            tracing::warn!("Empty RGBA data for gallery thumbnail");
            return;
        }

        // Convert RGBA to BGRA if needed
        let texture_data = if matches!(
            self.texture_format,
            wgpu::TextureFormat::Bgra8Unorm | wgpu::TextureFormat::Bgra8UnormSrgb
        ) {
            // Convert RGBA -> BGRA by swapping R and B channels
            let mut bgra_data = rgba_data.to_vec();
            for chunk in bgra_data.chunks_exact_mut(4) {
                chunk.swap(0, 2); // Swap R and B
            }
            bgra_data
        } else {
            rgba_data.to_vec()
        };

        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("gallery texture"),
            size: wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: self.texture_format,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });

        // Upload the image data
        queue.write_texture(
            wgpu::ImageCopyTexture {
                texture: &texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            &texture_data,
            wgpu::ImageDataLayout {
                offset: 0,
                bytes_per_row: Some(4 * width),
                rows_per_image: None,
            },
            wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
        );

        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());

        // Create viewport params buffer (will be updated during render)
        // Contains: size (2 floats), corner_radius (1 float), padding (1 float)
        let viewport_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("gallery viewport buffer"),
            size: std::mem::size_of::<[f32; 4]>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        // Initialize with default 40x40 size and 8.0 corner radius
        let viewport_data = [40.0f32, 40.0f32, 8.0f32, 0.0f32];
        queue.write_buffer(&viewport_buffer, 0, bytemuck::cast_slice(&viewport_data));

        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("gallery bind group"),
            layout: &self.bind_group_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&view),
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

        self.texture_cache.insert(
            image_id,
            GalleryTexture {
                bind_group,
                viewport_buffer,
            },
        );
    }

    fn update_viewport(
        &mut self,
        queue: &wgpu::Queue,
        image_handle: &cosmic::widget::image::Handle,
        width: f32,
        height: f32,
        corner_radius: f32,
    ) {
        let image_id = image_handle.id();

        if let Some(gallery_texture) = self.texture_cache.get(&image_id) {
            // Pack viewport params: size (2 floats), corner_radius (1 float), padding (1 float)
            let viewport_data = [width, height, corner_radius, 0.0f32];
            queue.write_buffer(
                &gallery_texture.viewport_buffer,
                0,
                bytemuck::cast_slice(&viewport_data),
            );
        }
    }

    pub fn render(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        target: &wgpu::TextureView,
        clip_bounds: &Rectangle<u32>,
        image_handle: &cosmic::widget::image::Handle,
    ) {
        let image_id = image_handle.id();

        if let Some(gallery_texture) = self.texture_cache.get(&image_id) {
            if clip_bounds.width == 0 || clip_bounds.height == 0 {
                return;
            }

            let mut render_pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("gallery render pass"),
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

            render_pass.set_viewport(
                clip_bounds.x as f32,
                clip_bounds.y as f32,
                clip_bounds.width as f32,
                clip_bounds.height as f32,
                0.0,
                1.0,
            );

            render_pass.set_scissor_rect(
                clip_bounds.x,
                clip_bounds.y,
                clip_bounds.width,
                clip_bounds.height,
            );

            render_pass.set_pipeline(&self.pipeline);
            render_pass.set_bind_group(0, &gallery_texture.bind_group, &[]);
            render_pass.draw(0..6, 0..1);
        }
    }
}
