//! Cables (`pulsar.cables`), the root crate for cable registrations.

pulsar_plugins::installed!("pulsar-plugin.toml", build);

pub use cables_component;
pub use cables_feature;

/// Registers authored cable types, their GPU rows, and the Helio feature.
pub fn build(p: &mut pulsar_plugins::PluginBuilder) {
    use engine_backend::scene::{ClassSpec, ComponentsBuilder};
    use helio_component::texture_assets::TextureBindingRegistration;
    p.components()
        .class::<cables_component::CableComponent>(ClassSpec::default())
        .class::<cables_component::CableConnectorComponent>(ClassSpec::default());

    p.add(engine_backend::subsystems::render::RenderFeatureRegistration {
        origin: p.origin(),
        features: cables_component::render_features,
        configure: |config| config,
    });
    p.add(engine_backend::scene::helio_bridge::SceneRowsRegistration {
        origin: p.origin(),
        name: "cables.rows",
        register_columns: cables_component::register_columns,
        derivation: Some(cables_component::row_derivation),
    });
    // Each frame, every renderer pairs the connectors that share a cable ID.
    p.add(engine_backend::scene::SpaceProducerRegistration {
        origin: p.origin(),
        name: "cables",
        new: || Box::new(cables_component::CableSync::default()),
    });
    p.add(TextureBindingRegistration {
        origin: p.origin(),
        name: "cables.textures",
        register: cables_component::register_texture_binding,
    });
}
