//! Scene-owned texture residency for graph materials: each texture is
//! registered once per scene texture store, keyed by that store.
use pulsar_scenedb::gpu::{GpuMirrorHandle, TextureStore};
use std::{
    collections::HashMap,
    path::PathBuf,
    sync::{Arc, Mutex, OnceLock, RwLock, Weak},
};

struct CachedTexture {
    owner: Weak<RwLock<TextureStore>>,
    fingerprint: u64,
    slot: u32,
}

type TextureCache = HashMap<(usize, PathBuf), CachedTexture>;

fn graph_texture_cache() -> &'static Mutex<TextureCache> {
    static CACHE: OnceLock<Mutex<TextureCache>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Register the image at `path` in `mirror`'s texture store and return its
/// slot. Re-registers only when the file changed; a file that fails to
/// decode leaves the current registration in place.
pub fn register_graph_texture(
    path: &std::path::Path,
    mirror: &GpuMirrorHandle,
) -> Result<u32, String> {
    use std::hash::{Hash, Hasher};
    let bytes = std::fs::read(path).map_err(|error| error.to_string())?;
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    bytes.hash(&mut hasher);
    let fingerprint = hasher.finish();
    let texture_store = mirror
        .texture_store()
        .ok_or_else(|| "SceneDB has no material texture store".to_string())?;
    let key = (Arc::as_ptr(&texture_store) as usize, path.to_path_buf());
    let mut cache = graph_texture_cache()
        .lock()
        .map_err(|_| "graph texture cache lock poisoned".to_string())?;
    cache.retain(|_, entry| entry.owner.strong_count() != 0);
    if let Some(entry) = cache.get(&key) {
        if entry.fingerprint == fingerprint {
            return Ok(entry.slot);
        }
    }
    let decoded = image::load_from_memory(&bytes)
        .map_err(|error| format!("could not decode texture image: {error}"))?
        .to_rgba8();
    let (width, height) = decoded.dimensions();
    if width == 0 || height == 0 {
        return Err("texture image has empty dimensions".to_string());
    }
    // Decode before replacing residency: malformed input must leave the
    // currently bound texture intact.
    let mut store = texture_store
        .write()
        .map_err(|_| "SceneDB texture store lock poisoned".to_string())?;
    if let Some(old) = cache.remove(&key) {
        let _ = store.unregister(old.slot);
    }
    let slot = upload_rgba(&mut store, mirror, width, height, decoded.as_raw())?;
    cache.insert(
        key,
        CachedTexture {
            owner: Arc::downgrade(&texture_store),
            fingerprint,
            slot,
        },
    );
    Ok(slot)
}

/// Upload an sRGB RGBA8 image of `width` by `height` into `store` (the
/// texture store of `mirror`'s scene) and return its slot.
pub(crate) fn upload_rgba(
    store: &mut TextureStore,
    mirror: &GpuMirrorHandle,
    width: u32,
    height: u32,
    rgba: &[u8],
) -> Result<u32, String> {
    let device = mirror.store().device_arc();
    let descriptor = wgpu::TextureDescriptor {
        label: Some("Scene Texture"),
        size: wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8UnormSrgb,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    };
    store
        .register(&device, mirror.queue(), &descriptor, rgba)
        .map_err(|error| format!("could not register texture in SceneDB: {error:?}"))
}

/// Upload a cooked texture (its full mip chain, in its own format) into
/// `store`, the texture store of `mirror`'s scene, and return its slot.
pub(crate) fn upload_cooked(
    store: &mut TextureStore,
    mirror: &GpuMirrorHandle,
    cooked: &crate::cooked_texture::CookedTexture,
) -> Result<u32, String> {
    let device = mirror.store().device_arc();
    let descriptor = wgpu::TextureDescriptor {
        label: Some("Scene Texture"),
        size: wgpu::Extent3d {
            width: cooked.width,
            height: cooked.height,
            depth_or_array_layers: 1,
        },
        mip_level_count: cooked.mip_count(),
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: cooked.wgpu_format(),
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    };
    store
        .register_mips(&device, mirror.queue(), &descriptor, &cooked.mips())
        .map_err(|error| format!("could not register texture in SceneDB: {error:?}"))
}
