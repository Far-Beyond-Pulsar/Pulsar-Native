//! 3D asset loading integration with SolidRS
//!
//! This crate provides a bridge between SolidRS (comprehensive 3D model loader)
//! and Helio's GPU-driven rendering pipeline. It handles conversion of CPU-side
//! scene data to GPU buffers while maintaining performance standards.

mod animation_system;
mod camera_converter;
mod ies;
mod light_converter;
mod lut;
mod material_converter;
mod mesh_converter;
mod rig_converter;
mod scene_converter;
mod texture_loader;

use std::io::Cursor;
use std::path::PathBuf;

pub use camera_converter::{extract_camera_data, CameraData};
pub use ies::{IesError, IesProfile};
pub use light_converter::convert_light;
pub use lut::{CubeLut, LutError};
pub use material_converter::{
    convert_material, ConvertedMaterial, ConvertedMaterialTextures, ConvertedTextureRef,
};
pub use mesh_converter::{convert_primitive, convert_vertex};
pub use rig_converter::{
    extract_rigs, ImportedBone, ImportedClip, ImportedInterpolation, ImportedJoint, ImportedKey,
    ImportedLod, ImportedRig, ImportedTrack, RigSection, RigVertex,
};
pub use scene_converter::{
    convert_scene, ConvertedMesh, ConvertedMeshSection, ConvertedScene, ConvertedSectionedMesh,
    MaterialImage, MaterialSource,
};
pub use texture_loader::TextureSemantic;

use std::path::Path;

/// Configuration for asset loading
/// Re-exports of the Solid3D configurator types, so hosts can build import
/// configurator UIs and value sets without depending on `solid-rs` directly.
pub use solid_rs::configurator::{
    OptionField, OptionKind, OptionValue, OptionValues, OptionsSchema,
};

#[derive(Debug, Clone)]
pub struct LoadConfig {
    /// Flip UV Y-axis (1.0 - v)
    /// - true: DirectX convention (0,0 at top-left) → OpenGL (0,0 at bottom-left)
    /// - false: Use UVs as-is
    pub flip_uv_y: bool,
    /// Merge all sub-meshes into a single mesh with vertex positions baked into
    /// world space.  Useful when you want to treat the whole asset as one draw
    /// call or one physics body.  The merged mesh gets `node_transform` = IDENTITY.
    pub merge_meshes: bool,
    /// Scale applied to the entire imported asset.  Applied before any other
    /// transform so it acts as a unit-conversion factor (e.g. `Vec3::splat(0.01)`
    /// to convert centimetres → metres).  Defaults to `Vec3::ONE` (no change).
    pub import_scale: glam::Vec3,
}

impl Default for LoadConfig {
    fn default() -> Self {
        Self {
            flip_uv_y: false,
            merge_meshes: false,
            import_scale: glam::Vec3::ONE,
        }
    }
}

impl LoadConfig {
    pub fn with_uv_flip(mut self, flip: bool) -> Self {
        self.flip_uv_y = flip;
        self
    }

    pub fn with_merge_meshes(mut self, merge: bool) -> Self {
        self.merge_meshes = merge;
        self
    }

    pub fn with_import_scale(mut self, scale: glam::Vec3) -> Self {
        self.import_scale = scale;
        self
    }

    /// Derive a `LoadConfig` from configurator [`OptionValues`], honouring the
    /// keys this conversion layer understands (`flip_uv_v`, and `import_scale`
    /// or `unit_scale`). Remaining keys are handled by the loader itself.
    pub fn from_option_values(values: &solid_rs::configurator::OptionValues) -> Self {
        use solid_rs::configurator::keys;
        let scale = values
            .get("import_scale")
            .or_else(|| values.get("unit_scale"))
            .and_then(|v| v.as_f64())
            .unwrap_or(1.0) as f32;
        Self {
            flip_uv_y: values.bool_or(keys::FLIP_UV_V, false),
            merge_meshes: values.bool_or("merge_meshes", false),
            import_scale: glam::Vec3::splat(scale),
        }
    }
}

