//! The sky texture layer: an equirectangular (latitude-longitude) panorama
//! drawn behind the atmosphere's stars, moon and shooting stars.
//!
//! The authored component names a *source* ([`source`]): one of the built-in
//! presets (the CC0 Deep-Space Skybox galaxy and nebula layers, fetched at
//! build time by `build.rs`, see `assets/deep-space/CREDITS.md`) or a file of
//! the user's choosing ([`custom_source`]). The row carries only that 32-bit
//! code; [`SkyTextureCache`] turns it into a GPU texture on demand:
//!
//! - decoding, the size cap and the mip chain run on a worker thread, so a
//!   large panorama never stalls a frame (the sky shows without the layer
//!   until it arrives; the layer is purely additive, so that is never a black
//!   quad);
//! - the GPU copy is `Rgba16Float` with a full mip chain, linear light.
//!   `.hdr` / `.exr` files are already scene-linear; 8-bit and 16-bit formats
//!   (png, jpg, ...) are sRGB and are linearised on load;
//! - only one panorama is resident, and it is released as soon as the layer
//!   is turned off. Panoramas wider than [`MAX_WIDTH`] are halved until they
//!   fit (4096x2048 half-float is 64 MiB, 85 MiB with mips);
//! - a custom file that changes on disk is reloaded.
//!
//! Orientation: the texture's centre looks along -Z and its top is +Y;
//! [`rotation`] (yaw about Y, then pitch about X, then roll about Z) turns
//! the panorama in the world. [`direction_to_uv`] and [`rotation`] mirror the
//! WGSL in `atmosphere_composite.wgsl`.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::{mpsc, Mutex, OnceLock};
use std::time::SystemTime;

/// Source codes carried by the atmosphere row's `sky_texture_source`.
pub mod source {
    /// No sky texture.
    pub const NONE: u32 = 0;
    /// Built-in: the Deep-Space Skybox galaxy band.
    pub const GALAXY: u32 = 1;
    /// Built-in: the Deep-Space Skybox nebula clouds.
    pub const NEBULA: u32 = 2;
    /// Built-in: galaxy band with the nebula brightened ([`super::NEBULA_GAIN`]) and added.
    pub const GALAXY_NEBULA: u32 = 3;
    /// Flag of a user file; the low bits index [`super::custom_source`]'s table.
    pub const CUSTOM: u32 = 0x8000_0000;
}

/// The nebula layer is two orders of magnitude fainter than the galaxy (peak
/// 0.018 against 0.162); this is the gain it is mixed with in the combined preset.
pub const NEBULA_GAIN: f32 = 6.0;
/// Widest panorama kept; wider ones are halved until they fit.
pub const MAX_WIDTH: u32 = 4096;

#[cfg(not(sky_presets_missing))]
static GALAXY_HDR: &[u8] = include_bytes!(concat!(
    env!("OUT_DIR"),
    "/deep-space/deep_space_galaxy_4k.hdr"
));
#[cfg(not(sky_presets_missing))]
static NEBULA_HDR: &[u8] = include_bytes!(concat!(
    env!("OUT_DIR"),
    "/deep-space/deep_space_nebula_4k.hdr"
));
#[cfg(sky_presets_missing)]
static GALAXY_HDR: &[u8] = &[];
#[cfg(sky_presets_missing)]
static NEBULA_HDR: &[u8] = &[];

/// Whether the built-in panoramas were embedded (`build.rs` could download
/// them). User textures work either way.
pub fn presets_available() -> bool {
    !GALAXY_HDR.is_empty() && !NEBULA_HDR.is_empty()
}

fn custom_table() -> &'static Mutex<Vec<PathBuf>> {
    static TABLE: OnceLock<Mutex<Vec<PathBuf>>> = OnceLock::new();
    TABLE.get_or_init(|| Mutex::new(Vec::new()))
}

/// The source code of the user file at `path` (the same path always gets the
/// same code). An empty path is [`source::NONE`].
pub fn custom_source(path: &Path) -> u32 {
    if path.as_os_str().is_empty() {
        return source::NONE;
    }
    let mut table = custom_table().lock().unwrap_or_else(|e| e.into_inner());
    let index = match table.iter().position(|known| known == path) {
        Some(index) => index,
        None => {
            table.push(path.to_path_buf());
            table.len() - 1
        }
    };
    source::CUSTOM | index as u32
}

