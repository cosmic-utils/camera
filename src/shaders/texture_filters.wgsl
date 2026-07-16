// SPDX-License-Identifier: GPL-3.0-only
// Filters that RE-SAMPLE the source texture — Chromatic Aberration (13) and
// Pencil (14) — for the fragment shaders that draw the camera frame.
//
// They cannot live in `filters.wgsl` next to the other thirteen: that prelude is
// also concatenated into COMPUTE modules (`filter_compute.wgsl`), and
// `textureSample` is fragment-only, so a compute module carrying one fails
// validation whether or not the entry point reaches it.
//
// The texture and sampler arrive as parameters rather than as globals so the
// sharp preview (`video_shader.wgsl`) and pass 0 of the frosted blur chain
// (`video_shader_blur.wgsl`) run the SAME code over their own bindings. That is
// the point, not tidiness: the frosted backdrop exists to line up with the sharp
// preview it sits on, so a Sobel threshold or a paper-grain constant that
// drifted between the two would put a visibly different sketch behind the
// overlay chrome than in front of it.
//
// Requires `filters.wgsl` (luminance, hash, apply_filter) ahead of it.

// Sample luminance at a UV, for edge detection.
fn sample_luminance_tex(uv: vec2<f32>, tex: texture_2d<f32>, samp: sampler) -> f32 {
    return luminance(textureSample(tex, samp, uv).rgb);
}

// Sobel edge detection for the pencil effect.
fn sobel_edge_tex(
    uv: vec2<f32>,
    texel_size: vec2<f32>,
    tex: texture_2d<f32>,
    samp: sampler,
) -> f32 {
    let tl = sample_luminance_tex(uv + vec2<f32>(-texel_size.x, -texel_size.y), tex, samp);
    let tm = sample_luminance_tex(uv + vec2<f32>(0.0, -texel_size.y), tex, samp);
    let tr = sample_luminance_tex(uv + vec2<f32>(texel_size.x, -texel_size.y), tex, samp);
    let ml = sample_luminance_tex(uv + vec2<f32>(-texel_size.x, 0.0), tex, samp);
    let mr = sample_luminance_tex(uv + vec2<f32>(texel_size.x, 0.0), tex, samp);
    let bl = sample_luminance_tex(uv + vec2<f32>(-texel_size.x, texel_size.y), tex, samp);
    let bm = sample_luminance_tex(uv + vec2<f32>(0.0, texel_size.y), tex, samp);
    let br = sample_luminance_tex(uv + vec2<f32>(texel_size.x, texel_size.y), tex, samp);

    let gx = -tl - 2.0 * ml - bl + tr + 2.0 * mr + br;
    let gy = -tl - 2.0 * tm - tr + bl + 2.0 * bm + br;

    return sqrt(gx * gx + gy * gy);
}

// Apply any filter (0-14) to a colour already sampled at `tex_coords`.
//
// Total over the whole filter range: modes 0-12 delegate to `apply_filter`, so a
// caller that draws the camera frame can route every mode through here and never
// has to know which ones re-sample.
fn apply_texture_filter(
    color: vec3<f32>,
    filter_mode: u32,
    tex_coords: vec2<f32>,
    tex: texture_2d<f32>,
    samp: sampler,
) -> vec3<f32> {
    if (filter_mode <= 12u) {
        return apply_filter(color, filter_mode, tex_coords);
    }

    if (filter_mode == 13u) {
        // Chromatic Aberration: RGB channel split (needs texture re-sampling)
        let offset_uv = 0.004; // 0.4% of width
        let color_r = textureSample(tex, samp, tex_coords + vec2<f32>(offset_uv, 0.0));
        let color_b = textureSample(tex, samp, tex_coords - vec2<f32>(offset_uv, 0.0));
        return vec3<f32>(color_r.r, color.g, color_b.b);
    }

    if (filter_mode == 14u) {
        // Pencil: pencil sketch drawing effect (needs texture re-sampling for Sobel).
        // When used with multi-pass pre-blur, input is already smoothed for clean edges.
        let tex_size = vec2<f32>(textureDimensions(tex));
        let texel_size = 1.0 / tex_size;
        let edge = sobel_edge_tex(tex_coords, texel_size, tex, samp);

        // Use smooth edge response for natural pencil pressure variation
        let edge_strength = smoothstep(0.02, 0.25, edge);

        // Invert: dark strokes on light paper
        let pencil = 1.0 - edge_strength;

        // Two-layer paper texture: coarse grain + symmetric fine noise
        let coarse = hash(floor(tex_coords * tex_size * 0.5) * 0.7) * 0.04;
        let fine = (hash(tex_coords * tex_size) - 0.5) * 0.06;
        let paper = 0.96 + coarse + fine;

        let final_val = clamp(pencil * paper, 0.0, 1.0);
        // Slight warm tint for natural paper look
        return vec3<f32>(final_val, final_val * 0.98, final_val * 0.95);
    }

    return color;
}
