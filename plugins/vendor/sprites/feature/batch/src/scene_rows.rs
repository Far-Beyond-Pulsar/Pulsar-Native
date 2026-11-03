//! Sprites' scene rows (a row join, see `helio_scene_join::RowJoin`): each
//! sprite with a size, at its owner's X and Y (2D), turned by its roll,
//! scaled by its X and Y scale; each keeps its source row (the sprite passes
//! cull every row).
use helio_scene_join::{placement::*, BufferKey, RowJoin, RowJoinKeys, RowTable};

/// `SpriteSourceRow`: the sprite passes' row (`SpriteRow`, 20 words) at
/// the 2D origin.
pub const SPRITE_SOURCE_ROW_BYTES: u64 = 20 * 4;
/// A sprite source row's width word: zero for a disabled sprite.
const SPRITE_SIZE_WORD: u32 = 2;

pub const SPRITES_KEY: BufferKey = BufferKey::of("sprite_instances");

/// Sprites' row join, from the sprite rows at `sprites`.
pub fn scene_rows(device: &wgpu::Device, keys: RowJoinKeys, sprites: BufferKey) -> RowJoin {
    RowJoin::new(
        device,
        "SpriteRows",
        keys,
        vec![RowTable::new(
            "Sprite Rows",
            SPRITES_KEY,
            sprites,
            SPRITE_SOURCE_ROW_BYTES,
            20,
            GATE_HIDDEN | PLANAR_2D,
        )
        .gated_on(SPRITE_SIZE_WORD)],
    )
}
