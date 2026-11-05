//! The meshes' shadow drawer: before the frame's shadow-caster phase it finds
//! the faces its casters made stale (`ShadowDirtyPass`: casters that moved,
//! and the faces whose light's matrices changed, from the lights'
//! [`LightDirtyFlags`]); then it opens the phase, rendering the stale faces
//! into the tiled atlas. The phase's views are the lights' shadow faces:
//! without the lights plugin (no [`LightDirtyFlags`]) both build nothing.

use std::sync::Arc;

use helio_feature_api::{missing, pass_node, Feature, FeatureRegistrar};
use helio_frame_api::{FrameNode, Phase};
use helio_light_api::{LightDirtyFlags, ShadowSettings};
use helio_pass_shadow_dirty::ShadowDirtyPass;

use crate::ShadowPass;

/// Meshes in the shadow atlas ([`crate`]).
pub struct ShadowDrawers;

impl Feature for ShadowDrawers {
    fn id(&self) -> &'static str {
        "helio.mesh.shadow"
    }

    fn register(&self, registrar: &mut FeatureRegistrar<'_>) {
        registrar.node(
            pass_node(
                "shadow_dirty",
                "Shadows: Find Stale Faces",
                "Passes: Scene Prep",
                "Marks which shadow faces need re-rendering this frame.",
                |build| {
                    // The phase draws the lights' shadow faces: without the
                    // lights plugin there are none, and nothing to draw.
                    let Some(dirty) = build
                        .env
                        .shared_existing::<LightDirtyFlags>()
                        .map(|flags| Arc::clone(&flags.0))
                    else {
                        return Ok(());
                    };
                    let pass = ShadowDirtyPass::new(build.env.device, dirty);
                    build.graph.add_pass(Box::new(pass));
                    Ok(())
                },
            )
            .before_phase(Phase::ShadowCasters, 0),
        );

        registrar.node(
            pass_node(
                "shadow",
                "Shadows: Render Atlas",
                "Passes: Scene Prep",
                "Renders the dirty shadow faces into the tiled shadow atlas.",
                |build| {
                    let settings = build
                        .env
                        .setting::<ShadowSettings>()
                        .copied()
                        .unwrap_or_default();
                    if build.env.shared_existing::<LightDirtyFlags>().is_none() {
                        // No shadow faces (no lights plugin): nothing to draw.
                        return Ok(());
                    }
                    let (_, atlas_size) =
                        settings.atlas(build.env.device.limits().max_texture_dimension_2d)?;
                    let (face_dirty, face_geom_count, face_pages) = build
                        .graph
                        .find_pass::<ShadowDirtyPass>()
                        .map(|dirty| {
                            (
                                Arc::clone(&dirty.face_dirty_buf),
                                Arc::clone(&dirty.face_geom_count_buf),
                                Arc::clone(&dirty.face_pages_buf),
                            )
                        })
                        .ok_or_else(|| missing("Shadow Maps", "Shadow Dirty Flags"))?;
                    build.graph.add_pass(Box::new(ShadowPass::new_tiled(
                        build.env.device,
                        build.env.queue,
                        face_dirty,
                        face_geom_count,
                        face_pages,
                        atlas_size,
                    )));
                    Ok(())
                },
            )
            .opens(Phase::ShadowCasters, 0),
        );
    }
}
