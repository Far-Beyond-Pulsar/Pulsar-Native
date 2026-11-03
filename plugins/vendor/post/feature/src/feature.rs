//! Post-process volumes as a feature: the blend runs at the frame's media
//! slot, ahead of the media and the tone map that read the post settings it
//! publishes.

use helio_feature_api::{pass_node, Feature, FeatureRegistrar};
use helio_frame_api::{FrameNode, Slot};

use crate::PostProcessVolumeBlendPass;

/// Post-process volumes ([`crate`]).
pub struct PostVolumesFeature;

impl Feature for PostVolumesFeature {
    fn id(&self) -> &'static str {
        "helio.post_volumes"
    }

    fn register(&self, registrar: &mut FeatureRegistrar<'_>) {
        registrar.consumes(crate::POST_PROCESS_VOLUMES);
        registrar.consumes(crate::CAMERA_POSTPROCESS);
        registrar.node(
            pass_node(
                "post_process_volume_blend",
                "Post: Blend Volumes",
                "Passes: Post",
                "Blends the post-process volumes around the camera into the frame's post settings.",
                |build| {
                    let pass = PostProcessVolumeBlendPass::new(build.env.device);
                    build.graph.add_pass(Box::new(pass));
                    Ok(())
                },
            )
            .in_slot(Slot::Media, -10),
        );
    }
}
