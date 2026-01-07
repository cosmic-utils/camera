// SPDX-License-Identifier: GPL-3.0-only

//! Shared GPU utilities for depth processors (point cloud, mesh)
//!
//! Provides common functionality used by both point cloud and mesh GPU processors:
//! - Bind group layout creation
//! - Registration buffer management
//! - Workgroup dispatch calculations

use crate::gpu::wgpu;
#[cfg(target_arch = "x86_64")]
use crate::shaders::point_cloud::RegistrationData;
#[cfg(not(target_arch = "x86_64"))]
use crate::shaders::RegistrationData;

/// Create the standard bind group layout for depth processors.
///
/// Both point cloud and mesh processors use identical bind group layouts with 7 bindings:
/// - 0: RGB input buffer (storage, read-only)
/// - 1: Depth input buffer (storage, read-only)
/// - 2: Output texture (storage texture, write-only)
/// - 3: Depth test buffer (storage, read-write, atomic)
/// - 4: Uniform parameters
/// - 5: Registration table buffer (storage, read-only)
/// - 6: Depth-to-RGB shift buffer (storage, read-only)
pub fn create_depth_processor_bind_group_layout(
    device: &wgpu::Device,
    label: &str,
) -> wgpu::BindGroupLayout {
    device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some(label),
        entries: &[
            // RGB input buffer
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
            // Depth input buffer
            wgpu::BindGroupLayoutEntry {
                binding: 1,
                visibility: wgpu::ShaderStages::COMPUTE,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Storage { read_only: true },
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            },
            // Output texture
            wgpu::BindGroupLayoutEntry {
                binding: 2,
                visibility: wgpu::ShaderStages::COMPUTE,
                ty: wgpu::BindingType::StorageTexture {
                    access: wgpu::StorageTextureAccess::WriteOnly,
                    format: wgpu::TextureFormat::Rgba8Unorm,
                    view_dimension: wgpu::TextureViewDimension::D2,
                },
                count: None,
            },
            // Depth test buffer (atomic)
            wgpu::BindGroupLayoutEntry {
                binding: 3,
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
                binding: 4,
                visibility: wgpu::ShaderStages::COMPUTE,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            },
            // Registration table buffer (640*480 [x,y] pairs)
            wgpu::BindGroupLayoutEntry {
                binding: 5,
                visibility: wgpu::ShaderStages::COMPUTE,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Storage { read_only: true },
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            },
            // Depth-to-RGB shift buffer (10001 i32 values)
            wgpu::BindGroupLayoutEntry {
                binding: 6,
                visibility: wgpu::ShaderStages::COMPUTE,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Storage { read_only: true },
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            },
        ],
    })
}

/// Registration buffers for GPU depth-to-RGB alignment
pub struct RegistrationBuffers {
    /// Registration table buffer: 640*480 [x_scaled, y] pairs (interleaved i32)
    pub table_buffer: wgpu::Buffer,
    /// Depth-to-RGB shift buffer: 10001 i32 values indexed by depth_mm
    pub shift_buffer: wgpu::Buffer,
    /// Target offset from pad_info
    pub target_offset: u32,
}

/// Create registration buffers from registration data.
///
/// The registration table is flattened from Vec<[i32; 2]> to Vec<i32> (interleaved x, y).
pub fn create_registration_buffers(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    data: &RegistrationData,
    label_prefix: &str,
) -> RegistrationBuffers {
    // Flatten registration table: [[x1, y1], [x2, y2], ...] -> [x1, y1, x2, y2, ...]
    let reg_table_data: Vec<i32> = data
        .registration_table
        .iter()
        .flat_map(|&[x, y]| [x, y])
        .collect();

    let table_buffer = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some(&format!("{}_registration_table_buffer", label_prefix)),
        size: (reg_table_data.len() * std::mem::size_of::<i32>()) as u64,
        usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });

    queue.write_buffer(&table_buffer, 0, bytemuck::cast_slice(&reg_table_data));

    let shift_buffer = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some(&format!("{}_depth_to_rgb_shift_buffer", label_prefix)),
        size: (data.depth_to_rgb_shift.len() * std::mem::size_of::<i32>()) as u64,
        usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });

    queue.write_buffer(
        &shift_buffer,
        0,
        bytemuck::cast_slice(&data.depth_to_rgb_shift),
    );

    RegistrationBuffers {
        table_buffer,
        shift_buffer,
        target_offset: data.target_offset,
    }
}
