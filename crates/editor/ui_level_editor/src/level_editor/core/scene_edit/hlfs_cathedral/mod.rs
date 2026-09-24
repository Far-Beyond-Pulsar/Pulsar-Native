//! Bakes the Helio HLFS cathedral demo into level data.
//!
//! The demo builds its scene procedurally (`indoor_cathedral_hlfs.rs` +
//! `cathedral_detail.rs`). This module reruns that exact geometry code once and
//! writes the result out as data: one `.mesh` per material batch under
//! `assets/meshes/cathedral/` and the level itself as `assets/default.level`.
//!
//! Run `cargo test -p ui_level_editor --lib hlfs_cathedral::generate -- --ignored`
//! to regenerate. The non-ignored tests check that the committed files still
//! match the generator's geometry and that the level loads.

mod architectural_mesh;
mod demo_data;
mod geometry;

use demo_data::{CANDLES, CHANDELIER_Z, COLUMN_Z, GLASS_LIGHTS};

use super::{
    components, level_io, objects, LevelEditorCameraState, LightType, MeshType, ObjectType,
    SceneObjectData, Transform,
};
use helio::{MeshUpload, PackedVertex};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::path::PathBuf;

const MESH_DIR: &str = "meshes/cathedral";
const GLASS_ALPHA: f32 = 0.65;
const MATERIAL_NAMES: [&str; 8] = [
    "Limestone",
    "Mouldings",
    "Basalt",
    "Marble",
    "Oak",
    "Bronze",
    "Wax",
    "Flame",
];
const GLASS_NAMES: [&str; 6] = ["Blue", "Red", "Green", "Amber", "Violet", "Cyan"];

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../..")
}

struct Batch {
    file: String,
    name: String,
    mesh: MeshUpload,
    material: Value,
}

fn material_json(
    base_color: [f32; 4],
    roughness: f32,
    metallic: f32,
    emissive: [f32; 3],
    emissive_intensity: f32,
    alpha: f32,
) -> Value {
    json!({
        "base_color": base_color,
        "metallic": metallic,
        "roughness": roughness,
        "emissive_color": emissive,
        "emissive_intensity": emissive_intensity,
        "alpha": alpha,
        "uv_scale_x": 1.0,
        "uv_scale_y": 1.0,
        "uv_offset_x": 0.0,
        "uv_offset_y": 0.0,
    })
}

fn build_batches() -> Vec<Batch> {
    let (meshes, panes, properties, colours) = geometry::build();
    let mut batches = Vec::new();
    for (index, (mut mesh, (colour, rough, metal, emission, strength))) in
        meshes.into_iter().zip(properties).enumerate()
    {
        if index == 0 {
            mesh.world_space_uv(2.0);
        }
        batches.push(Batch {
            file: format!("{MESH_DIR}/{}.mesh", MATERIAL_NAMES[index].to_lowercase()),
            name: MATERIAL_NAMES[index].to_string(),
            mesh: MeshUpload {
                vertices: mesh.vertices,
                indices: mesh.indices,
            },
            material: material_json(colour, rough, metal, emission, strength, 1.0),
        });
    }
    for (index, (mesh, colour)) in panes.into_iter().zip(colours).enumerate() {
        batches.push(Batch {
            file: format!("{MESH_DIR}/glass_{}.mesh", GLASS_NAMES[index].to_lowercase()),
            name: format!("Glass {}", GLASS_NAMES[index]),
            mesh: MeshUpload {
                vertices: mesh.vertices,
                indices: mesh.indices,
            },
            material: material_json(
                [colour[0], colour[1], colour[2], 1.0],
                0.12,
                0.0,
                colour,
                0.8,
                GLASS_ALPHA,
            ),
        });
    }
    batches
}

fn light_json(color: [f32; 3], intensity: f32, range: f32) -> Value {
    let mut light = helio_component::LightComponent::default();
    light.general.enabled = true;
    light.general.light_type = helio_component::LightType::Point;
    light.color.color = [color[0], color[1], color[2], 1.0];
    light.intensity.intensity = intensity;
    light.attenuation.range = range;
    serde_json::to_value(&light).expect("light serializes")
}

fn object(
    name: &str,
    object_type: ObjectType,
    position: [f32; 3],
    parent: Option<&str>,
) -> SceneObjectData {
    SceneObjectData {
        id: String::new(),
        name: name.to_string(),
        object_type,
        transform: Transform {
            position,
            ..Transform::default()
        },
        visible: true,
        locked: false,
        parent: parent.map(str::to_string),
        children: Vec::new(),
        scene_path: name.to_string(),
        props: HashMap::new(),
        component_instances: None,
    }
}

/// The demo's starting view in editor yaw/pitch (yaw 0 looks down -Z, toward
/// the altar).
fn start_camera() -> LevelEditorCameraState {
    LevelEditorCameraState {
        position: [0.0, 2.0, 24.0],
        yaw: 0.0,
        pitch: -0.05,
    }
}

type LightSpec = (String, [f32; 3], [f32; 3], f32, f32);

