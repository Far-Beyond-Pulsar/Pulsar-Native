//! Import companions: extra work plugins offer to do for an import.
//!
//! This crate knows nothing about what a companion makes. A plugin registers
//! an [`ImportCompanion`]; the configurator asks [`offers_for`] which of them
//! have something to say about the sources being imported, and the import
//! runs the ones the user accepted ([`run_companions`]) after the native
//! asset exists. See `plugin_editor_api::import_companion` for the contract.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use parking_lot::RwLock;
use plugin_editor_api::{
    CompanionOutput, ImportCompanion, ImportCompanionContext, ImportOffer, ImportProbe,
    SourceImage, SourceImageRole, SourceSurface,
};

use crate::db::{relative_key, ImportOutput};

static COMPANIONS: RwLock<Vec<Arc<dyn ImportCompanion>>> = RwLock::new(Vec::new());

/// Make `companion` available to imports. Registering an id again replaces
/// the earlier registration.
pub fn register_companion(companion: Arc<dyn ImportCompanion>) {
    let mut companions = COMPANIONS.write();
    companions.retain(|existing| existing.id() != companion.id());
    companions.push(companion);
}

/// A registered companion's offer for a set of sources.
#[derive(Clone, Debug)]
pub struct CompanionOffer {
    pub companion_id: String,
    pub offer: ImportOffer,
}

const IMAGE_EXTENSIONS: &[&str] = &["png", "jpg", "jpeg", "tga", "bmp", "webp", "tif", "tiff"];

fn is_image(path: &Path) -> bool {
    path.extension()
        .and_then(|ext| ext.to_str())
        .is_some_and(|ext| IMAGE_EXTENSIONS.contains(&ext.to_ascii_lowercase().as_str()))
}

fn images_in(dir: &Path, found: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for path in entries.flatten().map(|entry| entry.path()) {
        if path.is_file() && is_image(&path) && !found.contains(&path) {
            found.push(path);
        }
    }
}

/// Image files near `source`: its folder, the usual texture subfolders and
/// the `.fbm` folder some FBX exporters write beside the model.
pub fn nearby_images(source: &Path) -> Vec<PathBuf> {
    let Some(dir) = source.parent() else {
        return Vec::new();
    };
    let mut found = Vec::new();
    images_in(dir, &mut found);
    for sub in ["textures", "Textures", "tex", "maps", "Maps"] {
        images_in(&dir.join(sub), &mut found);
    }
    if let Some(stem) = source.file_stem() {
        let mut fbm = stem.to_os_string();
        fbm.push(".fbm");
        images_in(&dir.join(fbm), &mut found);
    }
    found.sort();
    found
}

/// The offers registered companions make for `sources`, to be imported by
/// the importer `importer_id`.
pub fn offers_for(importer_id: &str, sources: &[PathBuf]) -> Vec<CompanionOffer> {
    let companions = COMPANIONS.read().clone();
    if companions.is_empty() {
        return Vec::new();
    }
    let mut nearby = Vec::new();
    for source in sources {
        for image in nearby_images(source) {
            if !nearby.contains(&image) {
                nearby.push(image);
            }
        }
    }
    let probe = ImportProbe {
        importer: importer_id,
        sources,
        nearby_images: &nearby,
    };
    companions
        .iter()
        .filter_map(|companion| {
            companion.offer(&probe).map(|offer| CompanionOffer {
                companion_id: companion.id().to_owned(),
                offer,
            })
        })
        .collect()
}

/// Role hints in image file names, longest first so `metallicroughness`
/// wins over `metallic`.
const ROLE_SUFFIXES: &[(&str, SourceImageRole)] = &[
    ("metallicroughness", SourceImageRole::MetallicRoughness),
    ("metallic_roughness", SourceImageRole::MetallicRoughness),
    ("basecolor", SourceImageRole::BaseColor),
    ("base_color", SourceImageRole::BaseColor),
    ("roughness", SourceImageRole::Roughness),
    ("metalness", SourceImageRole::Metallic),
    ("metallic", SourceImageRole::Metallic),
    ("occlusion", SourceImageRole::Occlusion),
    ("emissive", SourceImageRole::Emissive),
    ("albedo", SourceImageRole::BaseColor),
    ("diffuse", SourceImageRole::BaseColor),
    ("normal", SourceImageRole::Normal),
    ("color", SourceImageRole::BaseColor),
    ("_nrm", SourceImageRole::Normal),
    ("_ao", SourceImageRole::Occlusion),
    ("_orm", SourceImageRole::MetallicRoughness),
];

/// Split `name` (an image file stem) into the surface name it is prefixed by
/// and the role its suffix names, if it names one.
fn role_of(stem: &str) -> Option<(String, SourceImageRole)> {
    let lower = stem.to_ascii_lowercase();
    ROLE_SUFFIXES.iter().find_map(|(suffix, role)| {
        let prefix = lower.strip_suffix(suffix)?;
        let prefix = prefix.trim_end_matches(['_', '-', '.', ' ']);
        Some((strip_resolution(prefix).to_owned(), *role))
    })
}

/// Drop trailing resolution tags (`_4K`, `-2k`, `_2048`) that texture packs
/// put between the name and the role: `xdhlcfc_4k` is `xdhlcfc`.
fn strip_resolution(mut name: &str) -> &str {
    while let Some(index) = name.rfind(['_', '-', '.', ' ']) {
        let tag = &name[index + 1..];
        let digits = tag.strip_suffix('k').unwrap_or(tag);
        let is_resolution = !digits.is_empty()
            && digits.bytes().all(|b| b.is_ascii_digit())
            && (tag.ends_with('k') || digits.len() >= 3);
        if !is_resolution {
            break;
        }
        name = name[..index].trim_end_matches(['_', '-', '.', ' ']);
    }
    name
}

