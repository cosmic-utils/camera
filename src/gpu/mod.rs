// SPDX-License-Identifier: GPL-3.0-only

//! GPU initialization utilities for compute pipelines.
//!
//! This module provides helpers for creating wgpu devices for compute operations.
//! Uses the same wgpu instance as libcosmic's UI rendering.

use std::sync::Arc;
use tokio::sync::OnceCell;
use tracing::{debug, info};

/// Re-export wgpu types from cosmic for use in compute pipelines
pub use cosmic::iced_wgpu::wgpu;

/// Information about the created GPU device
#[derive(Debug, Clone)]
pub struct GpuDeviceInfo {
    /// Name of the GPU adapter
    pub adapter_name: String,
    /// Backend being used (Vulkan, Metal, DX12, etc.)
    pub backend: wgpu::Backend,
    /// Whether low-priority queue was successfully configured (always false now)
    pub low_priority_enabled: bool,
}

/// Shared GPU context holding a single device and queue for all compute pipelines.
#[derive(Clone)]
pub struct SharedGpuContext {
    /// The shared wgpu device
    pub device: Arc<wgpu::Device>,
    /// The shared wgpu queue
    pub queue: Arc<wgpu::Queue>,
    /// Information about the GPU adapter
    pub info: GpuDeviceInfo,
}

/// Lazy-initialized shared GPU device singleton.
static SHARED_GPU: OnceCell<Result<SharedGpuContext, String>> = OnceCell::const_new();

/// Get or create the shared GPU device and queue for compute work.
///
/// All compute pipelines (filter, histogram, conversion, virtual camera) share
/// a single wgpu device instance to reduce resource usage.
pub async fn get_shared_gpu() -> Result<SharedGpuContext, String> {
    SHARED_GPU
        .get_or_init(|| async {
            create_low_priority_compute_device("shared_compute")
                .await
                .map(|(device, queue, info)| SharedGpuContext {
                    device,
                    queue,
                    info,
                })
        })
        .await
        .clone()
}

/// Create a wgpu device and queue for compute work.
///
/// This is a private helper used by the shared GPU singleton.
/// External code should use [`get_shared_gpu`] instead.
async fn create_low_priority_compute_device(
    label: &str,
) -> Result<(Arc<wgpu::Device>, Arc<wgpu::Queue>, GpuDeviceInfo), String> {
    info!(label = label, "Creating GPU device for compute");

    let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor {
        backends: wgpu::Backends::VULKAN,
        ..Default::default()
    });

    let adapter = instance
        .request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::HighPerformance,
            compatible_surface: None,
            force_fallback_adapter: false,
        })
        .await
        .map_err(|e| format!("Failed to find suitable GPU adapter: {}", e))?;

    let adapter_info = adapter.get_info();
    let adapter_limits = adapter.limits();

    info!(
        adapter = %adapter_info.name,
        backend = ?adapter_info.backend,
        "GPU adapter selected for compute"
    );

    debug!(
        backend = ?adapter_info.backend,
        "Using standard device creation"
    );

    let (device, queue) = adapter
        .request_device(&wgpu::DeviceDescriptor {
            label: Some(label),
            required_features: adapter.features() & wgpu::Features::TEXTURE_FORMAT_16BIT_NORM,
            required_limits: adapter_limits.clone(),
            memory_hints: wgpu::MemoryHints::Performance,
            trace: wgpu::Trace::Off,
            experimental_features: wgpu::ExperimentalFeatures::disabled(),
        })
        .await
        .map_err(|e| format!("Failed to create GPU device: {}", e))?;

    let info = GpuDeviceInfo {
        adapter_name: adapter_info.name.clone(),
        backend: adapter_info.backend,
        low_priority_enabled: false,
    };

    Ok((Arc::new(device), Arc::new(queue), info))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_create_low_priority_device() {
        // This test requires a GPU, so it may be skipped in CI
        match create_low_priority_compute_device("test_device").await {
            Ok((device, queue, info)) => {
                println!("Created device: {:?}", info);
                assert!(!info.adapter_name.is_empty());
                // Device and queue should be usable
                drop(queue);
                drop(device);
            }
            Err(e) => {
                // Skip if no GPU available
                println!("Skipping test (no GPU): {}", e);
            }
        }
    }
}
