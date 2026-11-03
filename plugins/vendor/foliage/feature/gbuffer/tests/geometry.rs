//! Properties of the tuft geometry, against its CPU mirror (`src/geometry.rs`).

use helio_pass_foliage_gbuffer::*;
use helio_pass_foliage_place::{FoliageKind, GpuFoliageType};
use std::collections::HashMap;

fn block_type(lean: [f32; 2], curve: f32, fall: f32, width: [f32; 2]) -> GpuFoliageType {
    let mut t = GpuFoliageType {
        height_range: [0.1, 0.6],
        width_range: width,
        lean_range: lean,
        curve,
        fall_chance: fall,
        kind_and_flags: helio_pass_foliage_place::pack_kind_and_flags(FoliageKind::Block, 0),
        ..GpuFoliageType::default()
    };
    t.set_blades_per_tuft([4, 15]);
    t
}

/// Every blade of many tufts of `ty`.
fn tufts(ty: &GpuFoliageType, tufts: u32) -> Vec<Vec<(u32, Blade)>> {
    (0..tufts)
        .map(|t| {
            let seed = hash(t * 7919 + 13) & 0xffff;
            let n = 4 + t % 12;
            let turn = (hash(t) & 0xff) as f32 / 256.0;
            let height = (hash(t ^ 0x55) & 0xff) as f32 / 255.0;
            (0..n)
                .map(|j| (j, tuft_blade(ty, seed, turn, height, j, n)))
                .collect()
        })
        .collect()
}

#[test]
fn blocky_blades_are_connected_staircases_off_their_tufts_root_column() {
    let ty = block_type([0.2, 0.9], 0.5, 0.15, [0.05, 0.05]);
    for tuft in tufts(&ty, 300) {
        for (j, b) in tuft {
            let cubes = block_cubes(&b, j, 0.05, 1.0);
            for pair in cubes.windows(2) {
                let (a, c) = (pair[0], pair[1]);
                // One step up or one step out from the last cube.
                assert!(
                    (c[0] == a[0] && c[1] == a[1] + 1) || (c[0] == a[0] + 1 && c[1] == a[1]),
                    "a gap between {a:?} and {c:?}"
                );
            }
            if j > 0 {
                assert!(
                    cubes.iter().all(|c| c[0] > 0),
                    "blade {j} in the root column"
                );
                assert!(
                    !cubes.is_empty() || block_steps(&b, 0.05, 1.0, STACK_STEPS)[0] == 0,
                    "blade {j} hidden"
                );
            } else if let Some(first) = cubes.first() {
                assert_eq!(*first, [0, 0]);
            }
        }
    }
}

#[test]
fn a_tufts_blades_never_draw_one_cube_twice() {
    // Two blades drawing the same cube at the same size fight for depth: the tuft's
    // first eight blades point eight ways from the shared column, and later ones nest
    // inside them a size smaller.
    let ty = block_type([0.3, 1.0], 0.4, 0.0, [0.05, 0.05]);
    for tuft in tufts(&ty, 300) {
        let mut owner: HashMap<[i32; 3], u32> = HashMap::new();
        for (j, b) in tuft {
            if j >= 8 {
                continue;
            }
            let dir = compass_step(b.compass);
            for c in block_cubes(&b, j, 0.05, 1.0) {
                let at = [
                    (dir[0] * c[0] as f32) as i32,
                    (dir[1] * c[0] as f32) as i32,
                    c[1] as i32,
                ];
                if let Some(other) = owner.insert(at, j) {
                    panic!("blades {other} and {j} both draw cube {at:?}");
                }
            }
        }
    }
}

#[test]
fn a_blocky_blades_boxes_fit_its_strip() {
    let ty = block_type([0.2, 0.9], 0.5, 0.15, [0.05, 0.05]);
    let mut over = 0;
    let mut total = 0;
    for tuft in tufts(&ty, 300) {
        for (j, b) in tuft {
            total += 1;
            if block_boxes(&b, j, 0.05, 1.0, false, STACK_STEPS).len() as u32 > L0_BOXES {
                over += 1;
            }
            assert!(block_boxes(&b, j, 0.05, 1.0, false, L1_STEPS).len() as u32 <= L1_BOXES);
        }
    }
    // A blade with more boxes than its strip loses its tip: rare enough not to see.
    assert!(
        over * 100 <= total,
        "{over} of {total} blades overflow L0's strip"
    );
}

