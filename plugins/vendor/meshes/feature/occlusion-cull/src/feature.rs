//! The meshes' culling as a feature, around the spine's depth pyramid in the
//! frame's visibility slot: the dispatch arguments before it, the frustum
//! and occlusion test against it after.

use std::sync::Arc;

use helio_feature_api::{missing, pass_node, Feature, FeatureRegistrar};
use helio_frame_api::{FrameNode, Slot};
use helio_pass_hiz::HiZBuildPass;
use helio_pass_indirect_dispatch::IndirectDispatchPass;

use crate::OcclusionCullPass;

/// The meshes' culling nodes ([`crate`]).
pub struct MeshCulling;

impl Feature for MeshCulling {
    fn id(&self) -> &'static str {
        "helio.mesh.cull"
    }

    fn register(&self, registrar: &mut FeatureRegistrar<'_>) {
        const CATEGORY: &str = "Passes: Scene Prep";

        registrar.node(
            pass_node(
                "indirect_dispatch",
                "Culling: Prepare Dispatch",
                CATEGORY,
                "Prepares the indirect dispatch arguments the culling passes use.",
                |build| {
                    let pass = IndirectDispatchPass::new(
                        build.env.device,
                        build.env.cull_stats_buffer.clone(),
                    );
                    build.graph.add_pass(Box::new(pass));
                    Ok(())
                },
            )
            .in_slot(Slot::Visibility, -10),
        );

        registrar.node(
            pass_node(
                "occlusion_cull",
                "Culling: Frustum + Occlusion",
                CATEGORY,
                "Frustum and Hi-Z occlusion culling of the object batch.",
                |build| {
                    let (iw, ih) = build.env.internal_size();
                    let sampler = build
                        .graph
                        .find_pass::<HiZBuildPass>()
                        .map(|hiz| Arc::clone(&hiz.hiz_sampler))
                        .ok_or_else(|| missing("Occlusion Cull", "Hi-Z Build"))?;
                    let pass = OcclusionCullPass::new(
                        build.env.device,
                        sampler,
                        iw,
                        ih,
                        build.env.cull_stats_buffer.clone(),
                    );
                    build.graph.add_pass(Box::new(pass));
                    Ok(())
                },
            )
            .in_slot(Slot::Visibility, 10),
        );
    }
}