/// The file behind a [`source::CUSTOM`] code.
pub fn custom_path(code: u32) -> Option<PathBuf> {
    if code & source::CUSTOM == 0 {
        return None;
    }
    let table = custom_table().lock().unwrap_or_else(|e| e.into_inner());
    table.get((code & !source::CUSTOM) as usize).cloned()
}

/// Texture-space orientation of the panorama in the world: yaw about Y, then
/// pitch about X, then roll about Z (radians), as column-major columns.
/// Mirrors `sky_texture_rotation` in `atmosphere_composite.wgsl`.
pub fn rotation(yaw: f32, pitch: f32, roll: f32) -> [[f32; 3]; 3] {
    let (sy, cy) = yaw.sin_cos();
    let (sx, cx) = pitch.sin_cos();
    let (sz, cz) = roll.sin_cos();
    let ry = [[cy, 0.0, -sy], [0.0, 1.0, 0.0], [sy, 0.0, cy]];
    let rx = [[1.0, 0.0, 0.0], [0.0, cx, sx], [0.0, -sx, cx]];
    let rz = [[cz, sz, 0.0], [-sz, cz, 0.0], [0.0, 0.0, 1.0]];
    mul(mul(ry, rx), rz)
}

fn mul(a: [[f32; 3]; 3], b: [[f32; 3]; 3]) -> [[f32; 3]; 3] {
    let mut out = [[0.0; 3]; 3];
    for (c, col) in out.iter_mut().enumerate() {
        for (r, value) in col.iter_mut().enumerate() {
            *value = (0..3).map(|k| a[k][r] * b[c][k]).sum();
        }
    }
    out
}

/// A world direction `d` in the panorama's UV space under `rotation`
/// (as built by [`rotation`]): u wraps around the horizon with the image
/// centre along -Z, v runs from 0 at the zenith to 1 at the nadir.
pub fn direction_to_uv(rotation: [[f32; 3]; 3], d: [f32; 3]) -> [f32; 2] {
    // transpose(rotation) * d: the world direction in texture space.
    let t: Vec<f32> = (0..3)
        .map(|c| rotation[c][0] * d[0] + rotation[c][1] * d[1] + rotation[c][2] * d[2])
        .collect();
    let u = t[0].atan2(-t[2]) / std::f32::consts::TAU + 0.5;
    let v = 0.5 - t[1].clamp(-1.0, 1.0).asin() / std::f32::consts::PI;
    [u, v]
}

/// A decoded panorama: half-float RGBA bits, one `Vec` per mip, linear light.
pub struct SkyImage {
    pub width: u32,
    pub height: u32,
    pub mips: Vec<Vec<u16>>,
}

fn srgb_to_linear(c: f32) -> f32 {
    if c <= 0.04045 {
        c / 12.92
    } else {
        ((c + 0.055) / 1.055).powf(2.4)
    }
}

/// Linear RGB floats of an image: floating-point formats are scene-linear,
/// integer formats are sRGB. Non-finite or negative values become zero.
fn linear_rgb(image: image::DynamicImage) -> (u32, u32, Vec<f32>) {
    let float = matches!(
        image,
        image::DynamicImage::ImageRgb32F(_) | image::DynamicImage::ImageRgba32F(_)
    );
    let rgb = image.to_rgb32f();
    let (w, h) = (rgb.width(), rgb.height());
    let mut data = rgb.into_raw();
    for c in &mut data {
        *c = if !c.is_finite() || *c < 0.0 {
            0.0
        } else if float {
            *c
        } else {
            srgb_to_linear(*c)
        };
    }
    (w, h, data)
}

fn halve(w: u32, h: u32, data: &[f32]) -> (u32, u32, Vec<f32>) {
    let (nw, nh) = ((w / 2).max(1), (h / 2).max(1));
    let mut out = vec![0.0; (nw * nh * 3) as usize];
    for y in 0..nh {
        for x in 0..nw {
            for c in 0..3 {
                let mut sum = 0.0;
                for (dx, dy) in [(0, 0), (1, 0), (0, 1), (1, 1)] {
                    let sx = (x * 2 + dx).min(w - 1);
                    let sy = (y * 2 + dy).min(h - 1);
                    sum += data[((sy * w + sx) * 3 + c) as usize];
                }
                out[((y * nw + x) * 3 + c) as usize] = sum * 0.25;
            }
        }
    }
    (nw, nh, out)
}

