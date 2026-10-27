use helio_component::{VoxelTerrainComponent, VoxelTerrainLayersComponent};
use pulsar_reflection::EngineClass;

#[test]
fn voxel_terrain_defaults_and_round_trips_as_scene_component_data() {
    let terrain = VoxelTerrainComponent::default();
    assert!(terrain.appearance_parameters.is_empty());
    assert_eq!(terrain.air, helio_component::VoxelAir::Custom);
    // New terrain rows start as a 4 km plane of Helio's terrain generator.
    assert_eq!(
        terrain.generator.id,
        helio_voxel_data::VOXEL_TERRAIN_GENERATOR
    );
    assert_eq!(terrain.voxel_size, 0.1);
    let terrain_json = serde_json::to_value(&terrain).expect("serialize terrain component");
    let terrain_restored: VoxelTerrainComponent =
        serde_json::from_value(terrain_json).expect("restore terrain component");
    assert_eq!(terrain_restored.generator.id, terrain.generator.id);
    assert_eq!(
        terrain_restored.generator.version,
        helio_voxel_data::VOXEL_TERRAIN_GENERATOR_VERSION
    );
    assert_eq!(terrain_restored.source_revision, terrain.source_revision);
    let mut older_json = serde_json::to_value(&terrain).unwrap();
    older_json
        .as_object_mut()
        .unwrap()
        .remove("generator_version");
    older_json
        .as_object_mut()
        .unwrap()
        .remove("appearance_parameters");
    let older: VoxelTerrainComponent = serde_json::from_value(older_json).unwrap();
    // Files without a version use the generator's registered version.
    assert_eq!(older.generator.version, 0);
    assert!(older.appearance_parameters.is_empty());
}

#[test]
fn the_inspector_shows_the_whole_world_description() {
    let terrain = VoxelTerrainComponent::default();
    let names: Vec<_> = terrain
        .get_properties()
        .iter()
        .map(|property| property.name)
        .collect();
    assert_eq!(
        names,
        [
            "enabled",
            "shape",
            "planet_radius",
            "plane_size",
            "voxel_size",
            "generator",
            "seed",
            "features",
            "air",
            "air_density",
            "air_tint",
            "air_haze",
            "appearance_parameters",
            "editable"
        ]
    );
    // Bookkeeping stays serialized but internal; a world has no chunk
    // layout of its own.
    let json = serde_json::to_value(&terrain).unwrap();
    for key in ["generator_id", "generator_version", "source_revision"] {
        assert!(json.get(key).is_some(), "{key} is serialized");
    }
    for key in [
        "chunk_edge_voxels",
        "max_chunk_lod",
        "lod_scale",
        "domain_mode",
        "renderer_id",
        "material_ids",
    ] {
        assert!(json.get(key).is_none(), "{key} is gone");
    }
    assert!(
        json.get("generator").is_none(),
        "the generator reference is flattened"
    );
    // The features list and its nested lists have the inspector's editors.
    for type_id in [
        std::any::TypeId::of::<Vec<helio_component::VoxelFeatureSpawn>>(),
        std::any::TypeId::of::<Vec<helio_component::VoxelFeatureGrowth>>(),
        std::any::TypeId::of::<helio_component::VoxelMaterialList>(),
    ] {
        assert!(
            pulsar_reflection::inventory::iter::<pulsar_reflection::UiPropertyEditorHint>
                .into_iter()
                .any(|hint| hint.type_id == type_id),
            "{type_id:?} has an editor"
        );
    }
}

#[test]
fn voxel_terrain_hydrates_as_a_typed_scenedb_world_row() {
    let mut world = pulsar_scenedb::World::new();
    let entity = world.spawn();
    let mut terrain = VoxelTerrainComponent::default();
    terrain.generator.id = "test.generator".into();
    terrain.seed = 1234;
    terrain.appearance_parameters = r#"{"detail":[0.6,0.12,0.04,0.0]}"#.into();
    let terrain_json = serde_json::to_value(&terrain).unwrap();
    assert!(pulsar_world_registry::hydrate_world_component_for_class(
        "VoxelTerrainComponent",
        &mut world,
        entity,
        &terrain_json,
    )
    .unwrap());
    let hydrated = world
        .get::<VoxelTerrainComponent>(entity)
        .expect("typed SceneDB component");
    assert_eq!(hydrated.generator.id, "test.generator");
    assert_eq!(hydrated.seed, 1234);
    assert_eq!(
        hydrated.appearance_parameters,
        terrain.appearance_parameters
    );
}

