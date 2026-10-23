//! The game's Helio renderer: one construction for the standalone window
//! and the embedded (Play-in-Editor) viewport, so both render the shared
//! world through the same scene and environment joins and the same graph
//! as the editor viewport (Pulsar-Native#1035).

use std::sync::Arc;

use engine_backend::scene::{
    ensure_gpu_mirror, environment_join, hierarchy_resolve, portal_projection, scene_join, SharedScene,
};
use helio::{Renderer, RendererBuilder, RendererConfig};

/// The game's renderer settings: the project's, as the editor viewport
/// reads them (`project_settings`). "Automatic" TSR is off here: the game's
/// graph has no voxel passes to prefer a preset.
pub fn project_renderer_config(
    width: u32,
    height: u32,
    format: wgpu::TextureFormat,
) -> RendererConfig {
    use engine_backend::subsystems::render::helio_renderer::project_settings;
    let settings = project_settings::renderer();
    let config = project_settings::renderer_config(width, height, format, &settings);
    match project_settings::tsr_quality(&settings, None) {
        Some(quality) => config.with_tsr_quality(quality),
        None => config,
    }
}

/// Where the renderer's device comes from.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum DeviceOwner {
    /// The game owns the device (the standalone window).
    Game,
    /// The host owns it (the editor's device under Play-in-Editor).
    Host,
}


/// [`build_game_renderer_with_portals`] for a host that draws no portals.
pub fn build_game_renderer(
    device: Arc<wgpu::Device>,
    queue: Arc<wgpu::Queue>,
    scene: &SharedScene,
    config: RendererConfig,
    editor_mode: bool,
    owner: DeviceOwner,
) -> Renderer {
    build_game_renderer_with_portals(
        device,
        queue,
        scene,
        config,
        editor_mode,
        owner,
        Default::default(),
    )
}

/// A renderer over `scene`. SceneDB's GPU mirror is attached first
/// (idempotent: a second renderer on one scene shares the mirror); Helio's
/// scene and environment joins derive the drawn rows from it on the GPU.
pub fn build_game_renderer_with_portals(
    device: Arc<wgpu::Device>,
    queue: Arc<wgpu::Queue>,
    scene: &SharedScene,
    config: RendererConfig,
    editor_mode: bool,
    owner: DeviceOwner,
    portals: helio_default_graphs::portal_projection::PortalProjectionInput,
) -> Renderer {
    let (width, height, format) = (config.width, config.height, config.surface_format);
    let scene_db_handle = ensure_gpu_mirror(&mut scene.write(), device.clone(), queue.clone());
    let mut builder = RendererBuilder::new(config, scene_db_handle);
    if owner == DeviceOwner::Host {
        builder = builder.with_external_device();
    }
    builder
        .with_editor_mode(editor_mode)
        // Parent-relative transforms become world-space first, under the same key.
        .with_scene_derivation(hierarchy_resolve(&device))
        .with_scene_derivation(scene_join(&device, editor_mode))
        .with_scene_derivation(environment_join(&device))
        // Authored portals, paired and projected each frame by the host's `SpaceSync`.
        .with_scene_derivation(portal_projection(&device, portals))
        // No default ambient: all illumination comes from the scene's lights,
        // as in the editor.
        .with_ambient([0.0, 0.0, 0.0], 0.0)
        .with_pass_build_context(Box::new(
            helio_default_graphs::build_default_graph_external_with_context,
        ))
        .build(device, queue, width, height, format)
}
