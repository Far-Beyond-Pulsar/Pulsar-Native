//! A 2D sprite (Pulsar-Native #1060): an image or a tint drawn over the 3D
//! scene, as part of 2D rendering.
//!
//! Sprites are separate from every 3D pass. They live in a 2D screen space
//! of their own: 1 unit is 1 pixel, the origin is the centre of the view, Y
//! points up, and the 3D camera does not move them. The only notion of
//! depth is the `z_index`: among sprites, a higher one draws on top, and
//! every sprite draws over the 3D scene, after post-processing (the default
//! graphs' final 2D overlay, `helio_pass_sprite_batch`'s scene overlay).
//!
//! The owner's transform places the sprite: its X and Y are the sprite's 2D
//! position, its roll (rotation about Z) turns it and its X and Y scale
//! scale it; the owner's Z position and the other rotations do not apply.
//!
//! The image is a project-relative asset path. The world loads it as a
//! texture asset ([`crate::texture_assets`]) and binds it into `image`; the
//! row registers that image in the scene's texture store as it is derived
//! (as decals do), so it carries its bindless slot. An image that is a sheet of equal frames (`atlas_columns`
//! by `atlas_rows`) shows the frame `atlas_frame`, counted row by row from
//! the top left.
//!
//! SceneDB derives a `SpriteSourceRow` (`environment_rows`) from the
//! authored value; the environment join places it as the sprite passes'
//! `helio_pass_sprite_batch::SpriteComponent` row in `"sprite_instances"`.

use engine_class_derive::{engine_class, register_world_component};
use serde::{Deserialize, Serialize};

/// The class name scripts and level files use.
pub const SPRITE_CLASS_NAME: &str = "SpriteComponent";

#[engine_class(category = "Rendering", clone, debug, serialize, deserialize)]
#[category("Sprite", category_color = "#E879F9")]
#[serde(default)]
pub struct SpriteComponent {
    #[property(category = "Sprite")]
    pub enabled: bool,
    /// Image to draw (project-relative path). Empty: the tint alone.
    #[property(category = "Sprite")]
    pub texture: String,
    /// Straight-alpha tint, multiplied with the image (or drawn alone).
    #[property(category = "Sprite")]
    pub tint: [f32; 4],
    /// Width in pixels, before the owner's X scale.
    #[property(min = 0.0, max = 16384.0, step = 1.0, category = "Sprite")]
    pub width: f32,
    /// Height in pixels, before the owner's Y scale.
    #[property(min = 0.0, max = 16384.0, step = 1.0, category = "Sprite")]
    pub height: f32,
    /// Draw order among sprites: a higher index draws on top. Every sprite
    /// draws over the 3D scene.
    #[property(min = -1000000, max = 1000000, category = "Sprite")]
    pub z_index: i32,
    /// Frames across the image, for a sprite sheet (1: the whole image).
    #[property(min = 1, max = 256, category = "Sprite")]
    pub atlas_columns: i32,
    /// Frames down the image, for a sprite sheet (1: the whole image).
    #[property(min = 1, max = 256, category = "Sprite")]
    pub atlas_rows: i32,
    /// The sheet frame to show, row by row from the top left.
    #[property(min = 0, max = 65535, category = "Sprite")]
    pub atlas_frame: i32,
    /// The image `texture` names, as the world loaded it. Never authored.
    #[serde(skip)]
    pub image: crate::texture_assets::BoundTexture,
}

impl Default for SpriteComponent {
    fn default() -> Self {
        Self {
            enabled: true,
            texture: String::new(),
            tint: [1.0, 1.0, 1.0, 1.0],
            width: 64.0,
            height: 64.0,
            z_index: 0,
            atlas_columns: 1,
            atlas_rows: 1,
            atlas_frame: 0,
            image: Default::default(),
        }
    }
}

impl SpriteComponent {
    /// The sprite passes' row for an owner at the 2D origin, with the image
    /// at `texture_slot` (`u32::MAX`: none): its size, the `z_index` as its
    /// sort depth, the frame's UV rectangle and the tint. The environment
    /// join writes the position and adds the owner's roll and scale. Zero
    /// while disabled.
    pub fn to_row(&self, texture_slot: u32) -> helio_pass_sprite_batch::SpriteComponent {
        if !self.enabled {
            return bytemuck::Zeroable::zeroed();
        }
        let columns = self.atlas_columns.max(1);
        let rows = self.atlas_rows.max(1);
        let frame = self.atlas_frame.clamp(0, columns * rows - 1);
        let (column, row) = (frame % columns, frame / columns);
        let (du, dv) = (1.0 / columns as f32, 1.0 / rows as f32);
        helio_pass_sprite_batch::SpriteComponent::from(
            helio_pass_sprite_batch::SpriteInstance::new(
                [0.0, 0.0],
                [self.width.max(0.0), self.height.max(0.0)],
            )
            .with_depth(self.z_index as f32)
            .with_uv_rect([
                column as f32 * du,
                row as f32 * dv,
                (column + 1) as f32 * du,
                (row + 1) as f32 * dv,
            ])
            .with_color(self.tint)
            .with_atlas_layer(texture_slot),
        )
    }
}

#[register_world_component]
impl SpriteComponent {}

/// The world counts the image `texture` names as a reference of the sprite,
/// whatever wrote the value, and binds it when it loads.
fn collect_sprite_handles(
    value: *const (),
    out: &mut Vec<pulsar_scenedb::handle_ledger::HandleId>,
) {
    // SAFETY: SceneDB reaches this only through `SpriteComponent`'s own
    // `ComponentId`, with a pointer to a live value.
    let sprite = unsafe { &*(value as *const SpriteComponent) };
    crate::texture_assets::collect_texture(&sprite.texture, out);
}

pulsar_scenedb::pulsar_reflection::inventory::submit! {
    pulsar_scenedb::handle_ledger::HandleLedgerRegistration {
        component_id: pulsar_scenedb::component_id::<SpriteComponent>,
        collect_from_value: collect_sprite_handles,
    }
}

/// How a sprite takes in its loaded (or failed) image.
pub(crate) fn bind_sprite_texture(sprite: &mut SpriteComponent, view: &pulsar_scenedb::AssetView) {
    crate::texture_assets::bind_texture(&mut sprite.image, &sprite.texture, view, "sprite");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registers_under_its_class_name() {
        assert_eq!(
            pulsar_world_registry::component_id_for_class(SPRITE_CLASS_NAME),
            Some(pulsar_scenedb::component_id::<SpriteComponent>())
        );
    }

    #[test]
    fn the_row_carries_the_z_index_frame_and_tint() {
        let sprite = SpriteComponent {
            tint: [1.0, 0.0, 0.0, 0.5],
            width: 32.0,
            height: 16.0,
            z_index: -3,
            atlas_columns: 4,
            atlas_rows: 2,
            atlas_frame: 5,
            ..Default::default()
        };
        let row = sprite.to_row(7);
        assert_eq!([row.size_x, row.size_y], [32.0, 16.0]);
        assert_eq!(row.depth, -3.0);
        assert_eq!(row.uv_rect, [0.25, 0.5, 0.5, 1.0]);
        assert_eq!(row.color, [1.0, 0.0, 0.0, 0.5]);
        assert_eq!(row.atlas_layer, 7);
        let off = SpriteComponent {
            enabled: false,
            ..sprite
        };
        assert_eq!(off.to_row(7), bytemuck::Zeroable::zeroed());
    }
}
