// SPDX-License-Identifier: GPL-3.0-only

//! GPU mesh processor for 3D depth visualization
//!
//! Takes depth + RGB data and renders a triangulated 3D mesh using grid-based
//! triangulation with depth discontinuity handling.

use crate::gpu::{self, wgpu};
use crate::gpu_processor_singleton;
use crate::shaders::common::Render3DParams;
use crate::shaders::compute_dispatch_size;
use crate::shaders::gpu_utils;
use crate::shaders::kinect_intrinsics as kinect;
use crate::shaders::point_cloud::DepthFormat;
use std::sync::Arc;
use tracing::{debug, info, warn};

// Mesh uses Render3DParams from common::params

/// Result of mesh rendering
pub struct MeshResult {
    /// Rendered RGBA image
    pub rgba: Vec<u8>,
    /// Width of output image
    pub width: u32,
    /// Height of output image
    pub height: u32,
}

/// GPU mesh processor
pub struct MeshProcessor {
    device: Arc<wgpu::Device>,
    queue: Arc<wgpu::Queue>,
    render_pipeline: wgpu::ComputePipeline,
    clear_pipeline: wgpu::ComputePipeline,
    bind_group_layout: wgpu::BindGroupLayout,
    uniform_buffer: wgpu::Buffer,
    // Cached resources
    cached_input_width: u32,
    cached_input_height: u32,
    cached_output_width: u32,
    cached_output_height: u32,
    cached_rgb_width: u32,
    cached_rgb_height: u32,
    rgb_buffer: Option<wgpu::Buffer>,
    depth_buffer: Option<wgpu::Buffer>,
    depth_test_buffer: Option<wgpu::Buffer>,
    output_texture: Option<wgpu::Texture>,
    staging_buffer: Option<wgpu::Buffer>,
    // Registration data buffers (None = using dummy buffers)
    registration_table_buffer: Option<wgpu::Buffer>,
    depth_to_rgb_shift_buffer: Option<wgpu::Buffer>,
    registration_target_offset: u32,
    has_registration_data: bool,
    // Dummy buffers for when no registration data is available
    dummy_reg_buffer: wgpu::Buffer,
    dummy_shift_buffer: wgpu::Buffer,
}

