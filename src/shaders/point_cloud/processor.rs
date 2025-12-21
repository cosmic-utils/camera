// SPDX-License-Identifier: GPL-3.0-only

//! GPU point cloud processor for 3D depth visualization
//!
//! Takes depth + RGB data and renders a rotatable 3D point cloud.

use crate::gpu::{self, wgpu};
use crate::gpu_processor_singleton;
use crate::shaders::compute_dispatch_size;
use crate::shaders::depth::kinect;
use crate::shaders::gpu_utils;
use std::sync::Arc;
use tracing::{debug, info, warn};

/// Depth data format for point cloud shader
#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub enum DepthFormat {
    /// Depth in millimeters (from native Kinect backend)
    Millimeters = 0,
    /// 10-bit disparity shifted to 16-bit (from V4L2 Y10B)
    Disparity16 = 1,
}

/// Point cloud rendering parameters
#[repr(C)]
#[derive(Debug, Copy, Clone, bytemuck::Pod, bytemuck::Zeroable)]
struct PointCloudParams {
    // Depth input dimensions (used for iteration and unprojection)
    input_width: u32,
    input_height: u32,
    // Output dimensions
    output_width: u32,
    output_height: u32,
    // RGB input dimensions (may differ from depth)
    rgb_width: u32,
    rgb_height: u32,
    // Camera intrinsics (Kinect defaults)
    fx: f32,
    fy: f32,
    cx: f32,
    cy: f32,
    // Depth format: 0 = millimeters, 1 = disparity (10-bit shifted to 16-bit)
    depth_format: u32,
    // Depth conversion coefficients (only used for disparity format)
    // Formula: depth_m = 1.0 / (raw * depth_coeff_a + depth_coeff_b)
    depth_coeff_a: f32,
    depth_coeff_b: f32,
    min_depth: f32,
    max_depth: f32,
    // Rotation
    pitch: f32,
    yaw: f32,
    // Rendering
    point_size: f32,
    fov: f32,
    view_distance: f32,
    // Registration parameters
    use_registration_tables: u32, // 1 = use lookup tables, 0 = use simple shift
    target_offset: u32,           // Y offset from pad_info (for registration tables)
    reg_x_val_scale: i32,         // Fixed-point scale factor (256)
    mirror: u32,                  // 1 = mirror horizontally, 0 = normal
    // High-res RGB scaling for registration
    reg_scale_x: f32,  // X scale factor (1.0 for 640, 2.0 for 1280)
    reg_scale_y: f32,  // Y scale factor (1.0 for 480, 2.0 for 960->1024)
    reg_y_offset: i32, // Y offset for high-res (32 for 1280x1024, 0 for 640x480)
}

/// Result of point cloud rendering
pub struct PointCloudResult {
    /// Rendered RGBA image
    pub rgba: Vec<u8>,
    /// Width of output image
    pub width: u32,
    /// Height of output image
    pub height: u32,
}

/// Registration data for depth-to-RGB alignment
#[derive(Clone)]
pub struct RegistrationData {
    /// Registration table: 640*480 [x_scaled, y] pairs
    pub registration_table: Vec<[i32; 2]>,
    /// Depth-to-RGB shift table: 10001 i32 values indexed by depth_mm
    pub depth_to_rgb_shift: Vec<i32>,
    /// Target offset from pad_info
    pub target_offset: u32,
}

/// GPU point cloud processor
pub struct PointCloudProcessor {
    device: Arc<wgpu::Device>,
    queue: Arc<wgpu::Queue>,
    render_pipeline: wgpu::ComputePipeline,
    clear_pipeline: wgpu::ComputePipeline,
    bind_group_layout: wgpu::BindGroupLayout,
    uniform_buffer: wgpu::Buffer,
    // Cached resources - depth/input dimensions
    cached_input_width: u32,
    cached_input_height: u32,
    cached_output_width: u32,
    cached_output_height: u32,
    // Cached resources - RGB dimensions (may differ from depth)
    cached_rgb_width: u32,
    cached_rgb_height: u32,
    rgb_buffer: Option<wgpu::Buffer>,
    depth_buffer: Option<wgpu::Buffer>,
    depth_test_buffer: Option<wgpu::Buffer>,
    output_texture: Option<wgpu::Texture>,
    staging_buffer: Option<wgpu::Buffer>,
    // Registration data buffers
    registration_table_buffer: Option<wgpu::Buffer>,
    depth_to_rgb_shift_buffer: Option<wgpu::Buffer>,
    registration_target_offset: u32,
    has_registration_data: bool,
    // CPU copy of registration data for retrieval
    registration_data_copy: Option<RegistrationData>,
}

