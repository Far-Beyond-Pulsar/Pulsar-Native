//! A decal's `albedo_texture` is an image asset (SceneDB#84): a decal
//! decoded from a level names it, the world loads it and binds it into the
//! decal, and saving writes the path, never the image. No GPU.

use std::path::Path;
use std::time::Duration;

use decals_component::DecalComponent;
use helio_component::texture_assets::texture_asset_id_of;
use pulsar_scenedb::World;

fn write_png(path: &Path, rgba: [u8; 4]) {
    image::save_buffer(path, &rgba.repeat(4), 2, 2, image::ColorType::Rgba8).unwrap();
}

const WHITE: [u8; 4] = [255, 255, 255, 255];

#[test]
fn a_decal_decoded_from_a_level_binds_its_albedo_image() {
    let project = tempfile::tempdir().unwrap();
    let images = project.path().join("images");
    std::fs::create_dir_all(&images).unwrap();
    write_png(&images.join("white.png"), WHITE);
    engine_state::EngineContext::new().set_global();
    engine_state::set_project_path(project.path().display().to_string());
    let white = texture_asset_id_of("images/white.png");

    let mut world = World::new();
    let decoded = pulsar_world_registry::decode_world_component_value(
        "DecalComponent",
        &serde_json::json!({ "albedo_texture": "images/white.png" }),
    )
    .expect("registered")
    .expect("decodes");
    let decal = world.spawn();
    world.insert(decal, *decoded.downcast::<DecalComponent>().unwrap());
    assert_eq!(world.asset_ref_count(white), 1);
    assert!(
        world.resolve_assets_blocking(Duration::from_secs(30)),
        "texture assets did not load"
    );
    let bound = world.get::<DecalComponent>(decal).unwrap();
    let texel = bound.albedo_image.image_for(white).and_then(|image| {
        let cooked = image
            .cooked(helio_component::cooked_texture::CookFormat::Rgba8)
            .ok()?;
        cooked.mip(0).get(..4).map(|t| [t[0], t[1], t[2], t[3]])
    });
    assert_eq!(texel, Some(WHITE));
    // Saving it writes the path, never the image.
    let saved = serde_json::to_value(bound).unwrap();
    assert_eq!(saved["albedo_texture"], "images/white.png");
    assert!(saved.get("albedo_image").is_none());

    world.despawn(decal);
    assert_eq!(world.asset_ref_count(white), 0);
}
