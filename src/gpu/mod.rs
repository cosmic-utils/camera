// SPDX-License-Identifier: GPL-3.0-only

//! GPU initialization utilities for compute pipelines.
//!
//! All compute work (burst-mode HDR+, virtual-camera filter, histogram
//! analysis) runs through a single shared `wgpu::Device` accessed via
//! [`get_shared_gpu`].
//!
//! # Device sharing with the renderer
//!
//! When the GUI is active, [`try_seed_shared_gpu_from_renderer`] seeds the
//! shared device with the renderer's own `Device`/`Queue` (handed to us by
//! `iced_wgpu` inside a primitive's `prepare()` callback). This avoids the
//! cost of opening a second `wgpu::Instance` and lets compute outputs stay
//! on the same device the renderer samples from — no extra Vulkan dispatch
//! tables, no cross-instance handoffs.
//!
//! When no renderer is up (CLI `process burst-mode`, headless tests),
//! [`get_shared_gpu`] falls back to creating its own compute-only device.

use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{Notify, OnceCell};
use tracing::{debug, info};

/// Re-export wgpu types from cosmic for use in compute pipelines
pub use iced_wgpu::wgpu;

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

/// Fires when `try_seed_shared_gpu_from_renderer` successfully seeds the
/// singleton. Lets [`wait_for_renderer_seed_or_timeout`] block warmup until
/// the renderer is ready, avoiding the race where startup warmup creates a
/// fresh compute device before the first `VideoPrimitive::prepare()` runs.
static SEED_NOTIFY: Notify = Notify::const_new();

/// Seed the shared GPU singleton with the renderer's own device and queue.
///
/// Call from the first render-side `prepare()` (or similar) so subsequent
/// compute work shares the same GPU context as iced/libcosmic — no second
/// `wgpu::Instance`, no second device, no CPU round-trip between contexts.
///
/// Returns `true` if this call performed the seeding, `false` if the shared
/// GPU was already initialized (subsequent calls are no-ops).
pub fn try_seed_shared_gpu_from_renderer(device: Arc<wgpu::Device>, queue: Arc<wgpu::Queue>) {
    let ctx = SharedGpuContext {
        device,
        queue,
        info: GpuDeviceInfo {
            // The renderer doesn't expose adapter info to us at this point.
            adapter_name: "renderer-shared".to_string(),
            backend: wgpu::Backend::Vulkan,
            low_priority_enabled: false,
        },
    };
    match SHARED_GPU.set(Ok(ctx)) {
        Ok(()) => {
            info!("Seeded shared GPU context from renderer device");
            SEED_NOTIFY.notify_waiters();
        }
        Err(_) => debug!("Shared GPU context already initialised; renderer seed ignored"),
    }
}

/// Wait for the renderer to seed the shared GPU, or fall back after `timeout`.
///
/// Call this from the startup warmup task before touching [`get_shared_gpu`]:
/// the GUI renderer will seed the singleton shortly after the first frame
/// arrives, and we want the warmup to bind its pipelines to *that* device
/// rather than a fresh compute-only one. If no renderer is up (headless
/// tests, libcosmic failed to start), the timeout lets warmup fall through
/// and `get_shared_gpu` creates its own device.
///
/// Subscribing to the `Notify` *before* checking the cell is important —
/// the renderer may seed between the cell check and the await, and a
/// `Notify` doesn't store permits for `notify_waiters`. Same pattern as the
/// still-frame notifier in `wait_for_still_frame`.
pub async fn wait_for_renderer_seed_or_timeout(timeout: Duration) {
    let notified = SEED_NOTIFY.notified();
    if SHARED_GPU.get().is_some() {
        return;
    }
    if tokio::time::timeout(timeout, notified).await.is_err() {
        debug!(
            timeout_ms = timeout.as_millis() as u64,
            "No renderer seed within timeout; warmup will create its own compute device"
        );
    }
}

/// Get or create the shared GPU device and queue for compute work.
///
/// When [`try_seed_shared_gpu_from_renderer`] has already been called, returns
/// the renderer-shared device. Otherwise creates a compute-only fallback —
/// used by `camera process burst-mode` and tests where no renderer is up.
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
