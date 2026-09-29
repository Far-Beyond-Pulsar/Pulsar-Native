//! The voxel terrain's scripting surface (blocks, fills, ray casts) and the
//! generator picker's entries, called the way scripts call them.

use std::any::Any;

use glam::DVec3;
use helio_component::voxel_generator_editor::generator_items;
use helio_component::voxel_world::terrain_world;
use helio_component::{VoxelFlatTerrainComponent, VoxelGeneratorRef, VoxelTerrainComponent};
use helio_pass_voxel_planet::terrain::{generators, material, GeneratorInfo};
use pulsar_scene_model::components::Transform;
use pulsar_scenedb::{component_id, Entity, World};

fn flat_world() -> (World, Entity) {
    let mut world = World::new();
    let entity = world.spawn();
    let mut terrain = VoxelTerrainComponent::plane(512.0);
    terrain.generator = VoxelGeneratorRef::new(helio_pass_voxel_planet::landform::FLAT_ID, 1);
    world.insert(entity, terrain);
    world.insert(entity, VoxelFlatTerrainComponent { height: 2.0, ..Default::default() });
    (world, entity)
}

fn call(world: &mut World, entity: Entity, name: &str, args: Vec<Box<dyn Any>>) -> Result<Option<Box<dyn Any>>, String> {
    let mut args = args;
    world
        .call_component_method(entity, component_id::<VoxelTerrainComponent>(), name, &mut args)
        .map_err(|e| format!("{e:?}"))
}

fn get(world: &mut World, entity: Entity, p: [f64; 3]) -> u32 {
    let out = call(world, entity, "get_block", vec![Box::new(p[0]), Box::new(p[1]), Box::new(p[2])]).unwrap();
    *out.unwrap().downcast::<u32>().unwrap()
}

fn set(world: &mut World, entity: Entity, p: [f64; 3], material: u32) -> Result<(), String> {
    call(world, entity, "set_block", vec![Box::new(p[0]), Box::new(p[1]), Box::new(p[2]), Box::new(material)]).map(drop)
}

fn down(world: &mut World, entity: Entity, x: f64) -> f64 {
    let args: Vec<Box<dyn Any>> = vec![Box::new(x), Box::new(10.0f64), Box::new(0.0f64), Box::new(0.0f64), Box::new(-1.0f64), Box::new(0.0f64), Box::new(100.0f64)];
    *call(world, entity, "raycast_distance", args).unwrap().unwrap().downcast::<f64>().unwrap()
}

#[test]
fn blocks_read_the_generated_ground_and_its_settings_component() {
    let (mut world, entity) = flat_world();
    assert_eq!(get(&mut world, entity, [3.0, 1.95, 4.0]), material::GRASS, "the settings component raised the ground to 2 m");
    assert_eq!(get(&mut world, entity, [3.0, 1.85, 4.0]), material::DIRT);
    assert_eq!(get(&mut world, entity, [3.0, 2.05, 4.0]), material::AIR);
}

#[test]
fn set_block_changes_exactly_one_block_and_is_journaled() {
    let (mut world, entity) = flat_world();
    // Cells are k * 0.1 m on the plane; any point inside a cell addresses it.
    set(&mut world, entity, [3.02, 2.07, 4.01], material::BRICK).unwrap();
    set(&mut world, entity, [5.05, 1.95, 5.05], 0).unwrap();
    assert_eq!(get(&mut world, entity, [3.05, 2.05, 4.05]), material::BRICK);
    assert_eq!(get(&mut world, entity, [3.15, 2.05, 4.05]), material::AIR, "the neighbour stays air");
    assert_eq!(get(&mut world, entity, [3.05, 2.15, 4.05]), material::AIR);
    assert_eq!(get(&mut world, entity, [5.05, 1.95, 5.05]), material::AIR);
    assert_eq!(get(&mut world, entity, [5.05, 1.85, 5.05]), material::DIRT);
    let terrain = world.get::<VoxelTerrainComponent>(entity).unwrap();
    assert_eq!((terrain.edits.len(), terrain.source_revision), (2, 2));
    assert!(set(&mut world, entity, [0.0, 0.0, 0.0], 99).is_err(), "unknown materials are rejected");
    assert_eq!(world.get::<VoxelTerrainComponent>(entity).unwrap().edits.len(), 2);
}

