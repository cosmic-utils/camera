// SPDX-License-Identifier: MPL-2.0
// Shared filter functions for all camera components
// This is the single source of truth for all image filters.

// BT.601 luminance calculation
fn luminance(color: vec3<f32>) -> f32 {
    return 0.299 * color.r + 0.587 * color.g + 0.114 * color.b;
}

// Pseudo-random hash for paper texture effects
fn hash(p: vec2<f32>) -> f32 {
    let h = dot(p, vec2<f32>(127.1, 311.7));
    return fract(sin(h) * 43758.5453123);
}

// Apply filter to RGB color (filter_mode 0-14)
// tex_coords: normalized texture coordinates (0-1) for position-dependent filters
fn apply_filter(color: vec3<f32>, filter_mode: u32, tex_coords: vec2<f32>) -> vec3<f32> {
    var result = color;

    if (filter_mode == 1u) {
        // Mono: Black & White filter using luminance formula (BT.601)
        let lum = luminance(color);
        result = vec3<f32>(lum, lum, lum);
    } else if (filter_mode == 2u) {
        // Sepia: Warm brownish vintage tone
        let lum = luminance(color);
        result = vec3<f32>(
            clamp(lum * 1.2 + 0.1, 0.0, 1.0),
            clamp(lum * 0.9 + 0.05, 0.0, 1.0),
            clamp(lum * 0.7, 0.0, 1.0)
        );
    } else if (filter_mode == 3u) {
        // Noir: High contrast black & white
        let lum = luminance(color);
        let contrast = 2.0;
        let adjusted = (lum - 0.5) * contrast + 0.5;
        let noir_val = clamp(adjusted, 0.0, 1.0);
        result = vec3<f32>(noir_val, noir_val, noir_val);
    } else if (filter_mode == 4u) {
        // Vivid: Boosted saturation and contrast for punchy colors
        let lum = luminance(color);
        // Boost saturation by 1.4x
        var r = clamp(lum + (color.r - lum) * 1.4, 0.0, 1.0);
        var g = clamp(lum + (color.g - lum) * 1.4, 0.0, 1.0);
        var b = clamp(lum + (color.b - lum) * 1.4, 0.0, 1.0);
        // Apply slight contrast boost
        r = clamp((r - 0.5) * 1.15 + 0.5, 0.0, 1.0);
        g = clamp((g - 0.5) * 1.15 + 0.5, 0.0, 1.0);
        b = clamp((b - 0.5) * 1.15 + 0.5, 0.0, 1.0);
        result = vec3<f32>(r, g, b);
    } else if (filter_mode == 5u) {
        // Cool: Blue color temperature shift
        result = vec3<f32>(
            clamp(color.r * 0.9, 0.0, 1.0),
            clamp(color.g * 0.95, 0.0, 1.0),
            clamp(color.b * 1.1, 0.0, 1.0)
        );
    } else if (filter_mode == 6u) {
        // Warm: Orange/amber color temperature
        result = vec3<f32>(
            clamp(color.r * 1.1, 0.0, 1.0),
            color.g,
            clamp(color.b * 0.85, 0.0, 1.0)
        );
    } else if (filter_mode == 7u) {
        // Fade: Lifted blacks with muted colors for vintage look
        var r = clamp(color.r * 0.85 + 0.1, 0.0, 1.0);
        var g = clamp(color.g * 0.85 + 0.1, 0.0, 1.0);
        var b = clamp(color.b * 0.85 + 0.1, 0.0, 1.0);
        // Reduce saturation slightly
        let lum = luminance(vec3<f32>(r, g, b));
        r = clamp(lum + (r - lum) * 0.7, 0.0, 1.0);
        g = clamp(lum + (g - lum) * 0.7, 0.0, 1.0);
        b = clamp(lum + (b - lum) * 0.7, 0.0, 1.0);
        result = vec3<f32>(r, g, b);
    } else if (filter_mode == 8u) {
        // Duotone: Two-color gradient mapping (deep blue to golden yellow)
        let lum = luminance(color);
        let dark = vec3<f32>(0.1, 0.1, 0.4);
        let light = vec3<f32>(1.0, 0.9, 0.5);
        result = mix(dark, light, lum);
    } else if (filter_mode == 9u) {
        // Vignette: Darkened edges
        let center = vec2<f32>(0.5, 0.5);
        let dist = distance(tex_coords, center);
        let vignette = 1.0 - smoothstep(0.3, 0.9, dist);
        result = color * vignette;
    } else if (filter_mode == 10u) {
        // Negative: Inverted colors
        result = vec3<f32>(1.0 - color.r, 1.0 - color.g, 1.0 - color.b);
    } else if (filter_mode == 11u) {
        // Posterize: Reduced color levels (pop-art style)
        let levels = 4.0;
        result = vec3<f32>(
            floor(color.r * levels) / levels,
            floor(color.g * levels) / levels,
            floor(color.b * levels) / levels
        );
    } else if (filter_mode == 12u) {
        // Solarize: Partially inverted tones (threshold-based)
        let threshold = 0.5;
        var r = color.r;
        var g = color.g;
        var b = color.b;
        if (r > threshold) { r = 1.0 - r; }
        if (g > threshold) { g = 1.0 - g; }
        if (b > threshold) { b = 1.0 - b; }
        result = vec3<f32>(r, g, b);
    }
    // Note: Filters 13 (ChromaticAberration) and 14 (Pencil) require texture sampling
    // and are handled separately in each shader that supports them.

    return result;
}
