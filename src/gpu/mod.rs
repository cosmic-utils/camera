// SPDX-License-Identifier: GPL-3.0-only

//! GPU initialization utilities for compute pipelines.
//!
//! This module provides helpers for creating wgpu devices for compute operations.
//! Uses the same wgpu instance as libcosmic's UI rendering.

use std::sync::Arc;
use tracing::{debug, info};

/// Re-export wgpu types from cosmic for use in compute pipelines
pub use cosmic::iced_wgpu::wgpu;

/// Information about the created GPU device
#[derive(Debug)]
pub struct GpuDeviceInfo {
    /// Name of the GPU adapter
    pub adapter_name: String,
    /// Backend being used (Vulkan, Metal, DX12, etc.)
    pub backend: wgpu::Backend,
    /// Whether low-priority queue was successfully configured (always false now)
    pub low_priority_enabled: bool,
}

/// Create a wgpu device and queue for compute work.
///
/// Uses standard device creation through cosmic's wgpu.
///
/// # Arguments
///
/// * `label` - A label for the device (for debugging)
///
/// # Returns
///
/// A tuple of (Device, Queue, GpuDeviceInfo) or an error message
pub async fn create_low_priority_compute_device(
    label: &str,
) -> Result<(Arc<wgpu::Device>, Arc<wgpu::Queue>, GpuDeviceInfo), String> {
    info!(label = label, "Creating GPU device for compute");

    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
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
        .ok_or_else(|| "Failed to find suitable GPU adapter".to_string())?;

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
        .request_device(
            &wgpu::DeviceDescriptor {
                label: Some(label),
                required_features: wgpu::Features::empty(),
                required_limits: adapter_limits.clone(),
                memory_hints: wgpu::MemoryHints::Performance,
                ..Default::default()
            },
            None,
        )
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
