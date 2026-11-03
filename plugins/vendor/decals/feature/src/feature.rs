//! Decals as a feature: the node that projects them into the G-buffer.

use helio_feature_api::{pass_node, Feature, FeatureRegistrar};
use helio_frame_api::{FrameNode, Phase};

use crate::DecalPass;

/// Projected decals ([`crate`]).
pub struct DecalFeature;

impl Feature for DecalFeature {
    fn id(&self) -> &'static str {
        "helio.decals"
    }

    fn register(&self, registrar: &mut FeatureRegistrar<'_>) {
        registrar.node(
            pass_node(
                "decal",
                "Geometry: Decals",
                "Passes: Lighting",
                "Projects decals into the GBuffer, after every surface is in it.",
                |build| {
                    let (iw, ih) = build.env.internal_size();
                    let pass = DecalPass::new(
                        build.env.device,
                        build.env.queue,
                        build.env.camera_buffer,
                        iw,
                        ih,
                    );
                    build.graph.add_pass(Box::new(pass));
                    Ok(())
                },
            )
            .drawer(Phase::GBuffer, 100),
        );
    }
}
