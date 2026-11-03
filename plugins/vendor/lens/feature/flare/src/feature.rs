//! Lens flares as a feature: the node that flares the lens image's bright
//! lights.

use helio_feature_api::{pass_node, Feature, FeatureRegistrar};
use helio_frame_api::{color, FrameNode, Slot};

use crate::LensFlarePass;

/// Lens flares ([`crate`]).
pub struct LensFlareFeature;

impl Feature for LensFlareFeature {
    fn id(&self) -> &'static str {
        "helio.lens_flare"
    }

    fn register(&self, registrar: &mut FeatureRegistrar<'_>) {
        registrar.node(
            pass_node(
                "lens_flare",
                "Lens: Flares",
                "Passes: Post",
                "Lens flares from the scene's bright lights.",
                |build| {
                    let (iw, ih) = build.env.internal_size();
                    let input = color::current(build.env);
                    let pass =
                        LensFlarePass::new_hdr(build.env.device, iw, ih).with_color_input(input);
                    build.graph.add_pass(Box::new(pass));
                    Ok(())
                },
            )
            .in_slot(Slot::Resolved, 10),
        );
    }
}
