//! Texture jobs of the museum: which image files feed which material channel,
//! decoding (JPEG/PNG), optional down-scaling to the quality budget and CPU
//! mip chains (SceneDB's texture store takes the full chain), plus the little
//! bit of glTF the tree packages need (alpha modes, UV transforms, image
//! names).

use super::materials::Mat;
use crate::vg_stress_prep::{
    catalog::{Config, TextureQuality},
    polyhaven,
};
use rayon::prelude::*;
use std::{
    collections::HashMap,
    path::{Path, PathBuf},
};

/// Material channels (the second element of a binding).
pub const CH_BASE: u8 = 0;
pub const CH_NORMAL: u8 = 1;
pub const CH_ARM: u8 = 2;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Role {
    /// Colour data stored sRGB.
    Srgb,
    /// Tangent-space normal map (re-normalised when averaged).
    Normal,
    /// Linear data (ARM: ambient occlusion, roughness, metalness).
    Linear,
}

#[derive(Clone, Debug)]
pub struct Job {
    pub key: String,
    pub path: PathBuf,
    /// Separate linear opacity for Poly Haven's JPEG leaf colour maps.
    pub opacity: Option<PathBuf>,
    pub role: Role,
    /// Longest side after loading; larger images are downscaled.
    pub cap: u32,
    pub bindings: Vec<(Mat, u8)>,
}

pub struct Decoded {
    pub width: u32,
    pub height: u32,
    /// RGBA8 levels, level 0 first, down to 1x1.
    pub mips: Vec<Vec<u8>>,
}

impl Decoded {
    pub fn bytes(&self) -> u64 {
        self.mips.iter().map(|m| m.len() as u64).sum()
    }
}

/// Longest side allowed for a texture of this kind at `quality`.
pub fn cap(quality: TextureQuality, marble: bool, role: Role) -> u32 {
    let colour = role == Role::Srgb;
    match (quality, marble, colour) {
        (TextureQuality::Low, true, true) => 4096,
        (TextureQuality::Low, true, false) => 2048,
        (TextureQuality::Low, false, _) => 1024,
        (TextureQuality::Balanced, true, true) => 8192,
        (TextureQuality::Balanced, true, false) => 4096,
        (TextureQuality::Balanced, false, true) => 2048,
        (TextureQuality::Balanced, false, false) => 1024,
        (TextureQuality::Max, true, _) => 16384,
        (TextureQuality::Max, false, true) => 4096,
        (TextureQuality::Max, false, false) => 2048,
    }
}

/// Bytes of a full RGBA8 mip chain for a `w` x `h` texture.
pub fn chain_bytes(w: u32, h: u32) -> u64 {
    let (mut w, mut h, mut total) = (w.max(1), h.max(1), 0u64);
    loop {
        total += u64::from(w) * u64::from(h) * 4;
        if w == 1 && h == 1 {
            return total;
        }
        w = (w / 2).max(1);
        h = (h / 2).max(1);
    }
}

// ── glTF bits ───────────────────────────────────────────────────────────────

#[derive(Clone, Debug, Default, PartialEq)]
pub struct GltfMaterial {
    /// `alphaMode` is `BLEND` or `MASK`.
    pub masked: bool,
    /// Image paths as written in the glTF (relative to it).
    pub base: Option<String>,
    pub normal: Option<String>,
    pub arm: Option<String>,
    /// `KHR_texture_transform` of the base colour texture: `(scale, offset)`.
    pub uv: Option<([f32; 2], [f32; 2])>,
}

