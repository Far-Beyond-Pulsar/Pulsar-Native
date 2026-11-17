//! GPU-owned cable pairing, strand state, and drawing feature.
//!
//! The feature consumes SceneDB's placed connector rows and authored cable
//! definition rows. Pair expansion, strand simulation and vertex generation
//! are GPU work; host code reads buffer capacity only and never pairs rows.

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
        registrar.consumes(cables_api::CABLE_CONNECTORS);
        registrar.node(helio_frame_api::host_input("cable_definitions"));
        registrar.node(helio_frame_api::host_input("cable_connectors"));
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
