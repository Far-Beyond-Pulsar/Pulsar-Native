//! The sky as a feature: the nodes it adds to preset graphs.
//!
//! The atmosphere's tables run at [`Slot::Prepare`], publishing the clouds'
//! shadow as the frame's sun transmittance for what lights points under
//! them (fog, GI); the clouds' trace joins at [`Slot::LightingInputs`], once
//! the depth is complete, publishing the sun's visibility under the clouds; the composite runs over the lit scene
//! at [`Slot::Lighting`] (before the overlays, which are not seen through
//! the air).

use helio_feature_api::{Feature, FeatureRegistrar, NodeDescriptor, ParamDesc};
use helio_frame_api::lighting::sun_transmittance;
use helio_frame_api::{FrameNode, Slot};

use crate::{AtmosphereCompositePass, AtmospherePass, CloudTracePass};

/// Atmosphere and clouds ([`crate`]).
pub struct SkyFeature;

impl Feature for SkyFeature {
    fn id(&self) -> &'static str {
        "helio.sky"
    }

    fn register(&self, registrar: &mut FeatureRegistrar<'_>) {
        registrar.node(
            NodeDescriptor::new(
                "helio.pass.atmosphere",
                "Sky: Atmosphere Tables",
                "Passes: Scene Prep",
                "Atmosphere lookup tables and the resolved sky frame. Idle without an atmosphere component.",
                |build| {
                    build
                        .graph
                        .add_pass(Box::new(AtmospherePass::new(build.env.device)));
                    sun_transmittance::publish(build.env, crate::clouds::SUN_TRANSMITTANCE);
                    Ok(())
                },
            )
            .with_param(ParamDesc::boolean("enabled", "Enabled", true))
            .with_generated_pins()
            .in_slot(Slot::Prepare, 0),
        );
        registrar.node(
            NodeDescriptor::new(
                "helio.pass.cloud_trace",
                "Sky: Cloud Trace",
                "Passes: Lighting",
                "The clouds' sun visibility over the finished depth, for deferred lighting. Idle without clouds.",
                |build| {
                    build
                        .graph
                        .add_pass(Box::new(CloudTracePass::new(build.env.device)));
                    Ok(())
                },
            )
            .with_param(ParamDesc::boolean("enabled", "Enabled", true))
            .with_generated_pins()
            .in_slot(Slot::LightingInputs, 10),
        );
        registrar.node(
            NodeDescriptor::new(
                "helio.pass.atmosphere_composite",
                "Sky: Composite",
                "Passes: Overlays",
                "Composites the sky and aerial perspective over the lit scene.",
                |build| {
                    let pass =
                        AtmosphereCompositePass::new(build.env.device, build.env.scene_format());
                    build.graph.add_pass(Box::new(pass));
                    Ok(())
                },
            )
            .with_param(ParamDesc::boolean("enabled", "Enabled", true))
            .with_generated_pins()
            .in_slot(Slot::Lighting, 0),
        );
    }
}
