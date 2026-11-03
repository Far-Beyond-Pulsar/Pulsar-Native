//! What one edit costs on the CPU through the game's paths, on Earth at
//! 0.1 m voxels with a sculpted history: a 1 m dig, a 10 m and a 100 m
//! blast (radii). A report, not a gate:
//! `cargo test --release --test edit_cost -- --ignored --nocapture`.
use glam::DVec3;
use helio_voxel_world::{Brush, BrushOp, BrushShape, Planet, PlanetRecipe};
use std::time::Instant;

/// A land direction (surface above sea level) near `face` coordinates.
fn land(planet: &Planet, face: u8, fi: f64, fj: f64) -> DVec3 {
    let grid = planet.grid();
    let n = f64::from(grid.cells());
    for step in 0..400 {
        let a = fi + 0.013 * f64::from(step % 20);
        let b = fj + 0.017 * f64::from(step / 20);
        let (i, j) = ((a.fract() * n) as i32, (b.fract() * n) as i32);
        if planet.column_top(face, i, j, 0) > 40 {
            return grid.direction(face, f64::from(i) + 0.5, f64::from(j) + 0.5);
        }
    }
    panic!("no land found");
}

fn ms(t: Instant) -> f64 {
    t.elapsed().as_secs_f64() * 1e3
}

/// A sculpted Earth: 2000 brushes (1 to 4 m digs and builds) over a 100 m
/// site, and the site's ground.
fn sculpted() -> (Planet, DVec3, DVec3) {
    let mut planet = Planet::new(PlanetRecipe::default()).unwrap();
    let dir = land(&planet, 1, 0.52, 0.48);
    let ground = planet.surface_point(dir, 0.0);
    let up = dir.normalize();
    let side = up.any_orthonormal_vector();
    let fwd = up.cross(side);
    for n in 0..2000u32 {
        let a = f64::from(n.wrapping_mul(2_654_435_761) % 1000) / 10.0 - 50.0;
        let b = f64::from(n.wrapping_mul(40_503) % 1000) / 10.0 - 50.0;
        planet
            .apply(Brush {
                center: (ground + side * a + fwd * b).to_array(),
                radius: 1.0 + f64::from(n % 4),
                shape: BrushShape::Sphere,
                op: if n % 3 == 0 {
                    BrushOp::Add
                } else {
                    BrushOp::Remove
                },
                material: 3,
                height: 0.0,
            })
            .unwrap();
    }
    (planet, ground, up)
}

#[test]
#[ignore]
fn edit_cost_through_the_game_paths() {
    let t = Instant::now();
    let (planet, ground, up) = sculpted();
    eprintln!("EDITCOST history of 2000 brushes built in {:.0} ms", ms(t));
    // A snapshot as a host publishes it, and the first query after.
    let t = Instant::now();
    let copy = planet.clone();
    let clone = ms(t);
    let cell = planet.grid().locate(ground - up * 0.5).0;
    let t = Instant::now();
    for d in 0..64 {
        std::hint::black_box(copy.material(helio_voxel_world::Cell::new(
            cell.face,
            cell.i + d,
            cell.j,
            cell.k,
        )));
    }
    let after_clone = ms(t);
    let t = Instant::now();
    for d in 0..64 {
        std::hint::black_box(copy.material(helio_voxel_world::Cell::new(
            cell.face,
            cell.i + d,
            cell.j,
            cell.k,
        )));
    }
    let warm = ms(t);
    eprintln!("EDITCOST snapshot: clone {clone:.2} ms; 64 material queries after it {after_clone:.2} ms, again {warm:.2} ms");
    for radius in [1.0, 10.0, 100.0] {
        let brush = Brush {
            center: (ground - up * (radius * 0.5)).to_array(),
            radius,
            shape: BrushShape::Sphere,
            op: BrushOp::Remove,
            material: 0,
            height: 0.0,
        };
        // voxel_craft's worker: apply, then a snapshot.
        let mut world = planet.clone();
        let t = Instant::now();
        world.apply(brush).unwrap();
        let apply = ms(t);
        let t = Instant::now();
        std::hint::black_box(world.clone());
        let snapshot = ms(t);
        // Sealed: what baking it costs when its seal comes.
        let t = Instant::now();
        world.seal_all().unwrap();
        let seal = ms(t);
        eprintln!(
            "EDITCOST r {radius} m: apply {apply:.2} ms, snapshot {snapshot:.2} ms, seal {seal:.1} ms"
        );
        // The scripting API's events (per block broken, placed or changed);
        // `EDITCOST_EVENTS_MAX_R` skips larger brushes (the per-cell path
        // takes hours at 100 m).
        let max_r: f64 = std::env::var("EDITCOST_EVENTS_MAX_R")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(f64::INFINITY);
        if radius > max_r {
            continue;
        }
        let t = Instant::now();
        let events = planet.edit_yield(&brush).unwrap();
        eprintln!("EDITCOST r {radius} m: events {:.1} ms ({events})", ms(t));
    }
}
