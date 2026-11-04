//! Texture conversion from SolidRS assets to Helio uploads.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use helio::{TextureSamplerDesc, TextureUpload};
use solid_rs::scene::{
    FilterMode as SolidFilter, Image, ImageSource, Sampler, Scene, Texture, WrapMode,
};

use crate::{AssetError, Result};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TextureSemantic {
    BaseColor,
    MetallicRoughness,
    Normal,
    Occlusion,
    Emissive,
    SpecularColor,
    SpecularWeight,
}

impl TextureSemantic {
    pub fn is_srgb(self) -> bool {
        matches!(
            self,
            TextureSemantic::BaseColor | TextureSemantic::Emissive | TextureSemantic::SpecularColor
        )
    }

    pub fn suffix(self) -> &'static str {
        match self {
            TextureSemantic::BaseColor => "base-color",
            TextureSemantic::MetallicRoughness => "metallic-roughness",
            TextureSemantic::Normal => "normal",
            TextureSemantic::Occlusion => "occlusion",
            TextureSemantic::Emissive => "emissive",
            TextureSemantic::SpecularColor => "specular-color",
            TextureSemantic::SpecularWeight => "specular-weight",
        }
    }
}

fn push_unique_path(candidates: &mut Vec<PathBuf>, candidate: PathBuf) {
    if !candidates.iter().any(|existing| existing == &candidate) {
        candidates.push(candidate);
    }
}

fn extension_variants(path: &Path) -> Vec<PathBuf> {
    let mut variants = vec![path.to_path_buf()];
    let Some(extension) = path.extension().and_then(|ext| ext.to_str()) else {
        return variants;
    };

    let alternates: &[&str] = match extension.to_ascii_lowercase().as_str() {
        "jpg" => &["jpeg", "png"],
        "jpeg" => &["jpg", "png"],
        "png" => &["jpg", "jpeg"],
        _ => &[],
    };

    for alternate in alternates {
        variants.push(path.with_extension(alternate));
    }

    variants
}

fn image_path_candidates(path: &str, base_dir: &Path) -> Vec<PathBuf> {
    let candidate = PathBuf::from(path);
    let mut base_candidates = Vec::new();

    // 1. Try the path exactly as embedded in the file.
    if candidate.is_absolute() {
        push_unique_path(&mut base_candidates, candidate.clone());
    } else {
        push_unique_path(&mut base_candidates, base_dir.join(&candidate));
    }

    if let Some(file_name) = candidate.file_name() {
        // 2. FBX `.fbm` sidecar folder: <model_name>.fbm/<texture_file>
        //    Try the parent folder name joined under base_dir.
        if let Some(parent_name) = candidate.parent().and_then(Path::file_name) {
            push_unique_path(
                &mut base_candidates,
                base_dir.join(parent_name).join(file_name),
            );
        }

        // 3. Common sub-folder names used by DCC tools and asset packs.
        for subfolder in &[
            "textures", "Textures", "tex", "Tex", "maps", "Maps", "material", "Material",
        ] {
            push_unique_path(
                &mut base_candidates,
                base_dir.join(subfolder).join(file_name),
            );
        }

        // 4. Flat next to the FBX.
        push_unique_path(&mut base_candidates, base_dir.join(file_name));

        // 5. Walk up to 3 parent directories and repeat the search in each.
        //    Handles cases where the texture lives beside the FBX in a parent dir.
        let mut ancestor = base_dir.to_path_buf();
        for _ in 0..3 {
            if !ancestor.pop() {
                break;
            }
            push_unique_path(&mut base_candidates, ancestor.join(file_name));
            for subfolder in &["textures", "Textures", "tex", "maps"] {
                push_unique_path(
                    &mut base_candidates,
                    ancestor.join(subfolder).join(file_name),
                );
            }
        }

        // 6. Case-insensitive stem match inside base_dir.
        //    Useful on case-sensitive Linux filesystems when the FBX was created
        //    on Windows (e.g. `Mat_Diffuse.PNG` embedded but file is `mat_diffuse.png`).
        if let Some(stem) = candidate.file_stem().and_then(|s| s.to_str()) {
            let stem_lower = stem.to_ascii_lowercase();
            // Scan base_dir itself.
            for search_dir in [base_dir, &base_dir.join("textures"), &base_dir.join("tex")] {
                if let Ok(entries) = fs::read_dir(search_dir) {
                    for entry in entries.flatten() {
                        let entry_path = entry.path();
                        if !entry_path.is_file() {
                            continue;
                        }
                        let Some(entry_stem) = entry_path.file_stem().and_then(|s| s.to_str())
                        else {
                            continue;
                        };
                        if entry_stem.to_ascii_lowercase() != stem_lower {
                            continue;
                        }
                        let Some(ext) = entry_path.extension().and_then(|e| e.to_str()) else {
                            continue;
                        };
                        let ext_l = ext.to_ascii_lowercase();
                        if matches!(
                            ext_l.as_str(),
                            "png" | "jpg" | "jpeg" | "tga" | "bmp" | "webp" | "tiff" | "tif"
                        ) {
                            push_unique_path(&mut base_candidates, entry_path);
                        }
                    }
                }
            }
        }
    }

    // Expand every base candidate with extension variants (jpg↔jpeg↔png etc.)
    // and also add common formats the FBX may not have listed explicitly.
    let mut candidates = Vec::new();
    for base_candidate in base_candidates {
        for variant in extension_variants(&base_candidate) {
            push_unique_path(&mut candidates, variant);
        }
        // Also try TGA / BMP / WebP / TIFF for each candidate stem — FBX
        // often embeds a .tga path but the artist converted to PNG later.
        for extra_ext in &["tga", "bmp", "webp", "tiff", "tif"] {
            push_unique_path(&mut candidates, base_candidate.with_extension(extra_ext));
        }
    }

    candidates
}

