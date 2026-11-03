//! Structures to find: cottages, watchtowers, ruined gates, wells and desert
//! pyramids, each a [`Blueprint`] built from a seed (so the same site always
//! gets the same building).

pub mod blueprint;

pub use blueprint::Blueprint;

use crate::blocks::Block;
use helio_pass_voxel_planet::flora::Rng;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Cottage,
    Tower,
    Ruins,
    Well,
    Pyramid,
}

impl Kind {
    pub fn build(self, seed: u64) -> Blueprint {
        let mut rng = Rng::new(seed);
        match self {
            Kind::Cottage => cottage(&mut rng),
            Kind::Tower => tower(&mut rng),
            Kind::Ruins => ruins(&mut rng),
            Kind::Well => well(),
            Kind::Pyramid => pyramid(),
        }
    }
}

/// A timber cottage: cobble floor and plinth, log frame, plank walls, a door
/// and window gaps, a stepped pitched roof, a chimney and a glowstone lamp.
fn cottage(rng: &mut Rng) -> Blueprint {
    let mut b = Blueprint::new(5, 4);
    let (hx, hz) = (4, 3);
    // Floor and plinth.
    b.fill(
        [-hx - 1, -1, -hz - 1],
        [hx + 1, -1, hz + 1],
        Block::Cobblestone,
    );
    b.fill([-hx, 0, -hz], [hx, 0, hz], Block::OakPlanks);
    // Walls: planks with a log frame.
    b.walls([-hx, 1, -hz], [hx, 4, hz], Block::OakPlanks);
    for (x, z) in [(-hx, -hz), (hx, -hz), (-hx, hz), (hx, hz)] {
        b.fill([x, 1, z], [x, 4, z], Block::OakLog);
    }
    b.fill([-hx, 1, 0], [-hx, 1, 0], Block::Cobblestone);
    // A cobble course along the base of the walls.
    for x in -hx..=hx {
        b.set([x, 1, -hz], Block::Cobblestone);
        b.set([x, 1, hz], Block::Cobblestone);
    }
    for z in -hz..=hz {
        b.set([-hx, 1, z], Block::Cobblestone);
        b.set([hx, 1, z], Block::Cobblestone);
    }
    for (x, z) in [(-hx, -hz), (hx, -hz), (-hx, hz), (hx, hz)] {
        b.set([x, 1, z], Block::OakLog);
    }
    // Door (front, -z) and windows.
    b.hollow([0, 1, -hz], [0, 2, -hz]);
    for x in [-2, 2] {
        b.hollow([x, 2, -hz], [x, 3, -hz]);
        b.hollow([x, 2, hz], [x, 3, hz]);
    }
    for z in [-1, 1] {
        b.hollow([-hx, 2, z], [-hx, 3, z]);
        b.hollow([hx, 2, z], [hx, 3, z]);
    }
    // Roof: a pitched roof of dark planks, ridge along x, with an overhang.
    for layer in 0..=hz + 1 {
        let z = hz + 1 - layer;
        let y = 5 + layer;
        for x in -hx - 1..=hx + 1 {
            b.set([x, y, z], Block::DarkOakPlanks);
            b.set([x, y, -z], Block::DarkOakPlanks);
        }
        if z == 0 {
            break;
        }
    }
    // Gable ends fill under the roof with planks.
    for layer in 0..=hz {
        let y = 5 + layer;
        let z = hz - layer;
        for x in [-hx, hx] {
            for zz in -z..=z {
                b.set([x, y, zz], Block::OakPlanks);
            }
        }
    }
    // Chimney on a side wall.
    let side = if rng.f64() < 0.5 { -1 } else { 1 };
    b.fill(
        [side * (hx + 1), 0, 1],
        [side * (hx + 1), 9, 2],
        Block::Cobblestone,
    );
    // Lamp and a hearth inside.
    b.set([0, 4, 0], Block::Glowstone);
    b.set([side * (hx - 1), 1, 1], Block::Cobblestone);
    b
}