fn build_world(batches: &[Batch]) -> pulsar_scenedb::World {
    let mut world = pulsar_scenedb::World::new();
    let architecture = objects::add_folder(&mut world, "Architecture", None);
    for batch in batches {
        let id = objects::add_object(
            &mut world,
            object(
                &batch.name,
                ObjectType::Mesh(MeshType::Custom),
                [0.0; 3],
                Some(&architecture),
            ),
            Some(architecture.clone()),
        );
        components::add_component(
            &mut world,
            &id,
            "StaticMeshComponent".to_string(),
            super::static_mesh_component_json(&batch.file),
        );
        components::add_component(
            &mut world,
            &id,
            "MaterialOverrideComponent".to_string(),
            batch.material.clone(),
        );
    }

    let mut add_lights = |folder: &str, lights: Vec<LightSpec>| {
        let folder = objects::add_folder(&mut world, folder, None);
        for (name, position, color, intensity, range) in lights {
            let id = objects::add_object(
                &mut world,
                object(
                    &name,
                    ObjectType::Light(LightType::Point),
                    position,
                    Some(&folder),
                ),
                Some(folder.clone()),
            );
            components::add_component(
                &mut world,
                &id,
                "LightComponent".to_string(),
                light_json(color, intensity, range),
            );
        }
    };
    add_lights(
        "Chandeliers",
        CHANDELIER_Z
            .iter()
            .enumerate()
            .map(|(i, &z)| {
                (
                    format!("Chandelier {}", i + 1),
                    [0.0, 15.0, z],
                    [1.0, 0.92, 0.78],
                    160.0,
                    22.0,
                )
            })
            .collect(),
    );
    add_lights(
        "Stained Glass Lights",
        GLASS_LIGHTS
            .iter()
            .enumerate()
            .map(|(i, &(x, y, z, r, g, b))| {
                (
                    format!("Glass Light {}", i + 1),
                    [x, y, z],
                    [r, g, b],
                    35.0,
                    10.0,
                )
            })
            .collect(),
    );
    add_lights(
        "Altar Candles",
        CANDLES
            .iter()
            .enumerate()
            .map(|(i, &(x, y, z))| {
                (
                    format!("Candle {}", i + 1),
                    [x, y, z],
                    [1.0, 0.6, 0.15],
                    8.0,
                    4.0,
                )
            })
            .collect(),
    );
    world
}

#[test]
#[ignore = "writes assets/meshes/cathedral/*.mesh and assets/default.level"]
fn generate() {
    let root = repo_root();
    let batches = build_batches();
    for batch in &batches {
        let path = root.join("assets").join(&batch.file);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let id = helio_component::mesh_cache::content_id_for_bytes(&batch.mesh);
        std::fs::write(&path, helio_component::mesh_cache::encode(&batch.mesh, id)).unwrap();
    }
    let world = build_world(&batches);
    level_io::save_to_file_with_editor_camera(
        &world,
        root.join("assets/default.level"),
        Some(start_camera()),
    )
    .expect("level saves");
}

#[test]
fn geometry_matches_demo_triangle_count() {
    let batches = build_batches();
    let triangles: usize = batches.iter().map(|b| b.mesh.indices.len() / 3).sum();
    assert_eq!(batches.len(), 14);
    assert_eq!(triangles, 242_180);
}

fn vertex_bits(v: &PackedVertex) -> [u32; 10] {
    [
        v.position[0].to_bits(),
        v.position[1].to_bits(),
        v.position[2].to_bits(),
        v.bitangent_sign.to_bits(),
        v.tex_coords0[0].to_bits(),
        v.tex_coords0[1].to_bits(),
        v.tex_coords1[0].to_bits(),
        v.tex_coords1[1].to_bits(),
        v.normal,
        v.tangent,
    ]
}

fn vertex_matches(a: &PackedVertex, b: &PackedVertex) -> bool {
    let float_fields = |v: &PackedVertex| {
        [
            v.position[0],
            v.position[1],
            v.position[2],
            v.bitangent_sign,
            v.tex_coords0[0],
            v.tex_coords0[1],
            v.tex_coords1[0],
            v.tex_coords1[1],
        ]
    };
    // The generator uses transcendental and vector math whose last bits can
    // differ across architectures. Eight f32 epsilons stay below visual scale.
    float_fields(a)
        .into_iter()
        .zip(float_fields(b))
        .all(|(a, b)| {
            a.is_finite()
                && b.is_finite()
                && (a - b).abs() <= 8.0 * f32::EPSILON * a.abs().max(b.abs()).max(1.0)
        })
        && a.normal == b.normal
        && a.tangent == b.tangent
}

#[test]
fn committed_meshes_match_generator() {
    let root = repo_root();
    for batch in build_batches() {
        let bytes = std::fs::read(root.join("assets").join(&batch.file))
            .unwrap_or_else(|e| panic!("{} missing: {e}", batch.file));
        let (decoded, _) = helio_component::mesh_cache::decode(&bytes).expect("valid PMSH");
        assert_eq!(decoded.indices, batch.mesh.indices, "{}", batch.file);
        assert_eq!(decoded.vertices.len(), batch.mesh.vertices.len(), "{}", batch.file);
        if let Some((index, (committed, generated))) = decoded
            .vertices
            .iter()
            .zip(&batch.mesh.vertices)
            .enumerate()
            .find(|(_, (committed, generated))| !vertex_matches(committed, generated))
        {
            panic!(
                "{} vertex {index}: committed {:?}, generated {:?}",
                batch.file,
                vertex_bits(committed),
                vertex_bits(generated)
            );
        }
    }
}

#[test]
fn default_level_loads_the_cathedral() {
    let mut world = pulsar_scenedb::World::new();
    let camera = level_io::load_from_file_with_editor_camera(
        &mut world,
        repo_root().join("assets/default.level"),
    )
    .expect("default level loads");
    let all = objects::get_all_objects(&world);
    let meshes = all
        .iter()
        .filter(|o| matches!(o.object_type, ObjectType::Mesh(_)))
        .count();
    let lights = all
        .iter()
        .filter(|o| matches!(o.object_type, ObjectType::Light(_)))
        .count();
    assert_eq!((meshes, lights), (14, 17));
    assert_eq!(camera.expect("editor camera saved").position, [0.0, 2.0, 24.0]);
}
