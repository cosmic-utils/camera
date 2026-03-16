// SPDX-License-Identifier: GPL-3.0-only
// AWB finalize shader: converts accumulated R/G/B sums to white balance gains.
//
// Runs as a single thread after the AWB accumulation pass.
// The scale factor (64) used during accumulation cancels out in the ratio:
//   gain_r = avg_g / avg_r = (sum_g / (N*2)) / (sum_r / N) = sum_g / (sum_r * 2)

struct AwbSums {
    sum_r: u32,
    sum_g: u32,
    sum_b: u32,
    _pad: u32,
}

struct AwbGains {
    gain_r: f32,
    gain_b: f32,
}

@group(0) @binding(0) var<storage, read> sums: AwbSums;
@group(0) @binding(1) var<storage, read_write> gains: AwbGains;

@compute @workgroup_size(1)
fn main() {
    let sr = f32(sums.sum_r);
    let sg = f32(sums.sum_g);
    let sb = f32(sums.sum_b);

    // gain = avg_green / avg_channel
    // sum_g has 2 green values per superpixel, so divide by 2 in the ratio
    gains.gain_r = select(1.0, sg / (sr * 2.0), sr > 0.0);
    gains.gain_b = select(1.0, sg / (sb * 2.0), sb > 0.0);
}
