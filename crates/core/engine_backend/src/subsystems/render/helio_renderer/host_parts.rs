//! Per-renderer state a plugin adds to every scene renderer: a render-host
//! part (the meshes' virtual-geometry host and authored LOD selection).
//!
//! Some content needs state that lives as long as one renderer and is
//! updated once a frame, not a component's rows: a streaming runtime that
//! settings feed and the graph reads, derivations that hold GPU caches. A
//! plugin's component crate submits a [`RenderHostPartRegistration`]; every
//! [`super::SceneRenderer`] makes one part of each enabled registration,
//! lets it configure the renderer, installs its derivations after the
//! object joins and before the row joins, and calls it once a frame. The
//! engine names no kind.
//!
//! How others do it: Unreal's `FSceneViewExtension`s and per-scene
//! subsystems (Nanite's streaming manager is renderer state the engine
//! module owns); Bevy's render-world resources that plugins insert and
//! update in the extract and prepare stages. Ours is Bevy's line through a
//! registration: the part is the plugin's, the renderer only drives it.

use std::any::Any;
use std::sync::Arc;

use helio_core::SceneDerivation;
use pulsar_scenedb::gpu::GpuMirrorHandle;

use crate::scene::SharedScene;

/// One renderer's part ([`RenderHostPartRegistration`]).
pub trait RenderHostPart: Any + Send {
    /// What the part adds to the renderer's configuration (a runtime the
    /// plugin's passes read), before the renderer is built.
    fn configure(&mut self, config: helio::RendererConfig) -> helio::RendererConfig {
        config
    }

    /// The part's scene derivations, installed after the object joins and
    /// before the row joins, in this order; asked once, when the renderer
    /// is built. `mirror`: the scene's GPU mirror.
    fn derivations(
        &mut self,
        _device: &Arc<wgpu::Device>,
        _mirror: &GpuMirrorHandle,
    ) -> Vec<Box<dyn SceneDerivation>> {
        Vec::new()
    }

    /// Once a frame, before the frame reads the scene.
    fn prepare_frame(&mut self, _scene: &SharedScene) {}

    /// Whether `world` holds content of the part's kind the scene's ground
    /// knows nothing about, so a camera keeps its own near plane even where
    /// the ground certifies a farther one (authored meshes near a voxel
    /// world's surface).
    fn keeps_near_plane(&self, _world: &pulsar_scenedb::World) -> bool {
        false
    }

    /// The part as `Any`, for [`super::SceneRenderer::host_part`].
    fn as_any(&self) -> &dyn Any;

    /// The part as `Any`, for [`super::SceneRenderer::host_part_mut`].
    fn as_any_mut(&mut self) -> &mut dyn Any;
}

/// A plugin's render-host part: `new` makes one for each scene renderer.
pub struct RenderHostPartRegistration {
    /// The package that submitted it (`pulsar_plugins::origin!()`): skipped
    /// while that package's plugin is disabled.
    pub origin: &'static str,
    /// For diagnostics; parts are made in name order.
    pub name: &'static str,
    pub new: fn() -> Box<dyn RenderHostPart>,
}

inventory::collect!(RenderHostPartRegistration);

impl pulsar_plugins::Registration for RenderHostPartRegistration {
    fn origin(&self) -> &'static str {
        self.origin
    }
}

/// A part of every enabled registration, in name order.
pub(super) fn new_parts() -> Vec<Box<dyn RenderHostPart>> {
    let mut registrations: Vec<_> =
        pulsar_plugins::registered::<RenderHostPartRegistration>().collect();
    registrations.sort_by_key(|registration| registration.name);
    registrations
        .into_iter()
        .map(|registration| (registration.new)())
        .collect()
}
