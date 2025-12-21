// SPDX-License-Identifier: GPL-3.0-only

//! Shared GPU processor infrastructure
//!
//! Provides common functionality for all GPU compute processors:
//! - Singleton management (OnceLock<Mutex<Option<T>>>)
//! - Device/queue creation with low-priority settings
//! - Buffer/texture allocation with dimension caching
//! - Async buffer readback utilities
//!
//! This module reduces code duplication across the depth, point_cloud,
//! mesh, and yuv_convert processors.

use crate::gpu::wgpu;
use std::sync::Arc;

/// Cached resource dimensions - avoids reallocation when dimensions match
///
/// Used by processors to track if buffers need to be recreated when
/// input/output dimensions change.
#[derive(Default, Clone, Copy, PartialEq, Debug)]
pub struct CachedDimensions {
    pub width: u32,
    pub height: u32,
}

impl CachedDimensions {
    /// Create new cached dimensions
    pub fn new(width: u32, height: u32) -> Self {
        Self { width, height }
    }

    /// Check if dimensions have changed and need update
    pub fn needs_update(&self, width: u32, height: u32) -> bool {
        self.width != width || self.height != height
    }

    /// Update cached dimensions
    pub fn update(&mut self, width: u32, height: u32) {
        self.width = width;
        self.height = height;
    }

    /// Check if dimensions are initialized (non-zero)
    pub fn is_initialized(&self) -> bool {
        self.width > 0 && self.height > 0
    }
}

/// Helper for async buffer readback (map, poll, read, unmap)
///
/// This is the common pattern used by all GPU processors to read data back
/// from GPU buffers to CPU memory.
///
/// # Arguments
/// * `device` - The wgpu device for polling
/// * `buffer` - The buffer to read from (must be MAP_READ)
///
/// # Returns
/// The buffer contents as a Vec<u8>
pub async fn read_buffer_async(
    device: &wgpu::Device,
    buffer: &wgpu::Buffer,
) -> Result<Vec<u8>, String> {
    let slice = buffer.slice(..);
    let (sender, receiver) = futures::channel::oneshot::channel();

    slice.map_async(wgpu::MapMode::Read, move |result| {
        let _ = sender.send(result);
    });

    let _ = device.poll(wgpu::PollType::wait_indefinitely());

    receiver
        .await
        .map_err(|_| "Failed to receive buffer mapping".to_string())?
        .map_err(|e| format!("Failed to map buffer: {:?}", e))?;

    let data = slice.get_mapped_range().to_vec();
    buffer.unmap();

    Ok(data)
}

/// Helper for texture-to-buffer copy and async readback
///
/// Copies texture contents to a staging buffer and reads back to CPU.
///
/// # Arguments
/// * `device` - The wgpu device
/// * `queue` - The wgpu queue
/// * `texture` - Source texture to read from
/// * `staging_buffer` - Staging buffer for readback (must be MAP_READ)
/// * `width` - Texture width
/// * `height` - Texture height
///
/// # Returns
/// The texture contents as a Vec<u8> (RGBA format, 4 bytes per pixel)
pub async fn read_texture_async(
    device: &Arc<wgpu::Device>,
    queue: &Arc<wgpu::Queue>,
    texture: &wgpu::Texture,
    staging_buffer: &wgpu::Buffer,
    width: u32,
    height: u32,
) -> Result<Vec<u8>, String> {
    let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
        label: Some("texture_readback_encoder"),
    });

    encoder.copy_texture_to_buffer(
        wgpu::TexelCopyTextureInfo {
            texture,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        wgpu::TexelCopyBufferInfo {
            buffer: staging_buffer,
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

    queue.submit(std::iter::once(encoder.finish()));
    read_buffer_async(device, staging_buffer).await
}

/// Calculate compute shader dispatch size (workgroups needed)
///
/// Given a dimension and workgroup size, returns the number of workgroups
/// needed to cover the entire dimension.
///
/// # Arguments
/// * `dimension` - The dimension to cover (width or height)
/// * `workgroup_size` - The workgroup size (typically 16)
///
/// # Returns
/// Number of workgroups needed
#[inline]
pub fn compute_dispatch_size(dimension: u32, workgroup_size: u32) -> u32 {
    dimension.div_ceil(workgroup_size)
}

/// Macro for generating singleton accessor functions
///
/// This eliminates the ~20 lines of boilerplate per processor for
/// singleton management. Each processor needs:
/// - A static OnceLock<Mutex<Option<Processor>>>
/// - A get_processor() function that lazily initializes
///
/// # Example
/// ```ignore
/// gpu_processor_singleton!(DepthProcessor, GPU_DEPTH_PROCESSOR, get_depth_processor);
/// ```
#[macro_export]
macro_rules! gpu_processor_singleton {
    ($processor:ty, $static_name:ident, $get_fn:ident) => {
        /// Cached GPU processor instance
        static $static_name: std::sync::OnceLock<tokio::sync::Mutex<Option<$processor>>> =
            std::sync::OnceLock::new();

        /// Get or create the shared GPU processor instance
        pub async fn $get_fn()
        -> Result<tokio::sync::MutexGuard<'static, Option<$processor>>, String> {
            let lock = $static_name.get_or_init(|| tokio::sync::Mutex::new(None));
            let mut guard = lock.lock().await;

            if guard.is_none() {
                match <$processor>::new().await {
                    Ok(processor) => {
                        *guard = Some(processor);
                    }
                    Err(e) => {
                        tracing::warn!(
                            concat!("Failed to initialize GPU ", stringify!($processor), ": {}"),
                            e
                        );
                        return Err(e);
                    }
                }
            }

            Ok(guard)
        }
    };
}

/// Create a storage buffer with the given size
///
/// # Arguments
/// * `device` - The wgpu device
/// * `size` - Buffer size in bytes
/// * `label` - Debug label for the buffer
pub fn create_storage_buffer(device: &wgpu::Device, size: u64, label: &str) -> wgpu::Buffer {
    device.create_buffer(&wgpu::BufferDescriptor {
        label: Some(label),
        size,
        usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    })
}

/// Create a staging buffer for GPU readback
///
/// # Arguments
/// * `device` - The wgpu device
/// * `size` - Buffer size in bytes
/// * `label` - Debug label for the buffer
pub fn create_staging_buffer(device: &wgpu::Device, size: u64, label: &str) -> wgpu::Buffer {
    device.create_buffer(&wgpu::BufferDescriptor {
        label: Some(label),
        size,
        usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    })
}

/// Create an RGBA storage texture
///
/// # Arguments
/// * `device` - The wgpu device
/// * `width` - Texture width
/// * `height` - Texture height
/// * `label` - Debug label for the texture
pub fn create_rgba_storage_texture(
    device: &wgpu::Device,
    width: u32,
    height: u32,
    label: &str,
) -> wgpu::Texture {
    device.create_texture(&wgpu::TextureDescriptor {
        label: Some(label),
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
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_cached_dimensions() {
        let mut dims = CachedDimensions::default();
        assert!(!dims.is_initialized());
        assert!(dims.needs_update(640, 480));

        dims.update(640, 480);
        assert!(dims.is_initialized());
        assert!(!dims.needs_update(640, 480));
        assert!(dims.needs_update(1280, 720));
    }

    #[test]
    fn test_compute_dispatch_size() {
        assert_eq!(compute_dispatch_size(640, 16), 40);
        assert_eq!(compute_dispatch_size(641, 16), 41);
        assert_eq!(compute_dispatch_size(16, 16), 1);
        assert_eq!(compute_dispatch_size(1, 16), 1);
    }
}
