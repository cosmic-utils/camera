// SPDX-License-Identifier: GPL-3.0-only
//
// Tile-based alignment for night mode
//
// Implements hierarchical coarse-to-fine alignment:
// - L2 cost (sum of squared differences) at coarse levels with sub-pixel refinement
// - L1 cost (sum of absolute differences) at finest level
// - Per-tile displacement vectors
//
// Based on HDR+ paper Section 4:
// "At coarse scales we compute a sub-pixel alignment, minimize L2 residuals,
// and use a large search radius."
//
// Sub-pixel alignment uses FFT-accelerated quadratic fitting from Section 4.1.

const PI: f32 = 3.14159265359;

struct AlignParams {
    width: u32,              // Image width at this level
    height: u32,             // Image height at this level
    tile_size: u32,          // Tile size (32, 16, or 8)
    tile_step: u32,          // Tile step (tile_size / 2 for 50% overlap)
    search_dist: u32,        // Search distance (typically 2)
    n_tiles_x: u32,          // Number of tiles in X
    n_tiles_y: u32,          // Number of tiles in Y
    use_l2: u32,             // 1 for L2 (coarse), 0 for L1 (fine)
    prev_tile_step: u32,     // Previous level's tile step (for proper upsampling)
    prev_n_tiles_y: u32,     // Previous level's n_tiles_y (for bounds checking)
    tile_row_offset: u32,    // Row offset for chunked dispatch (GPU preemption)
    _padding1: u32,
}

// Reference image (grayscale luminance)
@group(0) @binding(0)
var<storage, read> reference: array<f32>;

// Comparison image (grayscale luminance)
@group(0) @binding(1)
var<storage, read> comparison: array<f32>;

// Output alignment vectors (dx, dy per tile) - using f32 for sub-pixel precision
@group(0) @binding(2)
var<storage, read_write> alignment: array<vec2<f32>>;

// Previous level alignment (for hierarchical upsampling)
@group(0) @binding(3)
var<storage, read> prev_alignment: array<vec2<f32>>;

@group(0) @binding(4)
var<uniform> params: AlignParams;

// Previous level tile counts (for upsampling)
@group(0) @binding(5)
var<uniform> prev_n_tiles_x: u32;

//=============================================================================
// Utility functions
//=============================================================================

fn get_tile_idx(tx: u32, ty: u32) -> u32 {
    return ty * params.n_tiles_x + tx;
}

// Inline pixel access functions for each buffer (WGSL doesn't allow storage pointer params)
fn get_ref_pixel(x: i32, y: i32) -> f32 {
    // Use edge clamping for out-of-bounds access (repeat boundary pixels)
    // This avoids bias toward image center that would occur with zero-padding
    let cx = clamp(x, 0, i32(params.width) - 1);
    let cy = clamp(y, 0, i32(params.height) - 1);
    let idx = u32(cy) * params.width + u32(cx);
    return reference[idx];
}

fn get_comp_pixel(x: i32, y: i32) -> f32 {
    // Use edge clamping for out-of-bounds access (repeat boundary pixels)
    // This avoids bias toward image center that would occur with zero-padding
    let cx = clamp(x, 0, i32(params.width) - 1);
    let cy = clamp(y, 0, i32(params.height) - 1);
    let idx = u32(cy) * params.width + u32(cx);
    return comparison[idx];
}

//=============================================================================
// Compute tile cost between reference and displaced comparison
//=============================================================================

fn compute_tile_cost(
    tile_x: u32,
    tile_y: u32,
    dx: i32,
    dy: i32
) -> f32 {
    var cost = 0.0;
    let tile_start_x = tile_x * params.tile_step;
    let tile_start_y = tile_y * params.tile_step;

    for (var py = 0u; py < params.tile_size; py++) {
        for (var px = 0u; px < params.tile_size; px++) {
            let ref_x = i32(tile_start_x + px);
            let ref_y = i32(tile_start_y + py);

            let comp_x = ref_x + dx;
            let comp_y = ref_y + dy;

            let ref_val = get_ref_pixel(ref_x, ref_y);
            let comp_val = get_comp_pixel(comp_x, comp_y);
            let diff = ref_val - comp_val;

            if (params.use_l2 == 1u) {
                cost += diff * diff;  // L2: sum of squared differences
            } else {
                cost += abs(diff);    // L1: sum of absolute differences
            }
        }
    }

    return cost;
}

