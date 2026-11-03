//! The player: movement on a sphere, the camera, and breaking and placing
//! blocks.

pub mod avatar;
pub mod interaction;
mod physics;

pub use physics::Frame;

use crate::config::*;
use crate::input::Controls;
use crate::world::World;
use glam::DVec3;

/// How far around its feet the player's body needs the world resident to
/// move: its height, a step and a frame's fall.
const BODY_REACH_M: f64 = 3.0;

pub struct Player {
    /// The soles of the feet, planet-centred.
    pub position: DVec3,
    pub velocity: DVec3,
    /// Where the player faces (tangent to the sphere, kept unit).
    pub heading: DVec3,
    /// Look up (+) or down (-), radians.
    pub pitch: f64,
    pub on_ground: bool,
    pub flying: bool,
    pub sprinting: bool,
    pub third_person: bool,
    /// Walking animation phase (radians) and how much the legs are swinging.
    pub stride: f64,
    pub swing: f64,
    pub hotbar: usize,
    pub spawn: DVec3,
    jump_tap: f64,
    sneaking: bool,
}

impl Player {
    pub fn new(position: DVec3) -> Self {
        let up = position.normalize();
        Self {
            position,
            velocity: DVec3::ZERO,
            heading: up.any_orthonormal_vector(),
            pitch: -0.15,
            on_ground: false,
            flying: false,
            sprinting: false,
            third_person: false,
            stride: 0.0,
            swing: 0.0,
            hotbar: 0,
            spawn: position,
            jump_tap: 10.0,
            sneaking: false,
        }
    }

    pub fn frame(&self) -> Frame {
        Frame::new(self.position, self.heading)
    }

    /// Room movement uses the same collision solver as stick/keyboard movement.
    pub fn move_room(&mut self, world: &World, displacement: DVec3) {
        let frame = self.frame();
        let (position, _) = physics::slide(world, self.position, displacement, &frame, false);
        self.position = position;
    }

    pub fn turn(&mut self, angle: f64) {
        let frame = self.frame();
        self.heading = rotate(frame.forward, frame.up, angle);
    }

    /// The eye, planet-centred.
    pub fn eye(&self) -> DVec3 {
        let frame = self.frame();
        self.position
            + frame.up
                * if self.sneaking {
                    EYE_HEIGHT - 0.25
                } else {
                    EYE_HEIGHT
                }
    }

    /// The direction the player looks along.
    pub fn look(&self) -> DVec3 {
        let frame = self.frame();
        (frame.forward * self.pitch.cos() + frame.up * self.pitch.sin()).normalize()
    }

    /// The camera: eye position and look direction (behind and above the
    /// character in third person, pulled in where blocks are in the way).
    pub fn camera(&self, world: &World) -> (DVec3, DVec3) {
        let look = self.look();
        let eye = self.eye();
        if !self.third_person {
            return (eye, look);
        }
        let back = -look;
        let wanted = THIRD_PERSON_DISTANCE;
        let free = world
            .raycast(eye, back, wanted + 0.4)
            .map_or(wanted, |h| (h.distance - 0.4).max(0.5));
        (eye + back * free, look)
    }

