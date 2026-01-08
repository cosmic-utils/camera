// SPDX-License-Identifier: GPL-3.0-only
//! GPU-accelerated histogram analysis pipeline
//!
//! This module computes brightness metrics on the GPU using a histogram-based
//! approach. All histogram data stays on GPU; only the final metrics are
//! transferred to CPU.

use crate::gpu::{self, wgpu};
use std::sync::Arc;
use tracing::{debug, info, warn};

/// Brightness metrics computed from histogram
#[repr(C)]
#[derive(Debug, Copy, Clone, Default, bytemuck::Pod, bytemuck::Zeroable)]
pub struct BrightnessMetrics {
    /// Average luminance [0,1]
    pub mean_luminance: f32,
    /// Approximate median luminance [0,1]
    pub median_luminance: f32,
    /// 5th percentile luminance (shadow level) [0,1]
    pub percentile_5: f32,
    /// 95th percentile luminance (highlight level) [0,1]
    pub percentile_95: f32,
    /// Dynamic range in stops: log2(p95/p5)
    pub dynamic_range_stops: f32,
    /// Fraction of pixels in shadows (<0.1)
    pub shadow_fraction: f32,
    /// Fraction of pixels in highlights (>0.9)
    pub highlight_fraction: f32,
    /// Total pixel count
    pub total_pixels: u32,
}

/// Parameters uniform
#[repr(C)]
#[derive(Debug, Copy, Clone, bytemuck::Pod, bytemuck::Zeroable)]
struct Params {
    width: u32,
    height: u32,
    stage: u32,
    _padding: u32,
}

/// GPU histogram analysis pipeline
pub struct HistogramPipeline {
    device: Arc<wgpu::Device>,
    queue: Arc<wgpu::Queue>,
    histogram_pipeline: wgpu::ComputePipeline,
    reduce_pipeline: wgpu::ComputePipeline,
    bind_group_layout: wgpu::BindGroupLayout,
    uniform_buffer: wgpu::Buffer,
    // Cached resources for current dimensions
    cached_width: u32,
    cached_height: u32,
    input_texture: Option<wgpu::Texture>,
    histogram_buffer: Option<wgpu::Buffer>,
    metrics_buffer: Option<wgpu::Buffer>,
    staging_buffer: Option<wgpu::Buffer>,
}

