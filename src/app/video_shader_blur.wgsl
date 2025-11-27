// SPDX-License-Identifier: MPL-2.0
// GPU shader for efficient YUV (NV12) to RGB conversion with Gaussian blur

@group(0) @binding(0)
var texture_y: texture_2d<f32>;

@group(0) @binding(1)
var texture_uv: texture_2d<f32>;

@group(0) @binding(2)
var sampler_video: sampler;

struct ViewportUniform {
    viewport_size: vec2<f32>,
    content_fit_mode: u32,  // 0 = Contain, 1 = Cover
    filter_mode: u32,       // Unused in blur, but kept for struct compatibility
    corner_radius: f32,     // Unused in blur
    mirror_horizontal: u32, // 0 = normal, 1 = mirrored horizontally
    _padding1: f32,
    _padding2: f32,
}

@group(0) @binding(3)
var<uniform> viewport: ViewportUniform;

struct VertexOutput {
    @builtin(position) position: vec4<f32>,
    @location(0) tex_coords: vec2<f32>,
}

// Vertex shader - creates a fullscreen quad
@vertex
fn vs_main(@builtin(vertex_index) vertex_index: u32) -> VertexOutput {
    var out: VertexOutput;

    // Generate fullscreen triangle vertices
    let x = f32((vertex_index & 1u) << 2u) - 1.0;
    let y = f32((vertex_index & 2u) << 1u) - 1.0;

    out.position = vec4<f32>(x, -y, 0.0, 1.0);
    out.tex_coords = vec2<f32>((x + 1.0) * 0.5, (y + 1.0) * 0.5);

    return out;
}

// Fragment shader - YUV to RGB conversion with beautiful Gaussian blur
// Uses optimized dual-pass approximation for smooth, artifact-free results
@fragment
fn fs_main(in: VertexOutput) -> @location(0) vec4<f32> {
    var tex_coords = in.tex_coords;

    // Apply horizontal mirror if enabled (selfie mode)
    if (viewport.mirror_horizontal == 1u) {
        tex_coords.x = 1.0 - tex_coords.x;
    }

    // Apply Cover mode adjustment if enabled
    if (viewport.content_fit_mode == 1u) {
        // Get texture dimensions
        let tex_size_y_dim = vec2<f32>(textureDimensions(texture_y));

        // Calculate aspect ratios
        let tex_aspect = tex_size_y_dim.x / tex_size_y_dim.y;
        let viewport_aspect = viewport.viewport_size.x / viewport.viewport_size.y;

        // Calculate scale factor for "cover" behavior
        var scale: vec2<f32>;
        if (tex_aspect > viewport_aspect) {
            scale = vec2<f32>(viewport_aspect / tex_aspect, 1.0);
        } else {
            scale = vec2<f32>(1.0, tex_aspect / viewport_aspect);
        }

        // Adjust UV coordinates to center and scale the texture
        tex_coords = (tex_coords - vec2<f32>(0.5, 0.5)) * scale + vec2<f32>(0.5, 0.5);
    }

    // Get texture dimensions
    let tex_size_y = textureDimensions(texture_y);
    let tex_size_uv = textureDimensions(texture_uv);

    // Optimized blur settings - fewer samples, better distribution
    let blur_radius = 50.0;  // Large radius for smooth blur
    let samples = 16;  // 16 samples per ring for efficiency

    // Calculate pixel steps in texture coordinates
    let pixel_step_y = vec2<f32>(1.0 / f32(tex_size_y.x), 1.0 / f32(tex_size_y.y));
    let pixel_step_uv = vec2<f32>(1.0 / f32(tex_size_uv.x), 1.0 / f32(tex_size_uv.y));

    var y_sum = 0.0;
    var u_sum = 0.0;
    var v_sum = 0.0;
    var weight_sum = 0.0;

    // Standard deviation for Gaussian - controls blur spread
    let sigma = blur_radius / 2.5;
    let sigma_squared_2 = 2.0 * sigma * sigma;

    // Sample in a spiral pattern with optimized ring distribution
    // Using 3 rings with golden ratio offset for better coverage
    let angle_step = 6.28318530718 / f32(samples);  // 2*PI / samples
    let golden_angle = 2.399963229728653;  // Golden angle in radians

    for (var ring = 1; ring <= 3; ring++) {
        // Use exponential distribution for ring radii (more samples at outer edges)
        let ring_factor = f32(ring) / 3.0;
        let radius = blur_radius * ring_factor * ring_factor;

        // Offset each ring by golden angle for better sampling pattern
        let ring_offset = f32(ring - 1) * golden_angle;

        for (var i = 0; i < samples; i++) {
            let angle = f32(i) * angle_step + ring_offset;
            let offset_x = cos(angle) * radius;
            let offset_y = sin(angle) * radius;

            let offset_y_tex = vec2<f32>(offset_x * pixel_step_y.x, offset_y * pixel_step_y.y);
            let offset_uv_tex = vec2<f32>(offset_x * pixel_step_uv.x, offset_y * pixel_step_uv.y);

            let sample_coords_y = tex_coords + offset_y_tex;
            let sample_coords_uv = tex_coords + offset_uv_tex;

            // Sample textures
            let y = textureSample(texture_y, sampler_video, sample_coords_y).r;
            let uv = textureSample(texture_uv, sampler_video, sample_coords_uv).rg;

            // Gaussian weight based on actual distance from center
            let dist_squared = radius * radius;
            let weight = exp(-dist_squared / sigma_squared_2);

            y_sum += y * weight;
            u_sum += uv.r * weight;
            v_sum += uv.g * weight;
            weight_sum += weight;
        }
    }

    // Add center sample with higher weight for stability
    let center_y = textureSample(texture_y, sampler_video, tex_coords).r;
    let center_uv = textureSample(texture_uv, sampler_video, tex_coords).rg;
    let center_weight = 2.0;  // Stronger center weight

    y_sum += center_y * center_weight;
    u_sum += center_uv.r * center_weight;
    v_sum += center_uv.g * center_weight;
    weight_sum += center_weight;

    // Normalize by total weight
    let y_val = y_sum / weight_sum;
    let u_val = (u_sum / weight_sum) - 0.5;
    let v_val = (v_sum / weight_sum) - 0.5;

    // YUV to RGB conversion matrix (BT.601 standard)
    let r = y_val + 1.402 * v_val;
    let g = y_val - 0.344 * u_val - 0.714 * v_val;
    let b = y_val + 1.772 * u_val;

    // Clamp to valid range and return with slight darkening for subtle transition indication
    return vec4<f32>(
        clamp(r * 0.85, 0.0, 1.0),  // Subtle darkening
        clamp(g * 0.85, 0.0, 1.0),
        clamp(b * 0.85, 0.0, 1.0),
        1.0
    );
}
