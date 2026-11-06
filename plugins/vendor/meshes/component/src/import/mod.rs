//! The meshes' model importer (FBX, OBJ, glTF, ...): a static mesh, one
//! mesh per part when the source holds several, or a rigged model's
//! skeletal asset set ([`rig`]). Registered with the engine's import
//! service (`asset_import::ImporterRegistration`).

pub mod rig;

use std::path::{Path, PathBuf};

use asset_import::db::ImportOutput;
use asset_import::options::OptionsSchema;
use asset_import::{Imported, ImportedPart, Importer, OptionValues, Previous};
use plugin_editor_api::{SourceImage, SourceImageRole, SourceSurface};
use serde_json::Value;

use crate::mesh_cache;

struct ModelImporter;

impl Importer for ModelImporter {
    fn id(&self) -> &'static str {
        "mesh"
    }
    fn handles(&self, ext: &str) -> bool {
        mesh_cache::is_importable_model(ext)
    }
    fn options_schema(&self, ext: &str) -> Option<OptionsSchema> {
        mesh_cache::options_schema(ext)
    }
    fn asset_path(&self, dir: &Path, source: &Path) -> PathBuf {
        mesh_cache::native_mesh_path(dir, source)
    }
    fn stored_options(&self, asset: &Path, ext: &str, saved: Option<&Value>) -> OptionValues {
        saved
            .and_then(mesh_cache::option_values_from_json)
            .unwrap_or_else(|| mesh_cache::resolve_options(asset, ext))
    }
    fn serialize_options(&self, options: &OptionValues) -> Value {
        mesh_cache::option_values_to_json(options)
    }
    fn import(
        &self,
        source: &Path,
        asset: &Path,
        options: &OptionValues,
        previous: &Previous,
    ) -> Result<Imported, String> {
        let scene = mesh_cache::load_model_scene(source, options)?;
        // A rigged model becomes a linked skeleton, skeletal mesh and
        // animations; anything else is the static mesh it always was.
        if !scene.rigs.is_empty() {
            let surfaces = surfaces_of(&scene, source);
            rig::import_rigs(&scene, asset, previous, surfaces)
        } else {
            let surfaces = surfaces_of(&scene, source);
            if scene.parts.len() > 1 {
                return import_parts(&scene, source, asset, previous, surfaces);
            }
            mesh_cache::write_static_mesh(scene, asset).map(|asset| Imported {
                asset,
                outputs: Vec::new(),
                surfaces,
                parts: Vec::new(),
            })
        }
    }
    fn assign_default_materials(
        &self,
        asset: &Path,
        assignments: &[(u32, String)],
    ) -> Result<(), String> {
        assign_default_materials(asset, assignments)
    }
}

/// Make each `(slot, material)` the native mesh's default for that slot, for
/// slots that have none: what the user assigned stays.
pub fn assign_default_materials(
    native_asset: &Path,
    assignments: &[(u32, String)],
) -> Result<(), String> {
    if assignments.is_empty() || native_asset.extension().is_none_or(|ext| ext != "mesh") {
        return Ok(());
    }
    let bytes = std::fs::read(native_asset).map_err(|error| error.to_string())?;
    let (decoded, _) = mesh_cache::decode_asset(&bytes)
        .ok_or_else(|| format!("{} is not a valid .mesh", native_asset.display()))?;
    let mut materials: Vec<String> = decoded
        .material_slots
        .iter()
        .map(|slot| slot.material_asset.clone())
        .collect();
    let mut changed = false;
    for (slot, material) in assignments {
        if let Some(current) = materials.get_mut(*slot as usize) {
            if current.is_empty() {
                *current = material.clone();
                changed = true;
            }
        }
    }
    if changed {
        mesh_cache::set_default_materials(native_asset, &materials)?;
    }
    Ok(())
}

/// A source with several meshes becomes one `.mesh` per mesh, named
/// `<model>_<mesh>.mesh` beside where the single mesh would have gone. The
/// first is the import's primary asset and the rest are recorded as outputs
/// named for the mesh they came from.
fn import_parts(
    scene: &helio_asset_compat::ConvertedScene,
    source: &Path,
    asset: &Path,
    previous: &Previous,
    surfaces: Vec<SourceSurface>,
) -> Result<Imported, String> {
    let dir = asset.parent().unwrap_or_else(|| Path::new("."));
    let stem = source
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("model");
    let written = mesh_cache::write_mesh_parts(scene, dir, stem)?;
    let parts: Vec<ImportedPart> = written
        .iter()
        .map(|part| ImportedPart {
            asset: part.path.clone(),
            surfaces: part
                .source_materials
                .iter()
                .enumerate()
                .filter_map(|(slot, material)| {
                    let mut surface = surfaces.get((*material)?)?.clone();
                    surface.slot = slot as u32;
                    Some(surface)
                })
                .collect(),
        })
        .collect();
    let outputs = written
        .iter()
        .skip(1)
        .map(|part| ImportOutput {
            path: asset_import::db::relative_key(previous.project_root, &part.path),
            kind: "mesh".to_string(),
            id: None,
            source_name: Some(part.name.clone()),
            source_skin: None,
        })
        .collect();
    Ok(Imported {
        asset: written[0].path.clone(),
        outputs,
        surfaces,
        parts,
    })
}

fn role_of(semantic: helio_asset_compat::TextureSemantic) -> Option<SourceImageRole> {
    use helio_asset_compat::TextureSemantic as T;
    match semantic {
        T::BaseColor => Some(SourceImageRole::BaseColor),
        T::Normal => Some(SourceImageRole::Normal),
        T::MetallicRoughness => Some(SourceImageRole::MetallicRoughness),
        T::Occlusion => Some(SourceImageRole::Occlusion),
        T::Emissive => Some(SourceImageRole::Emissive),
        T::SpecularColor | T::SpecularWeight => None,
    }
}

/// The source's materials as neutral surfaces: the images their texture
/// slots name, plus images next to the model named after the material for
/// roles the file leaves empty. Slot `i` is the native mesh's slot `i`.
fn surfaces_of(scene: &helio_asset_compat::ConvertedScene, source: &Path) -> Vec<SourceSurface> {
    let nearby = asset_import::companion::nearby_images(source);
    let model_stem = source
        .file_stem()
        .and_then(|stem| stem.to_str())
        .unwrap_or_default();
    let only_surface = scene.material_sources.len() == 1;
    scene
        .material_sources
        .iter()
        .enumerate()
        .map(|(slot, material)| {
            let mut surface = SourceSurface {
                slot: slot as u32,
                name: material.name.clone(),
                images: material
                    .images
                    .iter()
                    .filter_map(|image| {
                        Some(SourceImage {
                            role: role_of(image.semantic)?,
                            path: image.path.clone(),
                        })
                    })
                    .collect(),
            };
            asset_import::companion::complete_from_siblings(
                &mut surface,
                model_stem,
                only_surface,
                &nearby,
            );
            surface
        })
        .collect()
}

inventory::submit! {
    asset_import::importer::ImporterRegistration {
        origin: pulsar_plugins::origin!(),
        importer: &ModelImporter,
    }
}
