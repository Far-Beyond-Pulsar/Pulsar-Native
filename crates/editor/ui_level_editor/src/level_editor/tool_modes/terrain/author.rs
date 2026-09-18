//! Foliage authoring: turns scattered [`InstanceSpec`]s into scene content.
//!
//! This file is the **only** place foliage painting touches the scene
//! database. Placement rules live in [`super::scatter`] and the sets model in
//! `state::foliage_sets`; everything here is "given these instances, make them
//! exist" — so when authoring moves onto `pulsar_scenedb` (or onto the
//! renderer's mesh-foliage path), this is the one file that changes.
//!
//! # Current backend: static-mesh scene objects
//!
//! Each instance becomes an ordinary scene object carrying a
//! `StaticMeshComponent`, parented under one folder per set
//! (`Foliage · <set name>`) so a forest is one collapsed hierarchy node, not
//! hundreds of siblings. One stamp is **one undo step** regardless of how
//! many instances it placed.

use engine_backend::services::terrain_edit::{Ray3, TerrainEditApi, TerrainHit};
use glam::{EulerRot, Quat, Vec3};
use helio_component::StaticMeshComponent;

use super::scatter::{scatter, tangent_basis, InstanceSpec};
use crate::level_editor::scene_database::{ObjectType, SceneObjectData, Transform};
use crate::level_editor::state::LevelEditorState;

/// How far above the intended ground point the projection ray starts. The
/// terrain hit test refines within ±6.4 m of the analytic surface, so this
/// must stay comfortably above that window.
const PROJECTION_LIFT_M: f32 = 8.0;

/// Scatter every enabled foliage member around `hit` and author the result.
/// Returns how many instances were placed.
pub fn stamp_foliage_sets(
    state: &mut LevelEditorState,
    api: &TerrainEditApi,
    hit: &TerrainHit,
    seed: u64,
) -> usize {
    let (radius_m, density) = {
        let terrain = &state.editor.terrain;
        (terrain.foliage.radius_m, terrain.foliage_paint_density.0)
    };
    let specs = scatter(&state.editor.terrain.foliage_sets, radius_m, density, seed);
    if specs.is_empty() {
        return 0;
    }

    let placed: Vec<(InstanceSpec, PlacedTransform)> = specs
        .into_iter()
        .filter_map(|spec| {
            let transform = project(api, hit, &spec)?;
            Some((spec, transform))
        })
        .collect();
    if placed.is_empty() {
        return 0;
    }

    // One checkpoint for the whole stamp, captured before the first write.
    let pre = state.scene.capture_history_snapshot();
    let mut added = 0;
    for (spec, transform) in &placed {
        let Some(parent) = ensure_set_folder(state, &spec.set_name) else {
            continue;
        };
        let object = instance_object(spec, transform);
        if !state
            .scene
            .database
            .add_object(object, Some(parent))
            .is_empty()
        {
            added += 1;
        }
    }
    if added > 0 {
        state.scene.bump_revision(true);
        state.scene.commit_undo_checkpoint(pre);
    }
    added
}

struct PlacedTransform {
    position: [f32; 3],
    rotation_deg: [f32; 3],
    scale: f32,
}

/// Put `spec` on the actual surface: cast a ray down the brush normal from
/// above its tangent-plane point and take the refined terrain hit.
fn project(api: &TerrainEditApi, hit: &TerrainHit, spec: &InstanceSpec) -> Option<PlacedTransform> {
    let normal = Vec3::from_array(hit.normal).normalize_or_zero();
    if normal == Vec3::ZERO {
        return None;
    }
    let (tangent, bitangent) = tangent_basis(normal.to_array());
    let base = Vec3::from_array(hit.position_m)
        + Vec3::from_array(tangent) * spec.offset_m[0]
        + Vec3::from_array(bitangent) * spec.offset_m[1];
    let ground = api.hit_terrain(Ray3 {
        origin: (base + normal * PROJECTION_LIFT_M).to_array(),
        direction: (-normal).to_array(),
    })?;

    let surface_normal = Vec3::from_array(ground.normal).normalize_or_zero();
    let up = if spec.align_to_normal && surface_normal != Vec3::ZERO {
        surface_normal
    } else {
        Vec3::Y
    };
    let position = Vec3::from_array(ground.position_m) + surface_normal * spec.ground_offset_m;
    let rotation = Quat::from_rotation_arc(Vec3::Y, up) * Quat::from_rotation_y(spec.yaw_rad);
    let (y, x, z) = rotation.to_euler(EulerRot::YXZ);

    Some(PlacedTransform {
        position: position.to_array(),
        rotation_deg: [x.to_degrees(), y.to_degrees(), z.to_degrees()],
        scale: spec.scale,
    })
}