/// A square stone watchtower with a spiral of steps up its inside wall and a
/// crenellated platform.
fn tower(rng: &mut Rng) -> Blueprint {
    let mut b = Blueprint::new(4, 4);
    let h = 3; // wall at |x| = |z| = 3, interior 5 x 5
    let top = 17;
    b.fill([-h - 1, -1, -h - 1], [h + 1, -1, h + 1], Block::Cobblestone);
    b.fill([-h, 0, -h], [h, 0, h], Block::StoneBricks);
    // Walls: bricks with a cobble band every fifth course.
    b.walls([-h, 1, -h], [h, top, h], Block::StoneBricks);
    for y in (5..=top).step_by(6) {
        b.walls([-h, y, -h], [h, y, h], Block::Cobblestone);
    }
    // Door and arrow slits.
    b.hollow([0, 1, -h], [0, 2, -h]);
    for y in [6, 11, 15] {
        for (x, z) in [(0, -h), (0, h), (-h, 0), (h, 0)] {
            b.clear([x, y, z]);
        }
    }
    // The stairs: one block up per step around the inside of the wall.
    let mut ring = Vec::new();
    for x in -2..=2 {
        ring.push([x, -2]);
    }
    for z in -1..=2 {
        ring.push([2, z]);
    }
    for x in (-2..=1).rev() {
        ring.push([x, 2]);
    }
    for z in (-1..=1).rev() {
        ring.push([-2, z]);
    }
    for (n, [x, z]) in ring.into_iter().enumerate() {
        let y = n as i32 + 1;
        b.fill([x, 1, z], [x, y, z], Block::Cobblestone);
    }
    // Platform, parapet and merlons.
    b.fill([-h, top, -h], [h, top, h], Block::OakPlanks);
    b.walls([-h, top + 1, -h], [h, top + 1, h], Block::StoneBricks);
    for x in -h..=h {
        for z in -h..=h {
            let edge = x.abs() == h || z.abs() == h;
            if edge && (x + z).rem_euclid(2) == 0 {
                b.set([x, top + 2, z], Block::StoneBricks);
            }
        }
    }
    // Beacons on the corners.
    for (x, z) in [(-h, -h), (h, -h), (-h, h), (h, h)] {
        b.set([x, top + 3, z], Block::Glowstone);
    }
    // A weathered tower: a few bricks lost to age (rolled in a fixed
    // order: a site's tower is the same whenever it is built).
    let upper: Vec<[i32; 3]> = b.solid_blocks().into_iter().filter(|p| p[1] > 8).collect();
    let lost: Vec<[i32; 3]> = upper.into_iter().filter(|_| rng.f64() < 0.03).collect();
    for p in lost {
        b.clear(p);
    }
    b
}

/// A ruined gate: two pillars and the stub of an arch with fallen stones.
fn ruins(rng: &mut Rng) -> Blueprint {
    let mut b = Blueprint::new(5, 3);
    for sign in [-1, 1] {
        let x = 3 * sign;
        let height = 5 + rng.int(0, 3);
        for y in 0..height {
            b.set(
                [x, y, 0],
                if rng.f64() < 0.3 {
                    Block::Cobblestone
                } else {
                    Block::StoneBricks
                },
            );
            b.set([x, y, 1], Block::Cobblestone);
        }
        b.set([x, 0, -1], Block::StoneBricks);
    }
    // The arch: steps leaning in from both pillars, broken at the crown.
    for (n, y) in (6..=8).enumerate() {
        let n = n as i32;
        for sign in [-1, 1] {
            if rng.f64() < 0.82 {
                b.set([sign * (2 - n), y, 0], Block::StoneBricks);
            }
        }
    }
    // A low wall each side, and rubble.
    for x in [-5, -4, 4, 5] {
        let h = rng.int(0, 2);
        for y in 0..=h {
            b.set([x, y, 0], Block::Cobblestone);
        }
    }
    for _ in 0..14 {
        let (x, z) = (rng.int(-5, 5), rng.int(-2, 3));
        b.set(
            [x, 0, z],
            *rng.choice(&[
                Block::Cobblestone,
                Block::Gravel,
                Block::StoneBricks,
                Block::Andesite,
            ]),
        );
    }
    b
}

/// A cobble well with a plank roof on log posts and a shaft below.
fn well() -> Blueprint {
    let mut b = Blueprint::new(3, 3);
    b.fill([-2, -1, -2], [2, -1, 2], Block::Cobblestone);
    b.walls([-2, 0, -2], [2, 1, 2], Block::Cobblestone);
    b.hollow([-1, 0, -1], [1, 1, 1]);
    // The shaft.
    b.hollow([-1, -8, -1], [1, -1, 1]);
    b.fill([-1, -9, -1], [1, -9, 1], Block::Cobblestone);
    for (x, z) in [(-2, -2), (2, -2), (-2, 2), (2, 2)] {
        b.fill([x, 2, z], [x, 4, z], Block::OakLog);
    }
    b.fill([-2, 5, -2], [2, 5, 2], Block::DarkOakPlanks);
    b.fill([-1, 6, -1], [1, 6, 1], Block::DarkOakPlanks);
    b.set([0, 3, 0], Block::Glowstone);
    b
}

/// A stepped sandstone pyramid with a hall and a glowstone treasure.
fn pyramid() -> Blueprint {
    let mut b = Blueprint::new(8, 8);
    let n = 7;
    for layer in 0..=n {
        let r = n - layer;
        b.fill(
            [-r - 1, layer, -r - 1],
            [r + 1, layer, r + 1],
            if layer % 3 == 2 {
                Block::Sandstone
            } else {
                Block::Sand
            },
        );
        b.walls(
            [-r - 1, layer, -r - 1],
            [r + 1, layer, r + 1],
            Block::Sandstone,
        );
    }
    // Entrance, hall and chamber.
    b.hollow([0, 1, -8], [0, 2, -4]);
    b.hollow([-2, 1, -3], [2, 4, 2]);
    b.fill([-2, 0, -3], [2, 0, 2], Block::Sandstone);
    b.set([0, 1, 0], Block::Glowstone);
    for x in [-2, 2] {
        b.fill([x, 1, 2], [x, 3, 2], Block::StoneBricks);
    }
    b
}
