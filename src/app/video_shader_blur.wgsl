// SPDX-License-Identifier: MPL-2.0
// GPU shader for efficient YUV (NV12) to RGB conversion with Gaussian blur
// Now includes filter support - filters are applied before blur

@group(0) @binding(0)
var texture_y: texture_2d<f32>;

@group(0) @binding(1)
var texture_uv: texture_2d<f32>;

@group(0) @binding(2)
var sampler_video: sampler;

struct ViewportUniform {
    viewport_size: vec2<f32>,
    content_fit_mode: u32,  // 0 = Contain, 1 = Cover
    filter_mode: u32,       // Filter index (0-15) - applied before blur
    corner_radius: f32,     // Unused in blur
    mirror_horizontal: u32, // 0 = normal, 1 = mirrored horizontally
    _padding1: f32,
    _padding2: f32,
}

// Sample luminance at offset for edge detection
fn sample_luminance_y(uv: vec2<f32>) -> f32 {
    return textureSample(texture_y, sampler_video, uv).r;
}

// Sobel edge detection for toon/pencil effects
fn sobel_edge(uv: vec2<f32>, texel_size: vec2<f32>) -> f32 {
    let tl = sample_luminance_y(uv + vec2<f32>(-texel_size.x, -texel_size.y));
    let tm = sample_luminance_y(uv + vec2<f32>(0.0, -texel_size.y));
    let tr = sample_luminance_y(uv + vec2<f32>(texel_size.x, -texel_size.y));
    let ml = sample_luminance_y(uv + vec2<f32>(-texel_size.x, 0.0));
    let mr = sample_luminance_y(uv + vec2<f32>(texel_size.x, 0.0));
    let bl = sample_luminance_y(uv + vec2<f32>(-texel_size.x, texel_size.y));
    let bm = sample_luminance_y(uv + vec2<f32>(0.0, texel_size.y));
    let br = sample_luminance_y(uv + vec2<f32>(texel_size.x, texel_size.y));

    let gx = -tl - 2.0 * ml - bl + tr + 2.0 * mr + br;
    let gy = -tl - 2.0 * tm - tr + bl + 2.0 * bm + br;

    return sqrt(gx * gx + gy * gy);
}

// Pseudo-random noise for pencil texture
fn hash(p: vec2<f32>) -> f32 {
    let h = dot(p, vec2<f32>(127.1, 311.7));
    return fract(sin(h) * 43758.5453123);
}

