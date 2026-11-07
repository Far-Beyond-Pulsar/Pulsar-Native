//! Light binning as a feature: the scene's lights binned into 16 px screen
//! tiles once the depth pyramid is built, for what lights per pixel from a
//! tile's list (deferred lighting, the forward function's drawers,
//! transparency, a terrain's local shadows). Light-field shading bins its
//! own lights, so under it this builds nothing.

use helio_feature_api::{pass_node, Feature, FeatureRegistrar};
use helio_frame_api::lighting::{lighting_model, LightingModel};
use helio_frame_api::{FrameNode, Slot};

use crate::LightCullPass;

/// Tiled light binning ([`crate`]).
pub struct LightCullFeature;

impl Feature for LightCullFeature {
    fn id(&self) -> &'static str {
        "helio.lights.binning"
    }

    fn register(&self, registrar: &mut FeatureRegistrar<'_>) {
        // What shader hot reload may swap without rebuilding the graph.
        registrar.swappable::<LightCullPass>();
        // The directional light of a light list a host wrote itself (the
        // lights' join records its own): what the sky, GI and media read.
        registrar.scene_derivation(crate::DirectionalLightsDerivation::FACTORY);
        registrar.node(
            pass_node(
                "light_cull",
                "Lighting: Tile Light Binning",
                "Passes: Scene Prep",
                "Bins the scene's lights into screen tiles.",
                |build| {
                    if lighting_model(build.env) == Some(LightingModel::LightField) {
                        return Ok(());
                    }
                    let (iw, ih) = build.env.internal_size();
                    let pass = LightCullPass::new(build.env.device, iw, ih);
                    build.graph.add_pass(Box::new(pass));
                    Ok(())
                },
            )
            // After a kind's culling against the pyramid.
            .in_slot(Slot::Visibility, 100),
        );
    }
}
