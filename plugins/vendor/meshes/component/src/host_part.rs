//! The meshes' per-renderer state (`RenderHostPartRegistration`): the
//! virtual-geometry host (builds, caches and streams the cluster assets of
//! meshes whose owner has a `VirtualGeometryComponent`; its runtime feeds
//! the VG passes) and the authored LOD selection.
//!
//! Its derivations run after the object joins and before the row joins, in
//! this order: which VG assets are ready, which instances are VG-active,
//! then the authored LOD selection (VG-active instances suppress it).

use std::sync::Arc;

use engine_backend::scene::SharedScene;
use engine_backend::subsystems::render::helio_renderer::{
    RenderHostPart, RenderHostPartRegistration,
};
use helio_core::SceneDerivation;
use pulsar_scenedb::gpu::{BufferKey, GpuMirrorHandle};

use crate::authored_lod::{AuthoredLodKeys, AuthoredLodSelection};
use crate::vg_settings::{VirtualGeometryMode, VirtualGeometrySettings};
use crate::virtual_geometry_host::VirtualGeometryHost;

/// One renderer's meshes part.
pub struct MeshesHostPart {
    virtual_geometry: VirtualGeometryHost,
    /// Replaces the project's virtual-geometry settings (tests, tools).
    settings_override: Option<VirtualGeometrySettings>,
}

impl MeshesHostPart {
    fn new() -> Self {
        Self {
            virtual_geometry: VirtualGeometryHost::new(),
            settings_override: None,
        }
    }

    /// The virtual-geometry streaming runtime (readiness, settings).
    pub fn virtual_geometry_runtime(&self) -> &helio_vg_stream::VgRuntime {
        self.virtual_geometry.runtime()
    }

    /// Use `settings` instead of the project's `project.virtual_geometry`
    /// ones (`None` restores them); takes effect next frame.
    pub fn set_virtual_geometry_settings(&mut self, settings: Option<VirtualGeometrySettings>) {
        self.settings_override = settings;
    }
}

/// Use `settings` instead of the project's virtual-geometry settings on
/// `renderer`'s scene renderers, now and when it builds one (`None`
/// restores them). Tests and tools.
pub fn set_virtual_geometry_settings(
    renderer: &mut engine_backend::subsystems::render::helio_renderer::HelioRenderer,
    settings: Option<VirtualGeometrySettings>,
) {
    renderer.set_up_host_parts(
        "meshes.virtual_geometry_settings",
        Arc::new(move |part: &mut dyn RenderHostPart| {
            if let Some(part) = part.as_any_mut().downcast_mut::<MeshesHostPart>() {
                part.set_virtual_geometry_settings(settings);
            }
        }),
    );
}

/// `renderer`'s virtual-geometry streaming runtime, once it is built.
pub fn virtual_geometry_runtime(
    renderer: &engine_backend::subsystems::render::helio_renderer::HelioRenderer,
) -> Option<&helio_vg_stream::VgRuntime> {
    renderer
        .host_part::<MeshesHostPart>()
        .map(|part| part.virtual_geometry_runtime())
}

/// The project's `project.virtual_geometry` settings as the runtime's.
pub fn virtual_geometry_runtime_settings(
    settings: VirtualGeometrySettings,
) -> helio_vg_stream::VgRuntimeSettings {
    let settings = settings.sanitized();
    helio_vg_stream::VgRuntimeSettings {
        mode: match settings.mode {
            VirtualGeometryMode::Auto => helio_vg_format::VgMode::Auto,
            VirtualGeometryMode::ForceNonRtFallback => helio_vg_format::VgMode::ForceNonRtFallback,
            VirtualGeometryMode::ForceHwRaster => helio_vg_format::VgMode::ForceHwRaster,
            VirtualGeometryMode::Off => helio_vg_format::VgMode::Off,
        },
        tau_px: settings.tau_pixels,
        page_budget_mb: settings.page_budget_mb,
        max_visible_clusters: settings.max_visible_clusters,
        two_phase_occlusion: settings.two_phase_occlusion,
        tier3: settings.tier3,
        ..helio_vg_stream::VgRuntimeSettings::default()
    }
}

impl RenderHostPart for MeshesHostPart {
    fn configure(&mut self, config: helio::RendererConfig) -> helio::RendererConfig {
        helio_pass_vg::with_virtual_geometry(config, self.virtual_geometry.runtime())
    }

    fn derivations(
        &mut self,
        device: &Arc<wgpu::Device>,
        mirror: &GpuMirrorHandle,
    ) -> Vec<Box<dyn SceneDerivation>> {
        let (owners, generations, hidden, transforms) = {
            let context = engine_backend::scene::object_join_context(false);
            (
                context.owners,
                context.generations,
                context.hidden,
                context.transforms,
            )
        };
        vec![
            self.virtual_geometry.asset_ready_derivation(device),
            self.virtual_geometry.instances_derivation(device),
            Box::new(AuthoredLodSelection::new(
                device,
                AuthoredLodKeys {
                    owners,
                    generations,
                    hidden,
                    transforms,
                    level_handles: BufferKey::of("lod_component_levels::handles"),
                    levels: BufferKey::of("lod_component_levels"),
                    camera: helio_core::CAMERA_BUFFER_KEY,
                },
                Some(mirror.clone()),
                // Pulsar's derivations record into one encoder: they share
                // one staging pool.
                engine_backend::gpu::Uploads::new(device),
            )),
        ]
    }

    /// Build and stream the virtual-geometry assets the scene asks for,
    /// with the project's settings (or the override).
    fn prepare_frame(&mut self, scene: &SharedScene) {
        let settings = self.settings_override.unwrap_or_else(|| {
            VirtualGeometrySettings::from_config(engine_state::settings::global_config())
        });
        let scene = scene.read();
        let Some(mirror) = scene.world.gpu_mirror() else {
            return;
        };
        self.virtual_geometry.sync(
            &scene.world,
            mirror,
            virtual_geometry_runtime_settings(settings),
        );
    }

    /// Authored meshes: the ground's certified planes say nothing about
    /// them, so a camera keeps its near plane while any is enabled.
    fn keeps_near_plane(&self, world: &pulsar_scenedb::World) -> bool {
        use pulsar_scene_model::attachments::ComponentOwner;
        world
            .query::<(&crate::StaticMeshComponent, &ComponentOwner)>()
            .any(|(_, (_, owner))| owner.is_enabled())
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }

    fn as_any_mut(&mut self) -> &mut dyn std::any::Any {
        self
    }
}

inventory::submit! {
    RenderHostPartRegistration {
        origin: pulsar_plugins::origin!(),
        name: "meshes",
        new: || Box::new(MeshesHostPart::new()),
    }
}