    /// Advances the player by `dt` seconds.
    pub fn update(&mut self, world: &World, controls: &Controls, dt: f64) {
        let dt = dt.min(0.05);
        // Turning: the heading turns about up, the pitch follows the mouse.
        let frame = self.frame();
        let sensitivity = 0.0022;
        let yaw = -controls.look.0 * sensitivity;
        self.heading = rotate(frame.forward, frame.up, yaw);
        self.pitch = (self.pitch - controls.look.1 * sensitivity).clamp(-1.52, 1.52);
        let frame = self.frame();
        // The body waits while the space it moves in streams in (a spawn, a
        // teleport): it is not moved against space not resident yet.
        if !world.loaded_around(self.position, BODY_REACH_M) {
            self.velocity = DVec3::ZERO;
            return;
        }

        // Double-tapping jump toggles flight, as in the original.
        self.jump_tap += dt;
        self.sneaking = controls.sneak && !self.flying;

        // Preserve analogue stick magnitude while limiting keyboard diagonals.
        let wish_planar = (frame.forward * controls.forward + frame.right * controls.strafe)
            .clamp_length_max(1.0);
        self.sprinting = controls.sprint && controls.forward > 0.0;
        if self.flying {
            let speed = if controls.sprint {
                FLY_FAST_SPEED
            } else {
                FLY_SPEED
            };
            let lift = f64::from(i32::from(controls.jump) - i32::from(controls.sneak));
            let target = (wish_planar + frame.up * lift).normalize_or_zero() * speed;
            self.velocity += (target - self.velocity) * (1.0 - (-8.0 * dt).exp());
            let (position, _) =
                physics::slide(world, self.position, self.velocity * dt, &frame, false);
            self.position = position;
            self.on_ground = false;
        } else {
            let speed = if self.sneaking {
                WALK_SPEED * 0.3
            } else if self.sprinting {
                SPRINT_SPEED
            } else {
                WALK_SPEED
            };
            let vertical = self.velocity.dot(frame.up);
            let planar = self.velocity - frame.up * vertical;
            let grip = if self.on_ground { 14.0 } else { 2.5 };
            let planar = planar + (wish_planar * speed - planar) * (1.0 - (-grip * dt).exp());
            // Standing: no fall to undo each frame (that bobs the camera).
            let mut vertical = if self.on_ground && vertical <= 0.0 {
                0.0
            } else {
                vertical - GRAVITY * dt
            };
            if self.on_ground && controls.jump {
                vertical = JUMP_SPEED;
                self.on_ground = false;
            }
            self.velocity = planar + frame.up * vertical;
            let (position, moved) = physics::slide(
                world,
                self.position,
                self.velocity * dt,
                &frame,
                self.on_ground,
            );
            self.position = position;
            if moved.landed {
                self.on_ground = true;
                self.velocity -= frame.up * self.velocity.dot(frame.up).min(0.0);
            } else if moved.hit_ceiling {
                self.velocity -= frame.up * self.velocity.dot(frame.up).max(0.0);
                self.on_ground = false;
            } else if !moved.stepped {
                // Walking off an edge: grounded only while a floor is at the soles.
                self.on_ground = physics::floor_below(world, self.position, &frame, 0.05).is_some()
                    && self.velocity.dot(frame.up) <= 0.0;
            }
            if moved.stepped {
                self.on_ground = true;
                self.velocity -= frame.up * self.velocity.dot(frame.up).min(0.0);
            }
        }

        // Re-aim the heading along the new tangent plane (parallel transport).
        let up = self.position.normalize();
        self.heading = (self.heading - up * self.heading.dot(up))
            .try_normalize()
            .unwrap_or_else(|| up.any_orthonormal_vector());

        // Animation follows the planar speed.
        let planar_speed = (self.velocity - up * self.velocity.dot(up)).length();
        let moving = if self.on_ground || self.flying {
            (planar_speed / WALK_SPEED).min(1.5)
        } else {
            0.0
        };
        self.swing += (moving - self.swing) * (1.0 - (-10.0 * dt).exp());
        self.stride += planar_speed * dt * 2.2;

        // Fell out of the world (a hole through the core).
        if self.position.length() < 2.0 {
            self.respawn();
        }
    }

    /// Back to the spawn point.
    pub fn respawn(&mut self) {
        self.position = self.spawn;
        self.velocity = DVec3::ZERO;
    }

    /// Registers a jump press; two close together toggle flight.
    pub fn jump_pressed(&mut self) -> bool {
        let double = self.jump_tap < 0.3;
        self.jump_tap = if double { 10.0 } else { 0.0 };
        double
    }
}

/// `v` rotated by `angle` about the unit axis `axis`.
fn rotate(v: DVec3, axis: DVec3, angle: f64) -> DVec3 {
    let (s, c) = angle.sin_cos();
    v * c + axis.cross(v) * s + axis * axis.dot(v) * (1.0 - c)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::blocks::Block;
    use crate::terrain::recipe::recipe;
    use crate::world::edits::Edit;
    use helio_pass_voxel_planet::Planet;

    /// A player standing on blocks with a gap under the middle of its soles
    /// (leaves, a ledge between two blocks) is held by the blocks under the
    /// rest of them: grounded every frame, and still. (Grounded by the sole's
    /// centre alone it fell and landed every few frames, its eye moving by
    /// rounding, and a still view on a tree never settled.)
    #[test]
    fn a_player_over_a_gap_stands_still() {
        let mut world = World::new(Planet::new(recipe()).unwrap());
        let grid = *world.planet().grid();
        let n = f64::from(grid.cells());
        let ground = world
            .planet()
            .surface_point(grid.position(2, [n * 0.5, n * 0.5, 0.0]), 6.0);
        let plate = world.cell_centre(world.cell_at(ground));
        let frame = Frame::new(plate, plate.normalize().any_orthonormal_vector());
        // A plate of blocks three across without its middle one.
        let mut edits = Vec::new();
        for a in -1..=1 {
            for b in -1..=1 {
                if (a, b) != (0, 0) {
                    edits.push(Edit::Build(
                        world.cell_at(
                            plate
                                + frame.right * (BLOCK_M * f64::from(a))
                                + frame.forward * (BLOCK_M * f64::from(b)),
                        ),
                        Block::Cobblestone,
                    ));
                }
            }
        }
        world.edit(&edits);
        world.follow(plate);
        world.settle();
        let mut player = Player::new(plate + frame.up * 1.0);
        let controls = Controls::default();
        let mut rest = Vec::new();
        for step in 0..240 {
            player.update(&world, &controls, 1.0 / 60.0);
            if step >= 120 {
                rest.push((player.position, player.on_ground));
            }
        }
        assert!(
            rest.iter().all(|r| r.1),
            "grounded every frame: {:?}",
            rest.iter().map(|r| r.1).collect::<Vec<_>>()
        );
        assert!(
            rest.iter().all(|r| r.0 == rest[0].0),
            "still: {:?}",
            rest.iter()
                .map(|r| (r.0 - rest[0].0).length())
                .fold(0.0, f64::max)
        );
        assert!(
            (player.position - plate).dot(frame.up) > 0.0
                && (player.position - plate).dot(frame.up) < BLOCK_M,
            "on the plate"
        );
    }
}
