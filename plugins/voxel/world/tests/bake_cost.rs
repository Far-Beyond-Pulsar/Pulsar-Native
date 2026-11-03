//! What baking a brush into the CPU edit store costs as the history grows:
//! harness-like sculpt strokes (digs 4 m and 6 m in radius, too large to
//! bake into base cells, and builds 3 m in radius, baked) on Earth at 0.1 m
//! voxels. A report, not a gate:
//! `cargo test --release --test bake_cost -- --ignored --nocapture`.
use glam::DVec3;
use helio_voxel_world::edit_store::EditStore;
use helio_voxel_world::planet::{bake_level, BAKE_RADIUS_HALF};
use helio_voxel_world::{Brush, BrushOp, BrushShape, Planet, PlanetRecipe};
use std::hash::{Hash, Hasher};
use std::time::Instant;

fn xorshift(mut state: u64) -> impl FnMut() -> u64 {
    move || {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        state
    }
}

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

/// Strokes: runs of brushes a third of their radius apart, on a random walk
/// over a 60 m site, cycling 4 m digs, 6 m digs and 3 m builds.
struct Strokes {
    ground: DVec3,
    frame: [DVec3; 3],
    at: [f64; 2],
    heading: f64,
    count: usize,
    next: Box<dyn FnMut() -> u64>,
}

impl Strokes {
    fn new(planet: &Planet) -> Self {
        let dir = land(planet, 1, 0.52, 0.48);
        let ground = planet.surface_point(dir, 0.0);
        let up = dir.normalize();
        let side = up.any_orthonormal_vector();
        Self {
            ground,
            frame: [side, up.cross(side), up],
            at: [0.0; 2],
            heading: 0.0,
            count: 0,
            next: Box::new(xorshift(0x9e37_79b9_7f4a_7c15)),
        }
    }
    fn unit(&mut self) -> f64 {
        ((self.next)() % 10_001) as f64 / 10_000.0
    }
    fn brush(&mut self) -> Brush {
        let stroke = self.count / 12;
        self.count += 1;
        let (radius, op, lift) = match stroke % 3 {
            0 => (4.0, BrushOp::Remove, 0.0),
            1 => (6.0, BrushOp::Remove, 0.0),
            _ => (3.0, BrushOp::Add, 2.0),
        };
        if self.count % 12 == 1 {
            self.at = [self.unit() * 60.0 - 30.0, self.unit() * 60.0 - 30.0];
            self.heading = self.unit() * std::f64::consts::TAU;
        }
        self.heading += self.unit() * 0.4 - 0.2;
        let step = radius / 3.0;
        self.at = [
            (self.at[0] + self.heading.cos() * step).clamp(-30.0, 30.0),
            (self.at[1] + self.heading.sin() * step).clamp(-30.0, 30.0),
        ];
        let [side, fwd, up] = self.frame;
        let centre = self.ground + side * self.at[0] + fwd * self.at[1] + up * lift;
        Brush {
            center: centre.to_array(),
            radius,
            shape: BrushShape::Sphere,
            op,
            material: 3,
            height: 0.0,
        }
    }
}

/// Every stored brick, hashed in key order.
fn fingerprint(store: &EditStore) -> u64 {
    let mut bricks: Vec<_> = store.bricks().collect();
    bricks.sort_unstable_by_key(|(key, _)| *key);
    let mut h = std::collections::hash_map::DefaultHasher::new();
    for (key, brick) in bricks {
        key.hash(&mut h);
        brick.cells.hash(&mut h);
    }
    h.finish()
}

#[test]
#[ignore]
fn bake_cost_against_history() {
    let mut planet = Planet::new(PlanetRecipe::default()).unwrap();
    let grid = *planet.grid();
    let mut strokes = Strokes::new(&planet);
    for history in [0usize, 192, 768, 3072] {
        // Grow the history, timing the applies that sealed.
        let mut seals = Vec::new();
        while planet.edits().len() < history {
            let sealed = planet.edits().sealed_len();
            let t = Instant::now();
            planet.apply(strokes.brush()).unwrap();
            if planet.edits().sealed_len() != sealed {
                seals.push(t.elapsed().as_secs_f64() * 1e3);
            }
        }
        if !seals.is_empty() {
            let mean = seals.iter().sum::<f64>() / seals.len() as f64;
            eprintln!("BAKECOST history {history}: {} seals, mean {mean:.1} ms ({:.1} ms per brush), max {:.1} ms", seals.len(), mean / 16.0, seals.iter().copied().fold(0.0, f64::max));
        }
        // Each kind of brush baked into a copy of the store as it stands.
        for (label, kind) in [("4 m dig", 0usize), ("6 m dig", 1), ("3 m build", 2)] {
            let mut ms = Vec::new();
            let mut probe = Strokes::new(&planet);
            probe.count = kind * 12;
            for _ in 0..6 {
                let brush = probe.brush();
                let faces = brush.resolve(&grid).unwrap();
                let mut store = planet.edits().baked.clone();
                let t = Instant::now();
                for fb in &faces {
                    if faces.iter().all(|fb| fb.radius_half <= BAKE_RADIUS_HALF) {
                        store.bake(&grid, fb, brush.op);
                    } else {
                        store.apply_over(&grid, fb, brush.op);
                        store.bake_at(&grid, fb, brush.op, bake_level(fb));
                    }
                }
                ms.push(t.elapsed().as_secs_f64() * 1e3);
            }
            ms.sort_by(f64::total_cmp);
            eprintln!(
                "BAKECOST history {history}, {label}: bake p50 {:.2} ms, max {:.2} ms",
                ms[3], ms[5]
            );
        }
        eprintln!(
            "BAKECOST history {history}: store fingerprint {:016x}, bricks per level {:?}",
            fingerprint(&planet.edits().baked),
            planet.edits().baked.brick_counts()
        );
    }
}
