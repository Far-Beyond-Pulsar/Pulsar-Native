//! What CPU world queries cost on Earth with its features (a measurement,
//! run with `--ignored --nocapture`).

use glam::DVec3;
use helio_voxel_world::feature_recipe::FeatureRecipe;
use helio_voxel_world::layers::TerrainLayers;
use helio_voxel_world::{Brush, BrushOp, BrushShape, Cell, Matter, Planet, PlanetRecipe};
use std::time::Instant;

fn earth() -> Planet {
    let voxel = 0.1;
    Planet::new(PlanetRecipe {
        voxel_size_m: voxel,
        terrain: TerrainLayers::earth().source(7),
        features: FeatureRecipe::earth().grow(voxel, 7),
        ..Default::default()
    })
    .unwrap()
}

/// A surface point (1.7 m up) under the most feature cells around a few
/// directions: a forest.
fn forest(planet: &Planet) -> DVec3 {
    let mut best = (0, DVec3::ZERO);
    for t in 0..64 {
        let a = t as f64 * 0.37;
        let dir = DVec3::new(a.cos(), 0.8 + 0.1 * (a * 1.3).sin(), a.sin()).normalize();
        let eye = planet.surface_point_of(dir * planet.outer_radius(), 1.7, Matter::Ground);
        let all = planet.ground_height_of(eye, Matter::All);
        let ground = planet.ground_height_of(eye, Matter::Ground);
        let crowns = (ground - all).max(0.0);
        let score = (crowns * 10.0) as i32;
        if score > best.0 {
            best = (score, eye);
        }
    }
    best.1
}

fn time<T>(what: &str, n: usize, mut f: impl FnMut(usize) -> T) {
    let started = Instant::now();
    for i in 0..n {
        std::hint::black_box(f(i));
    }
    let ms = started.elapsed().as_secs_f64() * 1e3;
    eprintln!(
        "QUERY_COST {what}: {:.4} ms each ({n} in {ms:.1} ms)",
        ms / n as f64
    );
}

#[test]
#[ignore]
fn query_costs_on_earth() {
    let started = Instant::now();
    let planet = earth();
    eprintln!(
        "QUERY_COST build {:.1} ms",
        started.elapsed().as_secs_f64() * 1e3
    );
    let eye = forest(&planet);
    let up = planet.grid().up(eye);
    let (cell, _) = planet.grid().locate(eye);
    eprintln!(
        "QUERY_COST eye {eye:?} ground {:.2} m, all {:.2} m",
        planet.ground_height_of(eye, Matter::Ground),
        planet.ground_height_of(eye, Matter::All)
    );
    // A player capsule's cells (0.6 x 1.8 x 0.6 m at 0.1 m voxels), a frame.
    let capsule = |offset: i32| {
        let mut solid = 0;
        for i in -3..3 {
            for j in -3..3 {
                for k in -17..2 {
                    solid += planet.solid_of(
                        Cell::new(cell.face, cell.i + i + offset, cell.j + j, cell.k + k),
                        Matter::All,
                    ) as u32;
                }
            }
        }
        solid
    };
    time("capsule cells, All (684 cells)", 200, |i| {
        capsule(i as i32 % 50)
    });
    time("solid_of one cell, All", 20_000, |i| {
        planet.solid_of(
            Cell::new(cell.face, cell.i + (i % 200) as i32, cell.j, cell.k - 10),
            Matter::All,
        )
    });
    time("solid_of one cell, Ground", 20_000, |i| {
        planet.solid_of(
            Cell::new(cell.face, cell.i + (i % 200) as i32, cell.j, cell.k - 10),
            Matter::Ground,
        )
    });
    let forward = up.any_orthonormal_vector();
    time("raycast 100 m level through the forest, All", 200, |i| {
        let d = (forward * (i as f64 * 0.05).cos() + up.cross(forward) * (i as f64 * 0.05).sin())
            .normalize();
        planet.raycast(eye, d, 100.0)
    });
    time("raycast 30 m down-forward, All", 200, |i| {
        let d = (forward * (i as f64 * 0.05).cos() - up).normalize();
        planet.raycast(eye, d, 30.0)
    });
    time("ground_height_of, All", 200, |i| {
        planet.ground_height_of(eye + forward * i as f64, Matter::All)
    });
    time("ground_height_of, Ground", 200, |i| {
        planet.ground_height_of(eye + forward * i as f64, Matter::Ground)
    });
    time("air_clearance", 200, |i| {
        planet.air_clearance(eye + forward * i as f64)
    });
    time("surface_point_of, Ground", 200, |i| {
        planet.surface_point_of(eye + forward * i as f64, 0.5, Matter::Ground)
    });
    let high = eye + up * 3000.0;
    time("ground_height_of at 3 km, All", 50, |i| {
        planet.ground_height_of(high + forward * i as f64 * 10.0, Matter::All)
    });
}