/// Builds the half-float mip chain of linear RGB `data`.
pub fn build_image(mut w: u32, mut h: u32, mut data: Vec<f32>) -> SkyImage {
    while w > MAX_WIDTH {
        (w, h, data) = halve(w, h, &data);
    }
    let (width, height) = (w, h);
    let mut mips = Vec::new();
    loop {
        let mut bits = Vec::with_capacity((w * h * 4) as usize);
        for px in data.chunks_exact(3) {
            for c in px {
                bits.push(half::f16::from_f32(c.min(65504.0)).to_bits());
            }
            bits.push(half::f16::from_f32(1.0).to_bits());
        }
        mips.push(bits);
        if w == 1 && h == 1 {
            break;
        }
        (w, h, data) = halve(w, h, &data);
    }
    SkyImage {
        width,
        height,
        mips,
    }
}

fn decode_hdr(bytes: &[u8]) -> Result<(u32, u32, Vec<f32>), String> {
    if bytes.is_empty() {
        return Err("the built-in sky panoramas were not embedded in this build".into());
    }
    let image = image::load_from_memory_with_format(bytes, image::ImageFormat::Hdr)
        .map_err(|e| format!("built-in sky panorama: {e}"))?;
    Ok(linear_rgb(image))
}

/// Decodes the panorama behind `code`. Slow; call off the render thread.
pub fn decode(code: u32) -> Result<SkyImage, String> {
    let (w, h, data) = match code {
        source::GALAXY => decode_hdr(GALAXY_HDR)?,
        source::NEBULA => decode_hdr(NEBULA_HDR)?,
        source::GALAXY_NEBULA => {
            let (w, h, mut galaxy) = decode_hdr(GALAXY_HDR)?;
            let (nw, nh, nebula) = decode_hdr(NEBULA_HDR)?;
            if (w, h) != (nw, nh) {
                return Err("the galaxy and nebula layers differ in size".into());
            }
            for (g, n) in galaxy.iter_mut().zip(nebula) {
                *g += n * NEBULA_GAIN;
            }
            (w, h, galaxy)
        }
        code => {
            let path = custom_path(code).ok_or("unknown sky texture")?;
            let image = image::open(&path).map_err(|e| format!("{}: {e}", path.display()))?;
            linear_rgb(image)
        }
    };
    if w == 0 || h == 0 {
        return Err("empty image".into());
    }
    Ok(build_image(w, h, data))
}

fn modified(code: u32) -> Option<SystemTime> {
    std::fs::metadata(custom_path(code)?).ok()?.modified().ok()
}

struct Loaded {
    code: u32,
    modified: Option<SystemTime>,
    _texture: wgpu::Texture,
    view: wgpu::TextureView,
}

struct Pending {
    code: u32,
    modified: Option<SystemTime>,
    receiver: Mutex<mpsc::Receiver<Result<SkyImage, String>>>,
}

/// Owns the one resident sky panorama and loads it as the row asks.
pub struct SkyTextureCache {
    fallback: wgpu::TextureView,
    loaded: Option<Loaded>,
    pending: Option<Pending>,
    failed: HashSet<u32>,
    frames: u32,
}

/// Frames between checks of a custom file's modification time.
const RELOAD_CHECK_FRAMES: u32 = 120;

