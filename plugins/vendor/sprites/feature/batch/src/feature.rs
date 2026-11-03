//! 2D sprites as a feature: the overlay that culls, sorts and draws the
//! scene's sprites over the final image.

use helio_feature_api::{pass_node, Feature, FeatureRegistrar};
use helio_frame_api::{FrameNode, Slot};
use helio_pass_sprite_cull::SpriteCullPass;

use crate::SpriteBatchPass;

/// Most sprites the overlay draws at once.
pub const MAX_VISIBLE_SPRITES: u32 = 16_384;

/// The scene's 2D sprites ([`crate`]): culled and sorted by their Z index on
/// the GPU, then drawn by their own orthographic camera (1 unit = 1 output
/// pixel, origin at the centre), so the 3D camera does not move them.
pub struct SpriteFeature;

impl Feature for SpriteFeature {
    fn id(&self) -> &'static str {
        "helio.sprites2d"
    }

    fn register(&self, registrar: &mut FeatureRegistrar<'_>) {
        // The culler and the batcher hand each other buffers and are one unit.
        registrar.node(
            pass_node(
                "sprite_overlay",
                "Output: Sprites",
                "Passes: Output",
                "Game sprites over the final image: GPU culling and batching.",
                |build| {
                    let device = build.env.device;
                    let queue = build.env.queue;
                    let frame = build.env.frame;
                    // The pass draws over the full-resolution image.
                    let half = [frame.width as f32 * 0.5, frame.height as f32 * 0.5];
                    let mut batch = SpriteBatchPass::scene_overlay(
                        device,
                        queue,
                        frame.surface_format,
                        helio_mats::MaterialBindingConfig::for_device(device),
                    );
                    batch.set_camera([0.0, 0.0], Some(half));
                    let mut cull = SpriteCullPass::new(
                        device,
                        queue,
                        batch.instances_buffer(),
                        batch.alive_buffer(),
                        1,
                        MAX_VISIBLE_SPRITES,
                    )
                    .skip_while_empty();
                    cull.set_view_rect([0.0, 0.0], half);
                    batch.use_gpu_culling(cull.draw_order_buf.clone(), cull.indirect_buf.clone());
                    build.graph.add_pass(Box::new(cull));
                    build.graph.add_pass(Box::new(batch));
                    Ok(())
                },
            )
            .with_marker_after()
            .in_slot(Slot::Display, 10),
        );
    }
}
