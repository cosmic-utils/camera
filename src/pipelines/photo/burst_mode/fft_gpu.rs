// SPDX-License-Identifier: GPL-3.0-only
//! GPU-accelerated FFT merge pipeline
//!
//! Implements the full FFT frequency domain merge using WGPU compute shaders.
//! Based on hdr-plus-swift and the HDR+ paper.
//!
//! # Pipeline
//!
//! 1. Initialize output buffer to zeros
//! 2. Calculate RMS (signal level) per tile
//! 3. For each aligned frame:
//!    a. Calculate mismatch (motion) per tile
//!    b. Run 4-pass merge with tile offsets (0,0), (4,0), (0,4), (4,4)
//!       - Each pass: Forward FFT → Wiener merge → Inverse FFT
//! 4. Normalize output by frame count

use super::GpuAlignedFrame;
use crate::gpu::wgpu;
use std::sync::Arc;
use tracing::{debug, info};

const TILE_SIZE: u32 = 16;
const FFT_MERGE_SHADER: &str = include_str!("../../../shaders/burst_mode/fft_merge.wgsl");
const SPATIAL_DENOISE_SHADER: &str =
    include_str!("../../../shaders/burst_mode/spatial_denoise.wgsl");
const CHROMA_DENOISE_SHADER: &str = include_str!("../../../shaders/burst_mode/chroma_denoise.wgsl");
#[cfg(test)]
const GUIDED_FILTER_SHADER: &str = include_str!("../../../shaders/burst_mode/guided_filter.wgsl");

// GPU parameter structs imported from params module
use super::params::{ChromaDenoiseParams, MergeParams, SpatialDenoiseParams};

use super::gpu_helpers::{self, BindingKind};

/// Set of pipelines for a specific FFT tile size (16x16 or 32x32)
///
/// Groups all the pipelines needed for FFT merge at a particular tile size.
/// This reduces parameter passing when selecting between tile sizes.
struct FftPipelineSet {
    init: wgpu::ComputePipeline,
    rms: wgpu::ComputePipeline,
    mismatch: wgpu::ComputePipeline,
    highlights_norm: wgpu::ComputePipeline,
    normalize_mismatch: wgpu::ComputePipeline,
    add_reference: wgpu::ComputePipeline,
    merge: wgpu::ComputePipeline,
    normalize: wgpu::ComputePipeline,
    reduce_artifacts: wgpu::ComputePipeline,
}

/// GPU pipeline for FFT-based frame merging
pub struct FftMergePipeline {
    device: Arc<wgpu::Device>,
    queue: Arc<wgpu::Queue>,

    /// FFT merge pipelines (32x32 tiles)
    pipelines: FftPipelineSet,

    // Spatial denoising pipelines (HDR+ Section 5 post-processing)
    spatial_denoise_init_pipeline: wgpu::ComputePipeline,
    spatial_denoise_pipeline: wgpu::ComputePipeline,
    spatial_denoise_normalize_pipeline: wgpu::ComputePipeline,
    spatial_denoise_bind_group_layout: wgpu::BindGroupLayout,

    // Chroma denoising pipeline (HDR+ Section 6 Step 5)
    chroma_denoise_pipeline: wgpu::ComputePipeline,
    chroma_denoise_bind_group_layout: wgpu::BindGroupLayout,

    // Bind group layout
    bind_group_layout: wgpu::BindGroupLayout,

    // Maximum storage buffer size for this GPU
    max_storage_buffer_size: u64,
}

/// GPU resources for a WOLA (Weighted Overlap-Add) compute pass
struct WolaPassResources<'a> {
    pipeline: &'a wgpu::ComputePipeline,
    bind_group: &'a wgpu::BindGroup,
    params_buffer: &'a wgpu::Buffer,
}

