//! Per-format source conversion, selected by file extension.

use std::any::Any;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

use serde_json::Value;

use helio_component::mesh_cache::{self, OptionsSchema};
use plugin_editor_api::{SourceImage, SourceImageRole, SourceSurface};

use crate::db::ImportOutput;

/// Import option values by field key, as the configurator produces them.
pub type OptionValues = HashMap<String, Box<dyn Any + Send>>;

/// What an earlier import of the same source wrote, so a re-import can give
/// each file the identity it had.
pub struct Previous<'a> {
    pub project_root: &'a Path,
    pub outputs: &'a [ImportOutput],
}

/// What an import wrote.
#[derive(Debug)]
pub struct Imported {
    /// The primary native asset: what [`crate::ImportRecord::asset`] records.
    /// The path asked for, unless the source turned out to need another kind
    /// of asset (a rigged model's is its skeletal mesh).
    pub asset: PathBuf,
    /// The other files written, or still belonging to this source. Empty for a
    /// plain mesh.
    pub outputs: Vec<ImportOutput>,
    /// The surfaces (materials) the source has and the image files found for
    /// each, for import companions. Empty when the importer has none.
    pub surfaces: Vec<SourceSurface>,
    /// One entry per mesh file when the source held several meshes and each
    /// got its own file; empty for a source that became one. `asset` is the
    /// first of them.
    pub parts: Vec<ImportedPart>,
}

/// One mesh file of a source that held several meshes.
#[derive(Debug)]
pub struct ImportedPart {
    pub asset: PathBuf,
    /// The surfaces this mesh uses; `slot` is the mesh's own slot.
    pub surfaces: Vec<SourceSurface>,
}

pub trait Importer: Send + Sync {
    /// Stable id stored in [`crate::ImportRecord::importer`].
    fn id(&self) -> &'static str;
    /// Whether this importer converts sources with extension `ext` (no dot).
    fn handles(&self, ext: &str) -> bool;
    /// The options a configurator should offer for `ext`.
    fn options_schema(&self, ext: &str) -> Option<OptionsSchema>;
    /// Where the native asset for `source` goes, in directory `dir`.
    fn asset_path(&self, dir: &Path, source: &Path) -> PathBuf;
    /// Decode saved options (falling back to legacy storage or schema defaults).
    fn stored_options(&self, asset: &Path, ext: &str, saved: Option<&Value>) -> OptionValues;
    /// Serialize runtime options for the import database.
    fn serialize_options(&self, options: &OptionValues) -> Value;
    /// Convert `source` into the native asset at `asset`, overwriting it.
    /// `previous` is what the last import of this source wrote, if any.
    fn import(
        &self,
        source: &Path,
        asset: &Path,
        options: &OptionValues,
        previous: &Previous,
    ) -> Result<Imported, String>;
}

struct MeshImporter;

impl Importer for MeshImporter {
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
        let mut scene = mesh_cache::load_model_scene(source, options)?;
        // A rigged model becomes a linked skeleton, skeletal mesh and
        // animations; anything else is the static mesh it always was.
        match scene.rig.take() {
            Some(rig) => crate::rig::import_rig(&rig, asset, previous),
            None => {
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
    }
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
            path: crate::db::relative_key(previous.project_root, &part.path),
            kind: "mesh".to_string(),
            id: None,
            source_name: Some(part.name.clone()),
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
    let nearby = crate::companion::nearby_images(source);
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
            crate::companion::complete_from_siblings(
                &mut surface,
                model_stem,
                only_surface,
                &nearby,
            );
            surface
        })
        .collect()
}

static IMPORTERS: &[&dyn Importer] = &[&MeshImporter];

/// The importer for sources with extension `ext` (no dot, any case).
pub fn importer_for(ext: &str) -> Option<&'static dyn Importer> {
    let ext = ext.to_ascii_lowercase();
    IMPORTERS.iter().copied().find(|i| i.handles(&ext))
}

/// Look an importer up by its stored [`Importer::id`].
pub fn importer_by_id(id: &str) -> Option<&'static dyn Importer> {
    IMPORTERS.iter().copied().find(|i| i.id() == id)
}
