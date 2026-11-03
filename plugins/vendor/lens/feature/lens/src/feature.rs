//! The camera lens as a feature: its elements (water drops, iris shapes,
//! user lenses) over the resolved scene colour, at the frame's resolved
//! slot. It reads the colour chain's current colour and advances it to
//! [`crate::LENS_COLOR`].

use helio_feature_api::{pass_node, Feature, FeatureRegistrar};
use helio_frame_api::{color, FrameNode, Slot};

use crate::{LensPass, LensUser, LENS_COLOR};

/// The camera lens ([`crate`]).
pub struct LensFeature;

impl Feature for LensFeature {
    fn id(&self) -> &'static str {
        "helio.lens"
    }

    fn register(&self, registrar: &mut FeatureRegistrar<'_>) {
        registrar.node(
            pass_node(
                "lens",
                "Lens: Elements",
                "Passes: Post",
                "Lens elements: water drops, iris shapes and user lenses.",
                |build| {
                    let (iw, ih) = build.env.internal_size();
                    let input = color::current(build.env);
                    let user = build.env.setting::<LensUser>().map(|user| user.0);
                    let pass = LensPass::new_with_user(build.env.device, iw, ih, user)
                        .with_color_input(input)
                        .with_color_alias(color::alias(build.env))
                        .with_bundled_resources();
                    build.graph.add_pass(Box::new(pass));
                    color::advance(build.env, LENS_COLOR);
                    Ok(())
                },
            )
            .in_slot(Slot::Resolved, 0),
        );
    }
}