pub fn gltf_materials(doc: &serde_json::Value) -> HashMap<String, GltfMaterial> {
    let image_of = |tex: &serde_json::Value| -> Option<String> {
        let t = doc["textures"].get(tex["index"].as_u64()? as usize)?;
        doc["images"].get(t["source"].as_u64()? as usize)?["uri"]
            .as_str()
            .map(str::to_string)
    };
    let mut out = HashMap::new();
    for m in doc["materials"].as_array().into_iter().flatten() {
        let Some(name) = m["name"].as_str() else {
            continue;
        };
        let pbr = &m["pbrMetallicRoughness"];
        let base = &pbr["baseColorTexture"];
        let pair = |v: &serde_json::Value| -> Option<[f32; 2]> {
            Some([v.get(0)?.as_f64()? as f32, v.get(1)?.as_f64()? as f32])
        };
        let tt = &base["extensions"]["KHR_texture_transform"];
        let rotation = tt["rotation"].as_f64().unwrap_or(0.0);
        let uv = (rotation == 0.0)
            .then(|| Some((pair(&tt["scale"])?, pair(&tt["offset"]).unwrap_or([0.0; 2]))))
            .flatten();
        out.insert(
            name.to_string(),
            GltfMaterial {
                masked: matches!(m["alphaMode"].as_str(), Some("BLEND") | Some("MASK")),
                base: image_of(base),
                normal: image_of(&m["normalTexture"]),
                arm: image_of(&pbr["metallicRoughnessTexture"]),
                uv,
            },
        );
    }
    out
}

pub fn read_gltf_materials(path: &Path) -> HashMap<String, GltfMaterial> {
    std::fs::read_to_string(path)
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
        .map(|d| gltf_materials(&d))
        .unwrap_or_default()
}

// ── jobs ────────────────────────────────────────────────────────────────────

/// Every texture upload the scene wants, deduplicated by file and role so
/// materials sharing an image share one GPU texture. Marble sets and tree
/// parts whose files are not on disk are simply left out (flat colours).
pub fn jobs(assets: &Path, mats: &[Mat], config: &Config) -> Vec<Job> {
    let mut by_key: HashMap<String, usize> = HashMap::new();
    let mut out: Vec<Job> = Vec::new();
    let mut add = |path: PathBuf,
                   opacity: Option<PathBuf>,
                   role: Role,
                   marble: bool,
                   mat: Mat,
                   channel: u8| {
        if !path.is_file() {
            return;
        }
        let key = format!("{}|{:?}|{:?}", path.display(), role, opacity);
        match by_key.get(&key) {
            Some(&i) => out[i].bindings.push((mat, channel)),
            None => {
                by_key.insert(key.clone(), out.len());
                out.push(Job {
                    key,
                    path,
                    opacity,
                    role,
                    cap: cap(config.quality, marble, role),
                    bindings: vec![(mat, channel)],
                });
            }
        }
    };
    let mut gltfs: HashMap<&'static str, Option<(PathBuf, HashMap<String, GltfMaterial>)>> =
        HashMap::new();
    for &mat in mats {
        if let Some(id) = mat.marble_set() {
            for (role_name, role, channel) in [
                ("diffuse", Role::Srgb, CH_BASE),
                ("normal", Role::Normal, CH_NORMAL),
                ("arm", Role::Linear, CH_ARM),
            ] {
                if let Some(path) = polyhaven::marble_map(assets, id, role_name) {
                    add(path, None, role, true, mat, channel);
                }
            }
        } else if let Some((tree, slot)) = mat.tree_part() {
            let entry = gltfs.entry(tree.id).or_insert_with(|| {
                polyhaven::tree_gltf(assets, tree.id).map(|p| {
                    let info = read_gltf_materials(&p);
                    (p, info)
                })
            });
            let Some((gltf, info)) = entry else { continue };
            let Some(m) = info.get(slot.name) else {
                continue;
            };
            let dir = gltf.parent().unwrap_or(assets).to_path_buf();
            for (file, role, channel) in [
                (&m.base, Role::Srgb, CH_BASE),
                (&m.normal, Role::Normal, CH_NORMAL),
                (&m.arm, Role::Linear, CH_ARM),
            ] {
                if let Some(file) = file {
                    let opacity = (m.masked
                        && channel == CH_BASE
                        && file.ends_with(".jpg")
                        && file.contains("_diff_"))
                    .then(|| {
                        dir.join(file.replace("_diff_", "_alpha_"))
                            .with_extension("png")
                    });
                    add(dir.join(file), opacity, role, false, mat, channel);
                }
            }
        }
    }
    out
}

