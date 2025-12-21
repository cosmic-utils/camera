// SPDX-License-Identifier: GPL-3.0-only

//! GPU depth processor for Y10B format unpacking
//!
//! Uses WGPU compute shaders to efficiently unpack Y10B depth data from
//! the Kinect depth sensor into:
//! - RGBA preview images for display
//! - 16-bit depth values for lossless storage

use crate::gpu::{self, wgpu};
use crate::gpu_processor_singleton;
use crate::shaders::{CachedDimensions, compute_dispatch_size};
use std::sync::Arc;
use tracing::{debug, info, warn};

// Import depth format constants from freedepth
use freedepth::{DEPTH_10BIT_NO_VALUE, y10b_packed_size};

/// Depth processing parameters
#[repr(C)]
#[derive(Debug, Copy, Clone, bytemuck::Pod, bytemuck::Zeroable)]
struct DepthParams {
    width: u32,
    height: u32,
    min_depth: u32,
    max_depth: u32,
    use_colormap: u32, // 0 = grayscale, 1 = turbo colormap
    depth_only: u32,   // 0 = normal, 1 = depth-only mode (always use colormap)
}

/// Result of depth unpacking containing both preview and depth data
pub struct DepthUnpackResult {
    /// RGBA data for preview display (width * height * 4 bytes)
    pub rgba_preview: Vec<u8>,
    /// 16-bit depth values for lossless storage (width * height values)
    pub depth_u16: Vec<u16>,
    /// Frame width
    pub width: u32,
    /// Frame height
    pub height: u32,
}

/// GPU depth processor for Y10B format
pub struct DepthProcessor {
    device: Arc<wgpu::Device>,
    queue: Arc<wgpu::Queue>,
    pipeline: wgpu::ComputePipeline,
    bind_group_layout: wgpu::BindGroupLayout,
    uniform_buffer: wgpu::Buffer,
    // Cached resources for current dimensions
    cached_dims: CachedDimensions,
    input_buffer: Option<wgpu::Buffer>,
    output_rgba_texture: Option<wgpu::Texture>,
    output_depth_buffer: Option<wgpu::Buffer>,
    staging_rgba_buffer: Option<wgpu::Buffer>,
    staging_depth_buffer: Option<wgpu::Buffer>,
}

