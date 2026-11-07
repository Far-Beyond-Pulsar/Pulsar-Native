//! The meshes' forward drawer: lit opaque meshes straight into the scene
//! colour. It opens the forward phase of graphs without a G-buffer phase,
//! where it draws every opaque mesh; it builds nothing in graphs that have
//! one.

use helio_feature_api::{pass_node, Feature, FeatureRegistrar};
use helio_frame_api::{phase_settings, FrameNode, Phase};

use crate::ForwardLitPass;

/// Meshes lit in one forward pass ([`crate`]).
pub struct ForwardDrawers;

impl Feature for ForwardDrawers {
    fn id(&self) -> &'static str {
        "helio.mesh.forward"
    }

    fn register(&self, registrar: &mut FeatureRegistrar<'_>) {
        registrar.node(
            pass_node(
                "forward_lit",
                "Geometry: Forward Lit",
                "Passes: Geometry",
                "Lit opaque meshes in one forward pass, for pipelines without a GBuffer.",
                |build| {
                    if phase_settings(build.env).runs(Phase::GBuffer) {
                        return Ok(());
                    }
                    let mut pass = ForwardLitPass::new(build.env.device, build.env.scene_format());
                    pass.render_all_opaque = true;
                    build.graph.add_pass(Box::new(pass));
                    Ok(())
                },
            )
            .with_marker_after()
            .opens(Phase::Forward, 0),
        );
    }
}
