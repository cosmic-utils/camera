// SPDX-License-Identifier: GPL-3.0-only
// GPU shader for Gaussian blur (for multi-pass blur transitions)

@group(0) @binding(0)
var texture_blur: texture_2d<f32>;

@group(0) @binding(1)
var sampler_blur: sampler;

struct ViewportUniform {
    viewport_size: vec2<f32>,   // Full widget size
    content_fit_mode: u32,      // 0 = Contain, 1 = Cover
    filter_mode: u32,           // Unused in blur, but kept for struct compatibility
    corner_radius: f32,         // Unused in blur
    mirror_horizontal: u32,     // 0 = normal, 1 = mirrored horizontally
    uv_offset: vec2<f32>,       // UV offset for scroll clipping (0-1)
    uv_scale: vec2<f32>,        // UV scale for scroll clipping (0-1)
    crop_uv_min: vec2<f32>,     // Crop UV min (u_min, v_min) - normalized 0-1
    crop_uv_max: vec2<f32>,     // Crop UV max (u_max, v_max) - normalized 0-1
    zoom_level: f32,            // Unused in blur, but kept for struct compatibility
    rotation: u32,              // Sensor rotation: 0=None, 1=90CW, 2=180, 3=270CW
}

@group(0) @binding(2)
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

// Fragment shader - Gaussian blur on RGB texture
@fragment
fn fs_main(in: VertexOutput) -> @location(0) vec4<f32> {
    // Apply scroll clipping UV transformation
    var tex_coords = viewport.uv_offset + in.tex_coords * viewport.uv_scale;

    // Apply horizontal mirror if enabled (selfie mode)
    // This happens BEFORE rotation so the mirror is in screen space
    if (viewport.mirror_horizontal == 1u) {
        tex_coords.x = 1.0 - tex_coords.x;
    }

    // Apply rotation correction for sensor orientation
    if (viewport.rotation == 1u) {
        // 90 CW sensor -> sample rotated 90 CW: (u,v) -> (1-v, u)
        tex_coords = vec2<f32>(1.0 - tex_coords.y, tex_coords.x);
    } else if (viewport.rotation == 2u) {
        // 180 sensor -> rotate 180: (u,v) -> (1-u, 1-v)
        tex_coords = vec2<f32>(1.0 - tex_coords.x, 1.0 - tex_coords.y);
    } else if (viewport.rotation == 3u) {
        // 270 CW sensor -> sample rotated 270 CW: (u,v) -> (v, 1-u)
        tex_coords = vec2<f32>(tex_coords.y, 1.0 - tex_coords.x);
    }

    // Apply crop UV mapping (aspect ratio cropping)
    // Remap tex_coords from 0-1 range to crop_uv_min to crop_uv_max range
    tex_coords = mix(viewport.crop_uv_min, viewport.crop_uv_max, tex_coords);

    // Apply Cover mode adjustment if enabled
    if (viewport.content_fit_mode == 1u) {
        // Get texture dimensions, accounting for rotation
        let raw_tex_size = vec2<f32>(textureDimensions(texture_blur));
        var tex_size_dim = raw_tex_size;
        if (viewport.rotation == 1u || viewport.rotation == 3u) {
            tex_size_dim = vec2<f32>(raw_tex_size.y, raw_tex_size.x);
        }

        // Calculate aspect ratios
        let tex_aspect = tex_size_dim.x / tex_size_dim.y;
        let viewport_aspect = viewport.viewport_size.x / viewport.viewport_size.y;

        // Calculate scale factor for "cover" behavior
        var scale: vec2<f32>;
        if (tex_aspect > viewport_aspect) {
            scale = vec2<f32>(viewport_aspect / tex_aspect, 1.0);
        } else {
            scale = vec2<f32>(1.0, tex_aspect / viewport_aspect);
        }

        // For 90/270 rotations, swap scale factors since we're in rotated UV space
        if (viewport.rotation == 1u || viewport.rotation == 3u) {
            scale = vec2<f32>(scale.y, scale.x);
        }

        // Adjust UV coordinates to center and scale the texture
        tex_coords = (tex_coords - vec2<f32>(0.5, 0.5)) * scale + vec2<f32>(0.5, 0.5);
    }

    // Get texture dimensions
    let tex_size = textureDimensions(texture_blur);

    // Optimized blur settings - fewer samples, better distribution
    let blur_radius = 50.0;  // Large radius for smooth blur
    let samples = 16;  // 16 samples per ring for efficiency

    // Calculate pixel steps in texture coordinates
    let pixel_step = vec2<f32>(1.0 / f32(tex_size.x), 1.0 / f32(tex_size.y));

    var rgb_sum = vec3<f32>(0.0, 0.0, 0.0);
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

            let offset_tex = vec2<f32>(offset_x * pixel_step.x, offset_y * pixel_step.y);
            let sample_coords = tex_coords + offset_tex;

            // Sample RGB texture
            let rgb = textureSample(texture_blur, sampler_blur, sample_coords).rgb;

            // Gaussian weight based on actual distance from center
            let dist_squared = radius * radius;
            let weight = exp(-dist_squared / sigma_squared_2);

            rgb_sum += rgb * weight;
            weight_sum += weight;
        }
    }

    // Add center sample with higher weight for stability
    let center_rgb = textureSample(texture_blur, sampler_blur, tex_coords).rgb;
    let center_weight = 2.0;  // Stronger center weight

    rgb_sum += center_rgb * center_weight;
    weight_sum += center_weight;

    // Normalize by total weight
    let rgb_val = rgb_sum / weight_sum;

    // Apply slight darkening for subtle transition indication
    return vec4<f32>(
        clamp(rgb_val.r * 0.85, 0.0, 1.0),
        clamp(rgb_val.g * 0.85, 0.0, 1.0),
        clamp(rgb_val.b * 0.85, 0.0, 1.0),
        1.0
    );
}