/// Expected GPU bytes of the texture jobs (full mip chains) given the files'
/// real dimensions are unknown here: uses the cap as an upper bound only for
/// files whose header says they are larger.
pub fn estimate_bytes(jobs: &[Job]) -> u64 {
    jobs.iter()
        .map(|j| {
            let side = image::image_dimensions(&j.path)
                .map(|(w, h)| w.max(h))
                .unwrap_or(j.cap)
                .min(j.cap);
            chain_bytes(side, side)
        })
        .sum()
}

// ── decoding and mips ───────────────────────────────────────────────────────

fn srgb_to_linear_lut() -> [f32; 256] {
    let mut lut = [0.0f32; 256];
    for (i, v) in lut.iter_mut().enumerate() {
        let c = i as f32 / 255.0;
        *v = if c <= 0.04045 {
            c / 12.92
        } else {
            ((c + 0.055) / 1.055).powf(2.4)
        };
    }
    lut
}

fn linear_to_srgb8(l: f32) -> u8 {
    let c = l.clamp(0.0, 1.0);
    let s = if c <= 0.003_130_8 {
        c * 12.92
    } else {
        1.055 * c.powf(1.0 / 2.4) - 0.055
    };
    (s * 255.0 + 0.5) as u8
}

/// One 2x box-filtered level (sRGB-aware for colour, renormalising for normals).
pub fn downsample(src: &[u8], w: u32, h: u32, role: Role) -> (Vec<u8>, u32, u32) {
    let (nw, nh) = ((w / 2).max(1), (h / 2).max(1));
    let lut = srgb_to_linear_lut();
    let mut out = vec![0u8; (nw * nh * 4) as usize];
    out.par_chunks_mut((nw * 4) as usize)
        .enumerate()
        .for_each(|(y, row)| {
            for x in 0..nw as usize {
                let mut taps = [[0u8; 4]; 4];
                for (i, (dx, dy)) in [(0, 0), (1, 0), (0, 1), (1, 1)].into_iter().enumerate() {
                    let sx = (x * 2 + dx).min(w as usize - 1);
                    let sy = (y * 2 + dy).min(h as usize - 1);
                    let o = (sy * w as usize + sx) * 4;
                    taps[i] = [src[o], src[o + 1], src[o + 2], src[o + 3]];
                }
                let px = &mut row[x * 4..x * 4 + 4];
                match role {
                    Role::Srgb => {
                        for c in 0..3 {
                            let lin = taps.iter().map(|t| lut[t[c] as usize]).sum::<f32>() * 0.25;
                            px[c] = linear_to_srgb8(lin);
                        }
                        px[3] = ((taps.iter().map(|t| u32::from(t[3])).sum::<u32>() + 2) / 4) as u8;
                    }
                    Role::Normal => {
                        let mut v = [0.0f32; 3];
                        for t in &taps {
                            for c in 0..3 {
                                v[c] += t[c] as f32 / 255.0 * 2.0 - 1.0;
                            }
                        }
                        let len = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt().max(1e-6);
                        for c in 0..3 {
                            px[c] = ((v[c] / len * 0.5 + 0.5) * 255.0 + 0.5) as u8;
                        }
                        px[3] = 255;
                    }
                    Role::Linear => {
                        for c in 0..4 {
                            px[c] =
                                ((taps.iter().map(|t| u32::from(t[c])).sum::<u32>() + 2) / 4) as u8;
                        }
                    }
                }
            }
        });
    (out, nw, nh)
}