//=============================================================================
// Sub-pixel refinement using quadratic fitting
// Based on HDR+ paper Section 4.1:
// "To produce a subpixel estimate of motion, we fit a bivariate polynomial
// to the 3×3 window surrounding (û, v̂) and find the minimum of that polynomial."
//=============================================================================

// Fit bivariate quadratic to 3x3 cost surface and find minimum
// D2(u,v) ≈ 1/2 [u v] A [u; v] + b^T [u; v] + c
// Minimum at: μ = -A^(-1) b
fn subpixel_refine(costs: array<f32, 9>) -> vec2<f32> {
    // 3x3 cost grid:
    // costs[0] costs[1] costs[2]   (-1,-1) (0,-1) (1,-1)
    // costs[3] costs[4] costs[5]   (-1, 0) (0, 0) (1, 0)
    // costs[6] costs[7] costs[8]   (-1, 1) (0, 1) (1, 1)

    // Fit 2D quadratic using least squares
    // f(x,y) = a*x^2 + b*y^2 + c*x*y + d*x + e*y + f

    // Simplified approach: fit parabolas along x and y independently
    // then combine. This is an approximation but faster.

    // X direction (row at y=0): costs[3], costs[4], costs[5]
    let c_left = costs[3];
    let c_center = costs[4];
    let c_right = costs[5];

    // Fit parabola: f(x) = ax^2 + bx + c
    // f(-1) = c_left, f(0) = c_center, f(1) = c_right
    // a = (c_left + c_right) / 2 - c_center
    // b = (c_right - c_left) / 2
    // Minimum at x = -b / (2a)
    let a_x = (c_left + c_right) * 0.5 - c_center;
    let b_x = (c_right - c_left) * 0.5;

    var sub_x = 0.0;
    if (abs(a_x) > 0.0001) {
        sub_x = clamp(-b_x / (2.0 * a_x), -0.5, 0.5);
    }

    // Y direction (column at x=0): costs[1], costs[4], costs[7]
    let c_top = costs[1];
    let c_bottom = costs[7];

    let a_y = (c_top + c_bottom) * 0.5 - c_center;
    let b_y = (c_bottom - c_top) * 0.5;

    var sub_y = 0.0;
    if (abs(a_y) > 0.0001) {
        sub_y = clamp(-b_y / (2.0 * a_y), -0.5, 0.5);
    }

    return vec2<f32>(sub_x, sub_y);
}

// Full 2D quadratic fit using weighted least squares (more accurate)
fn subpixel_refine_2d(costs: array<f32, 9>) -> vec2<f32> {
    // Solve for minimum of bivariate quadratic:
    // f(x,y) = Axx*x^2 + Ayy*y^2 + Axy*x*y + Bx*x + By*y + C
    //
    // Using the 6 filter kernels from HDR+ supplement for coefficient extraction

    // Filter kernels for 3x3 patch (each multiplied by cost at that position)
    // Kernel for Axx (second derivative in x):  [1 -2 1; 1 -2 1; 1 -2 1] / 6
    let Axx = (costs[0] - 2.0*costs[1] + costs[2] +
               costs[3] - 2.0*costs[4] + costs[5] +
               costs[6] - 2.0*costs[7] + costs[8]) / 6.0;

    // Kernel for Ayy (second derivative in y):  [1 1 1; -2 -2 -2; 1 1 1] / 6
    let Ayy = (costs[0] + costs[1] + costs[2] -
               2.0*costs[3] - 2.0*costs[4] - 2.0*costs[5] +
               costs[6] + costs[7] + costs[8]) / 6.0;

    // Kernel for Axy (cross derivative):  [1 0 -1; 0 0 0; -1 0 1] / 4
    let Axy = (costs[0] - costs[2] - costs[6] + costs[8]) / 4.0;

    // Kernel for Bx (first derivative in x):  [-1 0 1; -1 0 1; -1 0 1] / 6
    let Bx = (-costs[0] + costs[2] - costs[3] + costs[5] - costs[6] + costs[8]) / 6.0;

    // Kernel for By (first derivative in y):  [-1 -1 -1; 0 0 0; 1 1 1] / 6
    let By = (-costs[0] - costs[1] - costs[2] + costs[6] + costs[7] + costs[8]) / 6.0;

    // Solve 2x2 system: [Axx Axy/2; Axy/2 Ayy] * [x; y] = -[Bx; By]
    // Using Cramer's rule
    let det = Axx * Ayy - (Axy * Axy) / 4.0;

    if (abs(det) < 0.0001) {
        // Matrix nearly singular, fall back to separable solution
        return subpixel_refine(costs);
    }

    let sub_x = clamp((-(Ayy * Bx - Axy * By / 2.0)) / det, -0.5, 0.5);
    let sub_y = clamp((-(Axx * By - Axy * Bx / 2.0)) / det, -0.5, 0.5);

    return vec2<f32>(sub_x, sub_y);
}

