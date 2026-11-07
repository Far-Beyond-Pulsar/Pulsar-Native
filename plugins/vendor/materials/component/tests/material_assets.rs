//! Mesh materials as world assets (SceneDB#84) through material instances:
//! each slot draws with an instance (`helio_mats::instance`) the meshes'
//! subsystem acquires for it, shared by every slot drawing the same
//! material. The instance's material (a surface document or a shader-graph
//! folder) is counted as a reference however the mesh was written, loaded
//! off the write path and bound into the instance; the draw reads only what
//! was bound. A changed material file, or a texture a graph samples,
//! reloads the material once for every mesh using it.
//!
//! The meshes read the materials through the materials' api only
//! (`helio_mats`); this crate's loader serves them.
//!
//! One test, because the project path sources resolve against is a process
//! global. No GPU: a graph is compiled against texture slots when a mirror
//! derives the draw, which the render acceptance tests cover.

use std::path::Path;
use std::time::Duration;

use helio_mats::instance::MaterialInstanceComponent;
use materials_component::assets::material_sources_for_file;
use meshes_component::components::{
    StaticMeshComponent, StaticMeshMaterialSlot, resolve_slot_material,
};
use meshes_component::mesh_materials::{
    MeshMaterialInstances, MeshMaterials, material_asset_id_of,
};
use pulsar_scenedb::{AssetState, Entity, World};

fn write_surface(path: &Path, base_color: [f32; 4]) {
    std::fs::write(
        path,
        serde_json::json!({ "version": 1, "base_color": base_color }).to_string(),
    )
    .unwrap();
}

/// A graph whose base colour samples `texture` (a project-relative path),
/// and whose emissive animates with the clock when `animated`.
fn write_graph(folder: &Path, texture: &str, animated: bool) {
    use psgc::*;
    fn node(id: &str, kind: &str) -> NodeInstance {
        let meta = get_shader_nodes()
            .into_iter()
            .find(|m| m.name == kind)
            .unwrap_or_else(|| panic!("no shader node {kind}"));
        let mut node = NodeInstance::new(id, kind, Position { x: 0.0, y: 0.0 });
        for p in meta.params {
            node.inputs.push(PinInstance::new(
                &p.name,
                Pin::new(
                    &p.name,
                    &p.name,
                    DataType::Data(TypeInfo::new(&p.param_type)),
                    PinType::Input,
                ),
            ));
        }
        node
    }
    let connect = |graph: &mut GraphDescription, from: &str, to: &str, pin: &str| {
        graph.add_connection(Connection::new(
            from,
            "result",
            to,
            pin,
            ConnectionType::Data,
        ));
    };
    let mut graph = GraphDescription::new("material asset test");
    graph.add_node(node("output", "fragment_output"));
    graph.add_node(node("uv", "frag_uv"));
    let mut sample = node("texture", "sample_texture");
    sample
        .properties
        .insert("texture".into(), serde_json::json!(texture));
    graph.add_node(sample);
    connect(&mut graph, "uv", "texture", "uv");
    connect(&mut graph, "texture", "output", "base_color");
    if animated {
        graph.add_node(node("time", "time"));
        connect(&mut graph, "time", "output", "emissive_color");
    }
    std::fs::create_dir_all(folder).unwrap();
    std::fs::write(
        folder.join("shader_graph_save.json"),
        serde_json::json!({ "main_graph": graph }).to_string(),
    )
    .unwrap();
}

fn write_png(path: &Path, rgba: [u8; 4]) {
    image::save_buffer(path, &rgba, 1, 1, image::ColorType::Rgba8).unwrap();
}

fn slot(material: &str) -> StaticMeshMaterialSlot {
    StaticMeshMaterialSlot {
        material_asset: material.into(),
        ..Default::default()
    }
}

fn mesh_with(slots: &[&str]) -> StaticMeshComponent {
    let mut mesh = StaticMeshComponent::default();
    mesh.material_slots.slots = slots.iter().map(|material| slot(material)).collect();
    mesh
}

/// Load what the instances name, after the meshes' subsystem caught up.
fn resolve(world: &mut World, instances: &mut MeshMaterialInstances) {
    instances.step(world);
    assert!(
        world.resolve_assets_blocking(Duration::from_secs(30)),
        "material assets did not load"
    );
}

/// The instance slot `index` of `entity`'s mesh draws with.
fn instance(world: &World, entity: Entity, index: usize) -> &MaterialInstanceComponent {
    world
        .get::<MaterialInstanceComponent>(slot_instance(world, entity, index))
        .expect("the slot has an instance")
}

