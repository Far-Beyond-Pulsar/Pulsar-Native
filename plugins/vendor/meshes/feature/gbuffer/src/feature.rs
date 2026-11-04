//! The meshes' G-buffer drawer: it opens the frame's G-buffer phase (clears
//! the layout, `helio_frame_api::gbuffer`) and draws the culled opaque
//! batches into it.

use helio_feature_api::{pass_node, Feature, FeatureRegistrar};
use helio_frame_api::{FrameNode, Phase};

use crate::GBufferPass;

/// Meshes in the G-buffer ([`crate`]).
pub struct GBufferDrawers;

impl Feature for GBufferDrawers {
    fn id(&self) -> &'static str {
        "helio.mesh.gbuffer"
    }

    fn register(&self, registrar: &mut FeatureRegistrar<'_>) {
        registrar.node(
            pass_node(
                "gbuffer",
                "Geometry: GBuffer",
                "Passes: Geometry",
                "Opaque meshes into the GBuffer and the scene depth; opens the G-buffer phase.",
                |build| {
                    let mut pass = GBufferPass::new(build.env.device);
                    if let Some(templates) = build.env.setting::<helio_mats::MaterialTemplates>() {
                        pass = pass.with_template_registry(templates.opaque.clone());
                    }
                    build.graph.add_pass(Box::new(pass));
                    Ok(())
                },
            )
            .opens(Phase::GBuffer, 0),
        );
    }
}