impl PointCloudProcessor {
    /// Create a new GPU point cloud processor
    pub async fn new() -> Result<Self, String> {
        info!("Initializing GPU point cloud processor");

        let (device, queue, gpu_info) =
            gpu::create_low_priority_compute_device("point_cloud_gpu").await?;

        info!(
            adapter_name = %gpu_info.adapter_name,
            adapter_backend = ?gpu_info.backend,
            low_priority = gpu_info.low_priority_enabled,
            "GPU device created for point cloud processing"
        );

        // Create shader module (using concatenated shared geometry + main shader)
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("point_cloud_shader"),
            source: wgpu::ShaderSource::Wgsl(super::point_cloud_shader().into()),
        });

        // Create bind group layout (using shared depth processor layout)
        let bind_group_layout = gpu_utils::create_depth_processor_bind_group_layout(
            &device,
            "point_cloud_bind_group_layout",
        );

        // Create pipeline layout
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("point_cloud_pipeline_layout"),
            bind_group_layouts: &[&bind_group_layout],
            push_constant_ranges: &[],
        });

        // Create render pipeline
        let render_pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some("point_cloud_render_pipeline"),
            layout: Some(&pipeline_layout),
            module: &shader,
            entry_point: Some("main"),
            compilation_options: Default::default(),
            cache: None,
        });

        // Create clear pipeline
        let clear_pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some("point_cloud_clear_pipeline"),
            layout: Some(&pipeline_layout),
            module: &shader,
            entry_point: Some("clear_buffers"),
            compilation_options: Default::default(),
            cache: None,
        });

        // Create uniform buffer
        let uniform_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("point_cloud_uniform_buffer"),
            size: std::mem::size_of::<PointCloudParams>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
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
            registration_data_copy: None,
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
            debug!(rgb_width, rgb_height, "Allocating point cloud RGB buffer");

            // RGB buffer (RGBA u32 per pixel) - sized for RGB resolution
            self.rgb_buffer = Some(self.device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("point_cloud_rgb_buffer"),
                size: rgb_pixels * 4,
                usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            }));

            self.cached_rgb_width = rgb_width;
            self.cached_rgb_height = rgb_height;
        }

        if depth_changed {
            debug!(
                depth_width,
                depth_height, "Allocating point cloud depth buffer"
            );

            // Depth buffer (u32 per pixel) - sized for depth resolution
            self.depth_buffer = Some(self.device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("point_cloud_depth_buffer"),
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
                output_height, "Allocating point cloud output resources"
            );

            // Output texture
            self.output_texture = Some(self.device.create_texture(&wgpu::TextureDescriptor {
                label: Some("point_cloud_output_texture"),
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

            // Depth test buffer (atomic u32 per output pixel)
            self.depth_test_buffer = Some(self.device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("point_cloud_depth_test_buffer"),
                size: output_pixels * 4,
                usage: wgpu::BufferUsages::STORAGE,
                mapped_at_creation: false,
            }));

            // Staging buffer for readback
            self.staging_buffer = Some(self.device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("point_cloud_staging_buffer"),
                size: output_pixels * 4,
                usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
                mapped_at_creation: false,
            }));

            self.cached_output_width = output_width;
            self.cached_output_height = output_height;
        }
    }

    /// Set registration data for depth-to-RGB alignment.
    ///
    /// This should be called once after device initialization with calibration
    /// data from the Kinect device. Once set, the shader will use proper
    /// polynomial registration instead of simple hardcoded shifts.
    pub fn set_registration_data(&mut self, data: &RegistrationData) {
        // Log sample values for debugging
        let center_idx = 240 * 640 + 320;
        let corner_idx = 0;
        info!(
            table_len = data.registration_table.len(),
            shift_len = data.depth_to_rgb_shift.len(),
            target_offset = data.target_offset,
            center_reg_x = data
                .registration_table
                .get(center_idx)
                .map(|v| v[0])
                .unwrap_or(-1),
            center_reg_y = data
                .registration_table
                .get(center_idx)
                .map(|v| v[1])
                .unwrap_or(-1),
            corner_reg_x = data
                .registration_table
                .get(corner_idx)
                .map(|v| v[0])
                .unwrap_or(-1),
            corner_reg_y = data
                .registration_table
                .get(corner_idx)
                .map(|v| v[1])
                .unwrap_or(-1),
            shift_500mm = data.depth_to_rgb_shift.get(500).copied().unwrap_or(-1),
            shift_1000mm = data.depth_to_rgb_shift.get(1000).copied().unwrap_or(-1),
            shift_2000mm = data.depth_to_rgb_shift.get(2000).copied().unwrap_or(-1),
            "Setting registration data for point cloud processor"
        );

        // Create registration buffers using shared utility
        let buffers =
            gpu_utils::create_registration_buffers(&self.device, &self.queue, data, "point_cloud");

        self.registration_table_buffer = Some(buffers.table_buffer);
        self.depth_to_rgb_shift_buffer = Some(buffers.shift_buffer);
        self.registration_target_offset = buffers.target_offset;
        self.has_registration_data = true;

        // Store CPU copy for retrieval
        self.registration_data_copy = Some(RegistrationData {
            registration_table: data.registration_table.clone(),
            depth_to_rgb_shift: data.depth_to_rgb_shift.clone(),
            target_offset: data.target_offset,
        });

        debug!("Registration data uploaded to GPU");
    }

    /// Check if registration data has been set
    pub fn has_registration_data(&self) -> bool {
        self.has_registration_data
    }

    /// Get a clone of the registration data if set
    pub fn get_registration_data(&self) -> Option<RegistrationData> {
        self.registration_data_copy.clone()
    }

    /// Render point cloud from depth + RGB data
    ///
    /// # Arguments
    /// * `rgb_data` - RGBA data (4 bytes per pixel)
    /// * `depth_data` - 16-bit depth values
    /// * `rgb_width` - Width of RGB data (may differ from depth)
    /// * `rgb_height` - Height of RGB data (may differ from depth)
    /// * `depth_width` - Width of depth data (used as input dimensions)
    /// * `depth_height` - Height of depth data (used as input dimensions)
    /// * `output_width` - Width of output image
    /// * `output_height` - Height of output image
    /// * `pitch` - Rotation around X axis (radians)
    /// * `yaw` - Rotation around Y axis (radians)
    /// * `zoom` - Zoom level (1.0 = default, <1.0 = closer, >1.0 = farther)
    /// * `depth_format` - Format of depth data (millimeters or disparity)
    /// * `mirror` - Whether to mirror the point cloud horizontally
    /// * `apply_rgb_registration` - Whether to apply stereo registration (true for RGB camera, false for IR/depth)
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
    ) -> Result<PointCloudResult, String> {
        // Ensure resources are allocated for the various dimensions
        // - RGB buffer sized for RGB resolution
        // - Depth buffer sized for depth resolution (640x480 for Kinect)
        // - Output sized as requested
        self.ensure_resources(
            rgb_width,
            rgb_height,
            depth_width,
            depth_height,
            output_width,
            output_height,
        );

        // Depth dimensions are used for shader iteration (input_width/input_height in shader)
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
        // Kinect defaults are for 640x480
        let scale_x = input_width as f32 / 640.0;
        let scale_y = input_height as f32 / 480.0;

        // Calculate registration scale factors for high-res RGB
        // Registration tables are built for 640x480 RGB. For 1280x1024:
        // - The 640x480 comes from 1280x1024 cropped to 1280x960, then scaled by 0.5
        // - So to map back: scale by 2.0 (not 2.133), top-aligned (offset=0)
        let reg_scale_x = rgb_width as f32 / 640.0;
        // Use 4:3 aspect scaling (960 from 480), not full height scaling
        let reg_scale_y = reg_scale_x; // Same as X to maintain aspect ratio (2.0 for 1280)
        // Top-aligned: the 960 rows start at row 0, extra 64 rows are at bottom
        let reg_y_offset = 0i32;

        debug!(
            rgb_width,
            rgb_height,
            reg_scale_x,
            reg_scale_y,
            reg_y_offset,
            "Registration scaling for RGB resolution"
        );

        // Update uniform buffer with parameters
        let params = PointCloudParams {
            input_width,
            input_height,
            output_width,
            output_height,
            rgb_width,
            rgb_height,
            // Kinect camera intrinsics (scaled for depth resolution)
            fx: kinect::FX * scale_x,
            fy: kinect::FY * scale_y,
            cx: kinect::CX * scale_x,
            cy: kinect::CY * scale_y,
            // Depth format
            depth_format: depth_format as u32,
            // Kinect depth conversion coefficients from libfreenect glpclview.c
            // Formula: depth_m = 1.0 / (raw * coeff_a + coeff_b)
            // These convert the 10-bit disparity values to meters
            depth_coeff_a: kinect::DEPTH_COEFF_A,
            depth_coeff_b: kinect::DEPTH_COEFF_B,
            min_depth: 0.4, // 0.4m minimum (Kinect near limit)
            max_depth: 4.0, // 4.0m maximum (Kinect far limit)
            pitch,
            yaw,
            point_size: 1.0,
            fov: 1.0,            // ~57 degrees
            view_distance: zoom, // Camera Z position: 0=sensor position, higher=into scene
            // Registration parameters
            // Only apply registration if we have tables AND the color source is from RGB camera
            // (IR data is already aligned with depth, so no registration needed)
            use_registration_tables: if self.has_registration_data && apply_rgb_registration {
                1
            } else {
                0
            },
            target_offset: self.registration_target_offset,
            reg_x_val_scale: 256, // freedepth::REG_X_VAL_SCALE
            mirror: if mirror { 1 } else { 0 },
            // High-res RGB scaling
            reg_scale_x,
            reg_scale_y,
            reg_y_offset,
        };
        self.queue
            .write_buffer(&self.uniform_buffer, 0, bytemuck::bytes_of(&params));

        // Create bind group
        let output_view = output_texture.create_view(&wgpu::TextureViewDescriptor::default());

        // Get registration buffers or create dummy buffers if not set
        let (reg_table_buffer, shift_buffer) = if self.has_registration_data {
            (
                self.registration_table_buffer.as_ref().unwrap(),
                self.depth_to_rgb_shift_buffer.as_ref().unwrap(),
            )
        } else {
            // Create minimal dummy buffers for bind group compatibility
            // These won't be used when use_registration_tables == 0
            if self.registration_table_buffer.is_none() {
                self.registration_table_buffer =
                    Some(self.device.create_buffer(&wgpu::BufferDescriptor {
                        label: Some("dummy_registration_table_buffer"),
                        size: 8, // Minimum size
                        usage: wgpu::BufferUsages::STORAGE,
                        mapped_at_creation: false,
                    }));
            }
            if self.depth_to_rgb_shift_buffer.is_none() {
                self.depth_to_rgb_shift_buffer =
                    Some(self.device.create_buffer(&wgpu::BufferDescriptor {
                        label: Some("dummy_depth_to_rgb_shift_buffer"),
                        size: 8, // Minimum size
                        usage: wgpu::BufferUsages::STORAGE,
                        mapped_at_creation: false,
                    }));
            }
            (
                self.registration_table_buffer.as_ref().unwrap(),
                self.depth_to_rgb_shift_buffer.as_ref().unwrap(),
            )
        };

        let bind_group = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("point_cloud_bind_group"),
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
                label: Some("point_cloud_encoder"),
            });

        // Pass 1: Clear buffers and output
        {
            let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some("point_cloud_clear_pass"),
                timestamp_writes: None,
            });
            pass.set_pipeline(&self.clear_pipeline);
            pass.set_bind_group(0, Some(&bind_group), &[]);
            let workgroups_x = compute_dispatch_size(output_width, 16);
            let workgroups_y = compute_dispatch_size(output_height, 16);
            pass.dispatch_workgroups(workgroups_x, workgroups_y, 1);
        }

        // Pass 2: Render point cloud
        {
            let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some("point_cloud_render_pass"),
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

        // Map and read back
        let staging_slice = staging_buffer.slice(..);
        let (sender, receiver) = futures::channel::oneshot::channel();

        staging_slice.map_async(wgpu::MapMode::Read, move |result| {
            let _ = sender.send(result);
        });

        let _ = self.device.poll(wgpu::PollType::wait_indefinitely());

        receiver
            .await
            .map_err(|_| "Failed to receive staging buffer mapping")?
            .map_err(|e| format!("Failed to map staging buffer: {:?}", e))?;

        let rgba = staging_slice.get_mapped_range().to_vec();
        staging_buffer.unmap();

        Ok(PointCloudResult {
            rgba,
            width: output_width,
            height: output_height,
        })
    }
}