//=============================================================================
// Main alignment kernel - one thread per tile
// Performs exhaustive search over 5x5 grid with optional sub-pixel refinement
//=============================================================================

@compute @workgroup_size(1, 1, 1)
fn align_tiles_simple(@builtin(global_invocation_id) gid: vec3<u32>) {
    let tile_x = gid.x;
    let tile_y = gid.y;

    if (tile_x >= params.n_tiles_x || tile_y >= params.n_tiles_y) {
        return;
    }

    let tile_idx = get_tile_idx(tile_x, tile_y);

    // Get initial offset from previous level (sub-pixel, so multiply by 2.0)
    var initial_dx = 0.0;
    var initial_dy = 0.0;

    if (prev_n_tiles_x > 0u && params.prev_tile_step > 0u) {
        // Current tile's center pixel position
        let tile_center_x = tile_x * params.tile_step + params.tile_size / 2u;
        let tile_center_y = tile_y * params.tile_step + params.tile_size / 2u;

        // Scale to previous level coordinates (image is 2x smaller)
        let prev_pixel_x = tile_center_x / 2u;
        let prev_pixel_y = tile_center_y / 2u;

        // Find tile index in previous level that contains this pixel
        let prev_tx = min(prev_pixel_x / params.prev_tile_step, prev_n_tiles_x - 1u);
        let prev_ty = min(prev_pixel_y / params.prev_tile_step, params.prev_n_tiles_y - 1u);

        let prev_idx = prev_ty * prev_n_tiles_x + prev_tx;
        let prev_vec = prev_alignment[prev_idx];
        // Scale up by 2 for pyramid upsampling, round to nearest integer for search
        initial_dx = prev_vec.x * 2.0;
        initial_dy = prev_vec.y * 2.0;
    }

    // Round initial offset to integer for search grid
    let initial_dx_i = i32(round(initial_dx));
    let initial_dy_i = i32(round(initial_dy));

    // Search for best integer alignment
    var best_cost = 1e10;
    var best_dx = initial_dx_i;
    var best_dy = initial_dy_i;

    let search_dist_i = i32(params.search_dist);

    for (var dy = -search_dist_i; dy <= search_dist_i; dy++) {
        for (var dx = -search_dist_i; dx <= search_dist_i; dx++) {
            let test_dx = initial_dx_i + dx;
            let test_dy = initial_dy_i + dy;

            let cost = compute_tile_cost(tile_x, tile_y, test_dx, test_dy);

            if (cost < best_cost) {
                best_cost = cost;
                best_dx = test_dx;
                best_dy = test_dy;
            }
        }
    }

    // For L2 (coarse levels), apply sub-pixel refinement
    var final_dx = f32(best_dx);
    var final_dy = f32(best_dy);

    if (params.use_l2 == 1u) {
        // Compute 3x3 cost surface around best integer location
        var costs: array<f32, 9>;
        var idx = 0u;
        for (var dy = -1; dy <= 1; dy++) {
            for (var dx = -1; dx <= 1; dx++) {
                costs[idx] = compute_tile_cost(tile_x, tile_y, best_dx + dx, best_dy + dy);
                idx++;
            }
        }

        // Sub-pixel refinement using 2D quadratic fit
        let subpixel = subpixel_refine_2d(costs);
        final_dx = f32(best_dx) + subpixel.x;
        final_dy = f32(best_dy) + subpixel.y;
    }

    alignment[tile_idx] = vec2<f32>(final_dx, final_dy);
}

