// SPDX-License-Identifier: GPL-3.0-only
//
// Consolidated GPU parameter structs for burst mode pipeline
//
// All #[repr(C)] structs that are passed to WGSL shaders are defined here.
// This ensures:
// 1. Single source of truth for struct layouts
// 2. Easy to add size assertions to catch WGSL/Rust mismatches
// 3. Reduced duplication between mod.rs and fft_gpu.rs

/// Parameters for luminance extraction shader
#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
pub struct LuminanceParams {
    pub width: u32,
    pub height: u32,
    /// Channel to extract: 0=R, 1=G, 2=B, 3=luminance
    pub channel: u32,
    pub _padding1: u32,
}

/// Parameters for pyramid downsampling shader
#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
pub struct PyramidParams {
    pub src_width: u32,
    pub src_height: u32,
    pub dst_width: u32,
    pub dst_height: u32,
}

/// Parameters for tile alignment shader
#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
pub struct AlignParams {
    pub width: u32,
    pub height: u32,
    pub tile_size: u32,
    pub tile_step: u32,
    pub search_dist: u32,
    pub n_tiles_x: u32,
    pub n_tiles_y: u32,
    pub use_l2: u32,
    pub prev_tile_step: u32,
    pub prev_n_tiles_y: u32,
    /// Row offset for chunked dispatch (GPU preemption)
    pub tile_row_offset: u32,
    pub _padding1: u32,
}

/// Parameters for frame warping shader
#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
pub struct WarpParams {
    pub width: u32,
    pub height: u32,
    pub n_tiles_x: u32,
    pub n_tiles_y: u32,
    pub tile_size: u32,
    pub tile_step: u32,
    pub use_bilinear: u32,
    pub _padding0: u32,
    // CA correction parameters
    pub center_x: f32,
    pub center_y: f32,
    pub ca_r_coeff: f32,
    pub ca_b_coeff: f32,
    pub enable_ca_correction: u32,
    pub _padding: u32,
    pub _padding2: u32,
    pub _padding3: u32,
}

/// Parameters for chromatic aberration estimation shader
#[repr(C)]
#[derive(Debug, Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
pub struct CAEstimateParams {
    pub width: u32,
    pub height: u32,
    pub center_x: f32,
    pub center_y: f32,
    pub edge_threshold: f32,
    pub radial_alignment: f32,
    pub num_radius_bins: u32,
    pub search_radius: u32,
}

/// Parameters for FFT merge shader
#[repr(C)]
#[derive(Debug, Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
pub struct MergeParams {
    pub width: u32,
    pub height: u32,
    pub noise_sd: f32,
    pub robustness: f32,
    pub n_tiles_x: u32,
    pub n_tiles_y: u32,
    pub frame_count: u32,
    pub read_noise: f32,
    pub max_motion_norm: f32,
    pub tile_offset_x: i32,
    pub tile_offset_y: i32,
    /// Row offset for chunked dispatches
    pub tile_row_offset: u32,
    /// Exposure factor for non-uniform exposure bursts (1.0 for uniform)
    pub exposure_factor: f32,
    pub _padding: u32, // Align to 16 bytes for GPU
}

/// Parameters for spatial denoising shader
#[repr(C)]
#[derive(Debug, Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
pub struct SpatialDenoiseParams {
    pub width: u32,
    pub height: u32,
    pub noise_sd: f32,
    pub strength: f32,
    pub n_tiles_x: u32,
    pub n_tiles_y: u32,
    pub high_freq_boost: f32,
    pub tile_offset_x: i32,
    pub tile_offset_y: i32,
    /// Number of frames merged (for noise variance scaling per HDR+ paper)
    pub frame_count: u32,
}

/// Parameters for chroma denoising shader
#[repr(C)]
#[derive(Debug, Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
pub struct ChromaDenoiseParams {
    pub width: u32,
    pub height: u32,
    pub strength: f32,
    pub edge_threshold: f32,
}

/// Parameters for guided filter shader (currently only used by WGSL shader)
#[repr(C)]
#[derive(Debug, Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
#[allow(dead_code)]
pub struct GuidedFilterParams {
    pub width: u32,
    pub height: u32,
    pub radius: u32,
    pub epsilon: f32,
}

// Size assertions to catch WGSL/Rust struct mismatches at compile time
const _: () = assert!(std::mem::size_of::<LuminanceParams>() == 16);
const _: () = assert!(std::mem::size_of::<PyramidParams>() == 16);
const _: () = assert!(std::mem::size_of::<AlignParams>() == 48);
const _: () = assert!(std::mem::size_of::<WarpParams>() == 64);
const _: () = assert!(std::mem::size_of::<CAEstimateParams>() == 32);
const _: () = assert!(std::mem::size_of::<MergeParams>() == 56);
const _: () = assert!(std::mem::size_of::<SpatialDenoiseParams>() == 40);
const _: () = assert!(std::mem::size_of::<ChromaDenoiseParams>() == 16);
const _: () = assert!(std::mem::size_of::<GuidedFilterParams>() == 16);