/// Add images for the roles `surface` has none for, from `nearby` files
/// named after the surface (`Rock_Normal.png` for the surface `Rock`) or,
/// for a model with a single surface, after the model (`rock_albedo.png` next
/// to `rock.fbx`).
pub fn complete_from_siblings(
    surface: &mut SourceSurface,
    model_stem: &str,
    only_surface: bool,
    nearby: &[PathBuf],
) {
    let surface_name = surface.name.to_ascii_lowercase();
    let model_name = model_stem.to_ascii_lowercase();
    for path in nearby {
        let Some(stem) = path.file_stem().and_then(|stem| stem.to_str()) else {
            continue;
        };
        let Some((prefix, role)) = role_of(stem) else {
            continue;
        };
        let named_for_surface = !surface_name.is_empty() && prefix == surface_name;
        let named_for_model = only_surface && prefix == model_name;
        if (named_for_surface || named_for_model)
            && !surface.images.iter().any(|image| image.role == role)
        {
            surface.images.push(SourceImage {
                role,
                path: path.clone(),
            });
        }
    }
}

/// Run the registered companions named in `ids`. A companion that fails is
/// logged and skipped: the import itself has already succeeded.
///
/// Returns what they wrote, project-relative, and the surface slot each
/// output is the suggested default for.
pub fn run_companions(
    ids: &[String],
    project_root: &Path,
    source: &Path,
    native_asset: &Path,
    surfaces: &[SourceSurface],
) -> (Vec<ImportOutput>, Vec<(u32, String)>) {
    let companions = COMPANIONS.read().clone();
    let context = ImportCompanionContext {
        project_root,
        source,
        native_asset,
        surfaces,
    };
    let mut outputs = Vec::new();
    let mut assignments = Vec::new();
    for companion in companions
        .iter()
        .filter(|c| ids.iter().any(|id| id == c.id()))
    {
        let written: Vec<CompanionOutput> = match companion.run(&context) {
            Ok(written) => written,
            Err(error) => {
                tracing::warn!("import companion '{}' failed: {error}", companion.id());
                continue;
            }
        };
        for output in written {
            let path = relative_key(project_root, &output.path);
            if let Some(slot) = output.assign_to_slot {
                assignments.push((slot, path.clone()));
            }
            outputs.push(ImportOutput {
                path,
                kind: output.kind,
                id: None,
                source_name: output.source_name,
                source_skin: None,
            });
        }
    }
    (outputs, assignments)
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
    let (decoded, _) = helio_component::mesh_cache::decode_asset(&bytes)
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
        helio_component::mesh_cache::set_default_materials(native_asset, &materials)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn surface(name: &str) -> SourceSurface {
        SourceSurface {
            slot: 0,
            name: name.into(),
            images: Vec::new(),
        }
    }

    #[test]
    fn siblings_named_for_the_surface_fill_missing_roles_only() {
        let nearby = vec![
            PathBuf::from("t/Rock_BaseColor.png"),
            PathBuf::from("t/Rock_Normal.png"),
            PathBuf::from("t/Moss_Normal.png"),
        ];
        let mut rock = surface("Rock");
        rock.images.push(SourceImage {
            role: SourceImageRole::Normal,
            path: PathBuf::from("declared_normal.png"),
        });
        complete_from_siblings(&mut rock, "scene", false, &nearby);
        assert_eq!(rock.images.len(), 2);
        assert!(rock
            .images
            .iter()
            .any(|image| image.path == Path::new("declared_normal.png")));
        assert!(rock
            .images
            .iter()
            .any(|image| image.role == SourceImageRole::BaseColor));
    }

    #[test]
    fn a_single_surface_model_matches_images_named_for_the_model() {
        let nearby = vec![PathBuf::from("t/crate_albedo.jpg")];
        let mut only = surface("Material #25");
        complete_from_siblings(&mut only, "crate", true, &nearby);
        assert_eq!(only.images.len(), 1);
        let mut one_of_many = surface("Material #25");
        complete_from_siblings(&mut one_of_many, "crate", false, &nearby);
        assert!(one_of_many.images.is_empty());
    }

    #[test]
    fn a_texture_pack_with_resolution_tags_matches_the_model_name() {
        let nearby: Vec<PathBuf> = [
            "xdhlcfc_4K_AO.jpg",
            "xdhlcfc_4K_BaseColor.jpg",
            "xdhlcfc_4K_Bump.jpg",
            "xdhlcfc_4K_Normal.jpg",
            "xdhlcfc_4K_Roughness.jpg",
            "xdhlcfc_4K_Specular.jpg",
        ]
        .iter()
        .map(|name| PathBuf::from("bazzar").join(name))
        .collect();
        let mut only = surface("Material.001");
        complete_from_siblings(&mut only, "xdhlcfc", true, &nearby);
        let roles: Vec<_> = only.images.iter().map(|image| image.role).collect();
        for role in [
            SourceImageRole::Occlusion,
            SourceImageRole::BaseColor,
            SourceImageRole::Normal,
            SourceImageRole::Roughness,
        ] {
            assert!(roles.contains(&role), "{role:?} missing from {roles:?}");
        }
    }

    #[test]
    fn metallic_roughness_is_not_mistaken_for_a_plain_role() {
        let (prefix, role) = role_of("Rock_MetallicRoughness").unwrap();
        assert_eq!(
            (prefix.as_str(), role),
            ("rock", SourceImageRole::MetallicRoughness)
        );
    }
}
