//! Mapping a metre-scale OpenXR room onto the planet's double-precision frame.

use crate::{blocks::Block, input::Controls, player::Player, world::World};
use glam::{DMat4, DVec3, Mat4, Vec2, Vec3};

#[derive(Clone, Copy, Debug, Default)]
pub struct Buttons {
    pub mine: bool,
    pub place: bool,
    pub jump: bool,
    pub sneak: bool,
    pub sprint: bool,
    pub pick: bool,
    pub fly: bool,
    pub recenter: bool,
}

impl Buttons {
    fn any(self) -> bool {
        self.mine
            || self.place
            || self.jump
            || self.sneak
            || self.sprint
            || self.pick
            || self.fly
            || self.recenter
    }
}

#[derive(Clone, Copy, Debug)]
pub struct FrameInput {
    pub focused: bool,
    pub head: Mat4,
    pub grips: [Option<Mat4>; 2],
    pub aim: Option<Mat4>,
    pub movement: Vec2,
    pub turn: Vec2,
    pub buttons: Buttons,
}

/// Authored positions stay double precision until the renderer subtracts its origin.
pub struct RigView {
    pub world_from_stage: DMat4,
    pub head: DMat4,
    pub grips: [Option<DMat4>; 2],
    pub aim: (DVec3, DVec3),
}

#[derive(Default)]
pub struct Rig {
    previous_head: Option<Vec3>,
    anchor: Vec3,
    previous_buttons: Buttons,
    armed: bool,
    turn: AxisLatch,
    hotbar: AxisLatch,
}

/// Hysteresis: a held stick makes one step and must return to centre to repeat.
#[derive(Default)]
struct AxisLatch {
    held: bool,
}

impl AxisLatch {
    fn step(&mut self, axis: f32) -> i32 {
        if !axis.is_finite() || axis.abs() < 0.25 {
            self.held = false;
            return 0;
        }
        if self.held || axis.abs() < 0.65 {
            return 0;
        }
        self.held = true;
        if axis > 0.0 {
            1
        } else {
            -1
        }
    }
}

pub fn deadzone(stick: Vec2) -> Vec2 {
    let length = stick.length();
    if !length.is_finite() || length <= 0.15 {
        Vec2::ZERO
    } else {
        stick / length * ((length.min(1.0) - 0.15) / 0.85)
    }
}

/// Rigid, right-handed frame with +Y along gravity's opposite and -Z forward.
fn stage_transform(player: &Player, anchor: Vec3) -> DMat4 {
    let frame = player.frame();
    let basis = DMat4::from_cols(
        frame.right.extend(0.0),
        frame.up.extend(0.0),
        (-frame.forward).extend(0.0),
        player.position.extend(1.0),
    );
    basis * DMat4::from_translation(-anchor.as_dvec3())
}

impl Rig {
    pub fn reset(&mut self) {
        self.previous_head = None;
        self.armed = false;
        self.previous_buttons = Buttons::default();
        self.turn.held = true;
        self.hotbar.held = true;
    }

    pub fn update(
        &mut self,
        input: &FrameInput,
        player: &mut Player,
        world: &World,
        controls: &mut Controls,
    ) {
        player.third_person = false;
        player.pitch = 0.0;
        controls.look = (0.0, 0.0);
        let head = input.head.w_axis.truncate();
        self.anchor = Vec3::new(head.x, 0.0, head.z);
        if !input.focused {
            self.reset();
            *controls = Controls::default();
            return;
        }

        // Reacquiring tracking or recentering must never teleport the body. A large
        // discontinuity is a tracking-origin change, not physical room movement.
        let recenter = input.buttons.recenter && !self.previous_buttons.recenter;
        if let Some(previous) = self.previous_head.filter(|_| !recenter) {
            let delta = self.anchor - previous;
            if delta.length_squared() < 1.0 {
                let frame = player.frame();
                player.move_room(
                    world,
                    frame.right * f64::from(delta.x) - frame.forward * f64::from(delta.z),
                );
            }
        }
        self.previous_head = Some(self.anchor);

        // Wait for neutral input after the dashboard closes or tracking returns.
        // In particular, a trigger held in the dashboard must not dig on refocus.
        if !self.armed {
            self.armed = !input.buttons.any()
                && input.movement.length() < 0.15
                && input.turn.length() < 0.25;
            self.previous_buttons = input.buttons;
            *controls = Controls::default();
            return;
        }

        let snap = self.turn.step(input.turn.x);
        player.turn(-f64::from(snap) * std::f64::consts::FRAC_PI_6);
        let slot = self.hotbar.step(input.turn.y);
        player.hotbar =
            (player.hotbar as i32 + slot).rem_euclid(Block::HOTBAR.len() as i32) as usize;
        if input.buttons.fly && !self.previous_buttons.fly {
            player.flying = !player.flying;
            player.velocity = DVec3::ZERO;
        }

        // Head-relative motion stays horizontal even when looking straight up.
        let forward = input.head.transform_vector3(Vec3::NEG_Z);
        let forward = Vec3::new(forward.x, 0.0, forward.z)
            .try_normalize()
            .unwrap_or(Vec3::NEG_Z);
        let stick = deadzone(input.movement);
        let motion = forward * stick.y + forward.cross(Vec3::Y) * stick.x;
        controls.forward = (controls.forward - f64::from(motion.z)).clamp(-1.0, 1.0);
        controls.strafe = (controls.strafe + f64::from(motion.x)).clamp(-1.0, 1.0);
        controls.jump |= input.buttons.jump && (player.flying || !self.previous_buttons.jump);
        controls.sneak |= input.buttons.sneak;
        controls.sprint |= input.buttons.sprint;
        controls.mine |= input.buttons.mine;
        controls.place |= input.buttons.place && !self.previous_buttons.place;
        controls.pick |= input.buttons.pick && !self.previous_buttons.pick;
        self.previous_buttons = input.buttons;
    }