/// Load a 3D scene file (FBX, glTF, OBJ, USD, Unreal uasset/umap, etc.) and convert to Helio structures
///
/// This is the main entry point for loading 3D assets. It:
/// 1. Detects the file format from the extension
/// 2. Loads the file using the appropriate SolidRS loader
/// 3. Converts the scene to Helio-compatible structures
///
/// # Example
/// ```no_run
/// use helio_asset_compat::load_scene_file;
///
/// let scene = load_scene_file("models/character.fbx").unwrap();
/// println!("Loaded {} meshes, {} materials", scene.meshes.len(), scene.materials.len());
/// ```
pub fn load_scene_file<P: AsRef<Path>>(path: P) -> Result<ConvertedScene> {
    load_scene_file_with_config(path, LoadConfig::default())
}

/// Load with custom configuration (e.g., UV flipping)
pub fn load_scene_file_with_config<P: AsRef<Path>>(
    path: P,
    config: LoadConfig,
) -> Result<ConvertedScene> {
    let path = path.as_ref();

    // Detect format from extension
    let extension = path
        .extension()
        .and_then(|e| e.to_str())
        .ok_or_else(|| AssetError::UnsupportedFormat("File has no extension".to_string()))?;

    log::info!(
        "Loading 3D model: {} (UV flip: {})",
        path.display(),
        config.flip_uv_y
    );
    log::info!("Detected extension: {}", extension);

    // Create SolidRS registry and register loaders
    let mut registry = solid_rs::registry::Registry::new();
    registry.register_loader(solid_fbx::FbxLoader);
    registry.register_loader(solid_gltf::GltfLoader);
    registry.register_loader(solid_obj::ObjLoader);
    registry.register_loader(solid_usd::UsdLoader); // supports usda/usdc/usdz
    registry.register_loader(solid_unreal::UnrealLoader); // supports uasset/umap

    // Load the scene
    // A glTF names its `.bin` and textures relative to the file. `load_file`
    // passes no base directory, so they would resolve against the process's
    // working directory and fail for any glTF with external buffers.
    let mut options = solid_rs::traits::LoadOptions::default();
    options.base_dir = path.parent().map(Path::to_path_buf);
    let solid_scene = registry
        .load_file_with_options(path, &options)
        .map_err(|e| AssetError::Solid(e))?;

    log::info!(
        "Loaded SolidRS scene '{}' - {} meshes, {} materials, {} lights",
        solid_scene.name,
        solid_scene.meshes.len(),
        solid_scene.materials.len(),
        solid_scene.lights.len()
    );

    // Get the directory containing the model file for resolving relative texture paths
    let base_dir = path
        .parent()
        .map(|p| p.to_path_buf())
        .unwrap_or_else(|| PathBuf::from("."));

    // Convert to Helio structures
    convert_scene(&solid_scene, &base_dir, &config)
}

/// Build the loader registry used for asset import (shared by the configured and
/// unconfigured load paths).
fn import_registry() -> solid_rs::registry::Registry {
    let mut registry = solid_rs::registry::Registry::new();
    registry.register_loader(solid_fbx::FbxLoader);
    registry.register_loader(solid_gltf::GltfLoader);
    registry.register_loader(solid_obj::ObjLoader);
    registry.register_loader(solid_usd::UsdLoader); // supports usda/usdc/usdz
    registry.register_loader(solid_unreal::UnrealLoader); // supports uasset/umap
    registry
}

/// Returns the import-options [`OptionsSchema`] advertised by the loader for the
/// file extension `ext` (without leading dot), for driving a configurator UI.
/// Returns `None` if no bundled loader handles that extension.
pub fn options_schema_for_extension(ext: &str) -> Option<OptionsSchema> {
    import_registry().options_schema_for_extension(ext)
}

