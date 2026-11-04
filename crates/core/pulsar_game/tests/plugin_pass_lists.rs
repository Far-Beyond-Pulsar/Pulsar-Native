//! The engine's pipelines carry every linked plugin's features: Helio's
//! presets built the way the engine builds them (its plugins' features
//! added, their settings applied) list the same passes as before the leaf
//! kinds (batch 1), the media and placement kinds (batch 2: fog,
//! post-process volumes, water, foliage), the lighting kinds (batch 3:
//! GI, the sky) and the meshes (batch 4) left Helio. Dumps every preset's pass declarations for four
//! configs into `PRESET_SNAPSHOT_OUT` (the format of Helio's preset
//! snapshot), to diff against a snapshot of the presets from before the
//! move; skips without the variable. Also checks, always, that the plugins'
//! passes are in the default preset.
use std::fmt::Write as _;
use std::sync::{Arc, Mutex};

use engine_backend::subsystems::render::helio_renderer::render_features;
use helio::{DebugDrawState, PassBuildContext, RendererConfig};
use helio_core::GpuCameraUniforms;
use helio_presets::Preset;
use pulsar_scenedb::gpu::{EngineGpuContext, GpuMirrorHandle, SceneGpuConfig, SceneGpuStore};

// Linked for their registrations, as the game links them.
use decals_component as _;
use fog_component as _;
use foliage_component as _;
use gi_component as _;
use lens_component as _;
use meshes_component as _;
use particles_component as _;
use portals_component as _;
use post_component as _;
use sky_component as _;
use splines_component as _;
use sprites_component as _;
use water_component as _;

fn device() -> Option<(Arc<wgpu::Device>, Arc<wgpu::Queue>)> {
    pollster::block_on(async {
        let instance = wgpu::Instance::new(helio_core::wgpu_instance_descriptor());
        let adapter = instance.request_adapter(&Default::default()).await.ok()?;
        let (device, queue) = adapter
            .request_device(&wgpu::DeviceDescriptor {
                required_features: helio::required_wgpu_features(adapter.features()),
                required_limits: helio::required_wgpu_limits(adapter.limits()),
                experimental_features: helio::required_experimental_features(adapter.features()),
                ..Default::default()
            })
            .await
            .ok()?;
        Some((Arc::new(device), Arc::new(queue)))
    })
}

fn mirror(device: &Arc<wgpu::Device>, queue: &Arc<wgpu::Queue>) -> GpuMirrorHandle {
    let gpu_context = EngineGpuContext::new(Arc::clone(device), Arc::clone(queue));
    let gpu_store = SceneGpuStore::new(
        &gpu_context,
        SceneGpuConfig {
            classes: Vec::new(),
            tombstone_headroom: 0,
            max_cells_metadata: 0,
        },
    );
    GpuMirrorHandle::new(Arc::new(gpu_store), Arc::clone(queue))
}

fn uniform_buffer(device: &wgpu::Device, label: &str, size: u64) -> wgpu::Buffer {
    device.create_buffer(&wgpu::BufferDescriptor {
        label: Some(label),
        size,
        usage: wgpu::BufferUsages::STORAGE
            | wgpu::BufferUsages::UNIFORM
            | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    })
}

/// The snapshot's configs, with the plugins' settings applied as the
/// engine applies them (`render_features::configure`).
fn configs() -> Vec<(&'static str, RendererConfig)> {
    let base = render_features::configure(RendererConfig::new(
        320,
        180,
        wgpu::TextureFormat::Rgba8Unorm,
    ));
    let tsr = base.clone().with_tsr_quality(helio::TsrQuality::Quality);
    let mut reflections = base.clone();
    reflections.enable_ssr = true;
    reflections.enable_planar_reflections = true;
    let mut lean = base
        .clone()
        .with_feature(helio_pass_portal_cull::PortalSettings { enabled: false });
    lean.features
        .edit::<helio_pass_foliage_place::FoliageSettings>()
        .enabled = false;
    vec![
        ("default", base),
        ("tsr", tsr),
        ("reflections", reflections),
        ("no_portals_foliage", lean),
    ]
}