/// The full chain from level 0.
pub fn build_mips(level0: Vec<u8>, w: u32, h: u32, role: Role) -> Vec<Vec<u8>> {
    let mut mips = vec![level0];
    let (mut cw, mut ch) = (w, h);
    while cw > 1 || ch > 1 {
        let (next, nw, nh) = downsample(mips.last().unwrap(), cw, ch, role);
        mips.push(next);
        cw = nw;
        ch = nh;
    }
    mips
}

fn alpha_coverage(pixels: &[u8]) -> f64 {
    pixels.chunks_exact(4).filter(|p| p[3] >= 128).count() as f64 / (pixels.len() / 4) as f64
}

/// Match binary coverage at the 0.5 cutout threshold as closely as the mip's
/// texels permit. Leave RGB and zero alpha untouched; never fill empty cards.
fn preserve_coverage(pixels: &mut [u8], target: f64) {
    let mut histogram = [0usize; 256];
    for p in pixels.chunks_exact(4) {
        histogram[p[3] as usize] += 1;
    }
    let total = (pixels.len() / 4) as f64;
    let mut selected = 0usize;
    let mut threshold = 256usize;
    let mut best = target;
    for t in (1..=255).rev() {
        selected += histogram[t];
        let error = (selected as f64 / total - target).abs();
        if error < best || (error == best && t.abs_diff(128) < threshold.abs_diff(128)) {
            best = error;
            threshold = t;
        }
    }
    let scale = 127.5 / (threshold as f64 - 0.5);
    for p in pixels.chunks_exact_mut(4) {
        p[3] = (p[3] as f64 * scale).round().clamp(0.0, 255.0) as u8;
    }
}

fn cutout_mips(level0: Vec<u8>, w: u32, h: u32, coverage: f64) -> Vec<Vec<u8>> {
    // Filter the unscaled alpha chain, so compensation does not accumulate.
    let mut mips = build_mips(level0, w, h, Role::Srgb);
    for mip in &mut mips {
        preserve_coverage(mip, coverage);
    }
    mips
}

pub fn decode(path: &Path, role: Role, cap: u32) -> Result<Decoded, String> {
    decode_with_opacity(path, None, role, cap)
}