#[test]
fn boxes_cover_their_blades_cubes() {
    let ty = block_type([0.3, 1.0], 0.4, 0.1, [0.05, 0.05]);
    for tuft in tufts(&ty, 100) {
        for (j, b) in tuft {
            let cubes = block_cubes(&b, j, 0.05, 1.0);
            let boxes = block_boxes(&b, j, 0.05, 1.0, false, STACK_STEPS);
            let dir = compass_step(b.compass);
            let size = if j >= 8 { 0.05 * 0.86 } else { 0.05 };
            for c in &cubes {
                let centre = [
                    dir[0] * c[0] as f32 * size,
                    dir[1] * c[0] as f32 * size,
                    (c[1] as f32 + 0.5) * 0.05,
                ];
                let inside = boxes.iter().any(|bx| {
                    (centre[0] - bx.centre[0]).abs() <= bx.half[0] + 1e-5
                        && (centre[1] - bx.centre[1]).abs() <= bx.half[1] + 1e-5
                        && centre[2] >= bx.y0 - 1e-5
                        && centre[2] <= bx.y1 + 1e-5
                });
                assert!(inside, "cube {c:?} of blade {j} in no box");
            }
        }
    }
}

#[test]
fn smooth_blades_keep_their_length_as_they_lean() {
    let mut ty = GpuFoliageType {
        lean_range: [0.2, 1.2],
        curve: 0.5,
        ..GpuFoliageType::default()
    };
    ty.set_blades_per_tuft([5, 12]);
    for tuft in tufts(&ty, 50) {
        for (_, b) in tuft {
            let (root, _) = blade_spine(&b, b.height, 0.0);
            assert_eq!(root, [0.0; 3]);
            // The spine's length, walked in short steps.
            let mut length = 0.0;
            let mut last = root;
            for i in 1..=64 {
                let (at, tangent) = blade_spine(&b, b.height, i as f32 / 64.0);
                length += ((at[1] - last[1]).powi(2) + (at[2] - last[2]).powi(2)).sqrt();
                last = at;
                assert!((tangent[1].powi(2) + tangent[2].powi(2) - 1.0).abs() < 1e-4);
            }
            let reach = b.lean + b.curve;
            // Between the upright length and the leaning path's: never a stretched blade
            // or one collapsed to nothing.
            assert!(
                length > 0.6 * b.height && length < b.height * (1.0 + reach),
                "{length} for {} m leaning {reach}",
                b.height
            );
        }
    }
}

#[test]
fn a_tufts_blades_spread_around_it() {
    let mut ty = GpuFoliageType {
        lean_range: [0.2, 1.2],
        ..GpuFoliageType::default()
    };
    ty.set_blades_per_tuft([8, 8]);
    for tuft in tufts(&ty, 50) {
        let n = tuft.len() as f32;
        let (mut x, mut z) = (0.0f32, 0.0f32);
        for (_, b) in &tuft {
            x += b.angle.cos();
            z += b.angle.sin();
        }
        // Fanned around the tuft, not all one way.
        assert!((x * x + z * z).sqrt() < 0.5 * n, "a lopsided tuft");
        assert_eq!(tuft[0].1.rim, 0.0);
        assert!(tuft[1..].iter().all(|(_, b)| b.rim >= 0.35));
    }
}

#[test]
fn boxes_by_levels_are_the_walks_boxes() {
    // The draw finds a blade's boxes from its sideways steps' levels; they must be the
    // cube-by-cube walk's, box for box.
    let mut checked = 0;
    for (lean, curve, fall, width) in [
        ([0.0, 0.3], 0.15, 0.05, [0.05, 0.05]),
        ([0.2, 1.2], 0.6, 0.15, [0.05, 0.05]),
        ([0.9, 1.6], 0.3, 0.0, [0.05, 0.15]),
        ([0.0, 0.0], 0.0, 0.0, [0.05, 0.05]),
    ] {
        let ty = block_type(lean, curve, fall, width);
        let leafy = width[1] > 1.5 * width[0];
        for tuft in tufts(&ty, 300) {
            for (j, b) in tuft {
                for scale in [0.6, 1.0, 1.7] {
                    for steps in [STACK_STEPS, L1_STEPS] {
                        let walk = block_boxes(&b, j, 0.05, scale, leafy, steps);
                        let levels = block_boxes_by_levels(&b, j, 0.05, scale, leafy, steps);
                        assert_eq!(
                            walk.len(),
                            levels.len(),
                            "blade {j} {b:?} scale {scale} steps {steps}"
                        );
                        for (w, l) in walk.iter().zip(&levels) {
                            let close = |a: f32, b: f32| (a - b).abs() <= 1.0e-5;
                            assert!(
                                close(w.centre[0], l.centre[0])
                                    && close(w.centre[1], l.centre[1])
                                    && close(w.half[0], l.half[0])
                                    && close(w.half[1], l.half[1])
                                    && close(w.y0, l.y0)
                                    && close(w.y1, l.y1),
                                "blade {j}: {w:?} vs {l:?}"
                            );
                        }
                        checked += 1;
                    }
                }
            }
        }
    }
    assert!(checked > 10_000);
}
