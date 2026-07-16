// SPDX-License-Identifier: GPL-3.0-only
// Shared UI-geometry helpers for shaders that draw panels.
// This is the single source of truth for the rounded-rect silhouette.

// Signed distance from `pos` to a rounded rectangle centred on the origin with
// half-extents `size`. Negative inside, positive outside, so a caller gets an
// antialiased edge with `1.0 - smoothstep(-1.0, 1.0, dist)`.
fn rounded_box_sdf(pos: vec2<f32>, size: vec2<f32>, radius: f32) -> f32 {
    let d = abs(pos) - size + vec2<f32>(radius, radius);
    return min(max(d.x, d.y), 0.0) + length(max(d, vec2<f32>(0.0, 0.0))) - radius;
}