impl HistogramPipeline {
    /// Create a new histogram analysis pipeline
    pub async fn new() -> Result<Self, String> {
        info!("Initializing GPU histogram pipeline");

        let (device, queue, gpu_info) =
            gpu::create_low_priority_compute_device("histogram_pipeline_gpu").await?;

        info!(
            adapter_name = %gpu_info.adapter_name,
            adapter_backend = ?gpu_info.backend,
            low_priority = gpu_info.low_priority_enabled,
            "GPU device created for histogram pipeline"
        );

        let shader_source = include_str!("histogram_compute.wgsl");
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("histogram_shader"),
            source: wgpu::ShaderSource::Wgsl(shader_source.into()),
        });

        // Create bind group layout
        let bind_group_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("histogram_bind_group_layout"),
            entries: &[
                // Input texture
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
                // Histogram storage buffer (256 atomic u32)
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
                // Metrics output buffer
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
                // Uniform buffer
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

        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("histogram_pipeline_layout"),
            bind_group_layouts: &[&bind_group_layout],
            push_constant_ranges: &[],
        });

        // Create histogram pass pipeline
        let histogram_pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some("histogram_pass_pipeline"),
            layout: Some(&pipeline_layout),
            module: &shader,
            entry_point: Some("histogram_pass"),
            compilation_options: Default::default(),
            cache: None,
        });

        // Create reduce pass pipeline
        let reduce_pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some("reduce_pass_pipeline"),
            layout: Some(&pipeline_layout),
            module: &shader,
            entry_point: Some("reduce_pass"),
            compilation_options: Default::default(),
            cache: None,
        });

        // Create uniform buffer
        let uniform_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("histogram_uniform_buffer"),
            size: std::mem::size_of::<Params>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        Ok(Self {
            device,
            queue,
            histogram_pipeline,
            reduce_pipeline,
            bind_group_layout,
            uniform_buffer,
            cached_width: 0,
            cached_height: 0,
            input_texture: None,
            histogram_buffer: None,
            metrics_buffer: None,
            staging_buffer: None,
        })
    }

    /// Ensure resources are allocated for the given dimensions
    fn ensure_resources(&mut self, width: u32, height: u32) {
        if self.cached_width == width && self.cached_height == height {
            return;
        }

        debug!(width, height, "Allocating histogram pipeline resources");

        // Input texture
        self.input_texture = Some(self.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("histogram_input_texture"),
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

        // Histogram buffer (256 u32 bins)
        self.histogram_buffer = Some(self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("histogram_buffer"),
            size: 256 * std::mem::size_of::<u32>() as u64,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        }));

        // Metrics buffer
        let metrics_size = std::mem::size_of::<BrightnessMetrics>() as u64;
        self.metrics_buffer = Some(self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("metrics_buffer"),
            size: metrics_size,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
            mapped_at_creation: false,
        }));

        // Staging buffer for readback
        self.staging_buffer = Some(self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("metrics_staging_buffer"),
            size: metrics_size,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        }));

        self.cached_width = width;
        self.cached_height = height;
    }

    /// Analyze brightness from RGBA data
    ///
    /// Returns histogram-derived brightness metrics. All histogram computation
    /// stays on GPU; only the small metrics struct is transferred to CPU.
    pub fn analyze(
        &mut self,
        data: &[u8],
        width: u32,
        height: u32,
    ) -> Result<BrightnessMetrics, String> {
        self.ensure_resources(width, height);

        let input_texture = self.input_texture.as_ref().ok_or("No input texture")?;
        let histogram_buffer = self
            .histogram_buffer
            .as_ref()
            .ok_or("No histogram buffer")?;
        let metrics_buffer = self.metrics_buffer.as_ref().ok_or("No metrics buffer")?;
        let staging_buffer = self.staging_buffer.as_ref().ok_or("No staging buffer")?;

        // Upload texture
        self.queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: input_texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            data,
            wgpu::TexelCopyBufferLayout {
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

        // Clear histogram buffer
        self.queue
            .write_buffer(histogram_buffer, 0, &[0u8; 256 * 4]);

        // Update uniforms
        let params = Params {
            width,
            height,
            stage: 0,
            _padding: 0,
        };
        self.queue
            .write_buffer(&self.uniform_buffer, 0, bytemuck::bytes_of(&params));

        // Create texture view
        let texture_view = input_texture.create_view(&wgpu::TextureViewDescriptor::default());

        // Create bind group
        let bind_group = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("histogram_bind_group"),
            layout: &self.bind_group_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&texture_view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: histogram_buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: metrics_buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: self.uniform_buffer.as_entire_binding(),
                },
            ],
        });

        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("histogram_encoder"),
            });

        // Pass 1: Build histogram
        {
            let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some("histogram_pass"),
                timestamp_writes: None,
            });
            pass.set_pipeline(&self.histogram_pipeline);
            pass.set_bind_group(0, &bind_group, &[]);
            // Dispatch 16x16 workgroups
            let workgroups_x = (width + 15) / 16;
            let workgroups_y = (height + 15) / 16;
            pass.dispatch_workgroups(workgroups_x, workgroups_y, 1);
        }

        // Pass 2: Reduce to metrics
        {
            let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some("reduce_pass"),
                timestamp_writes: None,
            });
            pass.set_pipeline(&self.reduce_pipeline);
            pass.set_bind_group(0, &bind_group, &[]);
            // Single workgroup with 256 threads
            pass.dispatch_workgroups(1, 1, 1);
        }

        // Copy metrics to staging buffer
        encoder.copy_buffer_to_buffer(
            metrics_buffer,
            0,
            staging_buffer,
            0,
            std::mem::size_of::<BrightnessMetrics>() as u64,
        );

        self.queue.submit(std::iter::once(encoder.finish()));

        // Map staging buffer and read results
        let buffer_slice = staging_buffer.slice(..);
        buffer_slice.map_async(wgpu::MapMode::Read, |_| {});
        let _ = self.device.poll(wgpu::PollType::wait_indefinitely());

        let metrics: BrightnessMetrics = {
            let data = buffer_slice.get_mapped_range();
            *bytemuck::from_bytes(&data)
        };
        staging_buffer.unmap();

        debug!(
            mean = metrics.mean_luminance,
            median = metrics.median_luminance,
            p5 = metrics.percentile_5,
            p95 = metrics.percentile_95,
            dynamic_range = metrics.dynamic_range_stops,
            shadows = metrics.shadow_fraction,
            highlights = metrics.highlight_fraction,
            "GPU histogram analysis complete"
        );

        Ok(metrics)
    }
}

/// Singleton instance for shared histogram pipeline
static GPU_HISTOGRAM_PIPELINE: std::sync::OnceLock<std::sync::Mutex<Option<HistogramPipeline>>> =
    std::sync::OnceLock::new();

/// Get or initialize the shared histogram pipeline
fn get_histogram_pipeline() -> Option<std::sync::MutexGuard<'static, Option<HistogramPipeline>>> {
    let mutex =
        GPU_HISTOGRAM_PIPELINE.get_or_init(|| match pollster::block_on(HistogramPipeline::new()) {
            Ok(pipeline) => {
                info!("GPU histogram pipeline initialized");
                std::sync::Mutex::new(Some(pipeline))
            }
            Err(e) => {
                warn!("Failed to initialize GPU histogram pipeline: {}", e);
                std::sync::Mutex::new(None)
            }
        });

    let guard = mutex.lock().ok()?;
    if guard.is_some() { Some(guard) } else { None }
}

/// Analyze brightness using GPU histogram
///
/// Falls back to None if GPU is unavailable.
pub fn analyze_brightness_gpu(data: &[u8], width: u32, height: u32) -> Option<BrightnessMetrics> {
    let mut guard = get_histogram_pipeline()?;
    let pipeline = guard.as_mut()?;
    pipeline.analyze(data, width, height).ok()
}
