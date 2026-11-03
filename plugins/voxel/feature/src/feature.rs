//! Voxel terrain as a feature: the node that adds the chunk pass to preset
//! graphs.
//!
//! The terrain is a drawer of the G-buffer phase, after the other drawers:
//! it writes the G-buffer over theirs and traces GI's probe rays (made before
//! the phase). It draws the volumes the scene publishes
//! ([`crate::engine::VOXEL_FRAME`]) and offers its ground to foliage
//! placement ([`helio_foliage_api::OfferedGrounds`]).

use helio_feature_api::{Feature, FeatureRegistrar, NodeDescriptor, ParamDesc};
use helio_frame_api::{FrameNode, Phase};

use crate::chunks::{pass::ChunkPass, ResidencyConfig};

/// The voxel feature's settings (a renderer config's `features`): with
/// them, graphs draw the voxel volumes the scene publishes; without them
/// the voxel node builds nothing.
#[derive(Clone, Debug, Default)]
pub struct VoxelSettings {
    /// How chunks stream (budgets, cache; the pixel angle follows the
    /// camera).
    pub residency: ResidencyConfig,
}

/// Voxel worlds ([`crate`]).
pub struct VoxelFeature;

impl Feature for VoxelFeature {
    fn id(&self) -> &'static str {
        "helio.voxel"
    }

    fn register(&self, registrar: &mut FeatureRegistrar<'_>) {
        // With terrain to draw, it traces GI's probes every frame and its sun
        // samples rotate over blocks of pixels for TSR to integrate.
        registrar.prepare(std::sync::Arc::new(|env| {
            if env.setting::<VoxelSettings>().is_some() {
                env.insert_setting(helio_gi_api::TerrainTracer);
                env.insert_setting(helio_pass_tsr::StochasticSamples);
            }
        }));
        registrar.node(
            NodeDescriptor::new(
                "helio.pass.voxel_chunks",
                "Voxel: Chunk Terrain",
                "Passes: Geometry",
                "The voxel volumes the scene publishes: residency, generation, the G-buffer \
                 and the GI probes' rays. Builds nothing without VoxelSettings.",
                |build| {
                    let Some(settings) = build.env.setting::<VoxelSettings>().cloned() else {
                        return Ok(());
                    };
                    let pass = ChunkPass::new(settings.residency);
                    build
                        .env
                        .shared(helio_foliage_api::OfferedGrounds::default)
                        .0
                        .push(pass.foliage_ground());
                    build.graph.add_pass(Box::new(pass));
                    Ok(())
                },
            )
            .with_param(ParamDesc::boolean("enabled", "Enabled", true))
            .with_generated_pins()
            .drawer(Phase::GBuffer, 40),
        );
    }
}