pub fn decode_with_opacity(
    path: &Path,
    opacity: Option<&Path>,
    role: Role,
    cap: u32,
) -> Result<Decoded, String> {
    let mut reader = image::ImageReader::open(path)
        .map_err(|e| format!("{}: {e}", path.display()))?
        .with_guessed_format()
        .map_err(|e| format!("{}: {e}", path.display()))?;
    reader.no_limits();
    let img = reader
        .decode()
        .map_err(|e| format!("{}: {e}", path.display()))?;
    let (w, h) = (img.width(), img.height());
    let mut rgba = img.into_rgba8();
    if let Some(path) = opacity {
        // Apply before resizing; filtering a missing (opaque) alpha channel
        // first cannot reconstruct the leaf silhouette later.
        let mask = image::open(path)
            .map_err(|e| format!("opacity {}: {e}", path.display()))?
            .into_luma8();
        if mask.dimensions() != (w, h) {
            return Err(format!(
                "opacity {} dimensions {:?} do not match colour {w}x{h}",
                path.display(),
                mask.dimensions()
            ));
        }
        for (colour, alpha) in rgba.pixels_mut().zip(mask.pixels()) {
            colour[3] = ((u16::from(colour[3]) * u16::from(alpha[0]) + 127) / 255) as u8;
        }
    }
    let coverage = opacity.map(|_| alpha_coverage(rgba.as_raw()));
    let (mut w, mut h) = (w, h);
    if w.max(h) > cap {
        let scale = f64::from(cap) / f64::from(w.max(h));
        let (nw, nh) = (
            ((f64::from(w) * scale).round() as u32).max(1),
            ((f64::from(h) * scale).round() as u32).max(1),
        );
        rgba = image::imageops::resize(&rgba, nw, nh, image::imageops::FilterType::Triangle);
        w = nw;
        h = nh;
    }
    let mips = if let Some(coverage) = coverage {
        cutout_mips(rgba.into_raw(), w, h, coverage)
    } else {
        build_mips(rgba.into_raw(), w, h, role)
    };
    Ok(Decoded {
        width: w,
        height: h,
        mips,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cutout_mips_preserve_coverage_without_filling_empty_texels() {
        let base: Vec<u8> = (0..64 * 64)
            .flat_map(|i| [80, 160, 40, if (i % 64) < 24 { 255 } else { 0 }])
            .collect();
        let coverage = alpha_coverage(&base);
        let mips = cutout_mips(base, 64, 64, coverage);
        for mip in &mips[..5] {
            assert!((alpha_coverage(mip) - coverage).abs() <= 0.125);
            assert!(mip.chunks_exact(4).all(|p| p[..3] == [80, 160, 40]));
        }
        let mut empty = vec![0; 64];
        preserve_coverage(&mut empty, 1.0);
        assert!(empty.iter().all(|x| *x == 0));
    }

    #[test]
    fn jpeg_leaf_colour_requires_its_separate_linear_opacity() {
        let dir = std::env::temp_dir().join(format!("vg-opacity-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let colour = dir.join("leaves.jpg");
        let opacity = dir.join("alpha.png");
        image::RgbImage::from_pixel(8, 8, image::Rgb([30, 120, 40]))
            .save(&colour)
            .unwrap();
        assert!(
            decode_with_opacity(&colour, Some(&dir.join("missing.png")), Role::Srgb, 8).is_err()
        );
        image::GrayImage::from_fn(8, 8, |x, _| image::Luma([if x < 4 { 255 } else { 0 }]))
            .save(&opacity)
            .unwrap();
        let decoded = decode_with_opacity(&colour, Some(&opacity), Role::Srgb, 4).unwrap();
        assert_eq!((decoded.width, decoded.height), (4, 4));
        assert_eq!(alpha_coverage(&decoded.mips[0]), 0.5);
        image::GrayImage::new(4, 4).save(&opacity).unwrap();
        assert!(decode_with_opacity(&colour, Some(&opacity), Role::Srgb, 8).is_err());
    }

    fn fixture(name: &str) -> serde_json::Value {
        let p = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("vg_stress_prep/fixtures")
            .join(name);
        serde_json::from_str(&std::fs::read_to_string(p).unwrap()).unwrap()
    }

    #[test]
    fn real_gltf_alpha_modes_and_uv_transforms_are_read() {
        let leaves = gltf_materials(&fixture("island_tree_01.gltf"));
        assert!(leaves["island_tree_01_leaves"].masked);
        assert!(!leaves["island_tree_01"].masked && !leaves["island_tree_01_branches"].masked);
        assert_eq!(
            leaves["island_tree_01_leaves"].base.as_deref(),
            Some("textures/island_tree_01_leaves_diff_2k.jpg")
        );
        assert_eq!(
            leaves["island_tree_01_leaves"].normal.as_deref(),
            Some("textures/island_tree_01_leaves_nor_gl_2k.jpg")
        );
        assert_eq!(
            leaves["island_tree_01_leaves"].arm.as_deref(),
            Some("textures/island_tree_01_leaves_arm_2k.jpg")
        );
        let (scale, offset) = leaves["island_tree_01_branches"].uv.unwrap();
        assert!((scale[0] - 15.0).abs() < 1e-4 && (scale[1] - 3.4).abs() < 1e-4);
        assert!((offset[1] + 2.4).abs() < 1e-4);
        assert!(leaves["island_tree_01"].uv.is_none());
        let small = gltf_materials(&fixture("tree_small_02.gltf"));
        assert!(small["tree_small_02_leaves"].masked && !small["tree_small_02_trunk"].masked);
        assert_eq!(
            small["tree_small_02_trunk"].base.as_deref(),
            Some("textures/tree_small_02_diff_2k.jpg")
        );
    }

    #[test]
    fn catalogue_masked_slots_match_the_real_gltf_flags() {
        use crate::vg_stress_prep::catalog;
        for id in ["island_tree_01", "tree_small_02"] {
            let info = gltf_materials(&fixture(&format!("{id}.gltf")));
            let tree = catalog::tree(id).unwrap();
            assert_eq!(tree.slots.len(), info.len(), "{id}");
            for slot in tree.slots {
                assert_eq!(info[slot.name].masked, slot.masked, "{}", slot.name);
            }
        }
    }

    #[test]
    fn mip_chain_has_wgpu_sizes_and_averages() {
        let (w, h) = (8u32, 4u32);
        let level0: Vec<u8> = (0..w * h)
            .flat_map(|i| [(i % 2 * 200) as u8, 100, 50, 255])
            .collect();
        let mips = build_mips(level0, w, h, Role::Linear);
        let sizes: Vec<usize> = mips.iter().map(|m| m.len() / 4).collect();
        assert_eq!(sizes, [32, 8, 2, 1]);
        assert_eq!(mips[1][0], 100, "mean of 0 and 200");
        assert_eq!(chain_bytes(8, 4), 4 * (32 + 8 + 2 + 1));
        assert_eq!(chain_bytes(8192, 8192) / (1 << 20), 341);
    }

    #[test]
    fn normal_mips_stay_unit_length_and_srgb_mips_average_in_linear_light() {
        // Two opposite tilts average to flat, renormalised.
        let tilt = |x: u8| [x, 128, 255, 255];
        let px: Vec<u8> = [tilt(255), tilt(0), tilt(255), tilt(0)].concat();
        let (n, _, _) = downsample(&px, 2, 2, Role::Normal);
        assert!((i32::from(n[0]) - 128).abs() <= 1 && n[2] >= 253);
        // Black and white average to about 188 in sRGB, not 128.
        let bw: Vec<u8> = [
            [0, 0, 0, 255],
            [255, 255, 255, 255],
            [0, 0, 0, 255],
            [255, 255, 255, 255],
        ]
        .concat();
        let (s, _, _) = downsample(&bw, 2, 2, Role::Srgb);
        assert!((185..=190).contains(&s[0]), "{}", s[0]);
    }

    #[test]
    fn caps_follow_the_quality_budget() {
        assert_eq!(cap(TextureQuality::Balanced, true, Role::Srgb), 8192);
        assert_eq!(cap(TextureQuality::Balanced, true, Role::Normal), 4096);
        assert_eq!(cap(TextureQuality::Balanced, false, Role::Linear), 1024);
        assert_eq!(cap(TextureQuality::Low, true, Role::Srgb), 4096);
        assert_eq!(cap(TextureQuality::Max, true, Role::Linear), 16384);
    }

    #[test]
    fn decode_downscales_to_the_cap_and_jobs_dedupe_shared_images() {
        let dir = std::env::temp_dir().join(format!("vgtex-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let img =
            image::RgbImage::from_fn(64, 32, |x, y| image::Rgb([x as u8 * 4, y as u8 * 8, 77]));
        img.save(dir.join("t.png")).unwrap();
        let d = decode(&dir.join("t.png"), Role::Srgb, 16).unwrap();
        assert_eq!((d.width, d.height), (16, 8));
        assert_eq!(d.mips.len(), 5);
        let full = decode(&dir.join("t.png"), Role::Srgb, 4096).unwrap();
        assert_eq!((full.width, full.height), (64, 32));
        assert_eq!(full.bytes(), chain_bytes(64, 32));
        // No files provisioned: no jobs, and nothing panics.
        assert!(jobs(&dir, &Mat::all(&Config::default()), &Config::default()).is_empty());
    }
}
