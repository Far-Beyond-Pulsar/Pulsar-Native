//! Water as a feature: the light it focuses under itself, a lighting input
//! (the frame's `transmission`), and its simulation and surface, a drawer of
//! the overlay phase (over the lit scene, before media).

use helio_feature_api::{pass_node, Feature, FeatureRegistrar};
use helio_frame_api::lighting::{inputs_order, InputsOrder};
use helio_frame_api::{FrameNode, Phase, Slot};

use crate::transmission::WaterTransmissionPass;
use crate::WaterSimPass;

/// Water ([`crate`]).
pub struct WaterFeature;

impl Feature for WaterFeature {
    fn id(&self) -> &'static str {
        "helio.water"
    }

    fn register(&self, registrar: &mut FeatureRegistrar<'_>) {
        // What shader hot reload may swap without rebuilding the graph.
        registrar.swappable::<WaterSimPass>();
        registrar.swappable::<WaterTransmissionPass>();
        registrar.node(
            pass_node(
                "water_transmission",
                "Water: Caustics",
                "Passes: Lighting",
                "The light water focuses onto what lies under it, which lighting adds.                  Builds nothing where the lighting ran before it (forward).",
                |build| {
                    if inputs_order(build.env) == InputsOrder::BeforeLighting {
                        let pass = WaterTransmissionPass::new(build.env.device);
                        build.graph.add_pass(Box::new(pass));
                    }
                    Ok(())
                },
            )
            .in_slot(Slot::LightingInputs, 20),
        );
        registrar.node(
            pass_node(
                "water_sim",
                "Water: Simulate + Draw",
                "Passes: Overlays",
                "Simulates and draws water volumes.",
                |build| {
                    let (iw, ih) = build.env.internal_size();
                    let pass = WaterSimPass::new(
                        build.env.device,
                        build.env.camera_buffer,
                        iw,
                        ih,
                        build.env.scene_format(),
                    );
                    build.graph.add_pass(Box::new(pass));
                    Ok(())
                },
            )
            .with_marker_after()
            .drawer(Phase::Overlay, 30),
        );
    }
}