    pub fn view(&self, player: &Player, input: &FrameInput) -> RigView {
        let world_from_stage = stage_transform(player, self.anchor);
        let head = world_from_stage * input.head.as_dmat4();
        let aim = world_from_stage * input.aim.unwrap_or(input.head).as_dmat4();
        RigView {
            world_from_stage,
            head,
            grips: input
                .grips
                .map(|pose| pose.map(|pose| world_from_stage * pose.as_dmat4())),
            aim: (
                aim.w_axis.truncate(),
                aim.transform_vector3(DVec3::NEG_Z).normalize(),
            ),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snap_turn_requires_recentering_and_ignores_noise() {
        let mut latch = AxisLatch::default();
        assert_eq!(latch.step(0.6), 0);
        assert_eq!(latch.step(0.9), 1);
        assert_eq!(latch.step(1.0), 0);
        assert_eq!(latch.step(-1.0), 0);
        assert_eq!(latch.step(0.2), 0);
        assert_eq!(latch.step(-0.9), -1);
    }

    #[test]
    fn analogue_deadzone_is_continuous_and_bounded() {
        assert_eq!(deadzone(Vec2::new(0.1, 0.0)), Vec2::ZERO);
        assert!((deadzone(Vec2::new(0.575, 0.0)).x - 0.5).abs() < 1e-6);
        assert!((deadzone(Vec2::ONE).length() - 1.0).abs() < 1e-6);
        assert_eq!(deadzone(Vec2::splat(f32::NAN)), Vec2::ZERO);
    }

    #[test]
    fn stage_follows_gravity_and_turns_about_the_head_without_scaling_ipd() {
        for up in [DVec3::X, DVec3::Y, DVec3::NEG_Z, DVec3::ONE.normalize()] {
            let mut player = Player::new(up * 16_000.0);
            let anchor = Vec3::new(2.4, 0.0, -3.1);
            let head = anchor.as_dvec3() + DVec3::Y * 1.72;
            let before = stage_transform(&player, anchor).transform_point3(head);
            player.turn(std::f64::consts::FRAC_PI_6);
            let stage = stage_transform(&player, anchor);
            assert!(stage.transform_vector3(DVec3::Y).abs_diff_eq(up, 1e-12));
            assert!(stage.transform_point3(head).abs_diff_eq(before, 1e-9));
            let left = stage.transform_point3(head - DVec3::X * 0.032);
            let right = stage.transform_point3(head + DVec3::X * 0.032);
            assert!(((right - left).length() - 0.064).abs() < 1e-9);
            // Subtract the floating origin before narrowing to f32.
            let origin = helio::follow_origin(None, before);
            let relative = (DMat4::from_translation(-origin) * stage).as_mat4();
            let rendered = relative.transform_point3(head.as_vec3()).as_dvec3() + origin;
            assert!((rendered - before).length() < 0.0002);
        }
    }

    #[test]
    fn refocus_requires_trigger_release_and_tracking_recenter_does_not_teleport() {
        let world = World::new(
            helio_pass_voxel_planet::Planet::new(crate::terrain::recipe::recipe()).unwrap(),
        );
        let mut player = Player::new(DVec3::Y * 17_000.0);
        let mut rig = Rig::default();
        let mut frame = FrameInput {
            focused: true,
            head: Mat4::from_translation(Vec3::Y * 1.7),
            grips: [None; 2],
            aim: None,
            movement: Vec2::ZERO,
            turn: Vec2::ZERO,
            buttons: Buttons::default(),
        };
        let tick = |rig: &mut Rig, player: &mut Player, frame: &FrameInput| {
            let mut controls = Controls::default();
            rig.update(frame, player, &world, &mut controls);
            controls
        };
        tick(&mut rig, &mut player, &frame);
        frame.buttons.mine = true;
        assert!(tick(&mut rig, &mut player, &frame).mine);
        frame.focused = false;
        assert!(!tick(&mut rig, &mut player, &frame).mine);
        frame.focused = true;
        assert!(!tick(&mut rig, &mut player, &frame).mine);
        frame.buttons.mine = false;
        tick(&mut rig, &mut player, &frame);
        frame.buttons.place = true;
        assert!(tick(&mut rig, &mut player, &frame).place);
        assert!(!tick(&mut rig, &mut player, &frame).place);
        let position = player.position;
        frame.head = Mat4::from_translation(Vec3::new(50.0, 1.7, -20.0));
        tick(&mut rig, &mut player, &frame);
        assert_eq!(player.position, position);
        let view = rig.view(&player, &frame);
        assert!(view
            .head
            .w_axis
            .truncate()
            .abs_diff_eq(position + DVec3::Y * 1.7f32 as f64, 1e-9));
    }
}
