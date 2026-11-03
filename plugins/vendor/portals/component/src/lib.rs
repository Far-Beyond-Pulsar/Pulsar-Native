//! Portals (`pulsar.portals`): the component crate of the plugin that owns
//! portals.
//!
//! - The component ([`component`]): `PortalComponent`, an opening that shows
//!   what is behind its partner (the other portal with its Link ID).
//! - Pairing and projection ([`projection`]): each frame the authored
//!   portals are paired, expanded into views and recursion chains whose
//!   coordinate spaces come from the frame's one table, uploaded by the
//!   feature's derivation (`helio_pass_portal_cull::scene_rows`) and counted
//!   for the passes (`helio_pass_portal_cull::PORTAL_COUNTS`). The engine
//!   runs it for every renderer as a space producer.
//! - The registrations of the plugin's Helio features (portal culling,
//!   `helio_pass_portal_cull::PortalCullFeature`, and the portal draws,
//!   `helio_pass_portal_instances::PortalFeature`), their settings
//!   (`helio_pass_portal_cull::settings::PORTALS`) and the space producer,
//!   with the engine.
//!
//! What an editor shows through the portal overlay (labels, the geometry a
//! selected portal maps) are the overlay's payloads ([`PORTAL_DEBUG_TEXTS`],
//! [`PORTAL_DEBUG_GEOMETRY`]), which an editor publishes for its viewport.
//!
//! Everything registers through `inventory` once the crate is linked: hosts
//! link it (the game: `extern crate portals_component`; the editor through
//! the plugin's editor crate, `portals_plugin`) and know no portal type.

pub mod component;
pub mod projection;

pub use component::*;
pub use helio_pass_portal_instances::{PORTAL_DEBUG_GEOMETRY, PORTAL_DEBUG_TEXTS, PortalDebugText};
pub use projection::{
    Pairing, Portal, PortalProblem, PortalSync, authored_portals, build_resolver, pair_portals,
};

/// The editor viewport's debug mode for portal diagnostics: the renderer
/// keeps its colour output lit while the editor draws portal labels and
/// pairing overlays.
pub const PORTAL_DEBUG_VIEW_MODE: u32 = 35;

fn new_portal_sync() -> Box<dyn engine_backend::scene::SpaceProducer> {
    Box::new(PortalSync::default())
}

inventory::submit! {
    engine_backend::scene::SpaceProducerRegistration {
        name: "portals",
        new: new_portal_sync,
    }
}

/// Portal culling and the portal draws (a G-buffer drawer, the mask and the
/// editor overlay).
fn render_features() -> helio_feature_api::FeatureSet {
    helio_feature_api::FeatureSet::new()
        .with(helio_pass_portal_cull::PortalCullFeature)
        .with(helio_pass_portal_instances::PortalFeature)
}

/// The features' settings from the project's portal settings.
fn configure_renderer(config: helio::RendererConfig) -> helio::RendererConfig {
    use engine_backend::subsystems::render::helio_renderer::project_settings;
    config.with_feature(helio_pass_portal_cull::PortalSettings::from_settings(
        &project_settings::resolved(&helio_pass_portal_cull::settings::PORTALS),
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
        group: &helio_pass_portal_cull::settings::PORTALS,
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn registers_its_features_settings_and_space_producer() {
        let features =
            engine_backend::subsystems::render::helio_renderer::render_features::plugin_features();
        for id in ["helio.portal_cull", "helio.portals"] {
            assert!(
                features.ids().contains(&id),
                "{id}: features {:?}",
                features.ids()
            );
        }
        assert!(
            pulsar_settings::project::rendering::groups()
                .iter()
                .any(|group| std::ptr::eq(*group, &helio_pass_portal_cull::settings::PORTALS))
        );
        assert!(
            inventory::iter::<engine_backend::scene::SpaceProducerRegistration>
                .into_iter()
                .any(|registration| registration.name == "portals")
        );
    }
}
