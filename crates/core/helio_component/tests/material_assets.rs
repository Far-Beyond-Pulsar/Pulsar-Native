//! Mesh materials as world assets (SceneDB#84): each slot's material (a
//! surface document or a shader-graph folder) is counted as a reference of
//! the mesh however the value was written, loaded off the write path and
//! bound into the mesh; the draw reads only what was bound. A changed
//! material file, or a texture a graph samples, reloads the material in
//! every mesh using it.
//!
//! One test, because the project path sources resolve against is a process
//! global. No GPU: a graph is compiled against texture slots when a mirror
//! derives the draw, which the render acceptance tests cover.

use std::path::Path;
use std::time::Duration;

use helio_component::components::{
    StaticMeshComponent, StaticMeshMaterialSlot, mesh_reads_time, resolve_slot_material,
};
use helio_component::material_assets::{
    LoadedMaterial, material_asset_id_of, material_sources_for_file, resolve_material,
    slot_material_id,
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

fn resolve(world: &mut World) {
    assert!(
        world.resolve_assets_blocking(Duration::from_secs(30)),
        "material assets did not load"
    );
}

/// The base colour slot `index` of `entity`'s mesh draws with.
fn slot_color(world: &World, entity: Entity, index: usize) -> [f32; 4] {
    let mesh = world.get::<StaticMeshComponent>(entity).unwrap();
    let slot = &mesh.material_slots.slots[index];
    let id = slot_material_id(slot);
    let bound = mesh
        .materials
        .get(id)
        .map(|(version, material)| (id, version, material.as_ref()));
    resolve_material(Some(slot), bound, None).surface.base_color
}

fn is_graph(world: &World, entity: Entity, path: &str) -> bool {
    let mesh = world.get::<StaticMeshComponent>(entity).unwrap();
    mesh.materials
        .get(material_asset_id_of(path))
        .is_some_and(|(_, material)| matches!(material.as_ref(), LoadedMaterial::Graph(_)))
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
    let mesh = world.spawn();
    world.insert(mesh, mesh_with(&["materials/red.mat", "materials/glow"]));
    assert_eq!(world.asset_ref_count(red), 1);
    assert_eq!(world.asset_ref_count(glow), 1);
    let imported = slot_color(&world, mesh, 0);
    resolve(&mut world);
    assert_eq!(world.asset_state(red), AssetState::Ready);
    assert_eq!(slot_color(&world, mesh, 0), RED);
    assert_ne!(imported, RED, "nothing read in the write");
    assert!(is_graph(&world, mesh, "materials/glow"));
    assert!(!mesh_reads_time(
        world.get::<StaticMeshComponent>(mesh).unwrap()
    ));

    // ── A typed write swaps one slot's material; the other stays bound ─────
    world
        .get_mut::<StaticMeshComponent>(mesh)
        .unwrap()
        .material_slots
        .slots[0]
        .material_asset = "materials/blue.mat".into();
    assert_eq!(world.asset_ref_count(red), 0);
    assert_eq!(world.asset_ref_count(blue), 1);
    resolve(&mut world);
    assert_eq!(slot_color(&world, mesh, 0), BLUE);
    let bound = world.get::<StaticMeshComponent>(mesh).unwrap();
    assert!(
        bound.materials.get(red).is_none(),
        "an unnamed material is dropped"
    );

    // ── Adding a slot (a reimport with more sections) counts its material ──
    world
        .get_mut::<StaticMeshComponent>(mesh)
        .unwrap()
        .material_slots
        .slots
        .push(slot("materials/pulse"));
    let pulse = material_asset_id_of("materials/pulse");
    assert_eq!(world.asset_ref_count(pulse), 1);
    resolve(&mut world);
    assert!(mesh_reads_time(
        world.get::<StaticMeshComponent>(mesh).unwrap()
    ));

    // ── A surface override ignores the slot's material: no reference ───────
    world
        .get_mut::<StaticMeshComponent>(mesh)
        .unwrap()
        .material_slots
        .slots[2]
        .surface_override = Some(Default::default());
    assert_eq!(world.asset_ref_count(pulse), 0);
    resolve(&mut world);
    assert!(!mesh_reads_time(
        world.get::<StaticMeshComponent>(mesh).unwrap()
    ));

    // ── Reload: the document changes, every mesh using it is rebound ───────
    let other = world.spawn();
    world.insert(other, mesh_with(&["materials/blue.mat"]));
    resolve(&mut world);
    assert_eq!(slot_color(&world, other, 0), BLUE, "shared, already loaded");
    let version = world.asset_version(blue);
    write_surface(&materials.join("blue.mat"), GREEN);
    let sources = material_sources_for_file(&materials.join("blue.mat"));
    assert_eq!(sources, vec!["materials/blue.mat".to_string()]);
    for source in &sources {
        assert_eq!(world.asset_source_changed(source), 1);
    }
    resolve(&mut world);
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
    resolve(&mut world);
    assert!(world.asset_version(glow) > glow_version);
    assert!(is_graph(&world, mesh, "materials/glow"));

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
        resolve(&mut world);
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
    assert_eq!(world.asset_ref_count(red), 1);
    resolve(&mut world);
    assert_eq!(slot_color(&world, level_mesh, 0), RED);
    // Saving it writes the slot's path, never the loaded material.
    let saved =
        serde_json::to_value(world.get::<StaticMeshComponent>(level_mesh).unwrap()).unwrap();
    assert!(saved.get("materials").is_none());

    // ── Outside a world, a tool resolves a slot by loading it now ──────────
    assert_eq!(
        resolve_slot_material(Some(&slot("materials/red.mat")), None)
            .surface
            .base_color,
        RED
    );

    world.despawn(level_mesh);
    world.despawn(other);
    world.despawn(mesh);
    for id in [red, blue, glow, pulse] {
        assert_eq!(world.asset_ref_count(id), 0);
    }
}