impl DepthProcessor {
    /// Create a new GPU depth processor
    pub async fn new() -> Result<Self, String> {
        info!("Initializing GPU depth processor for Y10B format");

        // Create device with low-priority queue
        let (device, queue, gpu_info) =
            gpu::create_low_priority_compute_device("depth_processor_gpu").await?;

        info!(
            adapter_name = %gpu_info.adapter_name,
            adapter_backend = ?gpu_info.backend,
            low_priority = gpu_info.low_priority_enabled,
            "GPU device created for depth processing"
        );

        // Create shader
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("y10b_unpack_shader"),
            source: wgpu::ShaderSource::Wgsl(super::Y10B_UNPACK_SHADER.into()),
        });

        // Create bind group layout
        let bind_group_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("depth_bind_group_layout"),
            entries: &[
                // Input bytes buffer (read as u32)
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Storage { read_only: true },
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                // Output RGBA texture
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
                // Output depth buffer (u32 containing 16-bit values)
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
                // Uniform parameters
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

        // Create pipeline layout
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("depth_pipeline_layout"),
            bind_group_layouts: &[&bind_group_layout],
            push_constant_ranges: &[],
        });

        // Create compute pipeline
        let pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some("y10b_unpack_pipeline"),
            layout: Some(&pipeline_layout),
            module: &shader,
            entry_point: Some("main"),
            compilation_options: Default::default(),
            cache: None,
        });

        // Create uniform buffer
        let uniform_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("depth_uniform_buffer"),
            size: std::mem::size_of::<DepthParams>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        Ok(Self {
            device,
            queue,
            pipeline,
            bind_group_layout,
            uniform_buffer,
            cached_dims: CachedDimensions::default(),
            input_buffer: None,
            output_rgba_texture: None,
            output_depth_buffer: None,
            staging_rgba_buffer: None,
            staging_depth_buffer: None,
        })
    }

    /// Ensure resources are allocated for the given dimensions
    fn ensure_resources(&mut self, width: u32, height: u32) {
        if !self.cached_dims.needs_update(width, height) {
            return;
        }

        debug!(width, height, "Allocating depth processor resources");

        let pixel_count = (width * height) as u64;

        // Y10B input size from freedepth, rounded up to u32 alignment
        let y10b_size = y10b_packed_size(width, height) as u64;
        let input_size = ((y10b_size + 3) / 4) * 4; // Round up to u32 alignment

        // RGBA output size
        let rgba_size = pixel_count * 4;

        // Depth output size (u32 per pixel for alignment)
        let depth_size = pixel_count * 4;

        // Create input buffer
        self.input_buffer = Some(self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("depth_input_buffer"),
            size: input_size,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        }));

        // Create output RGBA texture
        self.output_rgba_texture = Some(self.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("depth_rgba_texture"),
            size: wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::STORAGE_BINDING | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        }));

        // Create output depth buffer
        self.output_depth_buffer = Some(self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("depth_output_buffer"),
            size: depth_size,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
            mapped_at_creation: false,
        }));

        // Create staging buffers for CPU readback
        self.staging_rgba_buffer = Some(self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("depth_staging_rgba"),
            size: rgba_size,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        }));

        self.staging_depth_buffer = Some(self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("depth_staging_depth"),
            size: depth_size,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        }));

        self.cached_dims.update(width, height);
    }

    /// Unpack Y10B data to RGBA preview and 16-bit depth
    ///
    /// # Arguments
    /// * `y10b_data` - Raw Y10B packed bytes from depth sensor
    /// * `width` - Frame width in pixels
    /// * `height` - Frame height in pixels
    /// * `min_depth` - Minimum depth value for visualization (default 0)
    /// * `max_depth` - Maximum depth value for visualization (default 1023)
    /// * `use_colormap` - Whether to apply turbo colormap (true) or grayscale (false)
    /// * `depth_only` - Whether to force colormap mode (depth-only visualization)
    pub async fn unpack(
        &mut self,
        y10b_data: &[u8],
        width: u32,
        height: u32,
        min_depth: u32,
        max_depth: u32,
        use_colormap: bool,
        depth_only: bool,
    ) -> Result<DepthUnpackResult, String> {
        self.ensure_resources(width, height);

        let input_buffer = self
            .input_buffer
            .as_ref()
            .ok_or("Input buffer not allocated")?;
        let output_rgba_texture = self
            .output_rgba_texture
            .as_ref()
            .ok_or("RGBA texture not allocated")?;
        let output_depth_buffer = self
            .output_depth_buffer
            .as_ref()
            .ok_or("Depth buffer not allocated")?;
        let staging_rgba_buffer = self
            .staging_rgba_buffer
            .as_ref()
            .ok_or("RGBA staging buffer not allocated")?;
        let staging_depth_buffer = self
            .staging_depth_buffer
            .as_ref()
            .ok_or("Depth staging buffer not allocated")?;

        // Calculate expected Y10B size for this resolution (from freedepth)
        let expected_y10b_size = y10b_packed_size(width, height);

        // Truncate input data to expected size if larger (device may add row padding)
        let actual_data = if y10b_data.len() > expected_y10b_size {
            debug!(
                got = y10b_data.len(),
                expected = expected_y10b_size,
                "Input data larger than expected, truncating"
            );
            &y10b_data[..expected_y10b_size]
        } else if y10b_data.len() < expected_y10b_size {
            warn!(
                got = y10b_data.len(),
                expected = expected_y10b_size,
                "Input data smaller than expected, depth unpacking may be incorrect"
            );
            y10b_data
        } else {
            y10b_data
        };

        // Upload Y10B data to input buffer (pad to 4-byte alignment if needed)
        let padded_data = if actual_data.len() % 4 != 0 {
            let padding = 4 - (actual_data.len() % 4);
            let mut padded = actual_data.to_vec();
            padded.extend(std::iter::repeat(0u8).take(padding));
            padded
        } else {
            actual_data.to_vec()
        };
        self.queue.write_buffer(input_buffer, 0, &padded_data);

        // Update uniform buffer
        let params = DepthParams {
            width,
            height,
            min_depth,
            max_depth,
            use_colormap: if use_colormap { 1 } else { 0 },
            depth_only: if depth_only { 1 } else { 0 },
        };
        self.queue
            .write_buffer(&self.uniform_buffer, 0, bytemuck::bytes_of(&params));

        // Create bind group
        let rgba_view = output_rgba_texture.create_view(&wgpu::TextureViewDescriptor::default());

        let bind_group = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("depth_bind_group"),
            layout: &self.bind_group_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: input_buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(&rgba_view),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: output_depth_buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: self.uniform_buffer.as_entire_binding(),
                },
            ],
        });

        // Create and submit command buffer
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("depth_encoder"),
            });

        {
            let mut compute_pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some("depth_compute_pass"),
                timestamp_writes: None,
            });

            compute_pass.set_pipeline(&self.pipeline);
            compute_pass.set_bind_group(0, Some(&bind_group), &[]);

            // Dispatch workgroups (16x16 threads per workgroup)
            let workgroups_x = compute_dispatch_size(width, 16);
            let workgroups_y = compute_dispatch_size(height, 16);
            compute_pass.dispatch_workgroups(workgroups_x, workgroups_y, 1);
        }

        // Copy RGBA texture to staging buffer
        let pixel_count = (width * height) as u64;
        encoder.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo {
                texture: output_rgba_texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::TexelCopyBufferInfo {
                buffer: staging_rgba_buffer,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(width * 4),
                    rows_per_image: Some(height),
                },
            },
            wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
        );

        // Copy depth buffer to staging
        encoder.copy_buffer_to_buffer(
            output_depth_buffer,
            0,
            staging_depth_buffer,
            0,
            pixel_count * 4,
        );

        self.queue.submit(std::iter::once(encoder.finish()));

        // Map and read back both buffers
        let rgba_slice = staging_rgba_buffer.slice(..);
        let depth_slice = staging_depth_buffer.slice(..);

        let (rgba_sender, rgba_receiver) = futures::channel::oneshot::channel();
        let (depth_sender, depth_receiver) = futures::channel::oneshot::channel();

        rgba_slice.map_async(wgpu::MapMode::Read, move |result| {
            let _ = rgba_sender.send(result);
        });
        depth_slice.map_async(wgpu::MapMode::Read, move |result| {
            let _ = depth_sender.send(result);
        });

        let _ = self.device.poll(wgpu::PollType::wait_indefinitely());

        rgba_receiver
            .await
            .map_err(|_| "Failed to receive RGBA buffer mapping")?
            .map_err(|e| format!("Failed to map RGBA buffer: {:?}", e))?;

        depth_receiver
            .await
            .map_err(|_| "Failed to receive depth buffer mapping")?
            .map_err(|e| format!("Failed to map depth buffer: {:?}", e))?;

        // Read RGBA data
        let rgba_preview = rgba_slice.get_mapped_range().to_vec();

        // Read depth data (stored as u32, extract lower 16 bits)
        let depth_data = depth_slice.get_mapped_range();
        let depth_u32: &[u32] = bytemuck::cast_slice(&depth_data);
        let depth_u16: Vec<u16> = depth_u32.iter().map(|&d| d as u16).collect();

        drop(depth_data);
        staging_rgba_buffer.unmap();
        staging_depth_buffer.unmap();

        Ok(DepthUnpackResult {
            rgba_preview,
            depth_u16,
            width,
            height,
        })
    }
}

// Use the shared singleton macro for GPU processor management
gpu_processor_singleton!(DepthProcessor, GPU_DEPTH_PROCESSOR, get_depth_processor);

/// Unpack Y10B data using the shared GPU processor
///
/// This is the main entry point for unpacking depth data.
///
/// # Arguments
/// * `y10b_data` - Raw Y10B packed bytes from depth sensor
/// * `width` - Frame width in pixels
/// * `height` - Frame height in pixels
/// * `use_colormap` - Whether to apply turbo colormap (true) or grayscale (false)
/// * `depth_only` - Whether to force colormap mode (depth-only visualization)
pub async fn unpack_y10b_gpu(
    y10b_data: &[u8],
    width: u32,
    height: u32,
    use_colormap: bool,
    depth_only: bool,
) -> Result<DepthUnpackResult, String> {
    let mut guard = get_depth_processor().await?;
    let processor = guard
        .as_mut()
        .ok_or("GPU depth processor not initialized")?;

    // Use full 10-bit range for visualization (0 to max valid value)
    processor
        .unpack(
            y10b_data,
            width,
            height,
            0,
            DEPTH_10BIT_NO_VALUE as u32,
            use_colormap,
            depth_only,
        )
        .await
}