fn resolve_image_path(path: &str, base_dir: &Path) -> Result<PathBuf> {
    let candidates = image_path_candidates(path, base_dir);
    let mut last_error: Option<(PathBuf, io::Error)> = None;

    for candidate in candidates.iter().cloned() {
        if candidate.is_file() {
            return Ok(candidate);
        }

        match fs::metadata(&candidate) {
            Ok(_) => {
                last_error = Some((
                    candidate,
                    io::Error::new(io::ErrorKind::InvalidData, "path exists but is not a file"),
                ));
            }
            Err(error) => {
                last_error = Some((candidate, error));
            }
        }
    }

    let attempted = candidates
        .iter()
        .map(|candidate| format!("'{}'", candidate.display()))
        .collect::<Vec<_>>()
        .join(", ");
    let detail = last_error
        .map(|(_, error)| error.to_string())
        .unwrap_or_else(|| "no candidate paths were generated".to_string());
    Err(AssetError::InvalidData(format!(
        "Failed to read image file '{}'. Tried {}. Last error: {}",
        path, attempted, detail
    )))
}

fn resolve_image_bytes(image: &Image, base_dir: &Path) -> Result<Vec<u8>> {
    match &image.source {
        ImageSource::Embedded { data, .. } => Ok(data.clone()),
        ImageSource::Uri(path) => {
            let resolved = resolve_image_path(path, base_dir)?;
            fs::read(&resolved).map_err(|e| {
                AssetError::InvalidData(format!(
                    "Failed to read image file '{}': {}",
                    resolved.display(),
                    e
                ))
            })
        }
    }
}

fn convert_sampler(sampler: &Sampler) -> TextureSamplerDesc {
    let address_mode = |wrap: WrapMode| match wrap {
        WrapMode::Repeat => wgpu::AddressMode::Repeat,
        WrapMode::ClampToEdge => wgpu::AddressMode::ClampToEdge,
        WrapMode::MirroredRepeat => wgpu::AddressMode::MirrorRepeat,
    };

    let mag_filter = match sampler.mag_filter {
        SolidFilter::Nearest
        | SolidFilter::NearestMipmapNearest
        | SolidFilter::NearestMipmapLinear => wgpu::FilterMode::Nearest,
        SolidFilter::Linear
        | SolidFilter::LinearMipmapNearest
        | SolidFilter::LinearMipmapLinear => wgpu::FilterMode::Linear,
    };

    let (min_filter, mipmap_filter) = match sampler.min_filter {
        SolidFilter::Nearest => (wgpu::FilterMode::Nearest, wgpu::MipmapFilterMode::Nearest),
        SolidFilter::Linear => (wgpu::FilterMode::Linear, wgpu::MipmapFilterMode::Linear),
        SolidFilter::NearestMipmapNearest => {
            (wgpu::FilterMode::Nearest, wgpu::MipmapFilterMode::Nearest)
        }
        SolidFilter::LinearMipmapNearest => {
            (wgpu::FilterMode::Linear, wgpu::MipmapFilterMode::Nearest)
        }
        SolidFilter::NearestMipmapLinear => {
            (wgpu::FilterMode::Nearest, wgpu::MipmapFilterMode::Linear)
        }
        SolidFilter::LinearMipmapLinear => {
            (wgpu::FilterMode::Linear, wgpu::MipmapFilterMode::Linear)
        }
    };

    TextureSamplerDesc {
        address_mode_u: address_mode(sampler.wrap_s),
        address_mode_v: address_mode(sampler.wrap_t),
        address_mode_w: address_mode(sampler.wrap_s),
        mag_filter,
        min_filter,
        mipmap_filter,
    }
}

/// The file on disk behind `scene.textures[texture_index]`, found the same
/// way [`load_texture_upload`] finds it. `None` for an embedded image, an
/// out-of-range index, or a file that cannot be located.
pub fn locate_texture_file(
    scene: &Scene,
    texture_index: usize,
    base_dir: &Path,
) -> Option<PathBuf> {
    let image = scene
        .images
        .get(scene.textures.get(texture_index)?.image_index)?;
    match &image.source {
        ImageSource::Uri(path) => resolve_image_path(path, base_dir).ok(),
        ImageSource::Embedded { .. } => None,
    }
}

