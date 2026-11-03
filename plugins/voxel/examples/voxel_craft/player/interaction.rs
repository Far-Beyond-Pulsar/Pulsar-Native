//! Digging and building where the player looks.

use super::physics::{occupies, Frame};
use super::Player;
use crate::config::REACH;
use crate::input::Controls;
use crate::world::edits::{block_cells, block_origin, Edit};
use crate::world::World;
use glam::DVec3;
use helio_pass_voxel_planet::Cell;

/// The voxel under the crosshair.
#[derive(Clone, Copy, Debug)]
pub struct Target {
    pub cell: Cell,
    /// The empty cell in front of the face that was hit.
    pub front: Cell,
    /// Where the ray met it (planet-centred).
    pub point: DVec3,
}

#[derive(Default)]
pub struct Interaction {
    pub target: Option<Target>,
    /// Seconds until the next dig or build.
    cooldown: f64,
}

/// Seconds between repeated digs while the button is held.
const DIG_INTERVAL: f64 = 0.12;

impl Interaction {
    /// Updates the target and applies this frame's clicks.
    pub fn update(&mut self, player: &mut Player, world: &mut World, controls: &Controls, dt: f64) {
        self.cooldown = (self.cooldown - dt).max(0.0);
        let (eye, look) = controls
            .aim
            .unwrap_or_else(|| (player.eye(), player.look()));
        self.target = world.raycast(eye, look, REACH).map(|hit| Target {
            cell: hit.cell,
            front: hit.previous,
            point: eye + look * hit.distance,
        });
        let Some(target) = self.target else { return };

        if controls.mine && self.cooldown == 0.0 {
            world.edit(&[Edit::Dig(target.point)]);
            self.cooldown = DIG_INTERVAL;
        }
        if controls.pick {
            if let Some(block) = world.block_at(target.cell) {
                if let Some(slot) = crate::blocks::Block::HOTBAR
                    .iter()
                    .position(|b| *b == block)
                {
                    player.hotbar = slot;
                }
            }
        }
        if controls.place && self.cooldown == 0.0 {
            let block = crate::blocks::Block::HOTBAR[player.hotbar];
            let edit = Edit::Build(target.front, block);
            let grid = *world.planet().grid();
            // Not into the player's own body.
            let frame: Frame = player.frame();
            if !edit
                .cells(&grid)
                .iter()
                .any(|(cell, _)| occupies(world, player.position, &frame, *cell))
            {
                world.edit(&[edit]);
                self.cooldown = 0.12;
            }
        }
    }

    /// The twelve edges of the block a build would lay, planet-centred.
    pub fn outline(&self, world: &World) -> Vec<(DVec3, DVec3)> {
        let Some(target) = self.target else {
            return Vec::new();
        };
        let grid = world.planet().grid();
        let c = block_origin(grid, target.front);
        let n = block_cells(grid);
        let corner = |a: i32, b: i32, k: i32| {
            grid.position(
                c.face,
                [
                    f64::from(c.i + a * n),
                    f64::from(c.j + b * n),
                    f64::from(c.k + k * n),
                ],
            )
        };
        let mut edges = Vec::with_capacity(12);
        for a in 0..2 {
            for b in 0..2 {
                edges.push((corner(a, b, 0), corner(a, b, 1)));
                edges.push((corner(a, 0, b), corner(a, 1, b)));
                edges.push((corner(0, a, b), corner(1, a, b)));
            }
        }
        edges
    }
}