impl MeshProcessor {
    /// Create a new GPU mesh processor
    pub async fn new() -> Result<Self, String> {
        info!("Initializing GPU mesh processor");

        let (device, queue, gpu_info) = gpu::create_low_priority_compute_device("mesh_gpu").await?;

        info!(
            adapter_name = %gpu_info.adapter_name,
            adapter_backend = ?gpu_info.backend,
            low_priority = gpu_info.low_priority_enabled,
            "GPU device created for mesh processing"
        );

        // Create shader module (using concatenated shared geometry + main shader)
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("mesh_shader"),
            source: wgpu::ShaderSource::Wgsl(super::mesh_shader().into()),
        });

        // Create bind group layout (using shared depth processor layout)
        let bind_group_layout =
            gpu_utils::create_depth_processor_bind_group_layout(&device, "mesh_bind_group_layout");

        // Create pipeline layout
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("mesh_pipeline_layout"),
            bind_group_layouts: &[&bind_group_layout],
            push_constant_ranges: &[],
        });

        // Create render pipeline
        let render_pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some("mesh_render_pipeline"),
            layout: Some(&pipeline_layout),
            module: &shader,
            entry_point: Some("main"),
            compilation_options: Default::default(),
            cache: None,
        });

        // Create clear pipeline
        let clear_pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some("mesh_clear_pipeline"),
            layout: Some(&pipeline_layout),
            module: &shader,
            entry_point: Some("clear_buffers"),
            compilation_options: Default::default(),
            cache: None,
        });

        // Create uniform buffer
        let uniform_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("mesh_uniform_buffer"),
            size: std::mem::size_of::<Render3DParams>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        // Create dummy buffers for when no registration data is available
        let dummy_reg_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("mesh_dummy_reg_buffer"),
            size: 8,
            usage: wgpu::BufferUsages::STORAGE,
            mapped_at_creation: false,
        });
        let dummy_shift_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("mesh_dummy_shift_buffer"),
            size: 8,
            usage: wgpu::BufferUsages::STORAGE,
            mapped_at_creation: false,
        });

        Ok(Self {
            device,
            queue,
            render_pipeline,
            clear_pipeline,
            bind_group_layout,
            uniform_buffer,
            cached_input_width: 0,
            cached_input_height: 0,
            cached_output_width: 0,
            cached_output_height: 0,
            cached_rgb_width: 0,
            cached_rgb_height: 0,
            rgb_buffer: None,
            depth_buffer: None,
            depth_test_buffer: None,
            output_texture: None,
            staging_buffer: None,
            registration_table_buffer: None,
            depth_to_rgb_shift_buffer: None,
            registration_target_offset: 0,
            has_registration_data: false,
            dummy_reg_buffer,
            dummy_shift_buffer,
        })
    }

    /// Ensure resources are allocated for given dimensions
    fn ensure_resources(
        &mut self,
        rgb_width: u32,
        rgb_height: u32,
        depth_width: u32,
        depth_height: u32,
        output_width: u32,
        output_height: u32,
    ) {
        let rgb_changed =
            self.cached_rgb_width != rgb_width || self.cached_rgb_height != rgb_height;
        let depth_changed =
            self.cached_input_width != depth_width || self.cached_input_height != depth_height;
        let output_changed =
            self.cached_output_width != output_width || self.cached_output_height != output_height;

        if !rgb_changed && !depth_changed && !output_changed {
            return;
        }

        let rgb_pixels = (rgb_width * rgb_height) as u64;
        let depth_pixels = (depth_width * depth_height) as u64;
        let output_pixels = (output_width * output_height) as u64;

        if rgb_changed {
            debug!(rgb_width, rgb_height, "Allocating mesh RGB buffer");

            self.rgb_buffer = Some(self.device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("mesh_rgb_buffer"),
                size: rgb_pixels * 4,
                usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            }));

            self.cached_rgb_width = rgb_width;
            self.cached_rgb_height = rgb_height;
        }

        if depth_changed {
            debug!(depth_width, depth_height, "Allocating mesh depth buffer");

            self.depth_buffer = Some(self.device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("mesh_depth_buffer"),
                size: depth_pixels * 4,
                usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            }));

            self.cached_input_width = depth_width;
            self.cached_input_height = depth_height;
        }

        if output_changed {
            debug!(
                output_width,
                output_height, "Allocating mesh output resources"
            );

            self.output_texture = Some(self.device.create_texture(&wgpu::TextureDescriptor {
                label: Some("mesh_output_texture"),
                size: wgpu::Extent3d {
                    width: output_width,
                    height: output_height,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::Rgba8Unorm,
                usage: wgpu::TextureUsages::STORAGE_BINDING | wgpu::TextureUsages::COPY_SRC,
                view_formats: &[],
            }));

            self.depth_test_buffer = Some(self.device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("mesh_depth_test_buffer"),
                size: output_pixels * 4,
                usage: wgpu::BufferUsages::STORAGE,
                mapped_at_creation: false,
            }));

            self.staging_buffer = Some(self.device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("mesh_staging_buffer"),
                size: output_pixels * 4,
                usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
                mapped_at_creation: false,
            }));

            self.cached_output_width = output_width;
            self.cached_output_height = output_height;
        }
    }

    /// Set registration data for depth-to-RGB alignment
    pub fn set_registration_data(&mut self, data: &crate::shaders::point_cloud::RegistrationData) {
        let buffers =
            gpu_utils::create_registration_buffers(&self.device, &self.queue, data, "mesh");

        self.registration_table_buffer = Some(buffers.table_buffer);
        self.depth_to_rgb_shift_buffer = Some(buffers.shift_buffer);
        self.registration_target_offset = buffers.target_offset;
        self.has_registration_data = true;

        debug!("Mesh registration data uploaded to GPU");
    }

    /// Check if registration data has been set
    pub fn has_registration_data(&self) -> bool {
        self.has_registration_data
    }

    /// Render mesh from depth + RGB data
    #[allow(clippy::too_many_arguments)]
    pub async fn render(
        &mut self,
        rgb_data: &[u8],
        depth_data: &[u16],
        rgb_width: u32,
        rgb_height: u32,
        depth_width: u32,
        depth_height: u32,
        output_width: u32,
        output_height: u32,
        pitch: f32,
        yaw: f32,
        zoom: f32,
        depth_format: DepthFormat,
        mirror: bool,
        apply_rgb_registration: bool,
        depth_discontinuity_threshold: f32,
        filter_mode: u32,
    ) -> Result<MeshResult, String> {
        self.ensure_resources(
            rgb_width,
            rgb_height,
            depth_width,
            depth_height,
            output_width,
            output_height,
        );

        let input_width = depth_width;
        let input_height = depth_height;

        let rgb_buffer = self.rgb_buffer.as_ref().ok_or("RGB buffer not allocated")?;
        let depth_buffer = self
            .depth_buffer
            .as_ref()
            .ok_or("Depth buffer not allocated")?;
        let depth_test_buffer = self
            .depth_test_buffer
            .as_ref()
            .ok_or("Depth test buffer not allocated")?;
        let output_texture = self
            .output_texture
            .as_ref()
            .ok_or("Output texture not allocated")?;
        let staging_buffer = self
            .staging_buffer
            .as_ref()
            .ok_or("Staging buffer not allocated")?;

        // Validate data sizes
        let expected_rgb_size = (rgb_width * rgb_height * 4) as usize;
        let expected_depth_size = (depth_width * depth_height) as usize;

        if rgb_data.len() != expected_rgb_size {
            warn!(
                actual = rgb_data.len(),
                expected = expected_rgb_size,
                rgb_width,
                rgb_height,
                "RGB data size mismatch - this may cause rendering issues"
            );
        }

        if depth_data.len() != expected_depth_size {
            warn!(
                actual = depth_data.len(),
                expected = expected_depth_size,
                depth_width,
                depth_height,
                "Depth data size mismatch - this may cause rendering issues"
            );
        }

        // Upload RGB data
        self.queue.write_buffer(rgb_buffer, 0, rgb_data);

        // Upload depth data (convert u16 to u32)
        let depth_u32: Vec<u32> = depth_data.iter().map(|&d| d as u32).collect();
        self.queue
            .write_buffer(depth_buffer, 0, bytemuck::cast_slice(&depth_u32));

        // Calculate camera intrinsics scaled for input resolution
        let scale_x = input_width as f32 / 640.0;
        let scale_y = input_height as f32 / 480.0;

        // Calculate registration scale factors for high-res RGB
        // Registration tables are built for 640x480 RGB. For 1280x1024:
        // - The 640x480 comes from 1280x1024 cropped to 1280x960, then scaled by 0.5
        // - So to map back: scale by 2.0 (not 2.133), top-aligned (offset=0)
        let reg_scale_x = rgb_width as f32 / 640.0;
        let reg_scale_y = reg_scale_x; // Same as X to maintain aspect ratio
        let reg_y_offset = 0i32;

        // Update uniform buffer with parameters
        let params = Render3DParams {
            input_width,
            input_height,
            output_width,
            output_height,
            rgb_width,
            rgb_height,
            fx: kinect::FX * scale_x,
            fy: kinect::FY * scale_y,
            cx: kinect::CX * scale_x,
            cy: kinect::CY * scale_y,
            depth_format: depth_format as u32,
            depth_coeff_a: kinect::DEPTH_COEFF_A,
            depth_coeff_b: kinect::DEPTH_COEFF_B,
            min_depth: 0.4,
            max_depth: 4.0,
            pitch,
            yaw,
            fov: 1.0,
            view_distance: zoom,
            use_registration_tables: if self.has_registration_data && apply_rgb_registration {
                1
            } else {
                0
            },
            target_offset: self.registration_target_offset,
            reg_x_val_scale: 256,
            mirror: if mirror { 1 } else { 0 },
            reg_scale_x,
            reg_scale_y,
            reg_y_offset,
            // Mode-specific parameters
            point_size: 0.0, // Not used for mesh
            depth_discontinuity_threshold,
            filter_mode,
        };
        self.queue
            .write_buffer(&self.uniform_buffer, 0, bytemuck::bytes_of(&params));

        // Create bind group
        let output_view = output_texture.create_view(&wgpu::TextureViewDescriptor::default());

        // Get registration buffers or use pre-allocated dummy buffers
        let (reg_table_buffer, shift_buffer) = if self.has_registration_data {
            (
                self.registration_table_buffer.as_ref().unwrap(),
                self.depth_to_rgb_shift_buffer.as_ref().unwrap(),
            )
        } else {
            (&self.dummy_reg_buffer, &self.dummy_shift_buffer)
        };

        let bind_group = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("mesh_bind_group"),
            layout: &self.bind_group_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: rgb_buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: depth_buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::TextureView(&output_view),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: depth_test_buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 4,
                    resource: self.uniform_buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 5,
                    resource: reg_table_buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 6,
                    resource: shift_buffer.as_entire_binding(),
                },
            ],
        });

        // Create and submit command buffer
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("mesh_encoder"),
            });

        // Pass 1: Clear buffers and output
        {
            let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some("mesh_clear_pass"),
                timestamp_writes: None,
            });
            pass.set_pipeline(&self.clear_pipeline);
            pass.set_bind_group(0, Some(&bind_group), &[]);
            let workgroups_x = compute_dispatch_size(output_width, 16);
            let workgroups_y = compute_dispatch_size(output_height, 16);
            pass.dispatch_workgroups(workgroups_x, workgroups_y, 1);
        }

        // Pass 2: Render mesh
        {
            let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some("mesh_render_pass"),
                timestamp_writes: None,
            });
            pass.set_pipeline(&self.render_pipeline);
            pass.set_bind_group(0, Some(&bind_group), &[]);
            let workgroups_x = compute_dispatch_size(input_width, 16);
            let workgroups_y = compute_dispatch_size(input_height, 16);
            pass.dispatch_workgroups(workgroups_x, workgroups_y, 1);
        }

        // Copy output to staging buffer
        encoder.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo {
                texture: output_texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::TexelCopyBufferInfo {
                buffer: staging_buffer,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(output_width * 4),
                    rows_per_image: Some(output_height),
                },
            },
            wgpu::Extent3d {
                width: output_width,
                height: output_height,
                depth_or_array_layers: 1,
            },
        );

        self.queue.submit(std::iter::once(encoder.finish()));

        // Map and read back using shared helper
        let rgba = crate::shaders::gpu_processor::read_buffer_async(&self.device, &staging_buffer).await?;

        Ok(MeshResult {
            rgba,
            width: output_width,
            height: output_height,
        })
    }
}