impl SkyTextureCache {
    pub fn new(device: &wgpu::Device) -> Self {
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("Sky texture (none)"),
            size: wgpu::Extent3d {
                width: 1,
                height: 1,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba16Float,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        // wgpu zero-initialises textures: the fallback is transparent black.
        Self {
            fallback: texture.create_view(&Default::default()),
            loaded: None,
            pending: None,
            failed: HashSet::new(),
            frames: 0,
        }
    }

    /// Whether a panorama for `code` is on the GPU.
    pub fn is_resident(&self, code: u32) -> bool {
        self.loaded.as_ref().is_some_and(|l| l.code == code)
    }

    /// The view to bind for `code`: its panorama once loaded, otherwise a
    /// 1x1 black texture (the layer is additive, so that draws nothing).
    pub fn view(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        code: u32,
    ) -> &wgpu::TextureView {
        self.frames = self.frames.wrapping_add(1);
        if code == source::NONE {
            self.loaded = None;
            self.pending = None;
            return &self.fallback;
        }
        if self.frames % RELOAD_CHECK_FRAMES == 0 {
            if let Some(loaded) = &self.loaded {
                if loaded.code == code && loaded.modified != modified(code) {
                    self.loaded = None;
                    self.failed.remove(&code);
                }
            }
        }
        if self.loaded.as_ref().is_some_and(|l| l.code == code) {
            return &self.loaded.as_ref().expect("checked").view;
        }
        if self.frames % 600 == 0 {
            tracing::info!(
                "sky texture {code:#x}: not resident yet (decoding: {}, failed: {})",
                self.pending.is_some(),
                self.failed.contains(&code)
            );
        }
        if self.failed.contains(&code) {
            return &self.fallback;
        }
        let polled = match &self.pending {
            Some(pending) if pending.code == code => Some((
                pending.modified,
                pending
                    .receiver
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .try_recv(),
            )),
            _ => None,
        };
        match polled {
            Some((modified, Ok(Ok(image)))) => {
                tracing::info!("sky texture {code:#x} loaded ({}x{}, {} mips)", image.width, image.height, image.mips.len());
                self.pending = None;
                self.loaded = Some(upload(device, queue, code, modified, &image));
            }
            Some((_, Ok(Err(error)))) => {
                tracing::warn!("sky texture: {error}");
                self.failed.insert(code);
                self.pending = None;
            }
            Some((_, Err(mpsc::TryRecvError::Empty))) => {}
            Some((_, Err(mpsc::TryRecvError::Disconnected))) => {
                self.failed.insert(code);
                self.pending = None;
            }
            None => {
                tracing::info!(
                    "sky texture {code:#x}: decoding on a worker thread (built-in presets embedded: {}, file: {:?})",
                    presets_available(),
                    custom_path(code)
                );
                let (sender, receiver) = mpsc::channel();
                std::thread::Builder::new()
                    .name("sky-texture-load".into())
                    .spawn(move || {
                        let _ = sender.send(decode(code));
                    })
                    .ok();
                self.pending = Some(Pending {
                    code,
                    modified: modified(code),
                    receiver: Mutex::new(receiver),
                });
            }
        }
        match &self.loaded {
            Some(loaded) if loaded.code == code => &loaded.view,
            _ => &self.fallback,
        }
    }
}

fn upload(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    code: u32,
    modified: Option<SystemTime>,
    image: &SkyImage,
) -> Loaded {
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("Sky texture"),
        size: wgpu::Extent3d {
            width: image.width,
            height: image.height,
            depth_or_array_layers: 1,
        },
        mip_level_count: image.mips.len() as u32,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba16Float,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });
    for (level, bits) in image.mips.iter().enumerate() {
        let (w, h) = (
            (image.width >> level).max(1),
            (image.height >> level).max(1),
        );
        queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &texture,
                mip_level: level as u32,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            bytemuck::cast_slice(bits),
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(w * 8),
                rows_per_image: Some(h),
            },
            wgpu::Extent3d {
                width: w,
                height: h,
                depth_or_array_layers: 1,
            },
        );
    }
    Loaded {
        code,
        modified,
        view: texture.create_view(&Default::default()),
        _texture: texture,
    }
}

static REQUESTED: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(source::NONE);

/// Publishes the source the level's sky wants drawn. The atmosphere row
/// carries only the code to the shader, so the component announces it here
/// when it derives its row (a level has one sky) and the composite pass reads
/// it with [`requested`] to keep the right panorama resident.
pub fn set_requested(code: u32) {
    let previous = REQUESTED.swap(code, std::sync::atomic::Ordering::Relaxed);
    if previous != code {
        tracing::info!("sky texture requested: {previous:#x} -> {code:#x}");
    }
}

/// The source last published with [`set_requested`].
pub fn requested() -> u32 {
    REQUESTED.load(std::sync::atomic::Ordering::Relaxed)
}