impl FftMergePipeline {
    /// Create a compute pipeline with common defaults
    fn create_pipeline(
        device: &wgpu::Device,
        label: &str,
        layout: &wgpu::PipelineLayout,
        module: &wgpu::ShaderModule,
        entry_point: &str,
    ) -> wgpu::ComputePipeline {
        device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some(label),
            layout: Some(layout),
            module,
            entry_point,
            compilation_options: Default::default(),
            cache: None,
        })
    }

    /// Create a new FFT merge pipeline using an existing GPU device and queue
    ///
    /// This shares the device/queue with BurstModeGpuPipeline to avoid:
    /// 1. Creating duplicate GPU contexts (slow, ~9 seconds on some systems)
    /// 2. Recompiling shaders that may already be cached
    /// 3. Memory overhead from multiple device instances
    pub fn new(
        device: Arc<wgpu::Device>,
        queue: Arc<wgpu::Queue>,
        max_storage_buffer_size: u64,
    ) -> Result<Self, String> {
        info!("Initializing FFT merge GPU pipeline (using shared device)");
        let init_start = std::time::Instant::now();

        // Create shader module
        let shader_start = std::time::Instant::now();
        let shader_module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("fft_merge_shader"),
            source: wgpu::ShaderSource::Wgsl(FFT_MERGE_SHADER.into()),
        });
        info!(
            elapsed_ms = shader_start.elapsed().as_millis(),
            "FFT merge shader compiled"
        );

        // Create bind group layout using helper
        // Bindings: reference(r), aligned(r), output(rw), params(u), rms(rw), mismatch(rw), highlights_norm(rw), weight(rw)
        use BindingKind::*;
        let bind_group_layout = gpu_helpers::create_layout(
            &device,
            "fft_merge_bind_group_layout",
            &[
                StorageRead,
                StorageRead,
                StorageReadWrite,
                Uniform,
                StorageReadWrite,
                StorageReadWrite,
                StorageReadWrite,
                StorageReadWrite,
            ],
        );

        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("fft_merge_pipeline_layout"),
            bind_group_layouts: &[&bind_group_layout],
            push_constant_ranges: &[],
        });

        // Create compute pipelines (32x32 tiles)
        let pipelines = FftPipelineSet {
            init: Self::create_pipeline(
                &device,
                "init_output_pipeline",
                &pipeline_layout,
                &shader_module,
                "init_output",
            ),
            rms: Self::create_pipeline(
                &device,
                "calculate_rms_pipeline",
                &pipeline_layout,
                &shader_module,
                "calculate_rms",
            ),
            mismatch: Self::create_pipeline(
                &device,
                "calculate_mismatch_pipeline",
                &pipeline_layout,
                &shader_module,
                "calculate_mismatch",
            ),
            highlights_norm: Self::create_pipeline(
                &device,
                "calculate_highlights_norm_pipeline",
                &pipeline_layout,
                &shader_module,
                "calculate_highlights_norm",
            ),
            normalize_mismatch: Self::create_pipeline(
                &device,
                "normalize_mismatch_pipeline",
                &pipeline_layout,
                &shader_module,
                "normalize_mismatch",
            ),
            add_reference: Self::create_pipeline(
                &device,
                "add_reference_pipeline",
                &pipeline_layout,
                &shader_module,
                "add_reference_to_output",
            ),
            merge: Self::create_pipeline(
                &device,
                "merge_tile_pipeline",
                &pipeline_layout,
                &shader_module,
                "merge_tile",
            ),
            normalize: Self::create_pipeline(
                &device,
                "normalize_output_pipeline",
                &pipeline_layout,
                &shader_module,
                "normalize_output",
            ),
            reduce_artifacts: Self::create_pipeline(
                &device,
                "reduce_tile_artifacts_pipeline",
                &pipeline_layout,
                &shader_module,
                "reduce_tile_artifacts",
            ),
        };

        // Create spatial denoising shader and pipeline (HDR+ Section 5)
        let shader_start = std::time::Instant::now();
        let spatial_denoise_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("spatial_denoise_shader"),
            source: wgpu::ShaderSource::Wgsl(SPATIAL_DENOISE_SHADER.into()),
        });
        info!(
            elapsed_ms = shader_start.elapsed().as_millis(),
            "Spatial denoise shader compiled"
        );

        // Bindings: input(r), output(rw), params(u), weight(rw)
        let spatial_denoise_bind_group_layout = gpu_helpers::create_layout(
            &device,
            "spatial_denoise_bind_group_layout",
            &[StorageRead, StorageReadWrite, Uniform, StorageReadWrite],
        );

        let spatial_denoise_pipeline_layout =
            device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("spatial_denoise_pipeline_layout"),
                bind_group_layouts: &[&spatial_denoise_bind_group_layout],
                push_constant_ranges: &[],
            });

        let spatial_denoise_init_pipeline = Self::create_pipeline(
            &device,
            "spatial_denoise_init_pipeline",
            &spatial_denoise_pipeline_layout,
            &spatial_denoise_shader,
            "spatial_denoise_init",
        );
        let spatial_denoise_pipeline = Self::create_pipeline(
            &device,
            "spatial_denoise_pipeline",
            &spatial_denoise_pipeline_layout,
            &spatial_denoise_shader,
            "spatial_denoise",
        );
        let spatial_denoise_normalize_pipeline = Self::create_pipeline(
            &device,
            "spatial_denoise_normalize_pipeline",
            &spatial_denoise_pipeline_layout,
            &spatial_denoise_shader,
            "spatial_denoise_normalize",
        );

        // Create chroma denoising shader and pipeline (HDR+ Section 6 Step 5)
        let shader_start = std::time::Instant::now();
        let chroma_denoise_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("chroma_denoise_shader"),
            source: wgpu::ShaderSource::Wgsl(CHROMA_DENOISE_SHADER.into()),
        });
        info!(
            elapsed_ms = shader_start.elapsed().as_millis(),
            "Chroma denoise shader compiled"
        );

        // Bindings: input(r), output(rw), temp(rw), params(u)
        let chroma_denoise_bind_group_layout = gpu_helpers::create_layout(
            &device,
            "chroma_denoise_bind_group_layout",
            &[StorageRead, StorageReadWrite, StorageReadWrite, Uniform],
        );

        let chroma_denoise_pipeline_layout =
            device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("chroma_denoise_pipeline_layout"),
                bind_group_layouts: &[&chroma_denoise_bind_group_layout],
                push_constant_ranges: &[],
            });

        let chroma_denoise_pipeline = Self::create_pipeline(
            &device,
            "chroma_denoise_pipeline",
            &chroma_denoise_pipeline_layout,
            &chroma_denoise_shader,
            "chroma_denoise_single",
        );

        info!(
            total_init_ms = init_start.elapsed().as_millis(),
            "FFT merge pipeline initialization complete"
        );

        Ok(Self {
            device,
            queue,
            pipelines,
            spatial_denoise_init_pipeline,
            spatial_denoise_pipeline,
            spatial_denoise_normalize_pipeline,
            spatial_denoise_bind_group_layout,
            chroma_denoise_pipeline,
            chroma_denoise_bind_group_layout,
            bind_group_layout,
            max_storage_buffer_size,
        })
    }

    /// Run a single compute pass with the given pipeline and bind group
    ///
    /// This helper eliminates boilerplate for the common pattern of:
    /// 1. Create command encoder
    /// 2. Begin compute pass
    /// 3. Set pipeline and bind group
    /// 4. Dispatch workgroups
    /// 5. Submit to queue
    fn run_compute_pass(
        &self,
        label: &str,
        pipeline: &wgpu::ComputePipeline,
        bind_group: &wgpu::BindGroup,
        workgroups: (u32, u32, u32),
    ) {
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some(&format!("{}_encoder", label)),
            });

        let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
            label: Some(&format!("{}_pass", label)),
            timestamp_writes: None,
        });
        pass.set_pipeline(pipeline);
        pass.set_bind_group(0, bind_group, &[]);
        pass.dispatch_workgroups(workgroups.0, workgroups.1, workgroups.2);
        drop(pass);

        self.queue.submit(std::iter::once(encoder.finish()));
    }

    /// Run multiple compute passes, yielding once at the end
    ///
    /// Batches all dispatches together for efficiency, then yields to compositor.
    async fn run_batched_compute_passes(
        &self,
        label: &str,
        dispatches: &[(&wgpu::ComputePipeline, &wgpu::BindGroup, (u32, u32, u32))],
    ) {
        for (idx, (pipeline, bind_group, workgroups)) in dispatches.iter().enumerate() {
            self.run_compute_pass(
                &format!("{}_{}", label, idx),
                pipeline,
                bind_group,
                *workgroups,
            );
        }
        // Yield once after all passes
        self.yield_to_compositor().await;
    }

    /// Maximum rows to dispatch at once to allow GPU preemption
    ///
    /// Breaking large dispatches into smaller chunks allows the GPU to preempt
    /// between chunks, letting the compositor render smoothly. With 16x16 tiles
    /// and ~188 tile rows for a 3000px tall image, dispatching 4 rows at a time
    /// gives ~47 chunks with frequent yield opportunities.
    const ROWS_PER_CHUNK: u32 = 4;

    /// Run a 4-pass WOLA (Weighted Overlap-Add) pattern with symmetric tile offsets
    ///
    /// This is the core pattern used throughout FFT merge for proper tile boundary
    /// handling. The 4 passes with half-tile offsets ensure every pixel is covered
    /// by the overlapping window function from multiple tiles, eliminating seams.
    ///
    /// Offsets: [(-half, -half), (0, -half), (-half, 0), (0, 0)]
    ///
    /// Yields once at the end of all 4 passes to let compositor render.
    async fn run_4pass_wola<P: bytemuck::Pod>(
        &self,
        label: &str,
        resources: &WolaPassResources<'_>,
        base_params: P,
        tile_size: u32,
        workgroups: (u32, u32, u32),
        update_params: impl Fn(&P, i32, i32) -> P,
    ) {
        let half_tile = (tile_size / 2) as i32;
        let offsets = [
            (-half_tile, -half_tile),
            (0, -half_tile),
            (-half_tile, 0),
            (0, 0),
        ];

        for (pass_idx, (offset_x, offset_y)) in offsets.iter().enumerate() {
            let params = update_params(&base_params, *offset_x, *offset_y);
            self.queue
                .write_buffer(resources.params_buffer, 0, bytemuck::cast_slice(&[params]));

            self.run_compute_pass(
                &format!("{}_{}", label, pass_idx),
                resources.pipeline,
                resources.bind_group,
                workgroups,
            );
        }

        // Yield once after all 4 passes to let compositor render
        self.yield_to_compositor().await;
    }

    /// Run a 4-pass WOLA pattern with chunked dispatches for GPU preemption
    ///
    /// Same as run_4pass_wola but breaks each pass into smaller row chunks,
    /// allowing the GPU to preempt between chunks for better compositor responsiveness.
    ///
    /// This version is specific to MergeParams which has tile_row_offset support.
    async fn run_4pass_wola_chunked(
        &self,
        label: &str,
        resources: &WolaPassResources<'_>,
        base_params: MergeParams,
        tile_size: u32,
        workgroups: (u32, u32, u32),
        update_params: impl Fn(&MergeParams, i32, i32) -> MergeParams,
    ) {
        let half_tile = (tile_size / 2) as i32;
        let offsets = [
            (-half_tile, -half_tile),
            (0, -half_tile),
            (-half_tile, 0),
            (0, 0),
        ];

        let n_tiles_y = workgroups.1;

        for (pass_idx, (offset_x, offset_y)) in offsets.iter().enumerate() {
            // Process rows in chunks for GPU preemption
            let mut row_offset = 0u32;
            while row_offset < n_tiles_y {
                let rows_this_chunk = Self::ROWS_PER_CHUNK.min(n_tiles_y - row_offset);

                // Update params with tile offsets AND row offset for this chunk
                let mut params = update_params(&base_params, *offset_x, *offset_y);
                params.tile_row_offset = row_offset;

                self.queue.write_buffer(
                    resources.params_buffer,
                    0,
                    bytemuck::cast_slice(&[params]),
                );

                self.run_compute_pass(
                    &format!("{}_{}_{}", label, pass_idx, row_offset),
                    resources.pipeline,
                    resources.bind_group,
                    (workgroups.0, rows_this_chunk, workgroups.2),
                );

                row_offset += rows_this_chunk;
            }

            // Yield after each of the 4 passes to let compositor render
            self.yield_to_compositor().await;
        }
    }

    /// Yield to allow other GPU work (like desktop compositor) to run
    ///
    /// With low-priority queue (Family 1 + VK_EXT_global_priority LOW) and
    /// small chunked dispatches, the GPU should automatically preempt our
    /// work for higher-priority compositor rendering.
    async fn yield_to_compositor(&self) {
        // Poll to submit pending work - the low-priority queue handles preemption
        let _ = self.device.poll(wgpu::Maintain::Wait);
    }

    /// Create a storage buffer for RGBA f32 pixel data
    fn create_rgba_buffer(
        &self,
        label: &str,
        pixel_count: usize,
        usage: wgpu::BufferUsages,
    ) -> wgpu::Buffer {
        self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some(label),
            size: (pixel_count * 4 * std::mem::size_of::<f32>()) as u64,
            usage,
            mapped_at_creation: false,
        })
    }

    /// Create a storage buffer for per-tile scalar data
    fn create_tile_buffer(
        &self,
        label: &str,
        tile_count: usize,
        usage: wgpu::BufferUsages,
    ) -> wgpu::Buffer {
        self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some(label),
            size: (tile_count * std::mem::size_of::<f32>()) as u64,
            usage,
            mapped_at_creation: false,
        })
    }

    /// Create a storage buffer for per-tile vec4 data (RGBA per tile)
    fn create_tile_rgba_buffer(
        &self,
        label: &str,
        tile_count: usize,
        usage: wgpu::BufferUsages,
    ) -> wgpu::Buffer {
        self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some(label),
            size: (tile_count * 4 * std::mem::size_of::<f32>()) as u64,
            usage,
            mapped_at_creation: false,
        })
    }

    /// Apply spatial denoising (HDR+ Section 5 post-processing)
    ///
    /// Uses 4-pass overlapped processing with raised cosine windows.
    async fn apply_spatial_denoise(
        &self,
        output_buffer: &wgpu::Buffer,
        buffer_size: u64,
        width: u32,
        height: u32,
        noise_sd: f32,
        frame_count: u32,
    ) {
        let pixel_count = (width * height) as usize;

        let spatial_output_buffer = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("spatial_denoise_output_buffer"),
            size: buffer_size,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
            mapped_at_creation: false,
        });

        let spatial_weight_buffer = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("spatial_denoise_weight_buffer"),
            size: (pixel_count * std::mem::size_of::<f32>()) as u64,
            usage: wgpu::BufferUsages::STORAGE,
            mapped_at_creation: false,
        });

        let denoise_tiles_x = width.div_ceil(TILE_SIZE) + 1;
        let denoise_tiles_y = height.div_ceil(TILE_SIZE) + 1;

        let spatial_params_buffer = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("spatial_denoise_params_buffer"),
            size: std::mem::size_of::<SpatialDenoiseParams>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        let spatial_bind_group = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("spatial_denoise_bind_group"),
            layout: &self.spatial_denoise_bind_group_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: output_buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: spatial_output_buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: spatial_params_buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: spatial_weight_buffer.as_entire_binding(),
                },
            ],
        });

        // Note: After temporal merge, noise is already reduced by ~sqrt(N)
        // Spatial denoise is a light polish pass, not primary noise reduction
        // HDR+ paper Section 5: "we update our estimate of the noise variance to be σ²/N"
        let base_spatial_params = SpatialDenoiseParams {
            width,
            height,
            noise_sd,
            strength: 0.15, // Very light spatial denoising (temporal merge is primary)
            n_tiles_x: denoise_tiles_x,
            n_tiles_y: denoise_tiles_y,
            high_freq_boost: 1.5, // Gentle high-freq filtering to preserve detail
            tile_offset_x: 0,
            tile_offset_y: 0,
            frame_count,
        };

        self.queue.write_buffer(
            &spatial_params_buffer,
            0,
            bytemuck::cast_slice(&[base_spatial_params]),
        );

        self.run_compute_pass(
            "spatial_denoise_init",
            &self.spatial_denoise_init_pipeline,
            &spatial_bind_group,
            (width.div_ceil(16), height.div_ceil(16), 1),
        );

        self.run_4pass_wola(
            "spatial_denoise",
            &WolaPassResources {
                pipeline: &self.spatial_denoise_pipeline,
                bind_group: &spatial_bind_group,
                params_buffer: &spatial_params_buffer,
            },
            base_spatial_params,
            TILE_SIZE,
            (denoise_tiles_x, denoise_tiles_y, 1),
            |params, offset_x, offset_y| SpatialDenoiseParams {
                tile_offset_x: offset_x,
                tile_offset_y: offset_y,
                ..*params
            },
        )
        .await;

        self.run_compute_pass(
            "spatial_denoise_normalize",
            &self.spatial_denoise_normalize_pipeline,
            &spatial_bind_group,
            (width.div_ceil(16), height.div_ceil(16), 1),
        );

        // Copy result back to output buffer
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("spatial_denoise_copy_encoder"),
            });

        encoder.copy_buffer_to_buffer(&spatial_output_buffer, 0, output_buffer, 0, buffer_size);

        self.queue.submit(std::iter::once(encoder.finish()));
    }

    /// Apply chroma denoising to reduce color noise
    fn apply_chroma_denoise(
        &self,
        output_buffer: &wgpu::Buffer,
        buffer_size: u64,
        width: u32,
        height: u32,
    ) {
        let chroma_output_buffer = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("chroma_output_buffer"),
            size: buffer_size,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
            mapped_at_creation: false,
        });

        let chroma_temp_buffer = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("chroma_temp_buffer"),
            size: buffer_size,
            usage: wgpu::BufferUsages::STORAGE,
            mapped_at_creation: false,
        });

        // Chroma denoise: very light to preserve color saturation
        let chroma_params = ChromaDenoiseParams {
            width,
            height,
            strength: 0.25,       // Reduced to preserve color saturation
            edge_threshold: 0.15, // Raised to better preserve edges
        };

        let chroma_params_buffer = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("chroma_params_buffer"),
            size: std::mem::size_of::<ChromaDenoiseParams>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        self.queue.write_buffer(
            &chroma_params_buffer,
            0,
            bytemuck::cast_slice(&[chroma_params]),
        );

        let chroma_bind_group = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("chroma_denoise_bind_group"),
            layout: &self.chroma_denoise_bind_group_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: output_buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: chroma_output_buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: chroma_temp_buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: chroma_params_buffer.as_entire_binding(),
                },
            ],
        });

        self.run_compute_pass(
            "chroma_denoise",
            &self.chroma_denoise_pipeline,
            &chroma_bind_group,
            (width.div_ceil(16), height.div_ceil(16), 1),
        );

        // Copy chroma output back to output buffer
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("copy_chroma_output"),
            });
        encoder.copy_buffer_to_buffer(&chroma_output_buffer, 0, output_buffer, 0, buffer_size);
        self.queue.submit(std::iter::once(encoder.finish()));
    }

    /// Merge frames using FFT with GPU-resident aligned frames
    ///
    /// This is the memory-optimized version that accepts GpuAlignedFrame buffers
    /// directly, avoiding CPU round-trips for aligned frame data.
    ///
    /// Uses 32x32 tiles for all scenes. HDR+ paper Section 5 notes that larger tiles
    /// provide better noise estimation and more frequency context.
    ///
    /// # Arguments
    /// * `reference` - Reference frame RGBA data (u8)
    /// * `aligned_frames` - GPU-resident aligned frames (already on GPU as f32)
    /// * `width` - Frame width
    /// * `height` - Frame height
    /// * `noise_sd` - Estimated noise standard deviation
    /// * `robustness` - Robustness parameter (higher = more aggressive merge)
    ///
    /// # Returns
    /// Merged frame data (RGBA u8)
    pub async fn merge_gpu(
        &self,
        reference: &[u8],
        aligned_frames: &[GpuAlignedFrame],
        width: u32,
        height: u32,
        noise_sd: f32,
        robustness: f32,
    ) -> Result<Vec<u8>, String> {
        let tile_size = TILE_SIZE;
        let pipelines = &self.pipelines;
        let merge_total_start = std::time::Instant::now();

        // Scale robustness based on resolution (same as merge())
        // See merge() for detailed explanation of this fix
        let pixel_count = (width * height) as usize;
        let reference_pixels = 1920.0 * 1080.0; // ~2MP baseline (1080p)
        let resolution_scale = ((pixel_count as f32) / reference_pixels).sqrt();
        let scaled_robustness = robustness * resolution_scale;

        info!(
            width,
            height,
            noise_sd,
            base_robustness = robustness,
            resolution_scale,
            scaled_robustness,
            tile_size,
            frames = aligned_frames.len() + 1,
            "Starting FFT merge (GPU-only, no CPU round-trip for aligned frames)"
        );

        // Check buffer size limit
        let buffer_size = (pixel_count * 4 * std::mem::size_of::<f32>()) as u64;
        if buffer_size > self.max_storage_buffer_size {
            return Err(format!(
                "Image too large for GPU FFT merge ({} bytes > {} max). Use spatial merge instead.",
                buffer_size, self.max_storage_buffer_size
            ));
        }

        let n_tiles_x = width.div_ceil(tile_size) + 1;
        let n_tiles_y = height.div_ceil(tile_size) + 1;
        let tile_count = (n_tiles_x * n_tiles_y) as usize;

        // Convert reference to normalized f32 and upload (only CPU->GPU transfer needed)
        let convert_start = std::time::Instant::now();
        let ref_f32 = super::u8_to_f32_normalized(reference);
        debug!(
            elapsed_ms = convert_start.elapsed().as_millis(),
            "Convert reference to f32"
        );

        // Create GPU buffers
        let buffer_create_start = std::time::Instant::now();
        let ref_buffer = self.create_rgba_buffer(
            "reference_buffer",
            pixel_count,
            wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
        );

        // Aligned buffer for GPU-to-GPU copy
        let aligned_buffer = self.create_rgba_buffer(
            "aligned_buffer",
            pixel_count,
            wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
        );

        let output_buffer = self.create_rgba_buffer(
            "output_buffer",
            pixel_count,
            wgpu::BufferUsages::STORAGE
                | wgpu::BufferUsages::COPY_SRC
                | wgpu::BufferUsages::COPY_DST,
        );

        let params_buffer = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("params_buffer"),
            size: std::mem::size_of::<MergeParams>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        let rms_buffer = self.create_tile_rgba_buffer(
            "rms_buffer",
            tile_count,
            wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
        );

        let mismatch_buffer = self.create_tile_buffer(
            "mismatch_buffer",
            tile_count,
            wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
        );

        let highlights_norm_buffer = self.create_tile_buffer(
            "highlights_norm_buffer",
            tile_count,
            wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
        );

        let weight_buffer =
            self.create_tile_buffer("weight_buffer", pixel_count, wgpu::BufferUsages::STORAGE);

        let staging_buffer = self.create_rgba_buffer(
            "staging_buffer",
            pixel_count,
            wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
        );

        debug!(
            elapsed_ms = buffer_create_start.elapsed().as_millis(),
            "Created GPU buffers"
        );

        // Upload reference frame
        let upload_start = std::time::Instant::now();
        self.queue
            .write_buffer(&ref_buffer, 0, bytemuck::cast_slice(&ref_f32));
        drop(ref_f32); // Free CPU memory immediately
        debug!(
            elapsed_ms = upload_start.elapsed().as_millis(),
            "Uploaded reference frame"
        );

        // Create bind group
        let bind_group = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("fft_merge_bind_group"),
            layout: &self.bind_group_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: ref_buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: aligned_buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: output_buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: params_buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 4,
                    resource: rms_buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 5,
                    resource: mismatch_buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 6,
                    resource: highlights_norm_buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 7,
                    resource: weight_buffer.as_entire_binding(),
                },
            ],
        });

        let frame_count = aligned_frames.len() as u32 + 1;
        let robustness_rev = 0.5 * (26.5 - scaled_robustness);
        let max_motion_norm = 1.0f32.max((1.3f32).powf(11.0 - robustness_rev));
        // Fix: ensure inner value is positive to avoid NaN from negative base with fractional exponent
        // With robustness=1.0, robustness_rev=12.75, so (-12.75+10.0)=-2.75 would cause NaN
        let read_noise_inner = (-robustness_rev + 10.0).max(0.01);
        let read_noise = (2.0f32).powf(read_noise_inner.powf(1.6));

        let base_params = MergeParams {
            width,
            height,
            noise_sd,
            robustness: scaled_robustness,
            n_tiles_x,
            n_tiles_y,
            frame_count,
            read_noise,
            max_motion_norm,
            tile_offset_x: 0,
            tile_offset_y: 0,
            tile_row_offset: 0,
            exposure_factor: 1.0, // TODO: Read from frame metadata for HDR brackets
            _padding: 0,
        };

        self.queue
            .write_buffer(&params_buffer, 0, bytemuck::cast_slice(&[base_params]));

        // Step 1 & 2: Initialize output and calculate RMS (yields between dispatches)
        let step_start = std::time::Instant::now();
        self.run_batched_compute_passes(
            "init_rms",
            &[
                (
                    &pipelines.init,
                    &bind_group,
                    (width.div_ceil(16), height.div_ceil(16), 1),
                ),
                (&pipelines.rms, &bind_group, (n_tiles_x, n_tiles_y, 1)),
            ],
        )
        .await;
        debug!(
            elapsed_ms = step_start.elapsed().as_millis(),
            "Step 1-2: init + rms"
        );

        // Step 2.5: Add reference frame to accumulator (HDR+ equation 6, z=0 term)
        //
        // The HDR+ paper equation 6 specifies:
        //   T̃₀(ω) = (1/N) Σ_{z=0}^{N-1} [T_z(ω) + A_z(ω)(T₀(ω) - T_z(ω))]
        //
        // For z=0 (reference frame), A₀=0, so this simply adds T₀ to the accumulator.
        // This must be run with 4-pass WOLA (same offsets as merge) to ensure proper
        // window coverage across all pixels.
        //
        // Uses chunked dispatch for GPU preemption.
        let ref_start = std::time::Instant::now();
        self.run_4pass_wola_chunked(
            "add_reference",
            &WolaPassResources {
                pipeline: &pipelines.add_reference,
                bind_group: &bind_group,
                params_buffer: &params_buffer,
            },
            base_params,
            tile_size,
            (n_tiles_x, n_tiles_y, 1),
            |params, offset_x, offset_y| MergeParams {
                tile_offset_x: offset_x,
                tile_offset_y: offset_y,
                ..*params
            },
        )
        .await;
        debug!(
            elapsed_ms = ref_start.elapsed().as_millis(),
            "Step 2.5: add reference to accumulator"
        );

        // Step 3: Process each aligned frame - GPU-to-GPU copy, no CPU involved!
        for (frame_idx, gpu_frame) in aligned_frames.iter().enumerate() {
            let frame_start = std::time::Instant::now();

            // GPU-to-GPU buffer copy - no CPU round-trip!
            let copy_start = std::time::Instant::now();
            let mut encoder = self
                .device
                .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                    label: Some(&format!("copy_aligned_{}", frame_idx)),
                });
            encoder.copy_buffer_to_buffer(&gpu_frame.buffer, 0, &aligned_buffer, 0, buffer_size);
            self.queue.submit(std::iter::once(encoder.finish()));
            debug!(
                frame = frame_idx,
                elapsed_ms = copy_start.elapsed().as_millis(),
                "GPU-to-GPU copy"
            );

            // Calculate mismatch and highlights_norm (yields between dispatches)
            let mismatch_start = std::time::Instant::now();
            self.run_batched_compute_passes(
                "mismatch_highlights",
                &[
                    (&pipelines.mismatch, &bind_group, (n_tiles_x, n_tiles_y, 1)),
                    (
                        &pipelines.highlights_norm,
                        &bind_group,
                        (n_tiles_x, n_tiles_y, 1),
                    ),
                ],
            )
            .await;
            debug!(
                frame = frame_idx,
                elapsed_ms = mismatch_start.elapsed().as_millis(),
                "mismatch + highlights_norm"
            );

            // Normalize mismatch
            let norm_start = std::time::Instant::now();
            self.run_compute_pass(
                "normalize_mismatch",
                &pipelines.normalize_mismatch,
                &bind_group,
                (1, 1, 1),
            );
            debug!(
                frame = frame_idx,
                elapsed_ms = norm_start.elapsed().as_millis(),
                "normalize_mismatch"
            );

            // 4-pass merge with symmetric tile offsets and chunked dispatch
            // Uses chunked dispatch for GPU preemption - breaks large dispatches
            // into smaller row chunks so compositor can render between chunks.
            let merge_start = std::time::Instant::now();
            self.run_4pass_wola_chunked(
                "merge",
                &WolaPassResources {
                    pipeline: &pipelines.merge,
                    bind_group: &bind_group,
                    params_buffer: &params_buffer,
                },
                base_params,
                tile_size,
                (n_tiles_x, n_tiles_y, 1),
                |params, offset_x, offset_y| MergeParams {
                    tile_offset_x: offset_x,
                    tile_offset_y: offset_y,
                    ..*params
                },
            )
            .await;
            debug!(
                frame = frame_idx,
                elapsed_ms = merge_start.elapsed().as_millis(),
                "4-pass merge (chunked)"
            );
            debug!(
                frame = frame_idx,
                total_frame_ms = frame_start.elapsed().as_millis(),
                "frame complete"
            );
        }

        // Step 4 & 5: Normalize output and reduce artifacts
        let normalize_start = std::time::Instant::now();
        self.queue
            .write_buffer(&params_buffer, 0, bytemuck::cast_slice(&[base_params]));

        self.run_batched_compute_passes(
            "normalize_clamp",
            &[
                (
                    &pipelines.normalize,
                    &bind_group,
                    (width.div_ceil(16), height.div_ceil(16), 1),
                ),
                (
                    &pipelines.reduce_artifacts,
                    &bind_group,
                    (width.div_ceil(16), height.div_ceil(16), 1),
                ),
            ],
        )
        .await;
        debug!(
            elapsed_ms = normalize_start.elapsed().as_millis(),
            "Step 4-5: normalize + clamp"
        );

        // Step 5.5: Spatial denoising (HDR+ Section 5 post-processing)
        // Note: apply_spatial_denoise yields internally through run_4pass_wola
        let spatial_start = std::time::Instant::now();
        debug!("Running spatial denoising (4-pass overlapped)");
        let frame_count = (aligned_frames.len() + 1) as u32; // +1 for reference frame
        self.apply_spatial_denoise(
            &output_buffer,
            buffer_size,
            width,
            height,
            noise_sd,
            frame_count,
        )
        .await;
        debug!(
            elapsed_ms = spatial_start.elapsed().as_millis(),
            "Step 5.5: spatial denoise (4-pass)"
        );

        // Chroma denoising
        let chroma_start = std::time::Instant::now();
        self.apply_chroma_denoise(&output_buffer, buffer_size, width, height);
        debug!(
            elapsed_ms = chroma_start.elapsed().as_millis(),
            "Chroma denoise"
        );

        // Yield to compositor before readback
        self.yield_to_compositor().await;

        // Read back result
        let readback_start = std::time::Instant::now();
        {
            let mut encoder = self
                .device
                .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                    label: Some("readback_encoder"),
                });
            encoder.copy_buffer_to_buffer(&output_buffer, 0, &staging_buffer, 0, buffer_size);
            self.queue.submit(std::iter::once(encoder.finish()));
        }
        debug!(
            elapsed_ms = readback_start.elapsed().as_millis(),
            "Copy to staging"
        );

        // Map and convert
        let map_start = std::time::Instant::now();
        let buffer_slice = staging_buffer.slice(..);
        let (sender, receiver) = futures::channel::oneshot::channel();
        buffer_slice.map_async(wgpu::MapMode::Read, move |result| {
            let _ = sender.send(result);
        });

        let _ = self.device.poll(wgpu::Maintain::Wait);
        receiver
            .await
            .map_err(|_| "Failed to receive map result")?
            .map_err(|e| format!("Failed to map buffer: {:?}", e))?;
        debug!(
            elapsed_ms = map_start.elapsed().as_millis(),
            "Map staging buffer"
        );

        let convert_start = std::time::Instant::now();
        let data = buffer_slice.get_mapped_range();
        let output_f32: &[f32] = bytemuck::cast_slice(&data);
        let output_u8: Vec<u8> = output_f32
            .iter()
            .map(|&x| (x.clamp(0.0, 1.0) * 255.0) as u8)
            .collect();

        drop(data);
        staging_buffer.unmap();
        debug!(
            elapsed_ms = convert_start.elapsed().as_millis(),
            "Convert f32 to u8"
        );

        info!(
            total_elapsed_ms = merge_total_start.elapsed().as_millis(),
            "FFT merge complete (GPU-only)"
        );
        Ok(output_u8)
    }
}

