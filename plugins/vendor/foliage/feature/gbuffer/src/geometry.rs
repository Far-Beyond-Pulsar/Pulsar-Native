//! CPU mirror of the procedural tuft geometry in `foliage_gbuffer.wgsl`.
//!
//! There is no vertex buffer and no index buffer anywhere in the grass path. Every vertex
//! is derived from `@builtin(vertex_index)` and a 16-byte record
//! ([`GpuBladeInstance`](helio_pass_foliage_place::GpuBladeInstance)), which is what makes
//! the world's foliage four `draw_indirect` calls. The cost of that is that the geometry
//! only exists inside a shader, where nothing can look at it — so its shapes are written
//! twice, here and in WGSL, and the tests in `tests/geometry.rs` check this copy's
//! properties: blocky blades are connected staircases that never share a cube, strips fit
//! their LOD's vertex count, smooth blades keep their length.
//!
//! A tuft's blades ([`tuft_blade`], [`block_steps`]) are shared with placement
//! (`helio_pass_foliage_place::blade`).

pub use helio_pass_foliage_place::blade::{block_steps, tuft_blade, Blade};
use helio_pass_foliage_place::FOLIAGE_LOD_VERTEX_COUNTS;

/// Number of drawable foliage LOD levels.
pub const LOD_COUNT: usize = 4;

/// Segments up a smooth blade at each LOD: L0 and L1 strips, L2 cards (1), L3 a fan (1).
pub const LOD_SEGMENTS: [u32; LOD_COUNT] = [5, 3, 1, 1];

/// Vertices per instance at each LOD for smooth blades (the producer's
/// [`FOLIAGE_LOD_VERTEX_COUNTS`]; blocky blades are drawn as boxes, nine corners each).
pub const LOD_VERTEX_COUNTS: [u32; LOD_COUNT] = FOLIAGE_LOD_VERTEX_COUNTS;

/// Cubes a blocky blade rises by at most (`FOLIAGE_STACK_CUBES`).
pub const STACK_CUBES: u32 = helio_pass_foliage_place::FOLIAGE_STACK_CUBES;
/// Cubes a blocky blade steps sideways by at most at L0 and at L1 (`FOLIAGE_L1_STEPS`).
pub const STACK_STEPS: u32 = helio_pass_foliage_place::blade::FOLIAGE_STACK_STEPS;
pub const L1_STEPS: u32 = 2;
/// Boxes a blocky blade's strip holds at L0 and L1.
pub const L0_BOXES: u32 = 8;
pub const L1_BOXES: u32 = 5;
/// A smooth tuft's base: its blades root within this many metres of its centre.
pub const TUFT_BASE: f32 = 0.03;

/// `foliage_hash` (PCG output permutation).
pub fn hash(x: u32) -> u32 {
    helio_pass_foliage_place::foliage_hash(x)
}

/// A grid compass step (`compass_step`): the eight neighbours of a cube, axis steps first.
pub fn compass_step(i: u32) -> [f32; 2] {
    [
        [1.0, 0.0],
        [0.0, 1.0],
        [-1.0, 0.0],
        [0.0, -1.0],
        [1.0, 1.0],
        [-1.0, 1.0],
        [-1.0, -1.0],
        [1.0, -1.0],
    ][(i & 7) as usize]
}

/// The sideways step the path has reached after rising to cube `v` (`block_target`).
pub fn block_target(b: &Blade, v: u32, rise: u32, side: u32) -> u32 {
    if rise <= 1 || side == 0 {
        return if rise <= 1 { side } else { 0 };
    }
    let t = v as f32 / (rise - 1) as f32;
    let g = (b.lean * t + b.curve * t * t) / (b.lean + b.curve).max(1.0e-4);
    ((side as f32 * g.clamp(0.0, 1.0)).round() as u32).min(side)
}