/// A brick of cells, sampled at once, is each of its cells exactly: in a
/// forest (trunks, crowns and far cover) and through edits (a dug hole and
/// an added block).
#[test]
fn bricks_are_their_cells() {
    let mut planet = earth();
    let eye = forest(&planet);
    let up = planet.grid().up(eye);
    let side = up.any_orthonormal_vector();
    planet
        .apply(Brush {
            center: (eye - up * 2.0).to_array(),
            radius: 1.2,
            shape: BrushShape::Sphere,
            op: BrushOp::Remove,
            material: 0,
            height: 0.0,
        })
        .unwrap();
    planet
        .apply(Brush {
            center: (eye + side * 1.5).to_array(),
            radius: 0.4,
            shape: BrushShape::Cube,
            op: BrushOp::Add,
            material: 7,
            height: 0.0,
        })
        .unwrap();
    let (cell, _) = planet.grid().locate(eye);
    const N: i32 = 16;
    let (mut solid, mut features) = (0, 0);
    for (di, dj, dk) in [
        (0, 0, -1),
        (-1, 0, -1),
        (0, -1, 0),
        (0, 0, 0),
        (1, 1, -2),
        (0, 0, 1),
    ] {
        let origin = [
            (cell.i.div_euclid(N) + di) * N,
            (cell.j.div_euclid(N) + dj) * N,
            (cell.k.div_euclid(N) + dk) * N,
        ];
        let brick = planet.sample_brick(cell.face, origin, N, true);
        let bare = planet.sample_brick(cell.face, origin, N, false);
        for dj in 0..N {
            for di in 0..N {
                for dk in 0..N {
                    let index = ((dj * N + di) * N + dk) as usize;
                    let c = Cell::new(cell.face, origin[0] + di, origin[1] + dj, origin[2] + dk);
                    let cell = brick[index];
                    assert_eq!(
                        (cell.kind, cell.material),
                        planet.sample_kind(0, c.face, c.i, c.j, c.k),
                        "{c:?}"
                    );
                    assert_eq!(
                        cell.ground,
                        planet.solid_of(c, Matter::Ground),
                        "{c:?} ground"
                    );
                    assert_eq!(bare[index].kind == 1, cell.ground, "{c:?} without features");
                    solid += cell.kind;
                    features += u32::from((cell.kind == 1) != cell.ground);
                }
            }
        }
    }
    eprintln!("BRICKS {solid} solid cells, {features} of them features");
    assert!(features > 100, "the bricks hold features: {features}");
}

#[test]
#[ignore]
fn brick_cost_on_earth() {
    let planet = earth();
    let eye = forest(&planet);
    let (cell, _) = planet.grid().locate(eye);
    for (n, with_features) in [(16, true), (16, false), (32, true)] {
        time(
            &format!("sample_brick {n}^3 features {with_features}"),
            20,
            |t| {
                let origin = [
                    (cell.i.div_euclid(n) + t as i32 % 5) * n,
                    cell.j.div_euclid(n) * n,
                    (cell.k.div_euclid(n) - 1) * n,
                ];
                planet.sample_brick(cell.face, origin, n, with_features)
            },
        );
    }
}
