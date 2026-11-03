//! The player's body against the voxels: a box that slides along walls, lands
//! on floors, steps up ledges to `STEP_M` and is pulled to the planet's
//! centre.
//!
//! The world is a cube sphere, so "down" is the direction to the centre and
//! the box's axes are the local up, forward and right. Collision samples
//! points over the box and asks which cells they are in.

use crate::config::{PLAYER_HEIGHT, PLAYER_WIDTH, STEP_M, VOXEL_M};
use crate::world::World;
use glam::DVec3;

/// The local frame at a point: up (away from the centre), and the tangent
/// axes.
#[derive(Clone, Copy, Debug)]
pub struct Frame {
    pub up: DVec3,
    pub forward: DVec3,
    pub right: DVec3,
}

impl Frame {
    /// The frame at `position` facing `heading` (projected onto the tangent
    /// plane).
    pub fn new(position: DVec3, heading: DVec3) -> Self {
        let up = position.normalize_or(DVec3::Y);
        let forward = (heading - up * heading.dot(up))
            .try_normalize()
            .unwrap_or_else(|| up.any_orthonormal_vector());
        Frame {
            up,
            forward,
            right: forward.cross(up),
        }
    }
}

/// Offsets of the sample points across the box (right, forward): its
/// centre, corners and edges' middles.
fn across() -> impl Iterator<Item = (f64, f64)> + Clone {
    let r = PLAYER_WIDTH * 0.5 - 0.02;
    [
        (0.0, 0.0),
        (r, r),
        (-r, r),
        (r, -r),
        (-r, -r),
        (r, 0.0),
        (-r, 0.0),
        (0.0, r),
        (0.0, -r),
    ]
    .into_iter()
}

/// Offsets of the sample points: across the box at five heights.
fn samples() -> impl Iterator<Item = (f64, f64, f64)> {
    let heights = [0.002, 0.55, 1.1, 1.65, PLAYER_HEIGHT - 0.04];
    heights
        .into_iter()
        .flat_map(|h| across().map(move |(x, z)| (h, x, z)))
}

/// Whether the box standing at `feet` (in `frame`) overlaps solid blocks.
pub fn collides(world: &World, feet: DVec3, frame: &Frame) -> bool {
    samples().any(|(h, x, z)| {
        world.is_solid(world.cell_at(feet + frame.up * h + frame.right * x + frame.forward * z))
    })
}

/// Whether the cell is inside the box standing at `feet`.
pub fn occupies(
    world: &World,
    feet: DVec3,
    frame: &Frame,
    cell: helio_pass_voxel_planet::Cell,
) -> bool {
    samples().any(|(h, x, z)| {
        world.cell_at(feet + frame.up * h + frame.right * x + frame.forward * z) == cell
    })
}

/// Movements shorter than this (metres) are not made.
const MIN_MOVE: f64 = 1e-9;

/// What a movement did.
#[derive(Clone, Copy, Debug, Default)]
pub struct Moved {
    pub landed: bool,
    pub hit_ceiling: bool,
    pub stepped: bool,
}

/// Moves the box from `feet` by `delta`, resolving vertical movement first and
/// then each horizontal axis, with step-up. Returns the new position.
pub fn slide(
    world: &World,
    mut feet: DVec3,
    delta: DVec3,
    frame: &Frame,
    can_step: bool,
) -> (DVec3, Moved) {
    let mut moved = Moved::default();
    let vertical = delta.dot(frame.up);
    let horizontal = [
        frame.right * delta.dot(frame.right),
        frame.forward * delta.dot(frame.forward),
    ];

    // Vertical, in steps shorter than a block. A movement under a
    // nanometre is a velocity's rounding residue (the grip's decay never
    // reaches zero), on every axis: applied, it moved a standing player by
    // a rounding step every frame, and a still view never settled.
    if vertical.abs() >= MIN_MOVE {
        let steps = (vertical.abs() / 0.2).ceil().max(1.0) as i32;
        for _ in 0..steps {
            let next = feet + frame.up * (vertical / f64::from(steps));
            if collides(world, next, frame) {
                if vertical < 0.0 {
                    moved.landed = true;
                    feet = rest_on_floor(world, feet, frame);
                } else {
                    moved.hit_ceiling = true;
                }
                break;
            }
            feet = next;
        }
    }

    // Horizontal axes.
    for part in horizontal {
        let length = part.length();
        if length < MIN_MOVE {
            continue;
        }
        let steps = (length / 0.2).ceil().max(1.0) as i32;
        for _ in 0..steps {
            let next = feet + part / f64::from(steps);
            if !collides(world, next, frame) {
                feet = next;
                continue;
            }
            // A ledge in the way: climb the lowest lift that clears it, up to
            // `STEP_M`, if there is room over it.
            if can_step {
                let lift = (1..=(STEP_M / VOXEL_M).round() as i32)
                    .map(|n| f64::from(n) * VOXEL_M + 0.02)
                    .find(|&h| {
                        !collides(world, next + frame.up * h, frame)
                            && !collides(world, feet + frame.up * h, frame)
                    });
                if let Some(h) = lift {
                    feet = rest_on_floor(world, next + frame.up * h, frame);
                    moved.stepped = true;
                    continue;
                }
            }
            break;
        }
    }
    (feet, moved)
}

/// `feet` lowered to rest on the floor under its sole: the top of the
/// highest solid cell under any of the sole's points within a block (the box
/// stands on whatever part of it is over, as `collides` holds it); otherwise
/// unchanged.
pub fn rest_on_floor(world: &World, feet: DVec3, frame: &Frame) -> DVec3 {
    let grid = world.planet().grid();
    let mut top: Option<f64> = None;
    for (x, z) in across() {
        let mut down = feet + frame.right * x + frame.forward * z;
        for _ in 0..24 {
            let next = down - frame.up * 0.05;
            let cell = world.cell_at(next);
            if world.is_solid(cell) {
                let radial = grid.layer_radius(f64::from(cell.k) + 1.0);
                top = Some(top.map_or(radial, |t: f64| t.max(radial)));
                break;
            }
            down = next;
        }
    }
    top.map_or(feet, |t| grid.at_radial(feet, t + 0.001))
}

/// Height of the sole above the nearest floor under any of its points, up
/// to `limit` metres (None if there is none within it): the box is held
/// wherever part of it is over a floor, as `collides` holds it. (The sole's
/// centre alone found none on leaves with a gap under the middle: a player
/// standing there fell and landed again every few frames, its eye moving by
/// rounding, and a still view never settled.)
pub fn floor_below(world: &World, feet: DVec3, frame: &Frame, limit: f64) -> Option<f64> {
    across()
        .filter_map(|(x, z)| {
            let at = feet + frame.right * x + frame.forward * z;
            let mut d = 0.0;
            while d <= limit {
                if world.is_solid(world.cell_at(at - frame.up * d)) {
                    return Some(d);
                }
                d += 0.01;
            }
            None
        })
        .reduce(f64::min)
}
