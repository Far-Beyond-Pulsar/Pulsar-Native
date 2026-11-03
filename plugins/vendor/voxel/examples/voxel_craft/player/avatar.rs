//! The character: a blocky figure made of box meshes (scene objects with
//! ordinary materials) that stands on the planet, turns with the player,
//! swings its arms and legs as it walks, and holds the selected block.

use super::Player;
use crate::blocks::Block;
use crate::common::{
    box_mesh, make_material, spawn_material, spawn_mesh, spawn_object_with_movability,
    update_object_transform,
};
use glam::{DMat3, DVec3, Mat4, Quat, Vec3};
use pulsar_scenedb::{Entity, World as SceneWorld};

/// One box of the figure: its size, where its joint is relative to the body's
/// origin (feet, middle), where the box hangs from the joint, and what swings it.
struct Part {
    entity: Entity,
    joint: Vec3,
    centre_from_joint: Vec3,
    swing: Swing,
}

#[derive(Clone, Copy)]
enum Swing {
    None,
    /// Rotates about the joint's x axis by `scale` times the stride swing.
    Limb(f32),
    /// The head follows the pitch.
    Head,
}

pub struct Avatar {
    parts: Vec<Part>,
    /// The block in hand: a cube per hotbar block, with its textured material.
    held: Vec<(Block, Entity)>,
}

/// The figure's pieces: (name, half extents, joint, centre from joint, swing,
/// colour, roughness).
fn design() -> Vec<([f32; 3], Vec3, Vec3, Swing, [f32; 3])> {
    let skin = [0.80, 0.58, 0.44];
    let hair = [0.20, 0.12, 0.07];
    let shirt = [0.02, 0.62, 0.68];
    let sleeve = [0.02, 0.55, 0.62];
    let pants = [0.24, 0.22, 0.58];
    let shoe = [0.30, 0.30, 0.33];
    let eye = [0.95, 0.97, 1.0];
    let pupil = [0.12, 0.2, 0.55];
    vec![
        // Legs (joint at the hip), shoes.
        (
            [0.115, 0.30, 0.115],
            Vec3::new(-0.12, 0.65, 0.0),
            Vec3::new(0.0, -0.30, 0.0),
            Swing::Limb(1.0),
            pants,
        ),
        (
            [0.115, 0.30, 0.115],
            Vec3::new(0.12, 0.65, 0.0),
            Vec3::new(0.0, -0.30, 0.0),
            Swing::Limb(-1.0),
            pants,
        ),
        (
            [0.12, 0.05, 0.14],
            Vec3::new(-0.12, 0.65, 0.0),
            Vec3::new(0.0, -0.62, 0.02),
            Swing::Limb(1.0),
            shoe,
        ),
        (
            [0.12, 0.05, 0.14],
            Vec3::new(0.12, 0.65, 0.0),
            Vec3::new(0.0, -0.62, 0.02),
            Swing::Limb(-1.0),
            shoe,
        ),
        // Torso.
        (
            [0.25, 0.30, 0.125],
            Vec3::new(0.0, 0.95, 0.0),
            Vec3::ZERO,
            Swing::None,
            shirt,
        ),
        // Arms (joint at the shoulder), hands.
        (
            [0.115, 0.22, 0.115],
            Vec3::new(-0.365, 1.22, 0.0),
            Vec3::new(0.0, -0.20, 0.0),
            Swing::Limb(-1.0),
            sleeve,
        ),
        (
            [0.115, 0.22, 0.115],
            Vec3::new(0.365, 1.22, 0.0),
            Vec3::new(0.0, -0.20, 0.0),
            Swing::Limb(1.0),
            sleeve,
        ),
        (
            [0.10, 0.09, 0.10],
            Vec3::new(-0.365, 1.22, 0.0),
            Vec3::new(0.0, -0.50, 0.0),
            Swing::Limb(-1.0),
            skin,
        ),
        (
            [0.10, 0.09, 0.10],
            Vec3::new(0.365, 1.22, 0.0),
            Vec3::new(0.0, -0.50, 0.0),
            Swing::Limb(1.0),
            skin,
        ),
        // Head with hair, eyes.
        (
            [0.25, 0.25, 0.25],
            Vec3::new(0.0, 1.25, 0.0),
            Vec3::new(0.0, 0.25, 0.0),
            Swing::Head,
            skin,
        ),
        (
            [0.265, 0.09, 0.265],
            Vec3::new(0.0, 1.25, 0.0),
            Vec3::new(0.0, 0.46, -0.0),
            Swing::Head,
            hair,
        ),
        (
            [0.265, 0.25, 0.04],
            Vec3::new(0.0, 1.25, 0.0),
            Vec3::new(0.0, 0.30, 0.22),
            Swing::Head,
            hair,
        ),
        (
            [0.055, 0.05, 0.02],
            Vec3::new(0.0, 1.25, 0.0),
            Vec3::new(-0.11, 0.24, -0.26),
            Swing::Head,
            eye,
        ),
        (
            [0.055, 0.05, 0.02],
            Vec3::new(0.0, 1.25, 0.0),
            Vec3::new(0.11, 0.24, -0.26),
            Swing::Head,
            eye,
        ),
        (
            [0.03, 0.045, 0.02],
            Vec3::new(0.0, 1.25, 0.0),
            Vec3::new(-0.09, 0.24, -0.275),
            Swing::Head,
            pupil,
        ),
        (
            [0.03, 0.045, 0.02],
            Vec3::new(0.0, 1.25, 0.0),
            Vec3::new(0.09, 0.24, -0.275),
            Swing::Head,
            pupil,
        ),
    ]
}

