//! The lights' shadow views as a feature: before the frame's shadow-caster
//! phase, the view-projection of every shadow-casting light face, fitted
//! into the atlas budget ([`ShadowSettings`]), and the per-caster dirty
//! flags ([`LightDirtyFlags`]) what casts reads to find its stale faces.
//! The casters are the drawers' (a meshes plugin's shadow pass opens the
//! phase); the views are the lights'.

use helio_feature_api::{pass_node, BuildEnv, Feature, FeatureRegistrar};
use helio_frame_api::{FrameNode, Phase};

use crate::{
    GpuShadowMatrix, LightDirtyFlags, ShadowMatrixPass, ShadowSettings, MAX_SHADOW_CASTERS,
    MAX_SHADOW_FACES,
};

/// The lights' shadow views ([`crate`]).
pub struct ShadowViewsFeature;

/// The shadow settings this graph is built with.
fn shadow_settings(env: &BuildEnv<'_>) -> ShadowSettings {
    env.setting::<ShadowSettings>().copied().unwrap_or_default()
}

/// SceneDB's buffer under `key`, or an empty stand-in of `fallback_size`
/// bytes while the scene has none.
fn scene_buffer_or_dummy(
    scene_db: &pulsar_scenedb::gpu::GpuMirrorHandle,
    device: &wgpu::Device,
    key: pulsar_scenedb::gpu::BufferKey,
    label: &str,
    fallback_size: u64,
) -> pulsar_scenedb::gpu::BufferHandle {
    scene_db
        .store()
        .resolve_buffer_handle(key)
        .unwrap_or_else(|| pulsar_scenedb::gpu::BufferHandle {
            buffer: device.create_buffer(&wgpu::BufferDescriptor {
                label: Some(label),
                size: fallback_size.max(4),
                usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            }),
            epoch: 0,
            row_bytes: 0,
            content_generation: 0,
        })
}

impl Feature for ShadowViewsFeature {
    fn id(&self) -> &'static str {
        "helio.lights.shadow_views"
    }

    fn register(&self, registrar: &mut FeatureRegistrar<'_>) {
        registrar.node(
            pass_node(
                "shadow_matrix",
                "Shadows: Light Matrices",
                "Passes: Scene Prep",
                "Computes the view-projection of every shadow-casting light face.",
                |build| {
                    let device = build.env.device;
                    let (budget, atlas_size) = shadow_settings(build.env)
                        .atlas(device.limits().max_texture_dimension_2d)?;
                    let scene_db = build
                        .env
                        .require::<pulsar_scenedb::gpu::GpuMirrorHandle>()?
                        .clone();
                    let lights = scene_buffer_or_dummy(
                        &scene_db,
                        device,
                        pulsar_scenedb::gpu::BufferKey::of(helio_light_api::LIGHT_LIST),
                        "SceneDB Lights",
                        96,
                    );
                    let matrices = device.create_buffer(&wgpu::BufferDescriptor {
                        label: Some("Shadow Matrices"),
                        size: MAX_SHADOW_FACES as u64
                            * std::mem::size_of::<GpuShadowMatrix>() as u64,
                        usage: wgpu::BufferUsages::STORAGE
                            | wgpu::BufferUsages::COPY_DST
                            | wgpu::BufferUsages::COPY_SRC,
                        mapped_at_creation: false,
                    });
                    let dirty = LightDirtyFlags::new(device);
                    let hashes = device.create_buffer(&wgpu::BufferDescriptor {
                        label: Some("Shadow Hashes"),
                        size: MAX_SHADOW_CASTERS as u64 * 4,
                        usage: wgpu::BufferUsages::STORAGE,
                        mapped_at_creation: false,
                    });
                    let (_, ih) = build.env.internal_size();
                    build.graph.add_pass(Box::new(
                        ShadowMatrixPass::new(
                            device,
                            &lights.buffer,
                            &matrices,
                            build.env.camera_buffer,
                            &dirty.0,
                            &hashes,
                            atlas_size,
                        )
                        .with_budget(budget, ih),
                    ));
                    build.env.shared(|| dirty);
                    Ok(())
                },
            )
            // First of the phase's compute: what casts reads the views'
            // dirty flags.
            .before_phase(Phase::ShadowCasters, -100),
        );
    }
}
