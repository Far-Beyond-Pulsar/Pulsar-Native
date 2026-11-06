//! Per-format source conversion, selected by file extension: each format's
//! importer is a plugin's ([`ImporterRegistration`]: the meshes' model
//! importer), so the import service names no asset kind.

use std::any::Any;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

use serde_json::Value;

use plugin_editor_api::SourceSurface;

use crate::db::ImportOutput;
use crate::options::OptionsSchema;

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
    /// Make each `(slot, material)` the native asset's default material for
    /// that slot where it has none (what the user assigned stays): import
    /// companions' outputs. An importer whose assets have no material slots
    /// keeps the default (nothing to assign).
    fn assign_default_materials(
        &self,
        _asset: &Path,
        _assignments: &[(u32, String)],
    ) -> Result<(), String> {
        Ok(())
    }
}

/// A plugin's importer, submitted by its component crate with
/// `inventory::submit!`.
pub struct ImporterRegistration {
    /// The package that submitted it (`pulsar_plugins::origin!()`): skipped
    /// while that package's plugin is disabled.
    pub origin: &'static str,
    pub importer: &'static dyn Importer,
}

inventory::collect!(ImporterRegistration);

impl pulsar_plugins::Registration for ImporterRegistration {
    fn origin(&self) -> &'static str {
        self.origin
    }
}

fn importers() -> impl Iterator<Item = &'static dyn Importer> {
    pulsar_plugins::registered::<ImporterRegistration>().map(|registration| registration.importer)
}

/// The importer for sources with extension `ext` (no dot, any case).
pub fn importer_for(ext: &str) -> Option<&'static dyn Importer> {
    let ext = ext.to_ascii_lowercase();
    importers().find(|i| i.handles(&ext))
}

/// Look an importer up by its stored [`Importer::id`].
pub fn importer_by_id(id: &str) -> Option<&'static dyn Importer> {
    importers().find(|i| i.id() == id)
}

/// A stand-in model importer for this crate's own tests (no plugin is
/// linked into them): claims `.fbx` and `.obj` sources, writes a placeholder
/// `.mesh`.
#[cfg(test)]
mod test_importer {
    use super::*;

    struct TestModelImporter;

    impl Importer for TestModelImporter {
        fn id(&self) -> &'static str {
            "mesh"
        }
        fn handles(&self, ext: &str) -> bool {
            matches!(ext, "fbx" | "obj")
        }
        fn options_schema(&self, _ext: &str) -> Option<OptionsSchema> {
            None
        }
        fn asset_path(&self, dir: &Path, source: &Path) -> PathBuf {
            dir.join(source.file_stem().unwrap_or_default())
                .with_extension("mesh")
        }
        fn stored_options(&self, _asset: &Path, _ext: &str, _saved: Option<&Value>) -> OptionValues {
            OptionValues::new()
        }
        fn serialize_options(&self, _options: &OptionValues) -> Value {
            Value::Null
        }
        fn import(
            &self,
            _source: &Path,
            asset: &Path,
            _options: &OptionValues,
            _previous: &Previous,
        ) -> Result<Imported, String> {
            std::fs::write(asset, b"test mesh").map_err(|error| error.to_string())?;
            Ok(Imported {
                asset: asset.to_path_buf(),
                outputs: Vec::new(),
                surfaces: Vec::new(),
                parts: Vec::new(),
            })
        }
    }

    inventory::submit! {
        ImporterRegistration {
            origin: pulsar_plugins::origin!(),
            importer: &TestModelImporter,
        }
    }
}
