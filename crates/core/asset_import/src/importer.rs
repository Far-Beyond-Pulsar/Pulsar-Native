//! Per-format source conversion, selected by file extension.

use std::any::Any;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

use helio_component::mesh_cache::{self, OptionsSchema};

/// Import option values by field key, as the configurator produces them.
pub type OptionValues = HashMap<String, Box<dyn Any + Send>>;

pub trait Importer: Send + Sync {
    /// Stable id stored in [`crate::ImportRecord::importer`].
    fn id(&self) -> &'static str;
    /// Whether this importer converts sources with extension `ext` (no dot).
    fn handles(&self, ext: &str) -> bool;
    /// The options a configurator should offer for `ext`.
    fn options_schema(&self, ext: &str) -> Option<OptionsSchema>;
    /// Where the native asset for `source` goes, in directory `dir`.
    fn asset_path(&self, dir: &Path, source: &Path) -> PathBuf;
    /// Options stored by the previous import of `asset` (else schema defaults).
    fn stored_options(&self, asset: &Path, ext: &str) -> OptionValues;
    /// Convert `source` into the native asset at `asset`, overwriting it.
    fn import(&self, source: &Path, asset: &Path, options: &OptionValues) -> Result<(), String>;
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
    fn stored_options(&self, asset: &Path, ext: &str) -> OptionValues {
        mesh_cache::resolve_options(asset, ext)
    }
    fn import(&self, source: &Path, asset: &Path, options: &OptionValues) -> Result<(), String> {
        mesh_cache::import_model_to_native(source, asset, options).map(|_| ())
    }
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