/// The level (cube up) a blocky blade takes its `h`-th sideways step at (`block_level`):
/// the first whose target reaches `h`, else the top. Found from the lean curve's inverse
/// and checked against [`block_target`], so it is the walk's level exactly.
pub fn block_level(b: &Blade, h: u32, rise: u32, side: u32) -> u32 {
    if rise <= 1 {
        return 0;
    }
    let top = rise - 1;
    let (lean, curve) = (b.lean, b.curve);
    let q = (h as f32 - 0.5) / side.max(1) as f32 * (lean + curve).max(1.0e-4);
    let t = if curve > 1.0e-6 {
        (-lean + (lean * lean + 4.0 * curve * q).max(0.0).sqrt()) / (2.0 * curve)
    } else if lean > 1.0e-6 {
        q / lean
    } else {
        1.0
    };
    let mut v = ((t * top as f32).ceil().max(0.0) as u32).min(top);
    while v > 0 && block_target(b, v - 1, rise, side) >= h {
        v -= 1;
    }
    while v < top && block_target(b, v, rise, side) < h {
        v += 1;
    }
    v
}

/// [`block_boxes`] from the levels of its sideways steps (`block_path`): a run up a column
/// to each step's level, the steps at it (one box along an axis, a box each diagonally),
/// and the run up to the top. A handful of steps, not a walk cube by cube.
pub fn block_boxes_by_levels(
    b: &Blade,
    j: u32,
    cube: f32,
    scale: f32,
    leafy: bool,
    max_steps: u32,
) -> Vec<BlockBox> {
    let [rise, side_steps] = block_steps(b, cube, scale, max_steps);
    let mut out = Vec::new();
    if rise == 0 {
        return out;
    }
    let side = if j > 0 { side_steps.max(1) } else { side_steps };
    let dir = compass_step(b.compass);
    let diagonal = b.compass >= 4;
    let size = cube * if j >= 8 { 0.86 } else { 1.0 };
    let height = cube * if leafy { 0.6 } else { 1.0 };
    let across = if leafy { b.width.max(size) } else { size };
    let perp = [dir[1].abs() * 0.5 * across, dir[0].abs() * 0.5 * across];
    let column = |h: u32, v0: u32, v1: u32| {
        let half = if diagonal {
            [0.5 * size; 2]
        } else {
            [
                (dir[0].abs() * 0.5 * size).max(perp[0]),
                (dir[1].abs() * 0.5 * size).max(perp[1]),
            ]
        };
        BlockBox {
            centre: [dir[0] * h as f32 * size, dir[1] * h as f32 * size],
            half,
            y0: v0 as f32 * height,
            y1: (v1 + 1) as f32 * height,
        }
    };
    let steps = |h0: u32, h1: u32, v: u32| {
        let extent = [
            dir[0] * (h1 - h0) as f32 * size,
            dir[1] * (h1 - h0) as f32 * size,
        ];
        let first = [dir[0] * h0 as f32 * size, dir[1] * h0 as f32 * size];
        let along = [
            dir[0].abs() * 0.5 * size + extent[0].abs() * 0.5,
            dir[1].abs() * 0.5 * size + extent[1].abs() * 0.5,
        ];
        BlockBox {
            centre: [first[0] + extent[0] * 0.5, first[1] + extent[1] * 0.5],
            half: [along[0].max(perp[0]), along[1].max(perp[1])],
            y0: v as f32 * height,
            y1: (v + 1) as f32 * height,
        }
    };
    let (mut h, mut from) = (0u32, 0u32);
    let mut step = 1;
    while step <= side {
        let level = block_level(b, step, rise, side);
        if h > 0 || j == 0 {
            out.push(column(h, from, level));
        }
        let mut last = step;
        while last < side && block_level(b, last + 1, rise, side) == level {
            last += 1;
        }
        if diagonal {
            for x in step..=last {
                out.push(BlockBox {
                    centre: [dir[0] * x as f32 * size, dir[1] * x as f32 * size],
                    half: [0.5 * size; 2],
                    y0: level as f32 * height,
                    y1: (level + 1) as f32 * height,
                });
            }
        } else {
            out.push(steps(step, last, level));
        }
        h = last;
        from = level + 1;
        step = last + 1;
    }
    if from < rise && (h > 0 || j == 0) {
        out.push(column(h, from, rise - 1));
    }
    out
}

/// A box of a blocky shape: centre across the ground, half extents, bottom and top.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct BlockBox {
    pub centre: [f32; 2],
    pub half: [f32; 2],
    pub y0: f32,
    pub y1: f32,
}