// Use the shared singleton macro for GPU processor management
gpu_processor_singleton!(MeshProcessor, GPU_MESH_PROCESSOR, get_mesh_processor);

/// Set registration data for depth-to-RGB alignment on the shared GPU mesh processor
pub async fn set_mesh_registration_data(
    data: &crate::shaders::point_cloud::RegistrationData,
) -> Result<(), String> {
    let mut guard = get_mesh_processor().await?;
    let processor = guard.as_mut().ok_or("GPU mesh processor not initialized")?;

    processor.set_registration_data(data);
    Ok(())
}

/// Render mesh using the shared GPU processor
#[allow(clippy::too_many_arguments)]
pub async fn render_mesh(
    rgb_data: &[u8],
    depth_data: &[u16],
    rgb_width: u32,
    rgb_height: u32,
    depth_width: u32,
    depth_height: u32,
    output_width: u32,
    output_height: u32,
    pitch: f32,
    yaw: f32,
    zoom: f32,
    depth_format: DepthFormat,
    mirror: bool,
    apply_rgb_registration: bool,
    depth_discontinuity_threshold: f32,
    filter_mode: u32,
) -> Result<MeshResult, String> {
    let mut guard = get_mesh_processor().await?;
    let processor = guard.as_mut().ok_or("GPU mesh processor not initialized")?;

    processor
        .render(
            rgb_data,
            depth_data,
            rgb_width,
            rgb_height,
            depth_width,
            depth_height,
            output_width,
            output_height,
            pitch,
            yaw,
            zoom,
            depth_format,
            mirror,
            apply_rgb_registration,
            depth_discontinuity_threshold,
            filter_mode,
        )
        .await
}
