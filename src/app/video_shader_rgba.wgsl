// SPDX-License-Identifier: MPL-2.0
// GPU shader for direct RGBA texture rendering with object-fit: cover support

@group(0) @binding(0)
var texture_rgba: texture_2d<f32>;

@group(0) @binding(1)
var sampler_video: sampler;

struct ViewportUniform {
    viewport_size: vec2<f32>,
    content_fit_mode: u32,  // 0 = Contain, 1 = Cover
    filter_mode: u32,       // Filter index (0-15)
    corner_radius: f32,     // Corner radius in pixels (0 = no rounding)
    mirror_horizontal: u32, // 0 = normal, 1 = mirrored horizontally
    _padding1: f32,         // Padding for 16-byte alignment
    _padding2: f32,
}

@group(0) @binding(2)
var<uniform> viewport: ViewportUniform;

struct VertexOutput {
    @builtin(position) position: vec4<f32>,
    @location(0) tex_coords: vec2<f32>,
}

// Sample luminance at offset for edge detection
fn sample_luminance(uv: vec2<f32>) -> f32 {
    let color = textureSample(texture_rgba, sampler_video, uv);
    return 0.299 * color.r + 0.587 * color.g + 0.114 * color.b;
}

// Sobel edge detection for toon/pencil effects
fn sobel_edge(uv: vec2<f32>, texel_size: vec2<f32>) -> f32 {
    let tl = sample_luminance(uv + vec2<f32>(-texel_size.x, -texel_size.y));
    let tm = sample_luminance(uv + vec2<f32>(0.0, -texel_size.y));
    let tr = sample_luminance(uv + vec2<f32>(texel_size.x, -texel_size.y));
    let ml = sample_luminance(uv + vec2<f32>(-texel_size.x, 0.0));
    let mr = sample_luminance(uv + vec2<f32>(texel_size.x, 0.0));
    let bl = sample_luminance(uv + vec2<f32>(-texel_size.x, texel_size.y));
    let bm = sample_luminance(uv + vec2<f32>(0.0, texel_size.y));
    let br = sample_luminance(uv + vec2<f32>(texel_size.x, texel_size.y));

    let gx = -tl - 2.0 * ml - bl + tr + 2.0 * mr + br;
    let gy = -tl - 2.0 * tm - tr + bl + 2.0 * bm + br;

    return sqrt(gx * gx + gy * gy);
}

// Pseudo-random noise for pencil texture
fn hash(p: vec2<f32>) -> f32 {
    let h = dot(p, vec2<f32>(127.1, 311.7));
    return fract(sin(h) * 43758.5453123);
}