#[test]
fn fills_and_raycasts_through_the_script_surface() {
    let (mut world, entity) = flat_world();
    assert!((down(&mut world, entity, 20.0) - 8.0).abs() < 1e-6, "flat ground at 2 m");
    call(&mut world, entity, "fill_sphere", vec![Box::new(0.0f64), Box::new(2.0f64), Box::new(0.0f64), Box::new(1.0f64), Box::new(0u32)]).unwrap();
    assert!(down(&mut world, entity, 0.05) > 8.5, "the crater is deeper than the ground");
    call(&mut world, entity, "fill_cube", vec![Box::new(20.0f64), Box::new(3.0f64), Box::new(0.0f64), Box::new(0.5f64), Box::new(material::COBBLE)]).unwrap();
    assert!((down(&mut world, entity, 20.05) - 6.5).abs() < 0.11, "a cube 1 m high on top");
    let size = *call(&mut world, entity, "voxel_size", vec![]).unwrap().unwrap().downcast::<f64>().unwrap();
    assert_eq!(size, 0.1);
}

#[test]
fn legacy_sample_edits_on_generated_terrain_become_journaled_blocks() {
    let (mut world, entity) = flat_world();
    // Sample (30, 20, 40) is the block around (3.05, 2.05, 4.05).
    call(&mut world, entity, "paint_sample", vec![Box::new(30i64), Box::new(20i64), Box::new(40i64), Box::new(material::BRICK as u8)]).unwrap();
    assert_eq!(get(&mut world, entity, [3.05, 2.05, 4.05]), material::BRICK);
    call(&mut world, entity, "erase_sample", vec![Box::new(30i64), Box::new(19i64), Box::new(40i64)]).unwrap();
    assert_eq!(get(&mut world, entity, [3.05, 1.95, 4.05]), material::AIR);
    let terrain = world.get::<VoxelTerrainComponent>(entity).unwrap();
    assert!(terrain.payload_store().read().unwrap().1.is_empty(), "no live sample chunks the renderer would reject");
    assert_eq!(terrain.edits.len(), 2);
}

#[test]
fn the_cached_world_follows_the_journal_and_the_settings() {
    let (mut world, entity) = flat_world();
    let first = terrain_world(&world, entity).unwrap();
    assert!(std::sync::Arc::ptr_eq(&first, &terrain_world(&world, entity).unwrap()));
    set(&mut world, entity, [1.0, 2.05, 1.0], material::SAND).unwrap();
    let edited = terrain_world(&world, entity).unwrap();
    assert!(!std::sync::Arc::ptr_eq(&first, &edited));
    world.get_mut::<VoxelFlatTerrainComponent>(entity).unwrap().height = 5.0;
    let raised = terrain_world(&world, entity).unwrap();
    let (cell, _) = raised.grid().locate(DVec3::new(9.0, 4.95, 9.0));
    assert_eq!(raised.material(cell), material::GRASS);
}

#[test]
fn terrains_off_the_origin_are_rejected() {
    let (mut world, entity) = flat_world();
    world.insert(entity, Transform { position: [1.0, 0.0, 0.0], ..Default::default() });
    assert!(call(&mut world, entity, "get_block", vec![Box::new(0.0f64), Box::new(0.0f64), Box::new(0.0f64)]).is_err());
}

fn choice(id: &str, version: u32, name: &str) -> GeneratorInfo {
    GeneratorInfo { id: id.into(), version, name: name.into(), description: format!("{name} terrain"), settings_component: None }
}

#[test]
fn the_generator_picker_lists_registered_generators_and_searches_them() {
    use ui::dropdown::DropdownItem;
    let titles = |items: &[helio_component::voxel_generator_editor::GeneratorItem]| {
        items.iter().map(|i| i.title().to_string()).collect::<Vec<_>>()
    };
    let items = generator_items(&VoxelGeneratorRef::default(), &generators());
    assert_eq!(titles(&items), ["Flat", "Landform"]);
    assert!(items[1].matches("LAND") && items[1].matches("helio.landform") && items[1].matches("mountain"));
    assert!(!items[0].matches("mountain"));

    // A generator from a plugin that is not loaded stays selected; versions
    // of one generator are told apart.
    let current = VoxelGeneratorRef::new("x.moon", 3);
    let items = generator_items(&current, &[choice("x.caves", 1, "Caves"), choice("x.caves", 2, "Caves")]);
    assert_eq!(titles(&items), ["x.moon v3 (not loaded)", "Caves (v1)", "Caves (v2)"]);
    assert_eq!(items[0].value(), &current);
}
