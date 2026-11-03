//! Portal culling as a feature: the node the portal draws read their buffers
//! from (`helio_pass_portal_instances::PortalFeature`), run before the
//! G-buffer phase they draw in.

use helio_feature_api::{pass_node, BuildEnv, Feature, FeatureRegistrar};
use helio_frame_api::{FrameNode, Phase};

use crate::PortalCullPass;

/// Portals' settings (a renderer config's `features`).
#[derive(Clone, Debug)]
pub struct PortalSettings {
    /// Off: no portal pass builds (a level without portals pays nothing).
    pub enabled: bool,
}

impl Default for PortalSettings {
    fn default() -> Self {
        Self { enabled: true }
    }
}

/// Whether the graph being built draws portals.
pub fn portals_enabled(env: &BuildEnv<'_>) -> bool {
    env.setting::<PortalSettings>()
        .is_none_or(|settings| settings.enabled)
}

/// Portal culling ([`crate`]).
pub struct PortalCullFeature;

impl Feature for PortalCullFeature {
    fn id(&self) -> &'static str {
        "helio.portal_cull"
    }

    fn register(&self, registrar: &mut FeatureRegistrar<'_>) {
        registrar.node(
            pass_node(
                "portal_cull",
                "Portals: Cull",
                "Passes: Scene Prep",
                "Culls portals and compacts the ones to draw. Portal Instance reads its buffers.",
                |build| {
                    if portals_enabled(build.env) {
                        let pass = PortalCullPass::new(build.env.device);
                        build.graph.add_pass(Box::new(pass));
                    }
                    Ok(())
                },
            )
            .with_marker_after()
            .before_phase(Phase::GBuffer, -10),
        );
    }
}