/// The instance entity of slot `index` of `entity`'s mesh.
fn slot_instance(world: &World, entity: Entity, index: usize) -> Entity {
    world
        .get::<MeshMaterials>(entity)
        .expect("the mesh has its instances")
        .0[index]
}

/// The base colour slot `index` of `entity`'s mesh draws with.
fn slot_color(world: &World, entity: Entity, index: usize) -> [f32; 4] {
    instance(world, entity, index).resolve(None).row.base_color
}

/// Whether slot `index` of `entity`'s mesh is bound to a graph: a material
/// that draws only in a scene (its textures take slots there).
fn is_graph(world: &World, entity: Entity, index: usize) -> bool {
    let instance = instance(world, entity, index);
    let id = instance.source().asset_id();
    instance
        .bound()
        .is_some_and(|material| material.resolve(id, 0, None, &Default::default()).is_err())
}

/// Whether any of `entity`'s slots draws with a material reading the clock.
fn reads_time(world: &World, entity: Entity) -> bool {
    let held = world.get::<MeshMaterials>(entity).unwrap();
    held.0.iter().any(|instance| {
        world
            .get::<MaterialInstanceComponent>(*instance)
            .is_some_and(|instance| instance.reads_time())
    })
}

const RED: [f32; 4] = [1.0, 0.0, 0.0, 1.0];
const BLUE: [f32; 4] = [0.0, 0.0, 1.0, 1.0];
const GREEN: [f32; 4] = [0.0, 1.0, 0.0, 1.0];