#[test]
fn the_engines_presets_carry_the_plugins_passes() {
    let Some((device, queue)) = device() else {
        eprintln!("GPU_VALIDATION_SKIPPED_NO_ADAPTER: plugin pass lists");
        return;
    };
    let camera = uniform_buffer(
        &device,
        "Scene Camera Buffer",
        (std::mem::size_of::<GpuCameraUniforms>() * 2) as u64,
    );
    let debug_camera = uniform_buffer(&device, "Debug Camera Buffer", 64);
    let cull_stats = uniform_buffer(&device, "Cull Stats Buffer", 64);
    let scene_db = mirror(&device, &queue);
    let features = render_features::plugin_features();
    let mut text = String::new();
    for (config_name, config) in configs() {
        for preset in Preset::ALL {
            let graph = preset.with_features(&features).build(PassBuildContext {
                device: &device,
                queue: &queue,
                config: config.clone(),
                debug_state: Arc::new(Mutex::new(DebugDrawState::default())),
                camera_buffer: &camera,
                debug_camera_buffer: &debug_camera,
                cull_stats_buffer: &cull_stats,
                owns_device: false,
                scene_db: scene_db.clone(),
            });
            let passes: Vec<_> = graph.build_order_declarations();
            if matches!(preset, Preset::Deferred) && config_name == "default" {
                for name in [
                    "Corona",
                    "SpriteCull",
                    "SpriteBatch",
                    "Lens",
                    "LensFlare",
                    "DecalApply",
                    "PortalCull",
                    "PortalMask",
                    "PortalInstance",
                    "PortalEditorOverlay",
                    "VolumetricFogPass",
                    "FogComposite",
                    "PostProcessVolumeBlendPass",
                    "WaterSim",
                    "WaterTransmission",
                    "FoliagePlace",
                    "FoliageGBuffer",
                    "Atmosphere",
                    "CloudTrace",
                    "AtmosphereComposite",
                    "ObjectBatch",
                    "ShadowDirty",
                    "Shadow",
                    "IndirectDispatch",
                    "OcclusionCull",
                    "GBuffer",
                    "TransparentPass",
                ] {
                    assert!(
                        passes.iter().any(|(pass, _)| pass.pass == name),
                        "{name} is missing from the engine's deferred preset"
                    );
                }
                // GI builds where something can trace its probes: in a
                // mesh scene, hardware ray queries.
                if helio_pass_gi::mesh_rays_supported(&device) {
                    for name in ["GI", "GIMeshRays", "GIResolve"] {
                        assert!(
                            passes.iter().any(|(pass, _)| pass.pass == name),
                            "{name} is missing from the engine's deferred preset"
                        );
                    }
                }
                assert!(graph
                    .external_input_names()
                    .iter()
                    .any(|input| *input == "corona_emitters"));
            }
            writeln!(text, "### {} / {config_name}", preset.name()).unwrap();
            let mut inputs = graph.external_input_names();
            inputs.sort_unstable();
            writeln!(text, "inputs: {inputs:?}").unwrap();
            for (pass, member) in passes {
                let dormant = if member { "" } else { " [dormant]" };
                writeln!(text, "- {} ({}){dormant}", pass.pass, pass.type_name).unwrap();
                let mut reads: Vec<_> = pass.reads.iter().map(|r| r.name).collect();
                reads.sort_unstable();
                writeln!(text, "    reads: {reads:?}").unwrap();
                let mut writes: Vec<_> = pass
                    .writes
                    .iter()
                    .map(|w| format!("{}:{:?}:{:?}", w.name, w.kind, w.format))
                    .collect();
                writes.sort_unstable();
                writeln!(text, "    writes: {writes:?}").unwrap();
                let mut scene = pass.scene_buffers.clone();
                scene.sort_unstable();
                writeln!(text, "    scene: {scene:?}").unwrap();
            }
        }
    }
    if let Ok(out) = std::env::var("PRESET_SNAPSHOT_OUT") {
        std::fs::write(&out, text).expect("write the snapshot");
    }
}
