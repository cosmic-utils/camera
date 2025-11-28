// SPDX-License-Identifier: MPL-2.0
// GPU shader for NV12 to filtered RGB conversion for virtual camera output

@group(0) @binding(0)
var texture_y: texture_2d<f32>;

@group(0) @binding(1)
var texture_uv: texture_2d<f32>;

@group(0) @binding(2)
var sampler_video: sampler;

struct FilterUniform {
    frame_size: vec2<f32>,
    filter_mode: u32,
    _padding: u32,
}

@group(0) @binding(3)
var<uniform> params: FilterUniform;

struct VertexOutput {
    @builtin(position) position: vec4<f32>,
    @location(0) tex_coords: vec2<f32>,
}

// Sample luminance at offset for edge detection
fn sample_luminance_y(uv: vec2<f32>) -> f32 {
    return textureSample(texture_y, sampler_video, uv).r;
}

// Sobel edge detection for pencil effect
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

// Fragment shader - NV12 to filtered RGB conversion
@fragment
fn fs_main(in: VertexOutput) -> @location(0) vec4<f32> {
    let tex_coords = in.tex_coords;

    // Sample Y and UV textures
    let y = textureSample(texture_y, sampler_video, tex_coords).r;
    let uv = textureSample(texture_uv, sampler_video, tex_coords).rg;

    // Convert from [0,1] range to YUV values
    let y_val = y;
    let u_val = uv.r - 0.5;
    let v_val = uv.g - 0.5;

    // YUV to RGB conversion matrix (BT.601 standard)
    var r = y_val + 1.402 * v_val;
    var g = y_val - 0.344 * u_val - 0.714 * v_val;
    var b = y_val + 1.772 * u_val;

    // Clamp to valid range
    r = clamp(r, 0.0, 1.0);
    g = clamp(g, 0.0, 1.0);
    b = clamp(b, 0.0, 1.0);

    // Apply color filter based on mode
    if (params.filter_mode == 1u) {
        // Mono: Black & White filter using luminance formula (BT.601)
        let luminance = 0.299 * r + 0.587 * g + 0.114 * b;
        r = luminance;
        g = luminance;
        b = luminance;
    } else if (params.filter_mode == 2u) {
        // Sepia: Warm brownish vintage tone
        let luminance = 0.299 * r + 0.587 * g + 0.114 * b;
        r = clamp(luminance * 1.2 + 0.1, 0.0, 1.0);
        g = clamp(luminance * 0.9 + 0.05, 0.0, 1.0);
        b = clamp(luminance * 0.7, 0.0, 1.0);
    } else if (params.filter_mode == 3u) {
        // Noir: High contrast black & white
        let luminance = 0.299 * r + 0.587 * g + 0.114 * b;
        let contrast = 2.0;
        let adjusted = (luminance - 0.5) * contrast + 0.5;
        let noir_val = clamp(adjusted, 0.0, 1.0);
        r = noir_val;
        g = noir_val;
        b = noir_val;
    } else if (params.filter_mode == 4u) {
        // Vivid: Boosted saturation and contrast for punchy colors
        let luminance = 0.299 * r + 0.587 * g + 0.114 * b;
        r = clamp(luminance + (r - luminance) * 1.4, 0.0, 1.0);
        g = clamp(luminance + (g - luminance) * 1.4, 0.0, 1.0);
        b = clamp(luminance + (b - luminance) * 1.4, 0.0, 1.0);
        r = clamp((r - 0.5) * 1.15 + 0.5, 0.0, 1.0);
        g = clamp((g - 0.5) * 1.15 + 0.5, 0.0, 1.0);
        b = clamp((b - 0.5) * 1.15 + 0.5, 0.0, 1.0);
    } else if (params.filter_mode == 5u) {
        // Cool: Blue color temperature shift
        r = clamp(r * 0.9, 0.0, 1.0);
        g = clamp(g * 0.95, 0.0, 1.0);
        b = clamp(b * 1.1, 0.0, 1.0);
    } else if (params.filter_mode == 6u) {
        // Warm: Orange/amber color temperature
        r = clamp(r * 1.1, 0.0, 1.0);
        g = clamp(g * 1.0, 0.0, 1.0);
        b = clamp(b * 0.85, 0.0, 1.0);
    } else if (params.filter_mode == 7u) {
        // Fade: Lifted blacks with muted colors for vintage look
        r = clamp(r * 0.85 + 0.1, 0.0, 1.0);
        g = clamp(g * 0.85 + 0.1, 0.0, 1.0);
        b = clamp(b * 0.85 + 0.1, 0.0, 1.0);
        let luminance = 0.299 * r + 0.587 * g + 0.114 * b;
        r = clamp(luminance + (r - luminance) * 0.7, 0.0, 1.0);
        g = clamp(luminance + (g - luminance) * 0.7, 0.0, 1.0);
        b = clamp(luminance + (b - luminance) * 0.7, 0.0, 1.0);
    } else if (params.filter_mode == 8u) {
        // Duotone: Two-color gradient mapping (deep blue to golden yellow)
        let luminance = 0.299 * r + 0.587 * g + 0.114 * b;
        let dark = vec3<f32>(0.1, 0.1, 0.4);
        let light = vec3<f32>(1.0, 0.9, 0.5);
        let result = mix(dark, light, luminance);
        r = result.x;
        g = result.y;
        b = result.z;
    } else if (params.filter_mode == 9u) {
        // Vignette: Darkened edges
        let center = vec2<f32>(0.5, 0.5);
        let dist = distance(tex_coords, center);
        let vignette = 1.0 - smoothstep(0.3, 0.9, dist);
        r = r * vignette;
        g = g * vignette;
        b = b * vignette;
    } else if (params.filter_mode == 10u) {
        // Negative: Inverted colors
        r = 1.0 - r;
        g = 1.0 - g;
        b = 1.0 - b;
    } else if (params.filter_mode == 11u) {
        // Posterize: Reduced color levels (pop-art style)
        let levels = 4.0;
        r = floor(r * levels) / levels;
        g = floor(g * levels) / levels;
        b = floor(b * levels) / levels;
    } else if (params.filter_mode == 12u) {
        // Solarize: Partially inverted tones (threshold-based)
        let threshold = 0.5;
        if (r > threshold) { r = 1.0 - r; }
        if (g > threshold) { g = 1.0 - g; }
        if (b > threshold) { b = 1.0 - b; }
    } else if (params.filter_mode == 13u) {
        // Chromatic Aberration: RGB channel split
        let tex_size = params.frame_size;
        let offset = tex_size.x * 0.004;
        let offset_uv = offset / tex_size.x;
        let y_r = textureSample(texture_y, sampler_video, tex_coords + vec2<f32>(offset_uv, 0.0)).r;
        let y_b = textureSample(texture_y, sampler_video, tex_coords - vec2<f32>(offset_uv, 0.0)).r;
        let uv_r = textureSample(texture_uv, sampler_video, tex_coords + vec2<f32>(offset_uv, 0.0)).rg;
        let uv_b = textureSample(texture_uv, sampler_video, tex_coords - vec2<f32>(offset_uv, 0.0)).rg;
        let u_r = uv_r.r - 0.5;
        let v_r = uv_r.g - 0.5;
        let u_b = uv_b.r - 0.5;
        let v_b = uv_b.g - 0.5;
        r = clamp(y_r + 1.402 * v_r, 0.0, 1.0);
        b = clamp(y_b + 1.772 * u_b, 0.0, 1.0);
    } else if (params.filter_mode == 14u) {
        // Pencil: Pencil sketch drawing effect
        let tex_size = params.frame_size;
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

    return vec4<f32>(r, g, b, 1.0);
}