#[cfg(test)]
mod tests {
    use super::super::CA_ESTIMATE_SHADER;
    use super::*;

    /// Validate that a WGSL shader compiles successfully using naga
    fn validate_shader(name: &str, source: &str) {
        let result = naga::front::wgsl::parse_str(source);
        match result {
            Ok(module) => {
                // Validate the parsed module
                let info = naga::valid::Validator::new(
                    naga::valid::ValidationFlags::all(),
                    naga::valid::Capabilities::all(),
                )
                .validate(&module);

                if let Err(e) = info {
                    panic!("Shader '{}' validation failed: {:?}", name, e);
                }
            }
            Err(e) => {
                panic!("Shader '{}' parse failed: {:?}", name, e);
            }
        }
    }

    #[test]
    fn test_fft_merge_shader_validates() {
        validate_shader("fft_merge", FFT_MERGE_SHADER);
    }

    #[test]
    fn test_spatial_denoise_shader_validates() {
        validate_shader("spatial_denoise", SPATIAL_DENOISE_SHADER);
    }

    #[test]
    fn test_chroma_denoise_shader_validates() {
        validate_shader("chroma_denoise", CHROMA_DENOISE_SHADER);
    }

    #[test]
    fn test_ca_estimate_shader_validates() {
        validate_shader("ca_estimate", CA_ESTIMATE_SHADER);
    }

    #[test]
    fn test_guided_filter_shader_validates() {
        validate_shader("guided_filter", GUIDED_FILTER_SHADER);
    }
}
