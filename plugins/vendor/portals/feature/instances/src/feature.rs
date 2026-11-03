//! Portal drawing as a feature: the footprint mask and the geometry seen
//! through portals (drawers of the G-buffer phase), and the editor's overlay
//! on their openings (a drawer of the overlay phase). Builds nothing while
//! portals are off (`helio_pass_portal_cull::PortalSettings`).

use std::sync::Arc;

use helio_feature_api::{missing, pass_node, Feature, FeatureRegistrar};
use helio_frame_api::{FrameNode, Phase};
use helio_pass_portal_cull::{portals_enabled, PortalCullPass};

use crate::{PortalEditorOverlayPass, PortalInstancePass, PortalMaskPass};

/// Portal drawing ([`crate`]); the cull is `helio_pass_portal_cull`'s.
pub struct PortalFeature;

impl Feature for PortalFeature {
    fn id(&self) -> &'static str {
        "helio.portals"
    }

    fn register(&self, registrar: &mut FeatureRegistrar<'_>) {
        const CATEGORY: &str = "Passes: Geometry";

        registrar.node(
            pass_node(
                "portal_mask",
                "Portals: Footprint Mask",
                CATEGORY,
                "Stamps each portal's visible footprint, respecting real occluders.",
                |build| {
                    if portals_enabled(build.env) {
                        let pass = PortalMaskPass::new(build.env.device);
                        build.graph.add_pass(Box::new(pass));
                    }
                    Ok(())
                },
            )
            .drawer(Phase::GBuffer, 10),
        );

        registrar.node(
            pass_node(
                "portal_instance",
                "Portals: Draw Through",
                CATEGORY,
                "Draws the geometry seen through portals. Needs Portal Cull before it.",
                |build| {
                    if !portals_enabled(build.env) {
                        return Ok(());
                    }
                    let (indirect, compacted, chains, segments) = build
                        .graph
                        .find_pass::<PortalCullPass>()
                        .map(|cull| {
                            (
                                Arc::clone(&cull.portal_indirect_buf),
                                Arc::clone(&cull.portal_compacted_indices_buf),
                                Arc::clone(&cull.portal_compacted_chains_buf),
                                Arc::clone(&cull.segments),
                            )
                        })
                        .ok_or_else(|| missing("Portal Instances", "Portal Cull"))?;
                    let pass = PortalInstancePass::new(
                        build.env.device,
                        indirect,
                        compacted,
                        chains,
                        segments,
                    );
                    build.graph.add_pass(Box::new(pass));
                    Ok(())
                },
            )
            .drawer(Phase::GBuffer, 11),
        );

        registrar.node(
            pass_node(
                "portal_editor_overlay",
                "Overlay: Portal Openings (Editor)",
                "Passes: Overlays",
                "Editor-only checkerboard over each portal's opening.",
                |build| {
                    if portals_enabled(build.env) {
                        let pass = PortalEditorOverlayPass::new(
                            build.env.device,
                            build.env.scene_format(),
                        );
                        build.graph.add_pass(Box::new(pass));
                    }
                    Ok(())
                },
            )
            .with_marker_after()
            .drawer(Phase::Overlay, 20),
        );
    }
}