impl Avatar {
    /// Spawns the figure's parts, and a cube for each block that can be held
    /// (their materials come from `block_material`).
    pub fn spawn(world: &mut SceneWorld, block_material: impl Fn(Block) -> Option<Entity>) -> Self {
        let mut parts = Vec::new();
        for (half, joint, centre_from_joint, swing, colour) in design() {
            let mesh = spawn_mesh(world, box_mesh([0.0; 3], half));
            let material = spawn_material(
                world,
                make_material(
                    [colour[0], colour[1], colour[2], 1.0],
                    0.85,
                    0.0,
                    [0.0; 3],
                    0.0,
                ),
            );
            let object = spawn_object_with_movability(
                world,
                mesh,
                material,
                hidden(),
                1.5,
                Some(helio::Movability::Movable),
            )
            .expect("figure part");
            parts.push(Part {
                entity: object,
                joint,
                centre_from_joint,
                swing,
            });
        }
        let cube = spawn_mesh(world, box_mesh([0.0; 3], [0.5; 3]));
        let mut held = Vec::new();
        for block in Block::HOTBAR {
            if let Some(material) = block_material(block) {
                let object = spawn_object_with_movability(
                    world,
                    cube,
                    material,
                    hidden(),
                    1.0,
                    Some(helio::Movability::Movable),
                )
                .expect("held block");
                held.push((block, object));
            }
        }
        Self { parts, held }
    }

    /// Poses the figure for `player` and puts the held block in hand, with
    /// positions relative to `origin` (the renderer's world origin).
    pub fn pose(
        &self,
        world: &mut SceneWorld,
        renderer: &mut helio::Renderer,
        player: &Player,
        camera: (DVec3, DVec3),
        origin: DVec3,
        time: f64,
        tracked_hand: Option<Option<Mat4>>,
    ) {
        let frame = player.frame();
        // Body axes: +X right, +Y up, +Z back (the figure faces -Z).
        let basis = DMat3::from_cols(frame.right, frame.up, -frame.forward);
        let base = Mat4::from_cols(
            basis.x_axis.as_vec3().extend(0.0),
            basis.y_axis.as_vec3().extend(0.0),
            basis.z_axis.as_vec3().extend(0.0),
            (player.position - origin).as_vec3().extend(1.0),
        );
        let visible = player.third_person;
        let stride = player.stride as f32;
        let swing = (stride.sin() * 0.85 * player.swing as f32).clamp(-1.2, 1.2);
        let sneak = if player.on_ground && player.velocity.length() < 1.0 {
            0.0
        } else {
            0.0
        };
        let _ = sneak;
        for part in &self.parts {
            let matrix = if visible {
                let angle = match part.swing {
                    Swing::None => 0.0,
                    Swing::Limb(scale) => swing * scale,
                    Swing::Head => 0.0,
                };
                let mut pose = Mat4::from_translation(part.joint);
                if let Swing::Head = part.swing {
                    pose *= Mat4::from_quat(Quat::from_rotation_x(-(player.pitch as f32) * 0.8));
                } else {
                    pose *= Mat4::from_quat(Quat::from_rotation_x(angle));
                }
                base * pose * Mat4::from_translation(part.centre_from_joint)
            } else {
                hidden()
            };
            let _ = update_object_transform(world, renderer, part.entity, matrix);
        }

        // The block in hand: beside the face in first person; at the figure's
        // right hand in third.
        let selected = Block::HOTBAR[player.hotbar];
        let (eye, look) = camera;
        let hand = if let Some(tracked) = tracked_hand {
            tracked.map_or_else(hidden, |pose| {
                pose * Mat4::from_translation(Vec3::new(0.0, 0.0, -0.12))
                    * Mat4::from_scale(Vec3::splat(0.17))
            })
        } else if player.third_person {
            let pose = Mat4::from_translation(Vec3::new(0.365, 1.22, 0.0))
                * Mat4::from_quat(Quat::from_rotation_x(-swing))
                * Mat4::from_translation(Vec3::new(0.0, -0.58, -0.14));
            base * pose * Mat4::from_scale(Vec3::splat(0.2))
        } else {
            let right = look.cross(frame.up).normalize_or_zero();
            let down = (-frame.up - look * (-frame.up).dot(look)).normalize_or_zero();
            let bob = (player.stride.sin() * 0.012 * player.swing) as f32;
            let at = eye + look * 0.55 + right * 0.30 + down * (0.26 + f64::from(bob));
            let rotation = Quat::from_mat3(&glam::Mat3::from_cols(
                right.as_vec3(),
                (-down).as_vec3(),
                (-look).as_vec3(),
            ));
            let spin = Quat::from_rotation_y(0.6) * Quat::from_rotation_x(-0.25);
            Mat4::from_rotation_translation(rotation * spin, (at - origin).as_vec3())
                * Mat4::from_scale(Vec3::splat(0.17))
        };
        let _ = time;
        for (block, entity) in &self.held {
            let matrix = if *block == selected { hand } else { hidden() };
            let _ = update_object_transform(world, renderer, *entity, matrix);
        }
    }
}

/// A transform that shows nothing (zero scale would give degenerate normals).
fn hidden() -> Mat4 {
    Mat4::from_scale(Vec3::splat(1e-4))
}
