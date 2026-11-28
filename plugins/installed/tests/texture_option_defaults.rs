//! The project's texture settings are what a new component starts with, and
//! nothing else: an existing or saved component keeps its own choice.

use engine_state::settings::{global_config, ConfigValue, NS_PROJECT};
use helio_component::asset_component::{new_component_value, seed_new_component};
use installed_plugins as _;
use installed_plugins::{decals_plugin, sprites_plugin};

use decals_plugin::decals_component::{DecalComponent, DECAL_CLASS_NAME};
use sprites_plugin::sprites_component::{
    register_texture_settings, SpriteComponent, SPRITE_CLASS_NAME, TEXTURE_SETTINGS_OWNER,
};

fn set_project(compressed: bool, max_size: i64) {
    let config = global_config();
    if config
        .owner_handle(NS_PROJECT, TEXTURE_SETTINGS_OWNER)
        .is_none()
    {
        register_texture_settings(config);
    }
    let owner = config
        .owner_handle(NS_PROJECT, TEXTURE_SETTINGS_OWNER)
        .expect("registered");
    owner
        .set("compress_textures", ConfigValue::Bool(compressed))
        .unwrap();
    owner
        .set("max_texture_size", ConfigValue::Int(max_size))
        .unwrap();
}

fn new_sprite() -> SpriteComponent {
    let value = new_component_value(SPRITE_CLASS_NAME).expect("sprites have project defaults");
    value
        .downcast_ref::<SpriteComponent>()
        .expect("a sprite")
        .clone()
}

fn choice(sprite: &SpriteComponent) -> (bool, i32) {
    (sprite.texture_compressed, sprite.texture_max_size)
}

// One test: the settings are process-wide.
#[test]
fn a_new_component_copies_the_project_defaults_and_keeps_them() {
    set_project(false, 512);
    let first = new_sprite();
    assert_eq!(choice(&first), (false, 512));

    // Changing the project changes later components, not that one.
    set_project(true, 256);
    assert_eq!(choice(&first), (false, 512));
    assert_eq!(choice(&new_sprite()), (true, 256));

    // A saved component without the properties, as in an older level, gets
    // the fixed fallback, whatever the project says.
    set_project(false, 64);
    let old = pulsar_world_registry::decode_world_component_value(
        SPRITE_CLASS_NAME,
        &serde_json::json!({ "width": 32.0 }),
    )
    .expect("registered")
    .expect("decodes");
    let old = old.downcast_ref::<SpriteComponent>().expect("a sprite");
    assert_eq!(choice(old), (true, 0));
    assert_eq!(old.width, 32.0);

    // A saved choice is kept.
    let saved = pulsar_world_registry::decode_world_component_value(
        SPRITE_CLASS_NAME,
        &serde_json::json!({ "texture_compressed": true, "texture_max_size": 1024 }),
    )
    .expect("registered")
    .expect("decodes");
    assert_eq!(
        choice(saved.downcast_ref::<SpriteComponent>().expect("a sprite")),
        (true, 1024)
    );

    // Seeding one class leaves another alone: each plugin has its own.
    let mut decal = DecalComponent::default();
    seed_new_component(DECAL_CLASS_NAME, &mut decal);
    assert_eq!(
        (decal.texture_compressed, decal.texture_max_size),
        (true, 0)
    );
    assert!(new_component_value("StaticMeshComponent").is_none());
}
