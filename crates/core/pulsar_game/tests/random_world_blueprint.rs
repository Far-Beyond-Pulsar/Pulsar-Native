//! The random world example (`assets/examples/random_world.blueprint.json`):
//! a Blueprint graph that builds a planet, a moon or a desert world from a
//! seed, No Man's Sky style, only through the terrain's script natives and
//! the seeded random nodes. Compiled from the saved graph and run on the
//! script VM against a real terrain entity: the blueprint path end to end.

use std::sync::Arc;

use blueprint_compiler::{authored::expand_graph, compile, ClassSource, VariableSource};
use blueprint_graph::codec::deserialize_blueprint;
use helio_component::voxel_world::terrain_world;
use helio_component::{VoxelLayerKind, VoxelMaterialStyle, VoxelTerrainComponent, VoxelTerrainLayersComponent, VoxelTerrainStack};
use pulsar_scenedb::World;
use pulsar_script_vm::{Budget, Host, NativeRegistry, Program, Value, Vm};

const EXAMPLE: &str = include_str!("../../../../assets/examples/random_world.blueprint.json");

fn program() -> Program {
    // Linking pulsar_std is what registers its nodes as natives.
    assert!(!pulsar_std::get_all_nodes().is_empty());
    let asset = deserialize_blueprint(EXAMPLE).expect("the example parses");
    let graph = expand_graph(&asset.main_graph, asset.local_macros.iter().map(|m| (m.id.clone(), m.graph.clone()))).expect("the example lowers");
    let natives = NativeRegistry::with_engine_natives();
    let variables = [VariableSource { id: Some("var_seed".into()), name: "seed".into(), type_name: "i64".into(), default: Some(serde_json::json!(7)) }];
    let source = ClassSource { name: "RandomWorld", graph: &graph, variables: &variables, events: &[], known_events: &[], version: 0 };
    let module = compile(&source, &natives).unwrap_or_else(|d| panic!("the example compiles: {d:?}"));
    Program::link(Arc::new(module), &natives).expect("the example links")
}

/// The stack the example builds for `seed` on a fresh Earth-preset entity.
fn build(program: &Program, seed: i64) -> VoxelTerrainStack {
    let mut world = World::new();
    let entity = world.spawn();
    world.insert(entity, VoxelTerrainComponent::planet(1_000_000.0));
    world.insert(entity, VoxelTerrainLayersComponent::default());
    let mut instance = program.instantiate();
    program.set_var(&mut instance, program.variable("seed").unwrap(), Value::Int(seed)).unwrap();
    Vm::new()
        .call(program, &mut instance, program.entry("begin_play").unwrap(), &[], &mut Host::new(&mut world, entity), &mut Budget::new(100_000))
        .unwrap_or_else(|e| panic!("seed {seed}: begin_play failed: {e}"));
    // The generator accepts it: the planet builds.
    terrain_world(&world, entity).unwrap_or_else(|e| panic!("seed {seed}: {e}"));
    world.get::<VoxelTerrainLayersComponent>(entity).unwrap().stack.clone()
}

#[test]
fn the_random_world_example_builds_varied_reproducible_worlds() {
    let program = program();
    let mut styles = std::collections::BTreeSet::new();
    let mut crater_sizes = Vec::new();
    for seed in 0..16 {
        let stack = build(&program, seed);
        styles.insert(format!("{:?}", stack.materials));
        let n = stack.layers.len();
        assert_eq!(stack.layers[n - 2].kind, VoxelLayerKind::Craters, "seed {seed}");
        assert_eq!(stack.layers[n - 1].kind, VoxelLayerKind::Hills, "seed {seed}");
        let craters = &stack.layers[n - 2];
        assert!((5.0..60.0).contains(&craters.scale_km) && (0.05..0.5).contains(&craters.coverage), "seed {seed}: {craters:?}");
        let hills = &stack.layers[n - 1];
        assert!((50.0..800.0).contains(&hills.height_m) && (2.0..40.0).contains(&hills.scale_km), "seed {seed}: {hills:?}");
        crater_sizes.push(craters.scale_km);
    }
    eprintln!("styles {styles:?}, crater sizes {crater_sizes:?}");
    // Moons, deserts and Earth-like planets all occur.
    for style in [VoxelMaterialStyle::Lunar, VoxelMaterialStyle::Rules, VoxelMaterialStyle::Earthlike] {
        assert!(styles.contains(&format!("{style:?}")), "{style:?} never rolled: {styles:?}");
    }
    crater_sizes.sort_by(f64::total_cmp);
    crater_sizes.dedup();
    assert!(crater_sizes.len() > 12, "seeds vary the worlds");
    // Equal seeds build equal worlds.
    assert_eq!(build(&program, 11), build(&program, 11));
}
