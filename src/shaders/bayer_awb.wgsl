// SPDX-License-Identifier: GPL-3.0-only
// GPU AWB (auto white balance) reduction shader for Bayer sensor data
//
// Accumulates scaled R, G, B channel sums from raw Bayer data using
// workgroup shared memory reduction + global atomics.
//
// Each thread processes one 2x2 Bayer superpixel (1R + 2G + 1B).
// Pixel values are scaled by 64 before accumulation to stay within u32 range.
// The scale factor cancels out when computing gain ratios in the finalize pass.

struct AwbParams {
    width: u32,
    height: u32,
    pattern: u32,   // 0=RGGB, 1=BGGR, 2=GRBG, 3=GBRG
    _pad: u32,
}

struct AwbSums {
    sum_r: atomic<u32>,
    sum_g: atomic<u32>,
    sum_b: atomic<u32>,
    _pad: u32,
}

@group(0) @binding(0) var tex_bayer: texture_2d<f32>;
@group(0) @binding(1) var<uniform> params: AwbParams;
@group(0) @binding(2) var<storage, read_write> sums: AwbSums;

var<workgroup> local_r: array<u32, 256>;
var<workgroup> local_g: array<u32, 256>;
var<workgroup> local_b: array<u32, 256>;

@compute @workgroup_size(16, 16)
fn main(
    @builtin(global_invocation_id) gid: vec3<u32>,
    @builtin(local_invocation_index) lid: u32,
) {
    // Each thread handles one 2x2 superpixel
    let sx = gid.x * 2u;
    let sy = gid.y * 2u;

    var r_val: u32 = 0u;
    var g_val: u32 = 0u;
    var b_val: u32 = 0u;

    if (sx + 1u < params.width && sy + 1u < params.height) {
        // Read 4 pixels of the superpixel, scale by 64 for integer accumulation
        let p00 = u32(textureLoad(tex_bayer, vec2(sx, sy), 0).r * 64.0);
        let p10 = u32(textureLoad(tex_bayer, vec2(sx + 1u, sy), 0).r * 64.0);
        let p01 = u32(textureLoad(tex_bayer, vec2(sx, sy + 1u), 0).r * 64.0);
        let p11 = u32(textureLoad(tex_bayer, vec2(sx + 1u, sy + 1u), 0).r * 64.0);

        // Map 2x2 positions to R/G/B based on Bayer pattern
        if (params.pattern == 0u) {
            // RGGB: (0,0)=R, (1,0)=G, (0,1)=G, (1,1)=B
            r_val = p00; g_val = p10 + p01; b_val = p11;
        } else if (params.pattern == 1u) {
            // BGGR: (0,0)=B, (1,0)=G, (0,1)=G, (1,1)=R
            b_val = p00; g_val = p10 + p01; r_val = p11;
        } else if (params.pattern == 2u) {
            // GRBG: (0,0)=G, (1,0)=R, (0,1)=B, (1,1)=G
            g_val = p00 + p11; r_val = p10; b_val = p01;
        } else {
            // GBRG: (0,0)=G, (1,0)=B, (0,1)=R, (1,1)=G
            g_val = p00 + p11; b_val = p10; r_val = p01;
        }
    }

    // Store in workgroup shared memory
    local_r[lid] = r_val;
    local_g[lid] = g_val;
    local_b[lid] = b_val;
    workgroupBarrier();

    // Parallel tree reduction within workgroup (256 → 1)
    for (var stride = 128u; stride > 0u; stride >>= 1u) {
        if (lid < stride) {
            local_r[lid] += local_r[lid + stride];
            local_g[lid] += local_g[lid + stride];
            local_b[lid] += local_b[lid + stride];
        }
        workgroupBarrier();
    }

    // Thread 0 atomically adds workgroup total to global sums
    if (lid == 0u) {
        atomicAdd(&sums.sum_r, local_r[0]);
        atomicAdd(&sums.sum_g, local_g[0]);
        atomicAdd(&sums.sum_b, local_b[0]);
    }
}