pub fn load_texture_upload(
    scene: &Scene,
    texture_index: usize,
    semantic: TextureSemantic,
    base_dir: &Path,
) -> Result<TextureUpload> {
    let texture = scene.textures.get(texture_index).ok_or_else(|| {
        AssetError::InvalidData(format!("Texture index {} is out of bounds", texture_index))
    })?;
    let image = scene.images.get(texture.image_index).ok_or_else(|| {
        AssetError::InvalidData(format!(
            "Texture '{}' references missing image index {}",
            texture.name, texture.image_index
        ))
    })?;

    load_texture_upload_from_parts(texture, image, semantic, base_dir)
}

fn load_texture_upload_from_parts(
    texture: &Texture,
    image: &Image,
    semantic: TextureSemantic,
    base_dir: &Path,
) -> Result<TextureUpload> {
    let bytes = resolve_image_bytes(image, base_dir)?;
    let decoded = image::load_from_memory(&bytes)
        .map_err(|e| AssetError::InvalidData(format!("Failed to decode image: {}", e)))?;
    let rgba = decoded.to_rgba8();
    let (width, height) = rgba.dimensions();
    let label = if texture.name.is_empty() {
        format!(
            "{} ({})",
            if image.name.is_empty() {
                "texture"
            } else {
                &image.name
            },
            semantic.suffix()
        )
    } else {
        format!("{} ({})", texture.name, semantic.suffix())
    };

    Ok(TextureUpload::rgba8(
        label,
        width,
        height,
        semantic.is_srgb(),
        rgba.into_raw(),
        convert_sampler(&texture.sampler),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    struct TestDir(PathBuf);

    impl TestDir {
        fn new(name: &str) -> Self {
            let unique = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("time went backwards")
                .as_nanos();
            let path = std::env::temp_dir().join(format!("helio-asset-compat-{name}-{unique}"));
            fs::create_dir_all(&path).expect("create temp dir");
            Self(path)
        }

        fn path(&self) -> &Path {
            &self.0
        }
    }

    impl Drop for TestDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn resolves_relative_texture_paths_from_base_dir() {
        let temp = TestDir::new("relative");
        let relative = PathBuf::from("materials").join("albedo.jpg");
        let expected = temp.path().join(&relative);
        fs::create_dir_all(expected.parent().expect("relative texture parent"))
            .expect("create parent");
        fs::write(&expected, b"jpg").expect("write texture");

        let resolved = resolve_image_path(&relative.to_string_lossy(), temp.path())
            .expect("resolve relative texture");

        assert_eq!(resolved, expected);
    }

    #[test]
    fn falls_back_to_baked_folder_name_under_base_dir() {
        let temp = TestDir::new("fbm-folder");
        let expected = temp
            .path()
            .join("Untitled.fbm")
            .join("Material _1_BaseColor.jpg");
        fs::create_dir_all(expected.parent().expect("fbm parent")).expect("create fbm dir");
        fs::write(&expected, b"jpg").expect("write texture");

        let baked = temp
            .path()
            .join("missing-root")
            .join("Untitled.fbm")
            .join("Material _1_BaseColor.jpg");
        let resolved = resolve_image_path(&baked.to_string_lossy(), temp.path())
            .expect("resolve baked folder fallback");

        assert_eq!(resolved, expected);
    }

    #[test]
    fn falls_back_to_textures_directory_for_baked_paths() {
        let temp = TestDir::new("textures-folder");
        let expected = temp
            .path()
            .join("textures")
            .join("Material _1_BaseColor.jpg");
        fs::create_dir_all(expected.parent().expect("textures parent"))
            .expect("create textures dir");
        fs::write(&expected, b"jpg").expect("write texture");

        let baked = temp
            .path()
            .join("missing-root")
            .join("Untitled.fbm")
            .join("Material _1_BaseColor.jpg");
        let resolved = resolve_image_path(&baked.to_string_lossy(), temp.path())
            .expect("resolve textures fallback");

        assert_eq!(resolved, expected);
    }

    #[test]
    fn falls_back_across_common_image_extensions() {
        let temp = TestDir::new("extension-fallback");
        let expected = temp
            .path()
            .join("textures")
            .join("Material _1s_BaseColor.jpeg");
        fs::create_dir_all(expected.parent().expect("textures parent"))
            .expect("create textures dir");
        fs::write(&expected, b"jpeg").expect("write texture");

        let baked = temp
            .path()
            .join("missing-root")
            .join("Untitled.fbm")
            .join("Material _1s_BaseColor.jpg");
        let resolved = resolve_image_path(&baked.to_string_lossy(), temp.path())
            .expect("resolve extension fallback");

        assert_eq!(resolved, expected);
    }
}