//=============================================================================
// Upsampling error correction
// At motion boundaries, upsampled alignment vectors may be inaccurate.
// Based on HDR+ paper Section 4:
// "We take as candidates the alignments for the 3 nearest coarse-scale tiles,
// the nearest neighbor tile plus the next-nearest tiles in each dimension."
//
// This tests 3 candidates from the PREVIOUS (coarser) pyramid level:
// 1. Nearest coarse tile (what we already upsampled from)
// 2. Next-nearest in X direction
// 3. Next-nearest in Y direction
//=============================================================================

@compute @workgroup_size(1, 1, 1)
fn correct_upsampling_error(@builtin(global_invocation_id) gid: vec3<u32>) {
    let tile_x = gid.x;
    // Add row offset for chunked dispatch support
    let tile_y = gid.y + params.tile_row_offset;

    if (tile_x >= params.n_tiles_x || tile_y >= params.n_tiles_y) {
        return;
    }

    // Skip if no previous level available
    if (prev_n_tiles_x == 0u || params.prev_tile_step == 0u) {
        return;
    }

    let tile_idx = get_tile_idx(tile_x, tile_y);

    // Current tile's center pixel position
    let tile_center_x = tile_x * params.tile_step + params.tile_size / 2u;
    let tile_center_y = tile_y * params.tile_step + params.tile_size / 2u;

    // Scale to previous level coordinates (image is 2x smaller)
    let prev_pixel_x = tile_center_x / 2u;
    let prev_pixel_y = tile_center_y / 2u;

    // Find nearest coarse-scale tile (candidate 0)
    let prev_tx = min(prev_pixel_x / params.prev_tile_step, prev_n_tiles_x - 1u);
    let prev_ty = min(prev_pixel_y / params.prev_tile_step, params.prev_n_tiles_y - 1u);

    // Determine which direction to look for next-nearest tiles
    // If we're in the left half of the coarse tile, look right; otherwise look left
    let coarse_tile_center_x = prev_tx * params.prev_tile_step + params.prev_tile_step / 2u;
    let coarse_tile_center_y = prev_ty * params.prev_tile_step + params.prev_tile_step / 2u;

    let x_dir = select(-1i, 1i, prev_pixel_x >= coarse_tile_center_x);
    let y_dir = select(-1i, 1i, prev_pixel_y >= coarse_tile_center_y);

    // Next-nearest tile in X direction (candidate 1)
    let prev_tx_neighbor = clamp(i32(prev_tx) + x_dir, 0i, i32(prev_n_tiles_x) - 1i);

    // Next-nearest tile in Y direction (candidate 2)
    let prev_ty_neighbor = clamp(i32(prev_ty) + y_dir, 0i, i32(params.prev_n_tiles_y) - 1i);

    // Get coarse-level indices
    let idx0 = prev_ty * prev_n_tiles_x + prev_tx;                              // nearest
    let idx1 = prev_ty * prev_n_tiles_x + u32(prev_tx_neighbor);                // next-nearest X
    let idx2 = u32(prev_ty_neighbor) * prev_n_tiles_x + prev_tx;                // next-nearest Y

    // Get alignment vectors from COARSE level, scaled up by 2
    let align0 = prev_alignment[idx0] * 2.0;
    let align1 = prev_alignment[idx1] * 2.0;
    let align2 = prev_alignment[idx2] * 2.0;

    // Compute L1 cost for each candidate at current level
    // Use L1 (sum of absolute differences) as paper suggests for finest level
    let cost0 = compute_tile_cost(tile_x, tile_y, i32(round(align0.x)), i32(round(align0.y)));
    let cost1 = compute_tile_cost(tile_x, tile_y, i32(round(align1.x)), i32(round(align1.y)));
    let cost2 = compute_tile_cost(tile_x, tile_y, i32(round(align2.x)), i32(round(align2.y)));

    // Select best candidate and re-run search from that starting point
    var best_initial: vec2<f32>;
    if (cost0 <= cost1 && cost0 <= cost2) {
        best_initial = align0;
    } else if (cost1 <= cost2) {
        best_initial = align1;
    } else {
        best_initial = align2;
    }

    // Round to integer for search grid
    let initial_dx = i32(round(best_initial.x));
    let initial_dy = i32(round(best_initial.y));

    // Re-run local search from best candidate
    var best_cost = 1e10;
    var best_dx = initial_dx;
    var best_dy = initial_dy;

    let search_dist_i = i32(params.search_dist);

    for (var dy = -search_dist_i; dy <= search_dist_i; dy++) {
        for (var dx = -search_dist_i; dx <= search_dist_i; dx++) {
            let test_dx = initial_dx + dx;
            let test_dy = initial_dy + dy;

            let cost = compute_tile_cost(tile_x, tile_y, test_dx, test_dy);

            if (cost < best_cost) {
                best_cost = cost;
                best_dx = test_dx;
                best_dy = test_dy;
            }
        }
    }

    // Store final alignment (integer for finest level)
    alignment[tile_idx] = vec2<f32>(f32(best_dx), f32(best_dy));
}

