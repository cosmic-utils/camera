// SPDX-License-Identifier: GPL-3.0-only
//
// Shared GPU helper utilities for burst mode pipeline
//
// Provides common bind group layout creation patterns used by both
// the main pipeline (mod.rs) and FFT merge pipeline (fft_gpu.rs).

use crate::gpu::wgpu;

/// Buffer binding type for bind group layout creation
#[derive(Clone, Copy)]
pub enum BindingKind {
    /// Read-only storage buffer
    StorageRead,
    /// Read-write storage buffer
    StorageReadWrite,
    /// Uniform buffer
    Uniform,
}

/// Create a bind group layout entry with common defaults
pub fn layout_entry(binding: u32, kind: BindingKind) -> wgpu::BindGroupLayoutEntry {
    let ty = match kind {
        BindingKind::StorageRead => wgpu::BufferBindingType::Storage { read_only: true },
        BindingKind::StorageReadWrite => wgpu::BufferBindingType::Storage { read_only: false },
        BindingKind::Uniform => wgpu::BufferBindingType::Uniform,
    };
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::COMPUTE,
        ty: wgpu::BindingType::Buffer {
            ty,
            has_dynamic_offset: false,
            min_binding_size: None,
        },
        count: None,
    }
}

/// Create a bind group layout from a specification of binding kinds
///
/// This consolidates the repetitive layout creation pattern into a single function.
pub fn create_layout(
    device: &wgpu::Device,
    label: &str,
    bindings: &[BindingKind],
) -> wgpu::BindGroupLayout {
    let entries: Vec<_> = bindings
        .iter()
        .enumerate()
        .map(|(i, kind)| layout_entry(i as u32, *kind))
        .collect();
    device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some(label),
        entries: &entries,
    })
}

/// Create a compute pipeline with common defaults
pub fn create_pipeline(
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
