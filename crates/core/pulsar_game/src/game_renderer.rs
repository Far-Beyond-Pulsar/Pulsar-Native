//! The game's Helio renderer: one construction for the standalone window
//! and the embedded (Play-in-Editor) viewport, so both render the shared
//! world through the same scene and environment joins and the same graph
//! as the editor viewport (Pulsar-Native#1035).

use std::sync::Arc;

use engine_backend::scene::{ensure_gpu_mirror, environment_join, scene_join, SharedScene};
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

/// A renderer over `scene`. SceneDB's GPU mirror is attached first
/// (idempotent: a second renderer on one scene shares the mirror); Helio's
/// scene and environment joins derive the drawn rows from it on the GPU.
pub fn build_game_renderer(
    device: Arc<wgpu::Device>,
    queue: Arc<wgpu::Queue>,
    scene: &SharedScene,
    config: RendererConfig,
    editor_mode: bool,
    owner: DeviceOwner,
) -> Renderer {
    let (width, height, format) = (config.width, config.height, config.surface_format);
    let scene_db_handle = ensure_gpu_mirror(&mut scene.write(), device.clone(), queue.clone());
    let mut builder = RendererBuilder::new(config, scene_db_handle);
    if owner == DeviceOwner::Host {
        builder = builder.with_external_device();
    }
    builder
        .with_editor_mode(editor_mode)
        .with_scene_derivation(scene_join(&device, editor_mode))
        .with_scene_derivation(environment_join(&device))
        // No default ambient: all illumination comes from the scene's lights,
        // as in the editor.
        .with_ambient([0.0, 0.0, 0.0], 0.0)
        .with_pass_build_context(Box::new(
            helio_default_graphs::build_default_graph_external_with_context,
        ))
        .build(device, queue, width, height, format)
}