/// The cubes of a blocky blade's staircase, in order, as `(sideways step, rise)`; blades
/// past the tuft's first leave out the shared root column (`block_path`'s walk).
pub fn block_cubes(b: &Blade, j: u32, cube: f32, scale: f32) -> Vec<[u32; 2]> {
    let [rise, side] = block_steps(b, cube, scale, STACK_STEPS);
    if rise == 0 {
        return Vec::new();
    }
    let side = if j > 0 { side.max(1) } else { side };
    let (mut h, mut v) = (0u32, 0u32);
    let mut cubes = vec![[0, 0]];
    loop {
        let way = if h < block_target(b, v, rise, side) {
            1
        } else if v + 1 < rise {
            0
        } else if h < side {
            1
        } else {
            break;
        };
        if way == 0 {
            v += 1;
        } else {
            h += 1;
        }
        cubes.push([h, v]);
    }
    if j > 0 {
        // The shared column (step 0) belongs to the tuft's first blade.
        cubes.retain(|c| c[0] > 0);
    }
    cubes
}

/// The boxes of a blocky blade stepping sideways at most `max_steps` (`block_path`, all of
/// them): runs up merged, axis runs sideways merged, diagonal steps a cube each.
pub fn block_boxes(
    b: &Blade,
    j: u32,
    cube: f32,
    scale: f32,
    leafy: bool,
    max_steps: u32,
) -> Vec<BlockBox> {
    let merge = STACK_CUBES;
    let [rise, side_steps] = block_steps(b, cube, scale, max_steps);
    let mut out = Vec::new();
    if rise == 0 {
        return out;
    }
    let side = if j > 0 { side_steps.max(1) } else { side_steps };
    let dir = compass_step(b.compass);
    let diagonal = b.compass >= 4;
    let size = cube * if j >= 8 { 0.86 } else { 1.0 };
    let height = cube * if leafy { 0.6 } else { 1.0 };
    let across = if leafy { b.width.max(size) } else { size };
    let (mut h, mut v) = (0u32, 0u32);
    let (mut run_h, mut run_v, mut run_len, mut run_way) = (0u32, 0u32, 1u32, 0u32);
    let mut started = j == 0;
    loop {
        let way = if h < block_target(b, v, rise, side) {
            1
        } else if v + 1 < rise {
            0
        } else if h < side {
            1
        } else {
            2
        };
        let last = way == 2;
        let breaks = last
            || way != run_way
            || (way == 0 && run_len >= merge)
            || (way == 1 && diagonal)
            || !started;
        if breaks && started {
            let first = [dir[0] * run_h as f32 * size, dir[1] * run_h as f32 * size];
            let extent = if run_way == 1 && !diagonal {
                [
                    dir[0] * (run_len - 1) as f32 * size,
                    dir[1] * (run_len - 1) as f32 * size,
                ]
            } else {
                [0.0, 0.0]
            };
            let centre = [first[0] + extent[0] * 0.5, first[1] + extent[1] * 0.5];
            let along = [
                dir[0].abs() * 0.5 * size + extent[0].abs() * 0.5,
                dir[1].abs() * 0.5 * size + extent[1].abs() * 0.5,
            ];
            let other = [dir[1].abs() * 0.5 * across, dir[0].abs() * 0.5 * across];
            let half = if diagonal {
                [0.5 * size; 2]
            } else {
                [along[0].max(other[0]), along[1].max(other[1])]
            };
            let up_len = if run_way == 0 { run_len } else { 1 };
            out.push(BlockBox {
                centre,
                half,
                y0: run_v as f32 * height,
                y1: (run_v + up_len) as f32 * height,
            });
        }
        if last {
            break;
        }
        if way == 0 {
            v += 1;
        } else {
            h += 1;
        }
        if breaks {
            run_h = h;
            run_v = v;
            run_len = 1;
            run_way = way;
            started = started || way == 1;
        } else {
            run_len += 1;
        }
    }
    out
}

/// A point of a smooth blade's spine at `s` in its own frame (+z its lean direction, +y
/// up), and its unit tangent (`blade_spine`).
pub fn blade_spine(b: &Blade, height: f32, s: f32) -> ([f32; 3], [f32; 3]) {
    let slope = b.lean + 2.0 * b.curve * s;
    let reach = height * (b.lean * s + b.curve * s * s);
    let lean_here = b.lean + b.curve * s;
    let rise = height * s / (1.0 + lean_here * lean_here).sqrt();
    let len = (1.0 + slope * slope).sqrt();
    ([0.0, rise, reach], [0.0, 1.0 / len, slope / len])
}
