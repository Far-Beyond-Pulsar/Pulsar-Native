//! Virtual geometry as a feature: the node that adds the chain at the end of
//! the classic geometry.

use helio_feature_api::{pass_node, Feature, FeatureRegistrar};
use helio_frame_api::{FrameNode, Phase};
use helio_vg_stream::VgSettings;

use crate::add_virtual_geometry_passes;

/// Virtual geometry ([`crate`]). Its runtime is the [`VgSettings`] setting.
pub struct VgFeature;

impl Feature for VgFeature {
    fn id(&self) -> &'static str {
        "helio.vg"
    }

    fn register(&self, registrar: &mut FeatureRegistrar<'_>) {
        // Last of the classic geometry: its resolve depth-tests against the
        // combined classic depth (portals and foliage included). Builds nothing
        // unless the host set up virtual geometry and the device supports it.
        registrar.node(
            pass_node(
                "virtual_geometry",
                "Geometry: Virtual Geometry",
                "Passes: Geometry",
                "Streams, culls, rasterises and shades virtual-geometry clusters into the GBuffer.",
                |build| {
                    let settings = build
                        .env
                        .setting::<VgSettings>()
                        .cloned()
                        .unwrap_or_default();
                    add_virtual_geometry_passes(build.graph, build.env.device, &settings);
                    Ok(())
                },
            )
            .with_marker_after()
            .drawer(Phase::GBuffer, 30),
        );
    }
}