fn instance_object(spec: &InstanceSpec, transform: &PlacedTransform) -> SceneObjectData {
    let mut component = StaticMeshComponent::default();
    component.mesh_asset = spec.mesh.clone().into();
    let data = serde_json::to_value(&component)
        .expect("StaticMeshComponent always serializes: plain leaves");

    SceneObjectData {
        id: String::new(),
        name: file_stem(&spec.mesh),
        object_type: ObjectType::Mesh(crate::level_editor::scene_database::MeshType::Custom),
        transform: Transform {
            position: transform.position,
            rotation: transform.rotation_deg,
            scale: [transform.scale; 3],
        },
        visible: true,
        locked: false,
        parent: None,
        children: Vec::new(),
        scene_path: String::new(),
        props: Default::default(),
        component_instances: Some(serde_json::json!([
            {
                "class_name": "StaticMeshComponent",
                "enabled": true,
                "data": data,
            }
        ])),
    }
}

fn file_stem(path: &str) -> String {
    let name = path.rsplit(['/', '\\']).next().unwrap_or(path);
    name.rsplit_once('.')
        .map(|(stem, _)| stem)
        .unwrap_or(name)
        .to_string()
}

/// Name of the folder that holds a set's instances.
pub fn set_folder_name(set_name: &str) -> String {
    format!("{SET_FOLDER_PREFIX}{set_name}")
}

/// The folder object for `set_name`, created on first use.
fn ensure_set_folder(state: &mut LevelEditorState, set_name: &str) -> Option<String> {
    let name = set_folder_name(set_name);
    if let Some(existing) = state
        .scene
        .database
        .get_all_objects()
        .into_iter()
        .find(|o| o.object_type == ObjectType::Folder && o.name == name)
    {
        return Some(existing.id);
    }
    let folder = SceneObjectData {
        id: String::new(),
        name,
        object_type: ObjectType::Folder,
        transform: Transform::default(),
        visible: true,
        locked: false,
        parent: None,
        children: Vec::new(),
        scene_path: String::new(),
        props: Default::default(),
        component_instances: None,
    };
    let id = state.scene.database.add_object(folder, None);
    (!id.is_empty()).then_some(id)
}

/// Prefix shared by every set folder ([`set_folder_name`]); erase only ever
/// touches objects parented under a folder with this prefix, so it can never
/// delete anything the user placed by hand.
const SET_FOLDER_PREFIX: &str = "Foliage · ";

/// Remove painted foliage within `radius_m` of `hit`. `erase_density` is the
/// fraction removed (1.0 removes all under the brush), resolved per instance
/// from `seed` so a stroke is reproducible. One undo step for the whole
/// stroke segment. Returns how many instances were removed.
pub fn erase_foliage(
    state: &mut LevelEditorState,
    hit: &TerrainHit,
    radius_m: f32,
    erase_density: f32,
    seed: u64,
) -> usize {
    let all = state.scene.database.get_all_objects();
    let folder_ids: std::collections::HashSet<&str> = all
        .iter()
        .filter(|o| o.object_type == ObjectType::Folder && o.name.starts_with(SET_FOLDER_PREFIX))
        .map(|o| o.id.as_str())
        .collect();
    if folder_ids.is_empty() {
        return 0;
    }

    let centre = Vec3::from_array(hit.position_m);
    let mut rng = seed.max(1);
    let mut victims = Vec::new();
    for object in &all {
        let Some(parent) = object.parent.as_deref() else {
            continue;
        };
        if !folder_ids.contains(parent) {
            continue;
        }
        if Vec3::from_array(object.transform.position).distance(centre) > radius_m {
            continue;
        }
        // xorshift64: cheap, deterministic per (seed, encounter order).
        rng ^= rng << 13;
        rng ^= rng >> 7;
        rng ^= rng << 17;
        if ((rng >> 40) as f32 / (1u64 << 24) as f32) < erase_density.clamp(0.0, 1.0) {
            victims.push(object.id.clone());
        }
    }
    if victims.is_empty() {
        return 0;
    }

    let pre = state.scene.capture_history_snapshot();
    let mut removed = 0;
    for id in &victims {
        if state.scene.database.remove_object(id) {
            removed += 1;
        }
    }
    if removed > 0 {
        state.scene.bump_revision(true);
        state.scene.commit_undo_checkpoint(pre);
    }
    removed
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn file_stem_drops_directories_and_extension() {
        assert_eq!(file_stem("meshes/trees/oak.mesh"), "oak");
        assert_eq!(file_stem("meshes\\bush.fbx"), "bush");
        assert_eq!(file_stem("plain"), "plain");
    }

    #[test]
    fn set_folders_are_named_after_the_set() {
        assert_eq!(set_folder_name("Meadow"), "Foliage · Meadow");
    }
}
