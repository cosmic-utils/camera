// SPDX-License-Identifier: GPL-3.0-only

//! GPU initialization utilities for compute pipelines.
//!
//! This module provides helpers for creating wgpu devices with low-priority queues
//! on backends that support it (Vulkan). This prevents compute-heavy operations
//! like burst mode processing from starving the UI rendering.
//!
//! # Architecture
//!
//! - Uses wgpu v27 (separate from libcosmic's wgpu for UI)
//! - On Vulkan: Sets queue priority to 0.2 (low) via `open_with_callback`
//! - On other backends: Falls back to standard device creation

use std::sync::Arc;
use tracing::{debug, info, warn};

/// Re-export wgpu-compute types for use in compute pipelines
pub use wgpu_compute as wgpu;

/// Default low priority value for compute queues (0.0=lowest, 1.0=highest)
/// Using 0.2 to give UI rendering priority while still making progress
pub const LOW_PRIORITY: f32 = 0.2;

/// Static array for low priority value to ensure lifetime outlives device creation
static LOW_PRIORITY_ARRAY: [f32; 1] = [LOW_PRIORITY];

/// Information about the created GPU device
#[derive(Debug)]
pub struct GpuDeviceInfo {
    /// Name of the GPU adapter
    pub adapter_name: String,
    /// Backend being used (Vulkan, Metal, DX12, etc.)
    pub backend: wgpu_compute::Backend,
    /// Whether low-priority queue was successfully configured
    pub low_priority_enabled: bool,
}

/// Create a wgpu device and queue optimized for background compute work.
///
/// On Vulkan backends, this sets the queue priority to [`LOW_PRIORITY`] (0.2)
/// so that compute operations don't starve UI rendering. On other backends,
/// it falls back to standard device creation.
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
) -> Result<
    (
        Arc<wgpu_compute::Device>,
        Arc<wgpu_compute::Queue>,
        GpuDeviceInfo,
    ),
    String,