/// Load a 3D scene file using configurator [`OptionValues`] and convert it to
/// Helio structures. Mirrors [`load_scene_file_with_config`] but sources its
/// options from a configurator value set (see [`options_schema_for_extension`]).
pub fn load_scene_file_with_values<P: AsRef<Path>>(
    path: P,
    values: &OptionValues,
) -> Result<ConvertedScene> {
    let path = path.as_ref();

    let extension = path
        .extension()
        .and_then(|e| e.to_str())
        .ok_or_else(|| AssetError::UnsupportedFormat("File has no extension".to_string()))?;

    log::info!(
        "Loading 3D model with configurator values: {} (.{})",
        path.display(),
        extension
    );

    let registry = import_registry();
    let solid_scene = registry
        .load_file_configured(path, values)
        .map_err(AssetError::Solid)?;

    log::info!(
        "Loaded SolidRS scene '{}' - {} meshes, {} materials, {} lights",
        solid_scene.name,
        solid_scene.meshes.len(),
        solid_scene.materials.len(),
        solid_scene.lights.len()
    );

    let base_dir = path
        .parent()
        .map(|p| p.to_path_buf())
        .unwrap_or_else(|| PathBuf::from("."));

    // Honour the conversion-layer keys (uv flip, scale) during conversion; the
    // loader has already honoured the ones it understands.
    let config = LoadConfig::from_option_values(values);
    convert_scene(&solid_scene, &base_dir, &config)
}

/// Load a 3D scene from embedded bytes using a known format identifier.
///
/// This is useful for examples or applications that bundle assets with
/// `include_bytes!` but still want Helio's normal scene conversion pipeline.
pub fn load_scene_bytes(
    bytes: &[u8],
    format_id: &str,
    base_dir: Option<&Path>,
) -> Result<ConvertedScene> {
    load_scene_bytes_with_config(bytes, format_id, base_dir, LoadConfig::default())
}

/// Load embedded scene bytes with custom configuration (e.g., UV flipping).
pub fn load_scene_bytes_with_config(
    bytes: &[u8],
    format_id: &str,
    base_dir: Option<&Path>,
    config: LoadConfig,
) -> Result<ConvertedScene> {
    log::info!(
        "Loading embedded 3D model as '{}' (UV flip: {})",
        format_id,
        config.flip_uv_y
    );

    let mut registry = solid_rs::registry::Registry::new();
    registry.register_loader(solid_fbx::FbxLoader);
    registry.register_loader(solid_gltf::GltfLoader);
    registry.register_loader(solid_obj::ObjLoader);
    registry.register_loader(solid_usd::UsdLoader);
    registry.register_loader(solid_unreal::UnrealLoader);

    let mut options = solid_rs::traits::LoadOptions::default();
    options.base_dir = base_dir.map(Path::to_path_buf);

    let solid_scene = registry
        .load_from(Cursor::new(bytes), format_id, &options)
        .map_err(AssetError::Solid)?;

    log::info!(
        "Loaded embedded SolidRS scene '{}' - {} meshes, {} materials, {} lights",
        solid_scene.name,
        solid_scene.meshes.len(),
        solid_scene.materials.len(),
        solid_scene.lights.len()
    );

    let conversion_base_dir = base_dir
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from("."));

    convert_scene(&solid_scene, &conversion_base_dir, &config)
}

/// Result type for asset loading operations
pub type Result<T> = std::result::Result<T, AssetError>;

/// Errors that can occur during asset loading
#[derive(Debug, thiserror::Error)]
pub enum AssetError {
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("SolidRS error: {0}")]
    Solid(#[from] solid_rs::SolidError),

    #[error("Unsupported format: {0}")]
    UnsupportedFormat(String),

    #[error("Invalid data: {0}")]
    InvalidData(String),
}

#[cfg(test)]
mod configurator_tests {
    use super::{options_schema_for_extension, LoadConfig, OptionValue, OptionValues};
    use solid_rs::configurator::keys;

    #[test]
    fn bundled_loaders_expose_configurator_schemas() {
        for extension in ["fbx", "gltf", "obj", "usd"] {
            let schema = options_schema_for_extension(extension)
                .unwrap_or_else(|| panic!("missing schema for .{extension}"));
            assert!(
                schema
                    .fields
                    .iter()
                    .any(|field| field.key == keys::FLIP_UV_V),
                ".{extension} schema omitted the shared UV option"
            );
        }
    }

    #[test]
    fn configurator_values_map_to_conversion_config() {
        let mut values = OptionValues::new();
        values.set(keys::FLIP_UV_V, OptionValue::Bool(true));
        values.set("import_scale", OptionValue::Float(0.01));

        let config = LoadConfig::from_option_values(&values);

        assert!(config.flip_uv_y);
        assert_eq!(config.import_scale, glam::Vec3::splat(0.01));
    }
}
