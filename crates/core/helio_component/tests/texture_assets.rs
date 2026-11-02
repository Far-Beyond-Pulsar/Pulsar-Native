//! Images as world assets (SceneDB#84): a sprite's `texture` and a decal's
//! `albedo_texture` are loaded by the world, off the write path, however
//! the value was written, and bound into the component; a changed file is
//! reloaded into every component showing it.
//!
//! One test, because the project path sources resolve against is a process
//! global. No GPU: the mirror's upload of the bound image is covered by the
//! sprite render acceptance test (`ui_level_editor/tests/render_acceptance.rs`).

use std::path::Path;
use std::time::Duration;

use helio_component::components::{DecalComponent, SpriteComponent};
use helio_component::texture_assets::{
    LoadedTexture, texture_asset_id, texture_asset_id_of, texture_source_for_file,
};
use pulsar_scenedb::{AssetState, Entity, World};

fn write_png(path: &Path, rgba: [u8; 4]) {
    image::save_buffer(path, &rgba.repeat(4), 2, 2, image::ColorType::Rgba8).unwrap();
}

fn resolve(world: &mut World) {
    assert!(
        world.resolve_assets_blocking(Duration::from_secs(30)),
        "texture assets did not load"
    );
}

/// The first texel of the image `entity`'s sprite is bound to, if it is
/// bound to the image its `texture` names.
fn sprite_texel(world: &World, entity: Entity) -> Option<[u8; 4]> {
    let sprite = world.get::<SpriteComponent>(entity).unwrap();
    texel(
        sprite
            .image
            .image_for(texture_asset_id_of(&sprite.texture))?,
    )
}

fn texel(image: &LoadedTexture) -> Option<[u8; 4]> {
    let cooked = image.cooked(helio_component::cooked_texture::CookFormat::Rgba8).ok()?;
    let rgba = cooked.mip(0);
    rgba.get(..4).map(|t| [t[0], t[1], t[2], t[3]])
}

fn set_texture(world: &mut World, entity: Entity, path: &str) {
    pulsar_world_registry::set_world_component_property(
        "SpriteComponent",
        world,
        entity,
        "texture",
        Box::new(path.to_string()),
    )
    .unwrap_or_else(|_| panic!("texture write to {path} refused"));
}

const RED: [u8; 4] = [255, 0, 0, 255];
const BLUE: [u8; 4] = [0, 0, 255, 255];
const GREEN: [u8; 4] = [0, 255, 0, 255];
const WHITE: [u8; 4] = [255, 255, 255, 255];

