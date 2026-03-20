// Gradient fade shader for carousel edge overlays.

struct FadeUniform {
    color: vec4<f32>,
    size: vec2<f32>,        // widget size in physical pixels
    corner_radius: f32,     // corner radius in physical pixels
    direction: u32,         // 0 = opaque LEFT, transparent RIGHT
                            // 1 = transparent LEFT, opaque RIGHT
}

@group(0) @binding(0)
var<uniform> fade: FadeUniform;

struct VertexOutput {
    @builtin(position) position: vec4<f32>,
    @location(0) uv: vec2<f32>,
}

@vertex
fn vs_main(@builtin(vertex_index) vi: u32) -> VertexOutput {
    var positions = array<vec2<f32>, 6>(
        vec2<f32>(0.0, 0.0),
        vec2<f32>(1.0, 0.0),
        vec2<f32>(0.0, 1.0),
        vec2<f32>(1.0, 0.0),
        vec2<f32>(1.0, 1.0),
        vec2<f32>(0.0, 1.0),
    );
    let pos = positions[vi];
    var out: VertexOutput;
    out.position = vec4<f32>(pos.x * 2.0 - 1.0, -(pos.y * 2.0 - 1.0), 0.0, 1.0);
    out.uv = pos;
    return out;
}

@fragment
fn fs_main(fin: VertexOutput) -> @location(0) vec4<f32> {
    let pixel = fin.uv * fade.size;
    let r = fade.corner_radius;

    var alpha: f32;
    if (fade.direction == 0u) {
        alpha = 1.0 - fin.uv.x;
    } else {
        alpha = fin.uv.x;
    }

    // Corner distances
    let tl_d = length(pixel - vec2<f32>(r, r)) - r;
    let bl_d = length(pixel - vec2<f32>(r, fade.size.y - r)) - r;
    let tr_d = length(pixel - vec2<f32>(fade.size.x - r, r)) - r;
    let br_d = length(pixel - vec2<f32>(fade.size.x - r, fade.size.y - r)) - r;

    // Corner region flags
    let in_tl = (1.0 - step(r, pixel.x)) * (1.0 - step(r, pixel.y));
    let in_bl = (1.0 - step(r, pixel.x)) * step(fade.size.y - r, pixel.y);
    let in_tr = step(fade.size.x - r, pixel.x) * (1.0 - step(r, pixel.y));
    let in_br = step(fade.size.x - r, pixel.x) * step(fade.size.y - r, pixel.y);

    // Corner alpha masks
    let tl_mask = 1.0 - smoothstep(-1.0, 1.0, tl_d);
    let bl_mask = 1.0 - smoothstep(-1.0, 1.0, bl_d);
    let tr_mask = 1.0 - smoothstep(-1.0, 1.0, tr_d);
    let br_mask = 1.0 - smoothstep(-1.0, 1.0, br_d);

    // Direction: left overlay rounds TL+BL, right overlay rounds TR+BR
    var is_left = 1.0;
    var is_right = 0.0;
    if (fade.direction == 1u) {
        is_left = 0.0;
        is_right = 1.0;
    }

    var cmask = 1.0;
    cmask = cmask * mix(1.0, tl_mask, in_tl * is_left);
    cmask = cmask * mix(1.0, bl_mask, in_bl * is_left);
    cmask = cmask * mix(1.0, tr_mask, in_tr * is_right);
    cmask = cmask * mix(1.0, br_mask, in_br * is_right);

    return vec4<f32>(fade.color.r, fade.color.g, fade.color.b, fade.color.a * alpha * cmask);
}
