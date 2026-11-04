//! Meshes (`pulsar.meshes`): the component crate of the plugin that owns
//! meshes, their virtual geometry and their asset import.
//!
//! The crate registers the plugin's Helio features with the engine: the
//! object batch (first in the frame's prepare slot), its culling around the
//! depth pyramid, the drawers (the G-buffer, shadow-caster, forward and
//! transparent phases) and virtual geometry. Everything registers through
//! `inventory` once the crate is linked: hosts link it (the game:
//! `extern crate meshes_component`; the editor through the plugin's editor
//! crate, `meshes_plugin`) and the engine's pipelines carry the meshes
//! without naming them.
//!
//! Still the engine's (see docs/architecture/plugin-migration.md, "As built,
//! batch 4"): the SceneDB components (`StaticMeshComponent`,
//! `StaticMeshDraw`, `SkeletalMeshComponent`, `LODComponent`,
//! `VirtualGeometryComponent`, in `helio_component`), the scene join's
//! installation, the authored LOD selection, the VG host and mesh picking,
//! which core crates (asset import, cooking, class records, the scene
//! loader) read by type.

/// The meshes' features.
fn render_features() -> helio_feature_api::FeatureSet {
    helio_feature_api::FeatureSet::new()
        .with(helio_pass_object_batch::ObjectBatchFeature)
        .with(helio_pass_occlusion_cull::MeshCulling)
        .with(helio_pass_gbuffer::GBufferDrawers)
        .with(helio_pass_shadow::ShadowDrawers)
        .with(helio_pass_forward_lit::ForwardDrawers)
        .with(helio_pass_transparent::TransparentDrawers)
        .with(helio_pass_vg::VgFeature)
}

/// The meshes' features read no project settings of their own: virtual
/// geometry's come in with its runtime (`VgSettings`), shadows' with the
/// lights' `ShadowSettings`.
fn configure_renderer(config: helio::RendererConfig) -> helio::RendererConfig {
    config
}

inventory::submit! {
    engine_backend::subsystems::render::RenderFeatureRegistration {
        features: render_features,
        configure: configure_renderer,
    }
}

#[cfg(test)]
mod tests {
    use engine_backend::subsystems::render::helio_renderer::render_features;

    #[test]
    fn registers_its_features() {
        let ids = render_features::plugin_features().ids();
        for id in [
            "helio.mesh.batch",
            "helio.mesh.cull",
            "helio.mesh.gbuffer",
            "helio.mesh.shadow",
        ] {
            assert!(ids.contains(&id), "{id} is missing: {ids:?}");
        }
    }
}
