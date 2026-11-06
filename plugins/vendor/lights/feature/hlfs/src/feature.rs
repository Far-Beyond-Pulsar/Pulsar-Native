//! HLFS as a feature: hybrid light-field shading, the lighting model of the
//! light-field presets (`helio_frame_api::lighting::LightingModel::LightField`).
//! It asks for its scene colour format when a graph uses it.

use std::sync::Arc;

use helio_feature_api::{pass_node, Feature, FeatureRegistrar, SceneColorFormat};
use helio_frame_api::lighting::{lighting_model, LightingModel};

use crate::HlfsPass;

/// Hybrid light-field shading ([`crate`]).
pub struct HlfsFeature;

impl Feature for HlfsFeature {
    fn id(&self) -> &'static str {
        "helio.hlfs"
    }

    fn register(&self, registrar: &mut FeatureRegistrar<'_>) {
        // What shader hot reload may swap without rebuilding the graph.
        registrar.swappable::<HlfsPass>();
        registrar.prepare(Arc::new(|env| {
            if lighting_model(env) == Some(LightingModel::LightField) {
                let format = HlfsPass::preferred_output_format(env.device);
                env.insert_setting(SceneColorFormat(format));
            }
        }));
        let point = LightingModel::LightField
            .point()
            .expect("light-field shading has a point");
        registrar.node(
            pass_node(
                "hlfs",
                "Lighting: HLFS",
                "Passes: Lighting",
                "Hybrid light-field shading of the GBuffer: the lighting of the HLFS pipelines.",
                |build| {
                    let (iw, ih) = build.env.internal_size();
                    let queue = build.env.queue;
                    let quality = build
                        .env
                        .setting::<helio_light_api::ShadowSettings>()
                        .copied()
                        .unwrap_or_default()
                        .quality;
                    let mut pass =
                        HlfsPass::new(build.env.device, queue, iw, ih, build.env.scene_format());
                    pass.set_shadow_quality(quality, queue);
                    build.graph.add_pass(Box::new(pass));
                    Ok(())
                },
            )
            .at(point, 0),
        );
    }
}