//=============================================================================
// Convert RGB to grayscale luminance or extract single channel
// (preprocessing step for per-channel alignment)
//=============================================================================

struct LuminanceParams {
    width: u32,
    height: u32,
    /// Channel to extract: 0=R, 1=G, 2=B, 3=luminance (for per-channel alignment)
    channel: u32,
    _padding1: u32,
}

@group(0) @binding(0)
var<storage, read> rgb_input: array<f32>;

@group(0) @binding(1)
var<storage, read_write> luminance_output: array<f32>;

@group(0) @binding(2)
var<uniform> lum_params: LuminanceParams;

/// Convert RGBA to luminance (BT.601 grayscale)
/// Legacy entry point - always uses luminance mode
@compute @workgroup_size(16, 16)
fn rgb_to_luminance(@builtin(global_invocation_id) gid: vec3<u32>) {
    let x = gid.x;
    let y = gid.y;

    if (x >= lum_params.width || y >= lum_params.height) {
        return;
    }

    let rgb_idx = (y * lum_params.width + x) * 4u;
    let r = rgb_input[rgb_idx];
    let g = rgb_input[rgb_idx + 1u];
    let b = rgb_input[rgb_idx + 2u];

    // BT.601 luminance
    let lum = 0.299 * r + 0.587 * g + 0.114 * b;

    let lum_idx = y * lum_params.width + x;
    luminance_output[lum_idx] = lum;
}

/// Extract single channel or compute luminance based on channel parameter
/// channel: 0=R, 1=G, 2=B, 3=luminance
@compute @workgroup_size(16, 16)
fn rgb_to_channel(@builtin(global_invocation_id) gid: vec3<u32>) {
    let x = gid.x;
    let y = gid.y;

    if (x >= lum_params.width || y >= lum_params.height) {
        return;
    }

    let rgb_idx = (y * lum_params.width + x) * 4u;
    let r = rgb_input[rgb_idx];
    let g = rgb_input[rgb_idx + 1u];
    let b = rgb_input[rgb_idx + 2u];

    var value: f32;
    switch (lum_params.channel) {
        case 0u: { value = r; }                               // Red
        case 1u: { value = g; }                               // Green
        case 2u: { value = b; }                               // Blue
        default: { value = 0.299 * r + 0.587 * g + 0.114 * b; } // Luminance
    }

    let out_idx = y * lum_params.width + x;
    luminance_output[out_idx] = value;
}

//=============================================================================
// Parallelized alignment kernel - 32 threads per tile
// Computes 25 search positions (5x5 grid) in parallel for ~5x speedup
//=============================================================================

// Shared memory for parallel search
var<workgroup> search_costs: array<f32, 32>;  // Cost for each search position (25 used)
var<workgroup> search_dx: array<i32, 32>;     // dx offset for each position
var<workgroup> search_dy: array<i32, 32>;     // dy offset for each position
var<workgroup> subpixel_costs: array<f32, 16>; // For 3x3 sub-pixel refinement (9 used)
var<workgroup> shared_tile_x: u32;
var<workgroup> shared_tile_y: u32;
var<workgroup> shared_initial_dx: i32;
var<workgroup> shared_initial_dy: i32;