#[test]
fn texture_assets_load_rebind_reload_and_survive_bad_input() {
    let project = tempfile::tempdir().unwrap();
    let images = project.path().join("images");
    std::fs::create_dir_all(&images).unwrap();
    write_png(&images.join("red.png"), RED);
    write_png(&images.join("blue.png"), BLUE);
    write_png(&images.join("white.png"), WHITE);
    std::fs::write(images.join("corrupt.png"), b"not an image").unwrap();
    engine_state::EngineContext::new().set_global();
    engine_state::set_project_path(project.path().display().to_string());

    let red = texture_asset_id("images/red.png");
    let blue = texture_asset_id("images/blue.png");
    assert!(!red.is_zero() && red != blue);
    assert_eq!(
        red,
        texture_asset_id("./images/../images/red.png"),
        "one source however the path is spelled"
    );
    assert_eq!(
        texture_asset_id_of(&images.join("red.png").display().to_string()),
        red,
        "a project-relative and an absolute path name the same asset"
    );
    assert!(texture_asset_id("").is_zero());

    // ── A typed insert names the image; the world loads and binds it ───────
    let mut world = World::new();
    let sprite = world.spawn();
    world.insert(
        sprite,
        SpriteComponent {
            texture: "images/red.png".into(),
            ..Default::default()
        },
    );
    assert_eq!(world.asset_ref_count(red), 1, "the insert counts the image");
    assert_eq!(
        sprite_texel(&world, sprite),
        None,
        "nothing read in the write"
    );
    resolve(&mut world);
    assert_eq!(world.asset_state(red), AssetState::Ready);
    assert_eq!(sprite_texel(&world, sprite), Some(RED));

    // ── A reflected write of the path rebinds to the new image ─────────────
    set_texture(&mut world, sprite, "images/blue.png");
    assert_eq!(world.asset_ref_count(red), 0);
    assert_eq!(world.asset_ref_count(blue), 1);
    assert_eq!(
        sprite_texel(&world, sprite),
        None,
        "the old image is not shown for the new path"
    );
    resolve(&mut world);
    assert_eq!(sprite_texel(&world, sprite), Some(BLUE));

    // ── A typed write back to an image the world still holds binds at once ─
    world.get_mut::<SpriteComponent>(sprite).unwrap().texture = "images/red.png".into();
    resolve(&mut world);
    assert_eq!(sprite_texel(&world, sprite), Some(RED));

    // ── A property edit neither reloads nor rebinds ────────────────────────
    let version = world.asset_version(red);
    world.get_mut::<SpriteComponent>(sprite).unwrap().tint = [0.5, 0.5, 0.5, 1.0];
    resolve(&mut world);
    assert_eq!(world.asset_version(red), version);
    assert_eq!(sprite_texel(&world, sprite), Some(RED));

    // ── Reload: the file changes, every component showing it is rebound ────
    let second = world.spawn();
    world.insert(
        second,
        SpriteComponent {
            texture: "images/red.png".into(),
            ..Default::default()
        },
    );
    resolve(&mut world);
    assert_eq!(
        sprite_texel(&world, second),
        Some(RED),
        "shared, already loaded"
    );
    write_png(&images.join("red.png"), GREEN);
    assert_eq!(
        world.asset_source_changed(&texture_source_for_file(&images.join("red.png"))),
        1
    );
    resolve(&mut world);
    assert!(world.asset_version(red) > version);
    assert_eq!(sprite_texel(&world, sprite), Some(GREEN));
    assert_eq!(sprite_texel(&world, second), Some(GREEN));

    // ── Missing and corrupt images: nothing bound, no panic ────────────────
    for path in ["images/missing.png", "images/corrupt.png"] {
        set_texture(&mut world, second, path);
        resolve(&mut world);
        assert!(
            matches!(
                world.asset_state(texture_asset_id_of(path)),
                AssetState::Failed(_)
            ),
            "{path}"
        );
        assert_eq!(sprite_texel(&world, second), None, "{path}");
    }
    // ...and an empty path shows the tint alone.
    set_texture(&mut world, second, "");
    resolve(&mut world);
    assert_eq!(sprite_texel(&world, second), None);

    // ── Rapid replacement: only the last path is bound ─────────────────────
    let white = texture_asset_id("images/white.png");
    for path in ["images/white.png", "images/blue.png", "images/white.png"] {
        set_texture(&mut world, second, path);
        world.pump_assets();
    }
    resolve(&mut world);
    assert_eq!(sprite_texel(&world, second), Some(WHITE));
    assert_eq!(world.asset_ref_count(white), 1);

    // ── Removed while its image loads: the result binds to nothing ─────────
    let removed = world.spawn();
    write_png(&images.join("late.png"), BLUE);
    world.insert(
        removed,
        SpriteComponent {
            texture: "images/late.png".into(),
            ..Default::default()
        },
    );
    world.pump_assets();
    assert!(world.remove::<SpriteComponent>(removed).is_some());
    resolve(&mut world);
    assert_eq!(
        world.asset_ref_count(texture_asset_id_of("images/late.png")),
        0
    );

    // ── A decal decoded from a level binds its albedo image the same way ───
    let decoded = pulsar_world_registry::decode_world_component_value(
        "DecalComponent",
        &serde_json::json!({ "albedo_texture": "images/white.png" }),
    )
    .expect("registered")
    .expect("decodes");
    let decal = world.spawn();
    world.insert(decal, *decoded.downcast::<DecalComponent>().unwrap());
    assert_eq!(world.asset_ref_count(white), 2);
    resolve(&mut world);
    let bound = world.get::<DecalComponent>(decal).unwrap();
    assert_eq!(
        bound
            .albedo_image
            .image_for(white)
            .and_then(|image| texel(image)),
        Some(WHITE)
    );
    // Saving it writes the path, never the image.
    let saved = serde_json::to_value(bound).unwrap();
    assert_eq!(saved["albedo_texture"], "images/white.png");
    assert!(saved.get("albedo_image").is_none());

    world.despawn(decal);
    world.despawn(second);
    world.despawn(sprite);
    assert_eq!(world.asset_ref_count(white), 0);
    assert_eq!(world.asset_ref_count(red), 0);
}
