//! GPU cable drawing feature.
//!
//! The feature draws the frame's cable spans (`cables_api::CABLE_SPANS`, the
//! connectors the host paired by ID) as ribbons and tubes, looking up each
//! cable's appearance in the packed definition rows. Vertex generation is
//! GPU work; pairing is the cable producer's.

use helio_feature_api::{pass_node, Feature, FeatureRegistrar};
use helio_frame_api::{FrameNode, Phase};

mod render;
pub use render::CableRenderPass;

/// Cable rendering and simulation feature.
pub struct CablesFeature;

impl Feature for CablesFeature {
    fn id(&self) -> &'static str {
        "pulsar.cables.render"
    }

    fn register(&self, registrar: &mut FeatureRegistrar<'_>) {
        registrar.consumes(cables_api::CABLE_DEFINITIONS);
        registrar.node(helio_frame_api::host_input("cable_definitions"));
        registrar.node(
            pass_node(
                "cables",
                "Overlay: Cables",
                "Passes: Overlays",
                "GPU-driven cable strands.",
                |build| {
                    build.graph.add_pass(Box::new(CableRenderPass::new(
                        build.env.device,
                        helio_mats::MaterialBindingConfig::for_device(build.env.device),
                        build.env.scene_format(),
                    )));
                    Ok(())
                },
            )
            .with_marker_after()
            .drawer(Phase::Overlay, 20),
        );
    }
}
