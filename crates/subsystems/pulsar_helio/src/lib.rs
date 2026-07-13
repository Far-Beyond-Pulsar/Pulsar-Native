//! Pulsar's runtime integration boundary for the Helio graphics engine.
//!
//! Helio intentionally exposes its scene, graph, and renderer construction
//! separately. Pulsar owns the policy that joins those pieces because its GPU
//! device is shared by GPUI and, in games, potentially by multiple windows.

use std::sync::{Arc, Mutex};

use helio::{DebugCameraUniform, DebugDrawState, Renderer, RendererConfig, Scene};
use helio_default_graphs::build_default_graph_external;

/// Helio revision audited against this Pulsar integration layer.
///
/// Generated game projects must use this exact revision as well. Otherwise
/// Cargo can resolve two different `helio` package identities and values from
/// a game's direct Helio dependency will not be compatible with Pulsar's API.
pub const HELIO_GIT_REVISION: &str = "3210590541e28a3c37ec6fe5c2bc0b80214a70db";

/// Creates an empty Helio renderer that borrows a device managed by Pulsar.
///
/// This is the only supported bootstrap for Pulsar runtime and editor callers.
/// It deliberately uses Helio's external-device graph and renderer paths so
/// Helio never blocks on or independently polls a device shared with GPUI or
/// another game window.
pub fn new_external_renderer(
    device: Arc<wgpu::Device>,
    queue: Arc<wgpu::Queue>,
    config: RendererConfig,
) -> Renderer {
    let scene = Scene::new(Arc::clone(&device), Arc::clone(&queue));
    let debug_state = Arc::new(Mutex::new(DebugDrawState::default()));

    let debug_camera_buffer = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("Pulsar Helio Debug Camera"),
        size: std::mem::size_of::<DebugCameraUniform>() as u64,
        usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    let cull_stats_buffer = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("Pulsar Helio Cull Stats"),
        size: 32,
        usage: wgpu::BufferUsages::STORAGE
            | wgpu::BufferUsages::COPY_SRC
            | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });

    let graph = build_default_graph_external(
        &device,
        &queue,
        &scene,
        config,
        Arc::clone(&debug_state),
        &debug_camera_buffer,
        &cull_stats_buffer,
        None,
    );

    Renderer::new_with_external_device(
        device,
        queue,
        config.surface_format,
        config.width,
        config.height,
        config.render_scale,
        config,
        scene,
        graph,
        debug_state,
        debug_camera_buffer,
        cull_stats_buffer,
    )
}

#[cfg(test)]
mod tests {
    use super::HELIO_GIT_REVISION;

    #[test]
    fn workspace_helio_dependencies_match_the_audited_revision() {
        let workspace_manifest = include_str!("../../../../Cargo.toml");

        for dependency in [
            "helio",
            "helio-snapshot",
            "helio-asset-compat",
            "helio-default-graphs",
        ] {
            let line = workspace_manifest
                .lines()
                .find(|line| line.trim_start().starts_with(dependency))
                .unwrap_or_else(|| panic!("missing workspace dependency `{dependency}`"));
            assert!(
                line.contains(HELIO_GIT_REVISION),
                "workspace dependency `{dependency}` drifted from audited Helio revision {HELIO_GIT_REVISION}: {line}"
            );
        }
    }
}
