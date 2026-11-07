//! The meshes' object batch as a feature: first in the frame's prepare slot,
//! so every pass after it (the shadow views' casters, culling, the drawers)
//! reads this frame's batches.

use helio_feature_api::{pass_node, Feature, FeatureRegistrar};
use helio_frame_api::{FrameNode, Slot};

use crate::ObjectBatchPass;

/// The object batch node ([`crate`]).
pub struct ObjectBatchFeature;

impl Feature for ObjectBatchFeature {
    fn id(&self) -> &'static str {
        "helio.mesh.batch"
    }

    fn register(&self, registrar: &mut FeatureRegistrar<'_>) {
        // What shader hot reload may swap without rebuilding the graph.
        registrar.swappable::<ObjectBatchPass>();
        registrar.node(
            pass_node(
                "object_batch",
                "Scene: Object Batches",
                "Passes: Scene Prep",
                "Gathers the scene's objects into GPU-driven batches every other pass reads.",
                |build| {
                    let pass = ObjectBatchPass::new(build.env.device);
                    build.graph.add_pass(Box::new(pass));
                    Ok(())
                },
            )
            .in_slot(Slot::Prepare, -100),
        );
    }
}