@compute @workgroup_size(32, 1, 1)
fn align_tiles_parallel(
    @builtin(workgroup_id) wid: vec3<u32>,
    @builtin(local_invocation_id) lid: vec3<u32>
) {
    let tile_x = wid.x;
    // Add row offset for chunked dispatch support
    let tile_y = wid.y + params.tile_row_offset;
    let thread_id = lid.x;

    if (tile_x >= params.n_tiles_x || tile_y >= params.n_tiles_y) {
        return;
    }

    // Thread 0 computes initial offset from previous level
    if (thread_id == 0u) {
        shared_tile_x = tile_x;
        shared_tile_y = tile_y;

        var initial_dx = 0.0;
        var initial_dy = 0.0;

        if (prev_n_tiles_x > 0u && params.prev_tile_step > 0u) {
            let tile_center_x = tile_x * params.tile_step + params.tile_size / 2u;
            let tile_center_y = tile_y * params.tile_step + params.tile_size / 2u;
            let prev_pixel_x = tile_center_x / 2u;
            let prev_pixel_y = tile_center_y / 2u;
            let prev_tx = min(prev_pixel_x / params.prev_tile_step, prev_n_tiles_x - 1u);
            let prev_ty = min(prev_pixel_y / params.prev_tile_step, params.prev_n_tiles_y - 1u);
            let prev_idx = prev_ty * prev_n_tiles_x + prev_tx;
            let prev_vec = prev_alignment[prev_idx];
            initial_dx = prev_vec.x * 2.0;
            initial_dy = prev_vec.y * 2.0;
        }

        shared_initial_dx = i32(round(initial_dx));
        shared_initial_dy = i32(round(initial_dy));
    }
    workgroupBarrier();

    let initial_dx_i = shared_initial_dx;
    let initial_dy_i = shared_initial_dy;
    let search_dist_i = i32(params.search_dist);
    let search_width = 2u * params.search_dist + 1u;  // e.g., 5 for search_dist=2
    let num_positions = search_width * search_width;  // e.g., 25

    // Phase 1: Parallel search - each thread computes cost for one search position
    if (thread_id < num_positions) {
        let sx = i32(thread_id % search_width) - search_dist_i;
        let sy = i32(thread_id / search_width) - search_dist_i;
        let test_dx = initial_dx_i + sx;
        let test_dy = initial_dy_i + sy;

        let cost = compute_tile_cost(tile_x, tile_y, test_dx, test_dy);

        search_costs[thread_id] = cost;
        search_dx[thread_id] = test_dx;
        search_dy[thread_id] = test_dy;
    } else {
        // Threads beyond num_positions store large cost to be ignored
        search_costs[thread_id] = 1e10;
    }
    workgroupBarrier();

    // Phase 2: Thread 0 finds minimum (simple sequential for small array)
    var best_idx = 0u;
    var best_cost = 1e10;
    var best_dx = initial_dx_i;
    var best_dy = initial_dy_i;

    if (thread_id == 0u) {
        for (var i = 0u; i < num_positions; i++) {
            if (search_costs[i] < best_cost) {
                best_cost = search_costs[i];
                best_idx = i;
            }
        }
        best_dx = search_dx[best_idx];
        best_dy = search_dy[best_idx];

        // Store for other threads to use in sub-pixel phase
        shared_initial_dx = best_dx;
        shared_initial_dy = best_dy;
    }
    workgroupBarrier();

    best_dx = shared_initial_dx;
    best_dy = shared_initial_dy;

    // Phase 3: Sub-pixel refinement for L2 (coarse levels) - 9 threads compute 3x3 costs
    var final_dx = f32(best_dx);
    var final_dy = f32(best_dy);

    if (params.use_l2 == 1u) {
        // Threads 0-8 compute the 3x3 cost surface in parallel
        if (thread_id < 9u) {
            let sx = i32(thread_id % 3u) - 1;
            let sy = i32(thread_id / 3u) - 1;
            subpixel_costs[thread_id] = compute_tile_cost(tile_x, tile_y, best_dx + sx, best_dy + sy);
        }
        workgroupBarrier();

        // Thread 0 applies quadratic fit
        if (thread_id == 0u) {
            var costs: array<f32, 9>;
            for (var i = 0u; i < 9u; i++) {
                costs[i] = subpixel_costs[i];
            }
            let subpixel = subpixel_refine_2d(costs);
            final_dx = f32(best_dx) + subpixel.x;
            final_dy = f32(best_dy) + subpixel.y;

            let tile_idx = get_tile_idx(tile_x, tile_y);
            alignment[tile_idx] = vec2<f32>(final_dx, final_dy);
        }
    } else {
        // For L1 (fine level), just write result
        if (thread_id == 0u) {
            let tile_idx = get_tile_idx(tile_x, tile_y);
            alignment[tile_idx] = vec2<f32>(final_dx, final_dy);
        }
    }
}
