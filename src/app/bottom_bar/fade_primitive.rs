// SPDX-License-Identifier: GPL-3.0-only

//! GPU-accelerated gradient fade primitive for carousel edge overlays.
//!
//! Uses a wgpu shader to render a pixel-perfect smooth gradient in a single
//! draw call, from an opaque color at one edge to fully transparent at the other.
//!
//! Two bind groups are maintained (one per direction) to avoid the shared
//! uniform buffer overwrite issue when multiple primitives draw per frame.

use cosmic::iced::Rectangle;
use cosmic::iced_wgpu::graphics::Viewport;
use cosmic::iced_wgpu::primitive::{Pipeline, Primitive};
use cosmic::iced_wgpu::wgpu;

/// Uniform data sent to the fade shader.
#[repr(C)]
#[derive(Debug, Copy, Clone, bytemuck::Pod, bytemuck::Zeroable)]
struct FadeUniform {
    color: [f32; 4],
    size: [f32; 2],
    corner_radius: f32,
    direction: u32,
}

/// Per-direction GPU resources.
struct DirectionBinding {
    uniform_buffer: wgpu::Buffer,
    bind_group: wgpu::BindGroup,
}

/// GPU pipeline for the fade gradient shader.
pub struct FadePipeline {
    pipeline: wgpu::RenderPipeline,
    /// One binding per direction (0 and 1)
    bindings: [DirectionBinding; 2],
}

impl Pipeline for FadePipeline {
    fn new(device: &wgpu::Device, _queue: &wgpu::Queue, format: wgpu::TextureFormat) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("carousel fade shader"),
            source: wgpu::ShaderSource::Wgsl(include_str!("fade_shader.wgsl").into()),
        });

        let bind_group_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("fade bind group layout"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            }],
        });

        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("fade pipeline layout"),
            bind_group_layouts: &[&bind_group_layout],
            push_constant_ranges: &[],
        });

        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("fade pipeline"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_main"),
                buffers: &[],
                compilation_options: Default::default(),
            },
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_main"),
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

        let create_binding = |label: &str| -> DirectionBinding {
            let uniform_buffer = device.create_buffer(&wgpu::BufferDescriptor {
                label: Some(label),
                size: std::mem::size_of::<FadeUniform>() as u64,
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
            let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some(label),
                layout: &bind_group_layout,
                entries: &[wgpu::BindGroupEntry {
                    binding: 0,
                    resource: uniform_buffer.as_entire_binding(),
                }],
            });
            DirectionBinding {
                uniform_buffer,
                bind_group,
            }
        };

        let bindings = [
            create_binding("fade uniform dir0"),
            create_binding("fade uniform dir1"),
        ];

        FadePipeline { pipeline, bindings }
    }

    fn trim(&mut self) {}
}

/// The fade gradient primitive. Each instance represents one edge overlay.
#[derive(Debug, Clone)]
pub struct FadePrimitive {
    /// RGBA color to fade from (at opaque edge)
    pub color: [f32; 4],
    /// 0 = opaque at left, transparent at right
    /// 1 = transparent at left, opaque at right
    pub direction: u32,
    /// Corner radius in logical pixels
    pub corner_radius: f32,
}

impl Primitive for FadePrimitive {
    type Pipeline = FadePipeline;

    fn prepare(
        &self,
        pipeline: &mut Self::Pipeline,
        _device: &wgpu::Device,
        queue: &wgpu::Queue,
        bounds: &Rectangle,
        viewport: &Viewport,
    ) {
        let scale = viewport.scale_factor() as f32;
        let uniform = FadeUniform {
            color: self.color,
            size: [bounds.width * scale, bounds.height * scale],
            corner_radius: self.corner_radius * scale,
            direction: self.direction,
        };
        let idx = self.direction.min(1) as usize;
        queue.write_buffer(
            &pipeline.bindings[idx].uniform_buffer,
            0,
            bytemuck::cast_slice(&[uniform]),
        );
    }

    fn draw(&self, pipeline: &Self::Pipeline, render_pass: &mut wgpu::RenderPass<'_>) -> bool {
        let idx = self.direction.min(1) as usize;
        render_pass.set_pipeline(&pipeline.pipeline);
        render_pass.set_bind_group(0, Some(&pipeline.bindings[idx].bind_group), &[]);
        render_pass.draw(0..6, 0..1);
        true
    }
}