#[test]
fn materials_load_rebind_reload_and_survive_bad_input() {
    let project = tempfile::tempdir().unwrap();
    let materials = project.path().join("materials");
    let textures = project.path().join("textures");
    std::fs::create_dir_all(&materials).unwrap();
    std::fs::create_dir_all(&textures).unwrap();
    write_surface(&materials.join("red.mat"), RED);
    write_surface(&materials.join("blue.mat"), BLUE);
    std::fs::write(materials.join("bad.mat"), b"not a material").unwrap();
    write_png(&textures.join("t.png"), [255, 255, 255, 255]);
    write_graph(&materials.join("glow"), "textures/t.png", false);
    write_graph(&materials.join("pulse"), "textures/t.png", true);
    engine_state::EngineContext::new().set_global();
    engine_state::set_project_path(project.path().display().to_string());

    let red = material_asset_id_of("materials/red.mat");
    let blue = material_asset_id_of("materials/blue.mat");
    let glow = material_asset_id_of("materials/glow");
    assert_eq!(
        red,
        material_asset_id_of(&materials.join("red.mat").display().to_string()),
        "a path inside the project names the same material however it is spelled"
    );
    assert_eq!(red, material_asset_id_of("materials\\./red.mat"));

    // ── A typed insert names its slots' materials; the world loads them ────
    let mut world = World::new();
    let mut instances = MeshMaterialInstances::default();
    let mesh = world.spawn();
    world.insert(mesh, mesh_with(&["materials/red.mat", "materials/glow"]));
    instances.step(&mut world);
    assert_eq!(world.asset_ref_count(red), 1);
    assert_eq!(world.asset_ref_count(glow), 1);
    let imported = slot_color(&world, mesh, 0);
    resolve(&mut world, &mut instances);
    assert_eq!(world.asset_state(red), AssetState::Ready);
    assert_eq!(slot_color(&world, mesh, 0), RED);
    assert_ne!(imported, RED, "nothing read in the write");
    assert!(is_graph(&world, mesh, 1));
    assert!(!reads_time(&world, mesh));

    // ── A typed write swaps one slot's material; the other stays bound ─────
    world
        .get_mut::<StaticMeshComponent>(mesh)
        .unwrap()
        .material_slots
        .slots[0]
        .material_asset = "materials/blue.mat".into();
    instances.step(&mut world);
    assert_eq!(
        world.asset_ref_count(red),
        0,
        "an unnamed material is dropped"
    );
    assert_eq!(world.asset_ref_count(blue), 1);
    resolve(&mut world, &mut instances);
    assert_eq!(slot_color(&world, mesh, 0), BLUE);

    // ── Adding a slot (a reimport with more sections) counts its material ──
    world
        .get_mut::<StaticMeshComponent>(mesh)
        .unwrap()
        .material_slots
        .slots
        .push(slot("materials/pulse"));
    let pulse = material_asset_id_of("materials/pulse");
    instances.step(&mut world);
    assert_eq!(world.asset_ref_count(pulse), 1);
    resolve(&mut world, &mut instances);
    assert!(reads_time(&world, mesh));

    // ── A surface override ignores the slot's material: no reference ───────
    world
        .get_mut::<StaticMeshComponent>(mesh)
        .unwrap()
        .material_slots
        .slots[2]
        .surface_override = Some(Default::default());
    instances.step(&mut world);
    assert_eq!(world.asset_ref_count(pulse), 0);
    resolve(&mut world, &mut instances);
    assert!(!reads_time(&world, mesh));

    // ── Reload: the document changes, every mesh using it draws it ─────────
    let other = world.spawn();
    world.insert(other, mesh_with(&["materials/blue.mat"]));
    resolve(&mut world, &mut instances);
    assert_eq!(slot_color(&world, other, 0), BLUE, "shared, already loaded");
    assert_eq!(
        slot_instance(&world, other, 0),
        slot_instance(&world, mesh, 0),
        "one instance for every slot drawing the same material"
    );
    assert_eq!(world.asset_ref_count(blue), 1);
    let version = world.asset_version(blue);
    write_surface(&materials.join("blue.mat"), GREEN);
    let sources = material_sources_for_file(&materials.join("blue.mat"));
    assert_eq!(sources, vec!["materials/blue.mat".to_string()]);
    for source in &sources {
        assert_eq!(world.asset_source_changed(source), 1);
    }
    resolve(&mut world, &mut instances);
    assert!(world.asset_version(blue) > version);
    assert_eq!(slot_color(&world, mesh, 0), GREEN);
    assert_eq!(slot_color(&world, other, 0), GREEN);

    // ── A texture a graph samples reloads the graph; a graph save its folder
    let glow_version = world.asset_version(glow);
    let sources = material_sources_for_file(&textures.join("t.png"));
    assert!(
        sources.contains(&"materials/glow".to_string()),
        "{sources:?}"
    );
    let save = materials.join("glow").join("shader_graph_save.json");
    assert!(material_sources_for_file(&save).contains(&"materials/glow".to_string()));
    for source in &sources {
        world.asset_source_changed(source);
    }
    resolve(&mut world, &mut instances);
    assert!(world.asset_version(glow) > glow_version);
    assert!(is_graph(&world, mesh, 1));

    // ── Missing and corrupt materials: the imported surface, no panic ──────
    for path in [
        "materials/missing.mat",
        "materials/bad.mat",
        "materials/nothing",
    ] {
        world
            .get_mut::<StaticMeshComponent>(other)
            .unwrap()
            .material_slots
            .slots[0]
            .material_asset = path.into();
        resolve(&mut world, &mut instances);
        assert!(
            matches!(
                world.asset_state(material_asset_id_of(path)),
                AssetState::Failed(_)
            ),
            "{path}"
        );
        assert_eq!(slot_color(&world, other, 0), imported, "{path}");
    }

    // ── A mesh decoded from a level counts and binds the same way ──────────
    let decoded = pulsar_world_registry::decode_world_component_value(
        "StaticMeshComponent",
        &serde_json::json!({
            "mesh_asset": "",
            "material_slots": { "slots": [{ "name": "a", "material_asset": "materials/red.mat" }] }
        }),
    )
    .expect("registered")
    .expect("decodes");
    let level_mesh = world.spawn();
    world.insert(
        level_mesh,
        *decoded.downcast::<StaticMeshComponent>().unwrap(),
    );
    instances.step(&mut world);
    assert_eq!(world.asset_ref_count(red), 1);
    resolve(&mut world, &mut instances);
    assert_eq!(slot_color(&world, level_mesh, 0), RED);
    // Saving it writes the slot's path, never the loaded material.
    let saved =
        serde_json::to_value(world.get::<StaticMeshComponent>(level_mesh).unwrap()).unwrap();
    assert!(saved.get("materials").is_none());

    // ── Outside a world, a tool resolves a slot by loading it now ──────────
    assert_eq!(
        resolve_slot_material(Some(&slot("materials/red.mat")), None)
            .row
            .base_color,
        RED
    );

    world.despawn(level_mesh);
    world.despawn(other);
    world.despawn(mesh);
    instances.step(&mut world);
    assert_eq!(
        world.query::<&MaterialInstanceComponent>().count(),
        0,
        "the last user's release despawns an instance"
    );
    for id in [red, blue, glow, pulse] {
        assert_eq!(world.asset_ref_count(id), 0);
    }
}
