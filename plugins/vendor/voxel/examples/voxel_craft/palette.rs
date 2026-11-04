//! The world's look: every block's appearance for the voxel planet
//! (`Block::look`: colours, patches, light through and given off), and a
//! flat material for each block on the hotbar, for things that look like
//! one (the block in hand).

use crate::blocks::Block;
use helio_components::MaterialComponent;
use helio_pass_voxel_planet::engine::TerrainAppearance;
use pulsar_scenedb::{Entity, World};
use std::collections::HashMap;

pub struct Palette {
    pub appearance: TerrainAppearance,
    hand: HashMap<Block, Entity>,
}

impl Palette {
    /// The material of a block held in hand.
    pub fn hand_material(&self, block: Block) -> Option<Entity> {
        self.hand.get(&block).copied()
    }
}

/// The palette, its hand materials registered in `world`.
pub fn load(world: &mut World) -> Palette {
    let mut appearance = TerrainAppearance::default();
    for block in Block::ALL {
        appearance.materials[block.material() as usize] = block.look();
    }
    // Small voxels: each one's pigment varies a little and the land's
    // patches show; voxel edges darken a touch; natural steps are lit
    // mostly by their slope (no dark contour lines over gentle ground).
    appearance.detail = [0.3, 0.16, 0.08, 0.75];
    let hand = Block::HOTBAR
        .into_iter()
        .map(|block| {
            let [r, g, b] = block.linear_colour();
            let (emissive, strength) = block.emission().unwrap_or(([0.0; 3], 0.0));
            let entity = world.spawn();
            world.insert(
                entity,
                MaterialComponent::from(helio_mats::MaterialRow::new(
                    [r, g, b, 1.0],
                    block.roughness(),
                    0.0,
                    emissive,
                    strength,
                )),
            );
            (block, entity)
        })
        .collect();
    Palette { appearance, hand }
}
