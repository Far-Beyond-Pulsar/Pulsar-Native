//! Billboards as a feature: camera-facing sprites over the lit scene, with the
//! editor's light icon as the default sprite.

use helio_feature_api::{pass_node, Feature, FeatureRegistrar};
use helio_frame_api::{host_input, FrameNode, Phase};

use crate::BillboardPass;

/// The editor's light icon: the sprite a billboard shows by default.
static SPOTLIGHT_PNG: &[u8] = include_bytes!("../spotlight.png");

/// Source dimensions used to fit the editor icon, shared with viewport picking.
pub fn light_icon_dimensions() -> [f32; 2] {
    static DIMENSIONS: std::sync::OnceLock<[f32; 2]> = std::sync::OnceLock::new();
    *DIMENSIONS.get_or_init(|| {
        image::load_from_memory(SPOTLIGHT_PNG)
            .map(|image| [image.width() as f32, image.height() as f32])
            .unwrap_or([1.0, 1.0])
    })
}

/// Camera-facing sprites ([`crate`]).
pub struct BillboardFeature;

impl Feature for BillboardFeature {
    fn id(&self) -> &'static str {
        "helio.billboards"
    }

    fn register(&self, registrar: &mut FeatureRegistrar<'_>) {
        // What shader hot reload may swap without rebuilding the graph.
        registrar.swappable::<BillboardPass>();
        // The host supplies the rows every frame.
        registrar.node(host_input("billboards"));
        registrar.node(
            pass_node(
                "billboard",
                "Overlay: Billboards",
                "Passes: Overlays",
                "Camera-facing sprites, depth-tested against the scene.",
                |build| {
                    let spotlight = image::load_from_memory(SPOTLIGHT_PNG)
                        .unwrap_or_else(|_| image::DynamicImage::new_rgba8(1, 1))
                        .into_rgba8();
                    let (width, height) = spotlight.dimensions();
                    let mut pass = BillboardPass::new_with_sprite_rgba(
                        build.env.device,
                        build.env.queue,
                        build.env.camera_buffer,
                        build.env.scene_format(),
                        spotlight.as_raw(),
                        width,
                        height,
                    );
                    pass.set_occluded_by_geometry(true);
                    build.graph.add_pass(Box::new(pass));
                    Ok(())
                },
            )
            .with_marker_after()
            .drawer(Phase::Overlay, 50),
        );
    }
}