/// The layer stack component serializes to the generator's settings JSON:
/// each preset is the generator's preset, and its layers are reflected.
#[test]
fn terrain_layers_component_is_the_generator_settings() {
    use helio_pass_voxel_planet::layers::TerrainLayers;
    for (component, stack) in [
        (VoxelTerrainLayersComponent::earth(), TerrainLayers::earth()),
        (VoxelTerrainLayersComponent::moon(), TerrainLayers::moon()),
        (
            VoxelTerrainLayersComponent::flat(2.0),
            TerrainLayers::flat_at(2.0),
        ),
        (
            VoxelTerrainLayersComponent::desert(),
            TerrainLayers::desert(),
        ),
    ] {
        let json = serde_json::to_value(&component).unwrap();
        let parsed: TerrainLayers = serde_json::from_value(json.clone()).unwrap();
        assert_eq!(parsed, stack, "{json}");
        let restored: VoxelTerrainLayersComponent =
            serde_json::from_value(serde_json::to_value(&stack).unwrap()).unwrap();
        assert_eq!(serde_json::to_value(&restored).unwrap(), json);
    }
    // Empty settings are the Earth preset, like the generator's.
    let empty: VoxelTerrainLayersComponent = serde_json::from_str("{}").unwrap();
    assert_eq!(
        serde_json::to_value(empty).unwrap(),
        serde_json::to_value(VoxelTerrainLayersComponent::earth()).unwrap()
    );
    // One property: the whole stack, edited (and undone) as one value.
    let properties = VoxelTerrainLayersComponent::default().get_properties();
    assert_eq!(
        properties.iter().map(|p| p.name).collect::<Vec<_>>(),
        ["stack"]
    );
}

/// A layer added, or switched to another kind, in the inspector starts from
/// that kind's defaults (the generator's table), keeping its toggle and mask.
#[test]
fn layers_take_their_kinds_defaults() {
    use helio_component::components::voxel_stack_editor::set_layer;
    use helio_component::{VoxelLayerKind, VoxelLayerMask, VoxelTerrainLayer};
    use helio_pass_voxel_planet::layers::{Layer, LayerKind};
    for kind in [
        "Hills",
        "Warp",
        "Continents",
        "Mountains",
        "Roughness",
        "Erosion",
        "Craters",
        "Basins",
        "Plateau",
    ] {
        let ours: VoxelLayerKind = serde_json::from_value(kind.into()).unwrap();
        let theirs: LayerKind = serde_json::from_value(kind.into()).unwrap();
        assert_eq!(
            serde_json::to_value(VoxelTerrainLayer::new(ours)).unwrap(),
            serde_json::to_value(Layer::new(theirs)).unwrap(),
            "{kind}"
        );
    }
    let craters = VoxelTerrainLayer::new(VoxelLayerKind::Craters);
    assert!(
        craters.ratio > 0.0 && craters.octaves > 4,
        "craters have depth: {craters:?}"
    );

    let mut layer = VoxelTerrainLayer {
        mask: VoxelLayerMask::Land,
        enabled: false,
        ..VoxelTerrainLayer::new(VoxelLayerKind::Hills)
    };
    // A field edit keeps the kind and just stores the value.
    let edited = VoxelTerrainLayer {
        height_m: 7.0,
        ..layer.clone()
    };
    assert!(!set_layer(&mut layer, edited));
    assert_eq!(layer.height_m, 7.0);
    // A kind edit brings the new kind's numbers, not the old ones.
    let edited = VoxelTerrainLayer {
        kind: VoxelLayerKind::Craters,
        ..layer.clone()
    };
    assert!(set_layer(&mut layer, edited));
    assert_eq!(
        layer,
        VoxelTerrainLayer {
            mask: VoxelLayerMask::Land,
            enabled: false,
            ..craters
        }
    );
}