// Use the shared singleton macro for GPU processor management
gpu_processor_singleton!(
    PointCloudProcessor,
    GPU_POINT_CLOUD_PROCESSOR,
    get_point_cloud_processor
);

/// Set registration data for depth-to-RGB alignment on the shared GPU processor.
///
/// This should be called once after the Kinect device starts streaming, with
/// calibration data fetched from the device. This enables proper depth-to-RGB
/// alignment using device-specific polynomial registration tables.
pub async fn set_point_cloud_registration_data(data: &RegistrationData) -> Result<(), String> {
    let mut guard = get_point_cloud_processor().await?;
    let processor = guard
        .as_mut()
        .ok_or("GPU point cloud processor not initialized")?;

    processor.set_registration_data(data);
    Ok(())
}

/// Check if the shared GPU processor has registration data set.
pub async fn has_point_cloud_registration_data() -> Result<bool, String> {
    let guard = get_point_cloud_processor().await?;
    let processor = guard
        .as_ref()
        .ok_or("GPU point cloud processor not initialized")?;

    Ok(processor.has_registration_data())
}

/// Get the registration data from the shared GPU processor if set.
///
/// This is useful for scene capture to use the same registration data as the preview shader.
pub async fn get_point_cloud_registration_data() -> Result<Option<RegistrationData>, String> {
    let guard = get_point_cloud_processor().await?;
    let processor = guard
        .as_ref()
        .ok_or("GPU point cloud processor not initialized")?;

    Ok(processor.get_registration_data())
}

/// Render point cloud using the shared GPU processor
///
/// # Arguments
/// * `rgb_data` - RGBA data (4 bytes per pixel)
/// * `depth_data` - 16-bit depth values
/// * `rgb_width` - Width of RGB data
/// * `rgb_height` - Height of RGB data
/// * `depth_width` - Width of depth data
/// * `depth_height` - Height of depth data
/// * `output_width` - Width of output image
/// * `output_height` - Height of output image
/// * `pitch` - Rotation around X axis (radians)
/// * `yaw` - Rotation around Y axis (radians)
/// * `zoom` - Zoom level (1.0 = default, <1.0 = closer, >1.0 = farther)
/// * `depth_format` - Format of depth data (millimeters or disparity)
/// * `mirror` - Whether to mirror the point cloud horizontally
/// * `apply_rgb_registration` - Whether to apply stereo registration (true for RGB camera, false for IR/depth)
pub async fn render_point_cloud(
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
) -> Result<PointCloudResult, String> {
    let mut guard = get_point_cloud_processor().await?;
    let processor = guard
        .as_mut()
        .ok_or("GPU point cloud processor not initialized")?;

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
        )
        .await
}