// Distance from point to rounded rectangle
fn rounded_box_sdf(pos: vec2<f32>, size: vec2<f32>, radius: f32) -> f32 {
    let d = abs(pos) - size + vec2<f32>(radius, radius);
    return min(max(d.x, d.y), 0.0) + length(max(d, vec2<f32>(0.0, 0.0))) - radius;
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

// Fragment shader - RGBA passthrough with Cover mode support
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
        let tex_size = vec2<f32>(textureDimensions(texture_rgba));

        // Calculate aspect ratios
        let tex_aspect = tex_size.x / tex_size.y;
        let viewport_aspect = viewport.viewport_size.x / viewport.viewport_size.y;

        // Calculate scale factor for "cover" behavior
        var scale: vec2<f32>;
        if (tex_aspect > viewport_aspect) {
            // Texture is wider than viewport - fit height, crop sides
            scale = vec2<f32>(viewport_aspect / tex_aspect, 1.0);
        } else {
            // Texture is taller than viewport - fit width, crop top/bottom
            scale = vec2<f32>(1.0, tex_aspect / viewport_aspect);
        }

        // Adjust UV coordinates to center and scale the texture
        tex_coords = (tex_coords - vec2<f32>(0.5, 0.5)) * scale + vec2<f32>(0.5, 0.5);
    }

    // Sample RGBA texture
    var color = textureSample(texture_rgba, sampler_video, tex_coords);

    // Apply color filter based on mode
    if (viewport.filter_mode == 1u) {
        // Mono: Black & White filter using luminance formula (BT.601)
        let luminance = 0.299 * color.r + 0.587 * color.g + 0.114 * color.b;
        color = vec4<f32>(luminance, luminance, luminance, color.a);
    } else if (viewport.filter_mode == 2u) {
        // Sepia: Warm brownish vintage tone
        let luminance = 0.299 * color.r + 0.587 * color.g + 0.114 * color.b;
        let r = clamp(luminance * 1.2 + 0.1, 0.0, 1.0);
        let g = clamp(luminance * 0.9 + 0.05, 0.0, 1.0);
        let b = clamp(luminance * 0.7, 0.0, 1.0);
        color = vec4<f32>(r, g, b, color.a);
    } else if (viewport.filter_mode == 3u) {
        // Noir: High contrast black & white
        let luminance = 0.299 * color.r + 0.587 * color.g + 0.114 * color.b;
        let contrast = 2.0;
        let adjusted = (luminance - 0.5) * contrast + 0.5;
        let noir_val = clamp(adjusted, 0.0, 1.0);
        color = vec4<f32>(noir_val, noir_val, noir_val, color.a);
    } else if (viewport.filter_mode == 4u) {
        // Vivid: Boosted saturation and contrast for punchy colors
        let luminance = 0.299 * color.r + 0.587 * color.g + 0.114 * color.b;
        var r = clamp(luminance + (color.r - luminance) * 1.4, 0.0, 1.0);
        var g = clamp(luminance + (color.g - luminance) * 1.4, 0.0, 1.0);
        var b = clamp(luminance + (color.b - luminance) * 1.4, 0.0, 1.0);
        r = clamp((r - 0.5) * 1.15 + 0.5, 0.0, 1.0);
        g = clamp((g - 0.5) * 1.15 + 0.5, 0.0, 1.0);
        b = clamp((b - 0.5) * 1.15 + 0.5, 0.0, 1.0);
        color = vec4<f32>(r, g, b, color.a);
    } else if (viewport.filter_mode == 5u) {
        // Cool: Blue color temperature shift
        let r = clamp(color.r * 0.9, 0.0, 1.0);
        let g = clamp(color.g * 0.95, 0.0, 1.0);
        let b = clamp(color.b * 1.1, 0.0, 1.0);
        color = vec4<f32>(r, g, b, color.a);
    } else if (viewport.filter_mode == 6u) {
        // Warm: Orange/amber color temperature
        let r = clamp(color.r * 1.1, 0.0, 1.0);
        let g = clamp(color.g * 1.0, 0.0, 1.0);
        let b = clamp(color.b * 0.85, 0.0, 1.0);
        color = vec4<f32>(r, g, b, color.a);
    } else if (viewport.filter_mode == 7u) {
        // Fade: Lifted blacks with muted colors for vintage look
        var r = clamp(color.r * 0.85 + 0.1, 0.0, 1.0);
        var g = clamp(color.g * 0.85 + 0.1, 0.0, 1.0);
        var b = clamp(color.b * 0.85 + 0.1, 0.0, 1.0);
        let luminance = 0.299 * r + 0.587 * g + 0.114 * b;
        r = clamp(luminance + (r - luminance) * 0.7, 0.0, 1.0);
        g = clamp(luminance + (g - luminance) * 0.7, 0.0, 1.0);
        b = clamp(luminance + (b - luminance) * 0.7, 0.0, 1.0);
        color = vec4<f32>(r, g, b, color.a);
    } else if (viewport.filter_mode == 8u) {
        // Duotone: Two-color gradient mapping
        let luminance = 0.299 * color.r + 0.587 * color.g + 0.114 * color.b;
        let dark = vec3<f32>(0.1, 0.1, 0.4);
        let light = vec3<f32>(1.0, 0.9, 0.5);
        let result = mix(dark, light, luminance);
        color = vec4<f32>(result, color.a);
    } else if (viewport.filter_mode == 9u) {
        // Vignette: Darkened edges
        let center = vec2<f32>(0.5, 0.5);
        let dist = distance(tex_coords, center);
        let vignette = 1.0 - smoothstep(0.3, 0.9, dist);
        color = vec4<f32>(color.rgb * vignette, color.a);
    } else if (viewport.filter_mode == 10u) {
        // Negative: Inverted colors
        color = vec4<f32>(1.0 - color.r, 1.0 - color.g, 1.0 - color.b, color.a);
    } else if (viewport.filter_mode == 11u) {
        // Posterize: Reduced color levels
        let levels = 4.0;
        let r = floor(color.r * levels) / levels;
        let g = floor(color.g * levels) / levels;
        let b = floor(color.b * levels) / levels;
        color = vec4<f32>(r, g, b, color.a);
    } else if (viewport.filter_mode == 12u) {
        // Solarize: Partially inverted tones
        let threshold = 0.5;
        var r = color.r;
        var g = color.g;
        var b = color.b;
        if (r > threshold) { r = 1.0 - r; }
        if (g > threshold) { g = 1.0 - g; }
        if (b > threshold) { b = 1.0 - b; }
        color = vec4<f32>(r, g, b, color.a);
    } else if (viewport.filter_mode == 13u) {
        // Chromatic Aberration: RGB channel split (scales with resolution)
        let tex_size = vec2<f32>(textureDimensions(texture_rgba));
        // Scale offset as percentage of width (0.4% for visible effect at any resolution)
        let offset_uv = 0.004;
        let color_r = textureSample(texture_rgba, sampler_video, tex_coords + vec2<f32>(offset_uv, 0.0));
        let color_b = textureSample(texture_rgba, sampler_video, tex_coords - vec2<f32>(offset_uv, 0.0));
        color = vec4<f32>(color_r.r, color.g, color_b.b, color.a);
    } else if (viewport.filter_mode == 14u) {
        // Pencil: Pencil sketch drawing effect
        let tex_size = vec2<f32>(textureDimensions(texture_rgba));
        let texel_size = 1.0 / tex_size;
        let edge = sobel_edge(tex_coords, texel_size);
        let pencil = 1.0 - edge * 2.0;
        let noise = hash(tex_coords * 500.0) * 0.05;
        let paper = 0.95 + noise;
        let final_val = clamp(pencil * paper, 0.0, 1.0);
        color = vec4<f32>(final_val, final_val, final_val, color.a);
    }

    // Calculate alpha for rounded corners
    var alpha = color.a;
    if (viewport.corner_radius > 0.0) {
        let pixel_pos = (in.tex_coords - vec2<f32>(0.5, 0.5)) * viewport.viewport_size;
        let half_size = viewport.viewport_size * 0.5;
        let dist = rounded_box_sdf(pixel_pos, half_size, viewport.corner_radius);
        let corner_alpha = 1.0 - smoothstep(-1.0, 1.0, dist);
        alpha = color.a * corner_alpha;
    }

    return vec4<f32>(color.rgb, alpha);
}
