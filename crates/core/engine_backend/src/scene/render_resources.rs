//! Persistent render-resource components owned by SceneDB.
//!
//! These types deliberately contain asset identity and authored data only.
//! They do not contain `wgpu` objects or Helio handles.  A renderer may
//! project them into transient GPU bindings, but the World remains the sole
//! owner of the data that can be saved, replicated, or reconstructed after a
//! renderer/device restart.

use serde::{Deserialize, Serialize};

use pulsar_scenedb::Entity;

/// Persistent texture asset data.  `pixels` is optional so an unloaded asset
/// can still be represented by its stable path and metadata.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct TextureResource {
    pub asset: String,
    pub width: u32,
    pub height: u32,
    pub format: String,
    #[serde(default)]
    pub pixels: Vec<u8>,
}

/// A persistent reference to a texture used by a material, including the
/// authored UV mapping.  The numeric GPU slot is intentionally absent.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct MaterialTextureResource {
    pub texture: String,
    pub uv_channel: u32,
    pub offset: [f32; 2],
    pub scale: [f32; 2],
    pub rotation_radians: f32,
}

/// Persistent material asset state.  Helio's `MaterialId` is a projection
/// detail and must never be serialized or used as scene identity.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct MaterialResource {
    pub asset: String,
    pub base_color: [f32; 4],
    pub metallic: f32,
    pub roughness: f32,
    pub emissive_color: [f32; 3],
    pub emissive_intensity: f32,
    pub alpha_cutoff: f32,
    #[serde(default)]
    pub base_color_texture: Option<MaterialTextureResource>,
    #[serde(default)]
    pub normal_texture: Option<MaterialTextureResource>,
    #[serde(default)]
    pub roughness_texture: Option<MaterialTextureResource>,
    #[serde(default)]
    pub emissive_texture: Option<MaterialTextureResource>,
    #[serde(default)]
    pub occlusion_texture: Option<MaterialTextureResource>,
}

impl Default for MaterialResource {
    fn default() -> Self {
        Self {
            asset: String::new(),
            base_color: [1.0; 4],
            metallic: 0.0,
            roughness: 1.0,
            emissive_color: [0.0; 3],
            emissive_intensity: 0.0,
            alpha_cutoff: 0.5,
            base_color_texture: None,
            normal_texture: None,
            roughness_texture: None,
            emissive_texture: None,
            occlusion_texture: None,
        }
    }
}

/// One section of a sectioned mesh.  Sections share the parent vertex array
/// and carry authored material identity rather than a renderer slot.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct MeshSectionResource {
    pub indices: Vec<u32>,
    pub material: String,
}

/// Persistent multi-section mesh geometry.  The renderer can allocate one
/// shared vertex range and one transient draw record per section.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct SectionedMeshResource {
    pub asset: String,
    #[serde(default)]
    pub vertices: Vec<u8>,
    #[serde(default)]
    pub sections: Vec<MeshSectionResource>,
}

/// Persistent object-side render state that was historically kept in Helio's
/// `ObjectRecord`/sectioned-object pools.  Handles and dense GPU indices are
/// intentionally not represented here.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct MeshObjectResource {
    pub mesh: String,
    #[serde(default)]
    pub sections: Vec<String>,
    pub bounds: [f32; 4],
    pub flags: u32,
    pub groups: u32,
    pub movable: bool,
}

pub type TextureComponent = TextureResource;
pub type MaterialComponent = MaterialResource;
pub type SectionedMeshComponent = SectionedMeshResource;
pub type MeshObjectComponent = MeshObjectResource;

impl Default for MeshObjectResource {
    fn default() -> Self {
        Self {
            mesh: String::new(),
            sections: Vec::new(),
            bounds: [0.0, 0.0, 0.0, 1.0],
            flags: 0,
            groups: 0,
            movable: false,
        }
    }
}

/// Insert a complete persistent render-resource bundle on an existing entity.
/// Keeping this operation on the SceneDB-facing store makes it difficult for
/// callers to accidentally create only a Helio-side half of an object.
pub fn insert_render_resources(
    world: &mut pulsar_scenedb::World,
    entity: Entity,
    texture: Option<TextureResource>,
    material: Option<MaterialResource>,
    mesh: Option<SectionedMeshResource>,
    object: Option<MeshObjectResource>,
) {
    if let Some(value) = texture {
        world.insert(entity, value);
    }
    if let Some(value) = material {
        world.insert(entity, value);
    }
    if let Some(value) = mesh {
        world.insert(entity, value);
    }
    if let Some(value) = object {
        world.insert(entity, value);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn persistent_render_bundle_is_stored_as_world_components() {
        let mut world = pulsar_scenedb::World::new();
        let entity = world.spawn();
        insert_render_resources(
            &mut world,
            entity,
            Some(TextureResource {
                asset: "albedo.png".into(),
                ..Default::default()
            }),
            Some(MaterialResource {
                asset: "metal.mat".into(),
                ..Default::default()
            }),
            Some(SectionedMeshResource {
                asset: "ship.mesh".into(),
                sections: vec![MeshSectionResource {
                    indices: vec![0, 1, 2],
                    material: "metal.mat".into(),
                }],
                ..Default::default()
            }),
            Some(MeshObjectResource {
                mesh: "ship.mesh".into(),
                ..Default::default()
            }),
        );

        assert_eq!(
            world.get::<TextureResource>(entity).unwrap().asset,
            "albedo.png"
        );
        assert_eq!(
            world.get::<MaterialResource>(entity).unwrap().asset,
            "metal.mat"
        );
        assert_eq!(
            world
                .get::<SectionedMeshResource>(entity)
                .unwrap()
                .sections
                .len(),
            1
        );
        assert_eq!(
            world.get::<MeshObjectResource>(entity).unwrap().mesh,
            "ship.mesh"
        );
    }
}