> {
    info!(
        label = label,
        "Creating low-priority GPU device for compute"
    );

    let instance = wgpu_compute::Instance::new(&wgpu_compute::InstanceDescriptor {
        backends: wgpu_compute::Backends::VULKAN,
        ..Default::default()
    });

    let adapter = instance
        .request_adapter(&wgpu_compute::RequestAdapterOptions {
            power_preference: wgpu_compute::PowerPreference::HighPerformance,
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

    // Try to create device with low-priority queue on Vulkan
    let (device, queue, low_priority_enabled) =
        if adapter_info.backend == wgpu_compute::Backend::Vulkan {
            match create_vulkan_low_priority_device(&adapter, label, &adapter_limits).await {
                Ok((device, queue)) => {
                    info!("Successfully created Vulkan device with low-priority queue");
                    (device, queue, true)
                }
                Err(e) => {
                    warn!(
                        error = %e,
                        "Failed to create low-priority Vulkan device, falling back to standard"
                    );
                    let (device, queue) =
                        create_standard_device(&adapter, label, &adapter_limits).await?;
                    (device, queue, false)
                }
            }
        } else {
            debug!(
                backend = ?adapter_info.backend,
                "Non-Vulkan backend, using standard device creation"
            );
            let (device, queue) = create_standard_device(&adapter, label, &adapter_limits).await?;
            (device, queue, false)
        };

    let info = GpuDeviceInfo {
        adapter_name: adapter_info.name.clone(),
        backend: adapter_info.backend,
        low_priority_enabled,
    };

    Ok((Arc::new(device), Arc::new(queue), info))
}

/// VK_EXT_global_priority extension name
const VK_EXT_GLOBAL_PRIORITY: &std::ffi::CStr =
    unsafe { std::ffi::CStr::from_bytes_with_nul_unchecked(b"VK_EXT_global_priority\0") };

/// Create a Vulkan device with LOW global priority using VK_EXT_global_priority.
///
/// Global priority affects GPU scheduling across ALL processes and devices, not just
/// within a single device. This is the correct way to make our compute work yield to
/// the compositor's rendering.
///
/// Priority levels (KHR/EXT):
/// - REALTIME - Highest, requires special permissions
/// - HIGH - Above normal
/// - MEDIUM - Default for most applications
/// - LOW - What we use for background compute
///
/// The compositor typically runs at MEDIUM or HIGH priority, so our LOW priority
/// compute work will be scheduled after the compositor's rendering.
///
/// We also attempt to use the compute-only queue family (Family 1 on AMD GPUs) which
/// has dedicated compute hardware separate from the graphics pipeline.
async fn create_vulkan_low_priority_device(
    adapter: &wgpu_compute::Adapter,
    label: &str,
    limits: &wgpu_compute::Limits,
) -> Result<(wgpu_compute::Device, wgpu_compute::Queue), String> {
    use wgpu_compute::hal::api::Vulkan;

    // Access the HAL adapter
    let hal_adapter_guard = unsafe {
        adapter
            .as_hal::<Vulkan>()
            .ok_or("Failed to get Vulkan HAL adapter")?
    };

    // Create device with callback that enables global priority extension
    // The callback sets LOW global priority which affects GPU-wide scheduling
    let hal_device = unsafe {
        hal_adapter_guard
            .open_with_callback(
                wgpu_compute::Features::empty(),
                &wgpu_compute::MemoryHints::Performance,
                Some(Box::new(move |args| {
                    // Add VK_EXT_global_priority extension
                    args.extensions.push(VK_EXT_GLOBAL_PRIORITY);
                    tracing::info!("Enabled VK_EXT_global_priority extension");

                    // Create global priority info with LOW priority
                    // Leak the Box FIRST to get a 'static reference that satisfies the 'pnext lifetime
                    let priority_info: &'static mut ash::vk::DeviceQueueGlobalPriorityCreateInfoKHR =
                        Box::leak(Box::new(
                            ash::vk::DeviceQueueGlobalPriorityCreateInfoKHR::default()
                                .global_priority(ash::vk::QueueGlobalPriorityKHR::LOW)
                        ));

                    // Set LOW global priority on the default queue family.
                    // We don't change the queue family index because wgpu's internals
                    // may have assumptions about the queue family being used.
                    if let Some(queue_info) = args.queue_create_infos.first_mut() {
                        let family = queue_info.queue_family_index;

                        // Rebuild queue info with low priority on same family
                        *queue_info = ash::vk::DeviceQueueCreateInfo::default()
                            .queue_family_index(family)
                            .queue_priorities(&LOW_PRIORITY_ARRAY)
                            .push_next(priority_info);

                        tracing::info!(
                            queue_family = family,
                            local_priority = LOW_PRIORITY,
                            global_priority = "LOW",
                            "Set LOW global priority on queue family {}",
                            family
                        );
                    }
                })),
            )
            .map_err(|e| format!("Failed to open Vulkan device with callback: {:?}", e))?
    };

    // Drop the guard before creating device from HAL
    drop(hal_adapter_guard);

    // Create wgpu device from HAL device
    let (device, queue) = unsafe {
        adapter
            .create_device_from_hal(
                hal_device,
                &wgpu_compute::DeviceDescriptor {
                    label: Some(label),
                    required_features: wgpu_compute::Features::empty(),
                    required_limits: limits.clone(),
                    memory_hints: wgpu_compute::MemoryHints::Performance,
                    ..Default::default()
                },
            )
            .map_err(|e| format!("Failed to create device from HAL: {}", e))?
    };

    Ok((device, queue))
}

/// Create a standard wgpu device without priority modifications
async fn create_standard_device(
    adapter: &wgpu_compute::Adapter,
    label: &str,
    limits: &wgpu_compute::Limits,
) -> Result<(wgpu_compute::Device, wgpu_compute::Queue), String> {
    adapter
        .request_device(&wgpu_compute::DeviceDescriptor {
            label: Some(label),
            required_features: wgpu_compute::Features::empty(),
            required_limits: limits.clone(),
            memory_hints: wgpu_compute::MemoryHints::Performance,
            ..Default::default()
        })
        .await
        .map_err(|e| format!("Failed to create GPU device: {}", e))
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
