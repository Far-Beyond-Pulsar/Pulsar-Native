//! The meshes' transparency drawer: alpha-blended meshes over the scene
//! colour the colour chain has where the transparent phase runs (with the
//! participating medium composited, when a media feature published one),
//! each fragment seen through the medium at its own depth.
//! With [`helio_frame_api::color::ReactiveMask`] it also writes the mask the
//! anti-aliasing reads.

use helio_feature_api::{pass_node, Feature, FeatureRegistrar};
use helio_frame_api::{color, media, FrameNode, Phase};

use crate::TransparentPass;

/// Transparent meshes ([`crate`]).
pub struct TransparentDrawers;

impl Feature for TransparentDrawers {
    fn id(&self) -> &'static str {
        "helio.mesh.transparent"
    }

    fn register(&self, registrar: &mut FeatureRegistrar<'_>) {
        registrar.node(
            pass_node(
                "transparent",
                "Transparency: Blend",
                "Passes: Post",
                "Alpha-blended meshes over the scene colour.",
                |build| {
                    let target = color::current(build.env);
                    let alias = color::alias(build.env);
                    let media = media::current(build.env);
                    let pass = TransparentPass::new(build.env.device, build.env.scene_format());
                    let pass = if build.env.setting::<color::ReactiveMask>().is_some() {
                        pass.with_reactive_mask()
                    } else {
                        pass
                    };
                    let pass = pass.with_color_target(target, alias).with_media(media);
                    build.graph.add_pass(Box::new(pass));
                    Ok(())
                },
            )
            .drawer(Phase::Transparent, 0),
        );
    }
}
