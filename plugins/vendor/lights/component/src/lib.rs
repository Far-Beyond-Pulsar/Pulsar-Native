//! Lights (`pulsar.lights`): the component crate of the plugin that owns
//! lights.
//!
//! - The components: [`LightComponent`] (a directional, point, spot or area
//!   light; its derived source row, [`LightSourceRow`]) and
//!   [`ReflectionCaptureComponent`] (a parallax reflection probe's volume).
//! - Its scene derivation: the lights' join
//!   (`helio_pass_light_cull::scene_rows`, the light list, its directional
//!   light and the editor's light icons), installed in the engine's object
//!   joins ([`ObjectJoinRegistration`](engine_backend::scene::helio_bridge::ObjectJoinRegistration)).
//! - Directional lights are not a level singleton: a level may hold several
//!   (the sky draws each as an emitter; [`directional_lights`] in level order).
//! - The registrations of the plugin's Helio features (the shadow views,
//!   the light binning, the lighting models, the billboards) and their
//!   settings ([`helio_light_api::SHADOWS`]) with the engine.
//!
//! Everything registers through `inventory` once the crate is linked: hosts
//! link it (the game: `extern crate lights_component`; the editor through the
//! plugin's editor crate, `lights_plugin`) and know no light type.

mod light;
mod reflection_capture;

pub use light::*;
pub use reflection_capture::*;

use engine_backend::scene::helio_bridge::{ObjectJoinContext, ObjectJoinRegistration};
use engine_backend::scene::level_rules::LevelSingletonRegistration;
use helio_pass_light_cull::scene_rows::{LightJoin, LightJoinKeys};
use pulsar_scene_model::attachments;
use pulsar_scenedb::gpu::BufferKey;
use pulsar_scenedb::{Entity, World};

/// The light component's class name.
pub const LIGHT_CLASS: &str = "LightComponent";

/// Whether `light`, on an enabled instance, is a directional light that
/// casts.
pub fn is_directional(light: &LightComponent) -> bool {
    light.general.enabled && light.general.light_type == LightType::Directional
}

/// Every casting directional light: the enabled `LightComponent` instances
/// whose light is enabled and directional, in level order (the first is the
/// primary sun of whatever reads one).
pub fn directional_lights(world: &World) -> Vec<Entity> {
    engine_backend::scene::level_rules::attached_in_level_order(
        world,
        world
            .query::<&LightComponent>()
            .filter(|(instance, light)| {
                is_directional(light) && attachments::is_enabled(world, *instance)
            })
            .map(|(instance, _)| instance),
    )
}

/// The lights' join over the engine's rows: the light list from the light
/// instances' source rows ([`LIGHT_SOURCES_BUFFER`]), placed by their
/// owners; in the editor, a light icon a light.
fn light_join(
    device: &wgpu::Device,
    context: &ObjectJoinContext,
) -> Box<dyn helio_scene_join::SceneDerivation> {
    Box::new(LightJoin::new(
        device,
        LightJoinKeys {
            owners: context.owners,
            generations: context.generations,
            hidden: context.hidden,
            transforms: context.transforms,
            light_sources: BufferKey::of(LIGHT_SOURCES_BUFFER),
        },
        context.editor,
    ))
}

inventory::submit! {
    ObjectJoinRegistration {
        name: "lights",
        register_columns: |store, device| {
            LightSourceRow::register_gpu_columns_growable(store, 64, device);
        },
        derivation: light_join,
    }
}

/// The lights' features: the shadow views, the light binning, deferred and
/// light-field lighting, and the billboards (the editor's light icons).
fn render_features() -> helio_feature_api::FeatureSet {
    helio_feature_api::FeatureSet::new()
        .with(helio_pass_shadow_matrix::ShadowViewsFeature)
        .with(helio_pass_light_cull::LightCullFeature)
        .with(helio_pass_deferred_light::DeferredLightingFeature)
        .with(helio_pass_hlfs::HlfsFeature)
        .with(helio_pass_billboard::BillboardFeature)
}

/// The shadows' settings from the project's shadow settings.
fn configure_renderer(config: helio::RendererConfig) -> helio::RendererConfig {
    use engine_backend::subsystems::render::helio_renderer::project_settings;
    config.with_feature(helio_light_api::ShadowSettings::from_settings(
        &project_settings::resolved(&helio_light_api::SHADOWS),
    ))
}

inventory::submit! {
    engine_backend::subsystems::render::RenderFeatureRegistration {
        features: render_features,
        configure: configure_renderer,
    }
}

inventory::submit! {
    pulsar_settings::project::rendering::SettingsGroupRegistration {
        group: &helio_light_api::SHADOWS,
    }
}

// What an object's light instances author fixes the object.
inventory::submit! {
    helio_component::components::MovabilityRegistration {
        of: |world, instance| {
            world
                .get::<LightComponent>(instance)
                .map(|light| light.general.movability)
        },
    }
}

#[cfg(test)]
mod tests {
    use engine_backend::subsystems::render::helio_renderer::render_features;

    #[test]
    fn registers_its_features() {
        let ids = render_features::plugin_features().ids();
        for id in [
            "helio.lights.shadow_views",
            "helio.lights.binning",
            "helio.lights.deferred",
            "helio.hlfs",
            "helio.billboards",
        ] {
            assert!(ids.contains(&id), "{id} missing from {ids:?}");
        }
    }

    #[test]
    fn registers_the_directional_light_rule_and_its_join() {
        assert!(
            inventory::iter::<engine_backend::scene::level_rules::LevelSingletonRegistration>
                .into_iter()
                .any(|rule| rule.class_name == super::LIGHT_CLASS)
        );
        assert!(
            inventory::iter::<engine_backend::scene::helio_bridge::ObjectJoinRegistration>
                .into_iter()
                .any(|join| join.name == "lights")
        );
    }
}
