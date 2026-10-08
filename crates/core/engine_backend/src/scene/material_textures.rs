//! Scene-owned texture residency for graph materials.
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

pub(crate) fn register_graph_texture(
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
    let device = mirror.store().device_arc();
    let descriptor = wgpu::TextureDescriptor {
        label: Some("Blueprint Material Texture"),
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
    let slot = store
        .register(&device, mirror.queue(), &descriptor, decoded.as_raw())
        .map_err(|error| format!("could not register texture in SceneDB: {error:?}"))?;
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
