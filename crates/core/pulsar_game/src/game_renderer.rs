//! The game's Helio renderer: one construction for the standalone window
//! and the embedded (Play-in-Editor) viewport, so both render the shared
//! world through the same scene and environment joins and the same graph
//! as the editor viewport (Pulsar-Native#1035).

use std::sync::Arc;

use engine_backend::scene::{
    authored_lod_selection, ensure_gpu_mirror, environment_join, hierarchy_resolve,
    portal_projection, scene_join, virtual_geometry_asset_ready, virtual_geometry_instances,
    SharedScene, VirtualGeometryHost,
};
use helio::{Renderer, RendererBuilder, RendererConfig};

/// The standalone game's renderer settings, from the project's
/// `rendering` settings (render scale, reflections, shadows, TSR, mode).
pub fn project_renderer_config(
    width: u32,
    height: u32,
    format: wgpu::TextureFormat,
) -> RendererConfig {
    let project_setting = |key: &str| {
        engine_state::settings::global_config().get(
            engine_state::settings::NS_PROJECT,
            "rendering",
            key,
        )
    };
    let project_string = |key: &str, fallback: &str| {
        project_setting(key)
            .ok()
            .and_then(|value| value.as_str().ok().map(str::to_owned))
            .unwrap_or_else(|| fallback.to_owned())
    };
    let project_bool = |key: &str, fallback: bool| {
        project_setting(key)
            .ok()
            .and_then(|value| value.as_bool().ok())
            .unwrap_or(fallback)
    };
    let render_scale = project_setting("render_scale")
        .ok()
        .and_then(|value| value.as_float().ok())
        .filter(|value| value.is_finite())
        .unwrap_or(0.75) as f32;
    let mut config = RendererConfig::new(width, height, format)
        .with_render_scale(render_scale.clamp(0.25, 1.0))
        .with_ssr(project_bool("screen_space_reflections", false))
        .with_planar_reflections(project_bool("planar_reflections", false));
    config =
        config.with_shadow_quality(match project_string("shadow_quality", "medium").as_str() {
            "low" => helio::ShadowQuality::Low,
            "high" => helio::ShadowQuality::High,
            "ultra" => helio::ShadowQuality::Ultra,
            _ => helio::ShadowQuality::Medium,
        });
    config.shadow_atlas_size = project_setting("shadow_atlas_size")
        .ok()
        .and_then(|value| {
            value
                .as_str()
                .ok()
                .and_then(|value| value.parse::<u32>().ok())
                .or_else(|| {
                    value
                        .as_int()
                        .ok()
                        .and_then(|value| u32::try_from(value).ok())
                })
        })
        .filter(|size| matches!(size, 512 | 1024 | 2048 | 4096))
        .unwrap_or(1024);
    config = match project_string("tsr_quality", "off").as_str() {
        "performance" => config.with_tsr_quality(helio::TsrQuality::Performance),
        "balanced" => config.with_tsr_quality(helio::TsrQuality::Balanced),
        "quality" => config.with_tsr_quality(helio::TsrQuality::Quality),
        "native" => config.with_tsr_quality(helio::TsrQuality::Native),
        _ => config.without_tsr(),
    };
    match project_string("render_mode", "deferred").as_str() {
        "forward_opaque" => config.with_render_mode(helio::RenderMode::ForwardOpaque),
        "forward_only" => config.with_render_mode(helio::RenderMode::ForwardOnly),
        _ => config.with_render_mode(helio::RenderMode::Deferred),
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
    portals: helio_default_passes::portal_projection::PortalProjectionInput,
) -> Renderer {
    // A host that never syncs its `VirtualGeometryHost` draws classically.
    build_game_renderer_with_virtual_geometry(
        device,
        queue,
        scene,
        config,
        editor_mode,
        owner,
        portals,
        &mut VirtualGeometryHost::new(),
    )
}

/// [`build_game_renderer_with_portals`] with virtual geometry: `vg` supplies the
/// streaming runtime and the two derivations; the host calls
/// `engine_backend::scene::sync_virtual_geometry(vg, scene, None)` before every
/// frame so meshes whose owner has an enabled `VirtualGeometryComponent` are
/// built and streamed.
pub fn build_game_renderer_with_virtual_geometry(
    device: Arc<wgpu::Device>,
    queue: Arc<wgpu::Queue>,
    scene: &SharedScene,
    config: RendererConfig,
    editor_mode: bool,
    owner: DeviceOwner,
    portals: helio_default_passes::portal_projection::PortalProjectionInput,
    vg: &mut VirtualGeometryHost,
) -> Renderer {
    let config = config.with_virtual_geometry(vg.runtime());
    let (width, height, format) = (config.width, config.height, config.surface_format);
    let scene_db_handle = ensure_gpu_mirror(&mut scene.write(), device.clone(), queue.clone());
    let mut builder = RendererBuilder::new(config, scene_db_handle.clone());
    if owner == DeviceOwner::Host {
        builder = builder.with_external_device();
    }
    // The level's render pipeline (World Settings); the engine default when it
    // names none, names one that is not registered, or names the default.
    use engine_backend::subsystems::render::helio_renderer::pipelines;
    let chosen = pipelines::resolve_pipeline(pipelines::level_pipeline().as_deref());
    let custom = chosen
        .as_deref()
        .filter(|id| *id != helio_default_passes::pipelines::DEFAULT)
        .and_then(|id| pipelines::create_pipeline(Some(id)));
    let builder = builder
        .with_editor_mode(editor_mode)
        // Parent-relative transforms become world-space first, under the same key.
        .with_scene_derivation(hierarchy_resolve(&device))
        .with_scene_derivation(scene_join(&device, editor_mode))
        .with_scene_derivation(virtual_geometry_asset_ready(vg, &device))
        .with_scene_derivation(virtual_geometry_instances(vg, &device))
        .with_scene_derivation(authored_lod_selection(&device, scene_db_handle))
        .with_scene_derivation(environment_join(&device))
        // Authored portals, paired and projected each frame by the host's `SpaceSync`.
        .with_scene_derivation(portal_projection(&device, portals))
        // No default ambient: all illumination comes from the scene's lights,
        // as in the editor.
        .with_ambient([0.0, 0.0, 0.0], 0.0);
    let builder = match custom {
        Some(pipeline) => builder.with_boxed_graph(pipeline),
        None => builder.with_pass_build_context(Box::new(
            helio_default_passes::build_default_graph_external_with_context,
        )),
    };
    builder.build(device, queue, width, height, format)
}