// Apply color filter to RGB values
fn apply_filter(r_in: f32, g_in: f32, b_in: f32, filter_mode: u32, tex_coords: vec2<f32>) -> vec3<f32> {
    var r = r_in;
    var g = g_in;
    var b = b_in;

    if (filter_mode == 1u) {
        // Mono: Black & White filter using luminance formula (BT.601)
        let luminance = 0.299 * r + 0.587 * g + 0.114 * b;
        r = luminance;
        g = luminance;
        b = luminance;
    } else if (filter_mode == 2u) {
        // Sepia: Warm brownish vintage tone
        let luminance = 0.299 * r + 0.587 * g + 0.114 * b;
        r = clamp(luminance * 1.2 + 0.1, 0.0, 1.0);
        g = clamp(luminance * 0.9 + 0.05, 0.0, 1.0);
        b = clamp(luminance * 0.7, 0.0, 1.0);
    } else if (filter_mode == 3u) {
        // Noir: High contrast black & white
        let luminance = 0.299 * r + 0.587 * g + 0.114 * b;
        let contrast = 2.0;
        let adjusted = (luminance - 0.5) * contrast + 0.5;
        let noir_val = clamp(adjusted, 0.0, 1.0);
        r = noir_val;
        g = noir_val;
        b = noir_val;
    } else if (filter_mode == 4u) {
        // Vivid: Boosted saturation and contrast for punchy colors
        let luminance = 0.299 * r + 0.587 * g + 0.114 * b;
        r = clamp(luminance + (r - luminance) * 1.4, 0.0, 1.0);
        g = clamp(luminance + (g - luminance) * 1.4, 0.0, 1.0);
        b = clamp(luminance + (b - luminance) * 1.4, 0.0, 1.0);
        r = clamp((r - 0.5) * 1.15 + 0.5, 0.0, 1.0);
        g = clamp((g - 0.5) * 1.15 + 0.5, 0.0, 1.0);
        b = clamp((b - 0.5) * 1.15 + 0.5, 0.0, 1.0);
    } else if (filter_mode == 5u) {
        // Cool: Blue color temperature shift
        r = clamp(r * 0.9, 0.0, 1.0);
        g = clamp(g * 0.95, 0.0, 1.0);
        b = clamp(b * 1.1, 0.0, 1.0);
    } else if (filter_mode == 6u) {
        // Warm: Orange/amber color temperature
        r = clamp(r * 1.1, 0.0, 1.0);
        g = clamp(g * 1.0, 0.0, 1.0);
        b = clamp(b * 0.85, 0.0, 1.0);
    } else if (filter_mode == 7u) {
        // Fade: Lifted blacks with muted colors for vintage look
        r = clamp(r * 0.85 + 0.1, 0.0, 1.0);
        g = clamp(g * 0.85 + 0.1, 0.0, 1.0);
        b = clamp(b * 0.85 + 0.1, 0.0, 1.0);
        let luminance = 0.299 * r + 0.587 * g + 0.114 * b;
        r = clamp(luminance + (r - luminance) * 0.7, 0.0, 1.0);
        g = clamp(luminance + (g - luminance) * 0.7, 0.0, 1.0);
        b = clamp(luminance + (b - luminance) * 0.7, 0.0, 1.0);
    } else if (filter_mode == 8u) {
        // Duotone: Two-color gradient mapping (deep blue to golden yellow)
        let luminance = 0.299 * r + 0.587 * g + 0.114 * b;
        let dark = vec3<f32>(0.1, 0.1, 0.4);
        let light = vec3<f32>(1.0, 0.9, 0.5);
        let result = mix(dark, light, luminance);
        r = result.x;
        g = result.y;
        b = result.z;
    } else if (filter_mode == 9u) {
        // Vignette: Darkened edges
        let center = vec2<f32>(0.5, 0.5);
        let dist = distance(tex_coords, center);
        let vignette = 1.0 - smoothstep(0.3, 0.9, dist);
        r = r * vignette;
        g = g * vignette;
        b = b * vignette;
    } else if (filter_mode == 10u) {
        // Negative: Inverted colors
        r = 1.0 - r;
        g = 1.0 - g;
        b = 1.0 - b;
    } else if (filter_mode == 11u) {
        // Posterize: Reduced color levels (pop-art style)
        let levels = 4.0;
        r = floor(r * levels) / levels;
        g = floor(g * levels) / levels;
        b = floor(b * levels) / levels;
    } else if (filter_mode == 12u) {
        // Solarize: Partially inverted tones (threshold-based)
        let threshold = 0.5;
        if (r > threshold) { r = 1.0 - r; }
        if (g > threshold) { g = 1.0 - g; }
        if (b > threshold) { b = 1.0 - b; }
    } else if (filter_mode == 14u) {
        // Pencil: Pencil sketch drawing effect
        let tex_size = vec2<f32>(textureDimensions(texture_y));
        let texel_size = 1.0 / tex_size;
        let edge = sobel_edge(tex_coords, texel_size);
        let pencil = 1.0 - edge * 2.0;
        let noise = hash(tex_coords * 500.0) * 0.05;
        let paper = 0.95 + noise;
        let final_val = clamp(pencil * paper, 0.0, 1.0);
        r = final_val;
        g = final_val;
        b = final_val;
    }
    // Note: filter_mode 13 (Chromatic Aberration) is skipped as it requires
    // special texture sampling that doesn't work well with blur

    return vec3<f32>(r, g, b);
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
    var r = y_val + 1.402 * v_val;
    var g = y_val - 0.344 * u_val - 0.714 * v_val;
    var b = y_val + 1.772 * u_val;

    // Clamp to valid range
    r = clamp(r, 0.0, 1.0);
    g = clamp(g, 0.0, 1.0);
    b = clamp(b, 0.0, 1.0);

    // Apply filter before blur darkening (so filter is visible during transition)
    let filtered = apply_filter(r, g, b, viewport.filter_mode, tex_coords);

    // Apply slight darkening for subtle transition indication
    return vec4<f32>(
        clamp(filtered.x * 0.85, 0.0, 1.0),
        clamp(filtered.y * 0.85, 0.0, 1.0),
        clamp(filtered.z * 0.85, 0.0, 1.0),
        1.0
    );
}
