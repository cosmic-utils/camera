// SPDX-License-Identifier: GPL-3.0-only
// GPU compute shader for unpacking CSI-2 packed Bayer data (10/12/14-bit)
//
// CSI-2 packed formats store pixel data in groups with high bits first,
// followed by packed low bits:
// - 10-bit: 4 pixels in 5 bytes (bytes 0-3 = high 8 bits, byte 4 = low 2 bits) → scale <<6
// - 12-bit: 2 pixels in 3 bytes (bytes 0-1 = high 8 bits, byte 2 = low 4 bits) → scale <<4
// - 14-bit: 4 pixels in 7 bytes (bytes 0-3 = high 8 bits, bytes 4-6 = low 6 bits) → scale <<2
//
// Each thread processes one pixel pair (2 pixels → 1 u32 output word containing two u16 values).
// Output is written as packed u16 pairs suitable for copy_buffer_to_texture into R16Unorm.

struct UnpackParams {
    width: u32,
    height: u32,
    packed_stride: u32,
    bit_depth: u32,         // 10, 12, or 14
    output_stride_u32: u32, // padded row stride in u32 units
}

@group(0) @binding(0) var<storage, read> packed_data: array<u32>;
@group(0) @binding(1) var<storage, read_write> unpacked_data: array<u32>;
@group(0) @binding(2) var<uniform> params: UnpackParams;

// Read a single byte from the packed u32 array
fn read_byte(offset: u32) -> u32 {
    let word_index = offset / 4u;
    let byte_index = offset % 4u;
    return (packed_data[word_index] >> (byte_index * 8u)) & 0xFFu;
}

@compute @workgroup_size(256)
fn main(@builtin(global_invocation_id) global_id: vec3<u32>) {
    let pair_index = global_id.x; // Which pixel pair (0, 1, 2, ...)
    let y = global_id.y;          // Row

    // Each thread handles 2 pixels (one u32 output word)
    let x0 = pair_index * 2u;
    let x1 = x0 + 1u;

    if (x0 >= params.width || y >= params.height) {
        return;
    }

    let row_offset = y * params.packed_stride;
    var val0: u32 = 0u;
    var val1: u32 = 0u;

    if (params.bit_depth == 10u) {
        // 10-bit: 4 pixels in 5 bytes
        // group = pixel / 4, pos = pixel % 4
        let group0 = x0 / 4u;
        let pos0 = x0 % 4u;
        let base0 = row_offset + group0 * 5u;
        let high0 = read_byte(base0 + pos0);
        let low_byte0 = read_byte(base0 + 4u);
        let low0 = (low_byte0 >> (pos0 * 2u)) & 0x03u;
        val0 = ((high0 << 2u) | low0) << 6u; // 10-bit → 16-bit

        if (x1 < params.width) {
            let group1 = x1 / 4u;
            let pos1 = x1 % 4u;
            let base1 = row_offset + group1 * 5u;
            let high1 = read_byte(base1 + pos1);
            let low_byte1 = read_byte(base1 + 4u);
            let low1 = (low_byte1 >> (pos1 * 2u)) & 0x03u;
            val1 = ((high1 << 2u) | low1) << 6u;
        }
    } else if (params.bit_depth == 12u) {
        // 12-bit: 2 pixels in 3 bytes
        let group0 = x0 / 2u;
        let pos0 = x0 % 2u;
        let base0 = row_offset + group0 * 3u;
        if (pos0 == 0u) {
            let high0 = read_byte(base0);
            let low_byte0 = read_byte(base0 + 2u);
            let low0 = low_byte0 & 0x0Fu;
            val0 = ((high0 << 4u) | low0) << 4u;
        } else {
            let high0 = read_byte(base0 + 1u);
            let low_byte0 = read_byte(base0 + 2u);
            let low0 = (low_byte0 >> 4u) & 0x0Fu;
            val0 = ((high0 << 4u) | low0) << 4u;
        }

        if (x1 < params.width) {
            let group1 = x1 / 2u;
            let pos1 = x1 % 2u;
            let base1 = row_offset + group1 * 3u;
            if (pos1 == 0u) {
                let high1 = read_byte(base1);
                let low_byte1 = read_byte(base1 + 2u);
                let low1 = low_byte1 & 0x0Fu;
                val1 = ((high1 << 4u) | low1) << 4u;
            } else {
                let high1 = read_byte(base1 + 1u);
                let low_byte1 = read_byte(base1 + 2u);
                let low1 = (low_byte1 >> 4u) & 0x0Fu;
                val1 = ((high1 << 4u) | low1) << 4u;
            }
        }
    } else if (params.bit_depth == 14u) {
        // 14-bit: 4 pixels in 7 bytes
        let group0 = x0 / 4u;
        let pos0 = x0 % 4u;
        let base0 = row_offset + group0 * 7u;
        let high0 = read_byte(base0 + pos0);
        // Low 6 bits packed across bytes 4-6 (24 bits total = 4 × 6 bits)
        let low_bits0 = read_byte(base0 + 4u)
            | (read_byte(base0 + 5u) << 8u)
            | (read_byte(base0 + 6u) << 16u);
        let low0 = (low_bits0 >> (pos0 * 6u)) & 0x3Fu;
        val0 = ((high0 << 6u) | low0) << 2u; // 14-bit → 16-bit

        if (x1 < params.width) {
            let group1 = x1 / 4u;
            let pos1 = x1 % 4u;
            let base1 = row_offset + group1 * 7u;
            let high1 = read_byte(base1 + pos1);
            let low_bits1 = read_byte(base1 + 4u)
                | (read_byte(base1 + 5u) << 8u)
                | (read_byte(base1 + 6u) << 16u);
            let low1 = (low_bits1 >> (pos1 * 6u)) & 0x3Fu;
            val1 = ((high1 << 6u) | low1) << 2u;
        }
    }

    // Write packed u16 pair: lower 16 bits = pixel 0, upper 16 bits = pixel 1
    let out_index = y * params.output_stride_u32 + pair_index;
    unpacked_data[out_index] = val0 | (val1 << 16u);
}
