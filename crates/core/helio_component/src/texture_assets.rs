//! Image files as SceneDB assets (SceneDB#84): what a sprite's `texture` and
//! a decal's `albedo_texture` name.
//!
//! A component names its image by path and nothing loads it on the caller's
//! side. Its handle-ledger collector turns the path into an [`AssetId`] of
//! [`TEXTURE_ASSET_KIND`], so the world counts the reference however the
//! value got there (a typed insert, a decoded level, a reflected write, a
//! script), loads the file on a worker thread and binds the decoded image
//! into the component's [`BoundTexture`]. The GPU mirror registers that
//! image in the scene's texture store when it derives the component's row;
//! it never reads the file.
//!
//! Reload is the world's: [`texture_source_for_file`] maps a changed file to
//! the source its asset was declared under, for
//! `World::asset_source_changed`, which rebinds every component showing it.

use pulsar_scenedb::{AssetId, AssetPayload, AssetView};
use std::sync::Arc;

use crate::cooked_texture::{cook_as, CookFormat, CookedTexture, TextureUse, COOK_VERSION};

/// The SceneDB asset kind of image files. A texture asset's source is the
/// file's absolute path, normalized lexically ([`texture_source`]).
pub const TEXTURE_ASSET_KIND: &str = "texture";

/// A decoded and cooked image: what every component showing the file is bound
/// to.
///
/// What reaches the GPU is the image's *cooked* form (see
/// [`crate::cooked_texture`]): capped in size, with a full mip chain, and
/// block-compressed where the device can sample it. That form is rebuildable
/// data, not owned data. It sits in SceneDB's budgeted residency cache
/// ([`pulsar_scenedb::Residency`]), so what stays in memory is bounded by the
/// cache budget rather than by how many images components reference, and the
/// cooked bytes are also kept on disk by SceneDB's derived-data cache
/// ([`pulsar_scenedb::derive_cached`]), so rebuilding one costs a file read,
/// not a decode and an encode.
pub struct LoadedTexture {
    /// Dimensions after the [`max_texture_size`] cap.
    pub width: u32,
    pub height: u32,
    source: std::path::PathBuf,
    cap: u32,
    /// One residency per (cook format, use), made the first time that pair is
    /// asked for. A device wants one format; a second device may want the
    /// other; and the same image can be a colour in one material and data in
    /// another, which are different bytes on the GPU.
    cooked: [[std::sync::OnceLock<pulsar_scenedb::Residency<CookedTexture>>; 3]; 2],
}

impl LoadedTexture {
    /// Load the image file at `path`: read its size, and cook it now in the
    /// engine's preferred format as a colour (see
    /// [`set_preferred_cook_format`]). A file that cannot be decoded is an
    /// error here.
    pub fn load(path: &std::path::Path) -> Result<Self, String> {
        decode_image_file(path)
    }

    /// The format to cook for a request of `format`: BC7 needs a width and
    /// height divisible by 4, so other sizes are cooked uncompressed.
    fn effective_format(&self, format: CookFormat) -> CookFormat {
        if format == CookFormat::Bc7 && (self.width % 4 != 0 || self.height % 4 != 0) {
            CookFormat::Rgba8
        } else {
            format
        }
    }

    fn residency(
        &self,
        format: CookFormat,
        usage: TextureUse,
    ) -> &pulsar_scenedb::Residency<CookedTexture> {
        let format = self.effective_format(format);
        let row = match format {
            CookFormat::Rgba8 => 0,
            CookFormat::Bc7 => 1,
        };
        let column = match usage {
            TextureUse::Color => 0,
            TextureUse::Data => 1,
            TextureUse::Normal => 2,
        };
        self.cooked[row][column].get_or_init(|| {
            let (path, cap) = (self.source.clone(), self.cap);
            pulsar_scenedb::Residency::lazy(
                |cooked: &CookedTexture| cooked.byte_size(),
                move || cook_from_source(&path, cap, format, usage),
            )
        })
    }

    /// The cooked colour texture in `format` (or the closest this size
    /// allows): [`Self::cooked_for`] with [`TextureUse::Color`].
    pub fn cooked(&self, format: CookFormat) -> Result<Arc<CookedTexture>, String> {
        self.cooked_for(format, TextureUse::Color)
    }

    /// The cooked texture in `format` for `usage`, read from the derived-data
    /// cache or cooked again if the residency cache dropped it. Hold the result
    /// only while using it: the cache can drop its own reference, but not
    /// yours.
    pub fn cooked_for(
        &self,
        format: CookFormat,
        usage: TextureUse,
    ) -> Result<Arc<CookedTexture>, String> {
        self.residency(format, usage).get()
    }

    /// Start producing the cooked colour texture on the asset loader threads:
    /// [`Self::prefetch_for`] with [`TextureUse::Color`].
    pub fn prefetch(&self, format: CookFormat) -> pulsar_scenedb::Prefetch<CookedTexture> {
        self.prefetch_for(format, TextureUse::Color)
    }

    /// Start producing the cooked texture on the asset loader threads, so a
    /// caller about to need several can have them cooked in parallel. A later
    /// [`Self::cooked_for`] joins the work in flight.
    pub fn prefetch_for(
        &self,
        format: CookFormat,
        usage: TextureUse,
    ) -> pulsar_scenedb::Prefetch<CookedTexture> {
        self.residency(format, usage).prefetch()
    }

    /// Whether the cooked colour texture in `format` is in memory right now.
    pub fn is_resident_in(&self, format: CookFormat) -> bool {
        self.residency(format, TextureUse::Color).is_resident()
    }

    /// Ask for every form of this texture that has been used and that the
    /// residency cache since dropped, in the engine's preferred format, to be
    /// brought back on the loader threads now, so the read is under way before
    /// a scene needs to upload it. Forms nobody has asked for are left alone,
    /// and so are forms already in memory.
    pub fn warm(&self) {
        let row = match self.effective_format(preferred_cook_format()) {
            CookFormat::Rgba8 => 0,
            CookFormat::Bc7 => 1,
        };
        for cell in &self.cooked[row] {
            if let Some(residency) = cell.get() {
                if !residency.is_resident() {
                    drop(residency.prefetch());
                }
            }
        }
    }

    /// Whether any cooked form is in memory right now.
    pub fn is_resident(&self) -> bool {
        self.cooked
            .iter()
            .flatten()
            .filter_map(std::sync::OnceLock::get)
            .any(pulsar_scenedb::Residency::is_resident)
    }
}

static PREFERRED_FORMAT: std::sync::atomic::AtomicU8 = std::sync::atomic::AtomicU8::new(0);

/// The format a texture is cooked in as it loads, before any device has asked
/// for one: the engine sets it from its device's features. A device that wants
/// the other format still gets it, cooked on first use.
pub fn set_preferred_cook_format(format: CookFormat) {
    let code = match format {
        CookFormat::Rgba8 => 0,
        CookFormat::Bc7 => 1,
    };
    PREFERRED_FORMAT.store(code, std::sync::atomic::Ordering::Relaxed);
}

pub fn preferred_cook_format() -> CookFormat {
    match PREFERRED_FORMAT.load(std::sync::atomic::Ordering::Relaxed) {
        1 => CookFormat::Bc7,
        _ => CookFormat::Rgba8,
    }
}

/// The largest side, in texels, an image is decoded at; larger images are
/// scaled down (aspect kept) as they load. Project setting
/// `streaming.max_texture_size`; 0 disables the cap.
pub const DEFAULT_MAX_TEXTURE_SIZE: u32 = 2048;

pub fn max_texture_size() -> u32 {
    let configured = engine_state::settings::global_config()
        .get(engine_state::settings::NS_PROJECT, "streaming", "max_texture_size")
        .ok();
    match configured {
        Some(engine_state::settings::ConfigValue::Int(0)) => 0,
        Some(engine_state::settings::ConfigValue::Int(size)) => size.clamp(64, 16_384) as u32,
        _ => DEFAULT_MAX_TEXTURE_SIZE,
    }
}

/// `(width, height)` after fitting the larger side to `cap` (0 = no cap).
pub(crate) fn capped_dimensions(width: u32, height: u32, cap: u32) -> (u32, u32) {
    let longest = width.max(height);
    if cap == 0 || longest <= cap {
        return (width, height);
    }
    let scale = f64::from(cap) / f64::from(longest);
    (
        ((f64::from(width) * scale).round() as u32).max(1),
        ((f64::from(height) * scale).round() as u32).max(1),
    )
}

/// The image a component is bound to, as the world loaded it: the asset it
/// came from and that asset's content version, so the GPU copy is replaced
/// when the file is reloaded. Empty until the world has loaded the image, or
/// when it failed to load. Never authored or saved.
#[derive(Clone, Default)]
pub struct BoundTexture(Option<BoundImage>);

#[derive(Clone)]
struct BoundImage {
    id: AssetId,
    version: u64,
    image: Arc<LoadedTexture>,
}

impl BoundTexture {
    /// The bound image, if it is the asset `expected` (a component whose
    /// path changed shows nothing until the new image is bound).
    pub fn image_for(&self, expected: AssetId) -> Option<&Arc<LoadedTexture>> {
        self.0
            .as_ref()
            .filter(|bound| !expected.is_zero() && bound.id == expected)
            .map(|bound| &bound.image)
    }
}

impl std::fmt::Debug for BoundTexture {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match &self.0 {
            Some(bound) => write!(
                f,
                "BoundTexture({:032x} v{}, {}x{})",
                bound.id.0, bound.version, bound.image.width, bound.image.height
            ),
            None => f.write_str("BoundTexture(none)"),
        }
    }
}

/// Registers the texture asset kind with SceneDB: its loader and payload
/// size, and how sprites and decals take in a loaded image. Idempotent; done
/// on the first [`texture_asset_id`].
pub fn register_texture_assets() {
    static REGISTER: std::sync::Once = std::sync::Once::new();
    REGISTER.call_once(|| {
        pulsar_scenedb::register_loader(TEXTURE_ASSET_KIND, load_texture_asset);
        pulsar_scenedb::register_payload_size(TEXTURE_ASSET_KIND, |payload| {
            // The texels are budgeted by SceneDB's residency cache; the
            // payload itself is a handle.
            payload
                .downcast_ref::<LoadedTexture>()
                .map_or(0, |_| std::mem::size_of::<LoadedTexture>())
        });
        crate::components::register_texture_bindings();
    });
}

/// The source string of the image at `path` (project-relative, or
/// absolute): the absolute path, with `.` and `..` resolved lexically. It
/// does not touch the file system, so the same path always gives the same
/// source whether or not the file exists. `None` for an empty path.
pub fn texture_source(path: &str) -> Option<String> {
    let path = path.trim();
    if path.is_empty() {
        return None;
    }
    let mut full = std::path::PathBuf::from(path);
    if full.is_relative() {
        if let Some(root) = engine_state::get_project_path() {
            full = std::path::Path::new(&root).join(full);
        }
    }
    Some(texture_source_for_file(&full))
}

/// The source string of the image file at `abs` (see [`texture_source`]).
pub fn texture_source_for_file(abs: &std::path::Path) -> String {
    use std::path::Component;
    let mut normal = std::path::PathBuf::new();
    for component in abs.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                if !normal.pop() {
                    normal.push(component);
                }
            }
            other => normal.push(other),
        }
    }
    normal.to_string_lossy().into_owned()
}

/// The asset id of the image `path` names, without declaring it.
/// [`AssetId::ZERO`] for an empty path.
pub fn texture_asset_id_of(path: &str) -> AssetId {
    texture_source(path).map_or(AssetId::ZERO, |source| {
        AssetId::of(TEXTURE_ASSET_KIND, &source)
    })
}

/// The asset id of the image `path` names, declared so the world can load
/// it. [`AssetId::ZERO`] for an empty path.
pub fn texture_asset_id(path: &str) -> AssetId {
    let Some(source) = texture_source(path) else {
        return AssetId::ZERO;
    };
    register_texture_assets();
    let id = AssetId::of(TEXTURE_ASSET_KIND, &source);
    // Collectors run on every write: take the sources' write lock only for
    // a source no one declared yet.
    if id.source().is_none() {
        AssetId::declare(TEXTURE_ASSET_KIND, &source);
    }
    id
}

/// The texture kind's loader: runs on a SceneDB worker thread.
fn load_texture_asset(source: &str) -> Result<AssetPayload, String> {
    Ok(Arc::new(decode_image_file(std::path::Path::new(source))?))
}

/// Load the image file at `path`: read its size, and cook it now, on this
/// loader thread, in the format the engine prefers. A file that cannot be
/// decoded fails the load here, and the derived-data cache is warm before any
/// scene asks for the texture.
pub(crate) fn decode_image_file(path: &std::path::Path) -> Result<LoadedTexture, String> {
    let shown = path.display();
    let cap = max_texture_size();
    let (source_width, source_height) = image::ImageReader::open(path)
        .map_err(|error| format!("could not read '{shown}': {error}"))?
        .with_guessed_format()
        .map_err(|error| format!("could not read '{shown}': {error}"))?
        .into_dimensions()
        .map_err(|error| format!("could not decode '{shown}': {error}"))?;
    if source_width == 0 || source_height == 0 {
        return Err(format!("'{shown}' has empty dimensions"));
    }
    let (width, height) = capped_dimensions(source_width, source_height, cap);
    let texture = LoadedTexture {
        width,
        height,
        source: path.to_path_buf(),
        cap,
        cooked: Default::default(),
    };
    texture.cooked(preferred_cook_format())?;
    Ok(texture)
}

/// The cooked texture for `path` at `cap`: read from SceneDB's derived-data
/// cache when the file and settings are unchanged, cooked and stored when
/// not. A cache entry that fails its own checks is forgotten and rebuilt.
fn cook_from_source(
    path: &std::path::Path,
    cap: u32,
    format: CookFormat,
    usage: TextureUse,
) -> Result<CookedTexture, String> {
    let settings = format!("cap={cap};format={};use={}", format.label(), usage.label());
    let request = pulsar_scenedb::DerivedRequest {
        kind: TEXTURE_ASSET_KIND,
        source: path,
        settings: &settings,
        version: COOK_VERSION,
    };
    let cook_now = || -> Result<CookedTexture, String> {
        let (width, height, rgba) = decode_rgba(path, cap)?;
        cook_as(&rgba, width, height, format, usage)
    };
    let bytes = pulsar_scenedb::derive_cached(&request, || cook_now().map(|cooked| cooked.to_bytes()))?;
    match CookedTexture::from_bytes(&bytes) {
        Ok(cooked) => Ok(cooked),
        Err(error) => {
            tracing::warn!("derived texture for '{}' is unusable ({error}); cooking it again", path.display());
            // Replace the entry through the cache, so the repair is stored and
            // the next load is a read.
            pulsar_scenedb::forget_derived(&request);
            let bytes = pulsar_scenedb::derive_cached(&request, || cook_now().map(|cooked| cooked.to_bytes()))?;
            CookedTexture::from_bytes(&bytes)
        }
    }
}

/// Decode `path` to RGBA8 with its larger side at most `cap` (0 = no cap).
/// The scale-down happens before the RGBA conversion so a 4K RGB source never
/// becomes a 64 MiB RGBA buffer on the way to a 16 MiB one.
fn decode_rgba(path: &std::path::Path, cap: u32) -> Result<(u32, u32, Vec<u8>), String> {
    let shown = path.display();
    let bytes =
        std::fs::read(path).map_err(|error| format!("could not read '{shown}': {error}"))?;
    let mut decoded = image::load_from_memory(&bytes)
        .map_err(|error| format!("could not decode '{shown}': {error}"))?;
    drop(bytes);
    let (width, height) = (decoded.width(), decoded.height());
    if width == 0 || height == 0 {
        return Err(format!("'{shown}' has empty dimensions"));
    }
    let (scaled_width, scaled_height) = capped_dimensions(width, height, cap);
    if (scaled_width, scaled_height) != (width, height) {
        decoded = decoded.resize_exact(
            scaled_width,
            scaled_height,
            image::imageops::FilterType::Triangle,
        );
    }
    let rgba = decoded.to_rgba8();
    Ok((rgba.width(), rgba.height(), rgba.into_raw()))
}

/// Bind the asset `view` into `bound` when it is the image `path` names: the
/// decoded image when it loaded, nothing when it failed (`what` names the
/// component in the warning).
pub(crate) fn bind_texture(bound: &mut BoundTexture, path: &str, view: &AssetView, what: &str) {
    if view.id.is_zero() || view.id != texture_asset_id_of(path) {
        return;
    }
    match view.payload::<LoadedTexture>() {
        Some(image) => {
            // An image the residency cache dropped starts coming back now, ahead
            // of the scene that will upload it.
            image.warm();
            bound.0 = Some(BoundImage {
                id: view.id,
                version: view.version,
                image,
            });
        }
        None => {
            tracing::warn!("{what} texture '{path}' is unavailable ({:?})", view.state);
            bound.0 = None;
        }
    }
}

/// The handle-ledger reference of the image `path` names: exactly one entry,
/// [`AssetId::ZERO`] for none (the ledger diffs a value's handles by
/// position, as it does a derived collector's fields).
pub(crate) fn collect_texture(path: &str, out: &mut Vec<pulsar_scenedb::handle_ledger::HandleId>) {
    out.push(texture_asset_id(path).handle());
}

/// The scene texture store slot of `bound`'s image when it is the one
/// `path` names, uploading it on first use and again after a reload;
/// `u32::MAX` for none (the tint alone).
pub(crate) fn texture_slot(
    bound: &BoundTexture,
    path: &str,
    mirror: &pulsar_scenedb::gpu::GpuMirrorHandle,
    what: &str,
) -> u32 {
    let Some(image) = bound.image_for(texture_asset_id_of(path)) else {
        return u32::MAX;
    };
    let current = bound.0.as_ref().expect("image_for found it");
    match resident_slot(current.id, current.version, image, TextureUse::Color, mirror) {
        Ok(slot) => slot,
        Err(error) => {
            tracing::warn!("{what} texture '{path}': {error}");
            u32::MAX
        }
    }
}

struct ResidentTexture {
    owner: std::sync::Weak<std::sync::RwLock<pulsar_scenedb::gpu::TextureStore>>,
    version: u64,
    /// The detail bias it was uploaded at ([`crate::texture_detail`]).
    bias: u32,
    slot: u32,
}

/// Each texture's slot in each scene texture store, with the content version
/// it holds. A texture is keyed by its asset *and* its use, because an image
/// sampled as a colour and the same image sampled as data are different bytes
/// on the GPU. The store is keyed by its address.
type Residency = std::collections::HashMap<(usize, AssetId, TextureUse), ResidentTexture>;

fn resident() -> &'static std::sync::Mutex<Residency> {
    static RESIDENT: std::sync::OnceLock<std::sync::Mutex<Residency>> = std::sync::OnceLock::new();
    RESIDENT.get_or_init(Default::default)
}

/// A texture uploaded to a device, shared by every scene texture store on
/// that device that shows it.
struct SharedTexture {
    version: u64,
    bias: u32,
    /// What the texture would take in full, for the detail budget.
    native_bytes: u64,
    texture: wgpu::Texture,
    /// The stores holding a clone; the entry goes when none is left.
    users: Vec<std::sync::Weak<std::sync::RwLock<pulsar_scenedb::gpu::TextureStore>>>,
}

/// One GPU upload per (device, texture asset, use): a texture is a device
/// resource, so two scenes on one device (two viewports of a level) show the
/// same image from the same allocation instead of one private copy each.
type SharedTextures = std::collections::HashMap<(usize, AssetId, TextureUse), SharedTexture>;

fn shared_textures() -> &'static std::sync::Mutex<SharedTextures> {
    static SHARED: std::sync::OnceLock<std::sync::Mutex<SharedTextures>> =
        std::sync::OnceLock::new();
    SHARED.get_or_init(Default::default)
}

/// The shared texture for `key` at `version`, with `store` recorded as a user.
fn shared_texture(
    key: (usize, AssetId, TextureUse),
    version: u64,
    bias: u32,
    store: &Arc<std::sync::RwLock<pulsar_scenedb::gpu::TextureStore>>,
) -> Option<wgpu::Texture> {
    let mut shared = shared_textures().lock().ok()?;
    // Forget textures no store holds any more, so a dropped scene's images
    // can be freed.
    shared.retain(|_, entry| {
        entry.users.retain(|user| user.strong_count() != 0);
        !entry.users.is_empty()
    });
    let entry = shared.get_mut(&key)?;
    if entry.version != version || entry.bias != bias {
        return None;
    }
    let me = Arc::downgrade(store);
    if !entry.users.iter().any(|user| user.ptr_eq(&me)) {
        entry.users.push(me);
    }
    Some(entry.texture.clone())
}

/// The slot `image` (content `version` of the texture `id`) holds in
/// `mirror`'s texture store, uploading it when the store does not hold that
/// version yet. A device that already holds that version for another scene
/// shares it: no decode, no second upload, no second GPU allocation.
pub(crate) fn resident_slot(
    id: AssetId,
    version: u64,
    image: &LoadedTexture,
    usage: TextureUse,
    mirror: &pulsar_scenedb::gpu::GpuMirrorHandle,
) -> Result<u32, String> {
    let texture_store = mirror
        .texture_store()
        .ok_or_else(|| "SceneDB has no texture store".to_string())?;
    let key = (Arc::as_ptr(&texture_store) as usize, id, usage);
    let bias = crate::texture_detail::detail_bias();
    {
        let mut resident = resident()
            .lock()
            .map_err(|_| "texture residency lock poisoned".to_string())?;
        resident.retain(|_, entry| entry.owner.strong_count() != 0);
        if let Some(entry) = resident.get(&key) {
            if entry.version == version && entry.bias == bias {
                return Ok(entry.slot);
            }
        }
    }

    // Not in this store yet. Another scene on the same device may have
    // uploaded it; otherwise the texels may have to be decoded again, which
    // takes a while. Either way, no lock is held while finding out.
    let device = mirror.store().device_arc();
    let device_key = (Arc::as_ptr(&device) as usize, id, usage);
    let shared = shared_texture(device_key, version, bias, &texture_store);
    let cooked = match shared {
        Some(_) => None,
        None => Some(image.cooked_for(
            CookFormat::best_for(device.features(), image.width, image.height),
            usage,
        )?),
    };

    let mut resident = resident()
        .lock()
        .map_err(|_| "texture residency lock poisoned".to_string())?;
    if let Some(entry) = resident.get(&key) {
        if entry.version == version && entry.bias == bias {
            return Ok(entry.slot);
        }
    }
    let mut store = texture_store
        .write()
        .map_err(|_| "SceneDB texture store lock poisoned".to_string())?;
    if let Some(old) = resident.remove(&key) {
        let _ = store.unregister(old.slot);
    }
    let (slot, uploaded) = match (shared, cooked) {
        (Some(texture), _) => (
            store
                .register_shared(texture)
                .map_err(|error| format!("could not share a texture: {error:?}"))?,
            None,
        ),
        (None, Some(cooked)) => {
            let native_bytes = cooked.byte_size() as u64;
            let slot = if bias == 0 {
                crate::material_textures::upload_cooked(&mut store, mirror, &cooked)?
            } else {
                crate::material_textures::upload_cooked(
                    &mut store,
                    mirror,
                    &cooked.without_top_mips(bias),
                )?
            };
            (slot, store.texture(slot).cloned().map(|texture| (texture, native_bytes)))
        }
        (None, None) => unreachable!("the cooked texture is fetched whenever nothing is shared"),
    };
    resident.insert(
        key,
        ResidentTexture {
            owner: Arc::downgrade(&texture_store),
            version,
            bias,
            slot,
        },
    );
    drop(store);
    drop(resident);
    if let (Some((texture, native_bytes)), Ok(mut shared)) = (uploaded, shared_textures().lock()) {
        shared.insert(
            device_key,
            SharedTexture {
                version,
                bias,
                native_bytes,
                texture,
                users: vec![Arc::downgrade(&texture_store)],
            },
        );
        // Fit the detail of what scenes show to the video memory budget; the
        // next texture bound (or this one, next time) uses the new bias.
        let native_total = shared.values().map(|entry| entry.native_bytes).sum();
        crate::texture_detail::rebalance(native_total);
    }
    Ok(slot)
}


/// Start cooking, on the asset loader threads, every texture in `images` that
/// `mirror`'s store would have to upload at `version`, so the cooks run in
/// parallel instead of one after another on the thread that is about to need
/// them. A texture the store already holds, or that another scene on the same
/// device has uploaded, is left alone: nothing about it would be used. The
/// `resident_slot` calls that follow join the work in flight.
pub(crate) fn prefetch_missing<'a>(
    images: impl IntoIterator<Item = (AssetId, TextureUse, &'a LoadedTexture)>,
    version: u64,
    mirror: &pulsar_scenedb::gpu::GpuMirrorHandle,
) {
    let Some(texture_store) = mirror.texture_store() else {
        return;
    };
    let device = mirror.store().device_arc();
    let features = device.features();
    for (id, usage, image) in images {
        let key = (Arc::as_ptr(&texture_store) as usize, id, usage);
        let held = resident()
            .lock()
            .ok()
            .and_then(|resident| resident.get(&key).map(|entry| entry.version == version && entry.bias == crate::texture_detail::detail_bias()))
            .unwrap_or(false);
        let shared = shared_textures()
            .lock()
            .ok()
            .and_then(|shared| {
                shared
                    .get(&(Arc::as_ptr(&device) as usize, id, usage))
                    .map(|entry| entry.version == version && entry.bias == crate::texture_detail::detail_bias())
            })
            .unwrap_or(false);
        if !held && !shared {
            drop(image.prefetch_for(
                CookFormat::best_for(features, image.width, image.height),
                usage,
            ));
        }
    }
}
#[cfg(test)]
mod residency_tests {
    use super::*;

    fn write_png(dir: &std::path::Path, name: &str, width: u32, height: u32) -> std::path::PathBuf {
        let path = dir.join(name);
        let mut image = image::RgbImage::new(width, height);
        for (x, y, pixel) in image.enumerate_pixels_mut() {
            *pixel = image::Rgb([(x % 251) as u8, (y % 241) as u8, 7]);
        }
        image.save(&path).unwrap();
        path
    }

    #[test]
    fn dimensions_fit_the_cap_and_keep_their_aspect() {
        assert_eq!(capped_dimensions(4096, 4096, 2048), (2048, 2048));
        assert_eq!(capped_dimensions(8192, 4096, 2048), (2048, 1024));
        assert_eq!(capped_dimensions(1000, 500, 2048), (1000, 500), "small images are untouched");
        assert_eq!(capped_dimensions(4096, 4096, 0), (4096, 4096), "0 disables the cap");
        assert_eq!(capped_dimensions(4096, 1, 64), (64, 1), "never collapses to zero");
    }

    #[test]
    fn decoding_applies_the_cap_before_the_rgba_conversion() {
        let dir = tempfile::tempdir().unwrap();
        let path = write_png(dir.path(), "big.png", 300, 150);
        let (width, height, rgba) = decode_rgba(&path, 100).unwrap();
        assert_eq!((width, height), (100, 50));
        assert_eq!(rgba.len(), 100 * 50 * 4);
        let (width, height, rgba) = decode_rgba(&path, 0).unwrap();
        assert_eq!((width, height, rgba.len()), (300, 150, 300 * 150 * 4));
    }

    #[test]
    fn dropped_texels_are_decoded_again_identically() {
        let dir = tempfile::tempdir().unwrap();
        let first = decode_image_file(&write_png(dir.path(), "a.png", 64, 64)).unwrap();
        let before = first.cooked(CookFormat::Rgba8).unwrap();
        assert!(first.is_resident());

        // A budget below one image makes every newer image evict the older.
        let old_budget = pulsar_scenedb::residency_budget();
        pulsar_scenedb::set_residency_budget(1024);
        let second = decode_image_file(&write_png(dir.path(), "b.png", 64, 64)).unwrap();
        assert!(second.is_resident());
        assert!(!first.is_resident(), "the older image was dropped for room");

        let again = first.cooked(CookFormat::Rgba8).unwrap();
        assert_eq!(*again, *before, "the rebuilt texels match the originals");
        assert!(first.is_resident());
        pulsar_scenedb::set_residency_budget(old_budget);
    }
}

#[cfg(test)]
mod shared_upload_tests {
    use super::*;
    use pulsar_scenedb::gpu::{
        EngineGpuContext, GpuMirrorHandle, SceneGpuConfig, SceneGpuStore, TextureStore,
    };
    use std::sync::RwLock;

    fn context() -> Option<EngineGpuContext> {
        // Creating devices from many tests at once stalls some drivers; take turns.
        static DEVICE_CREATION: std::sync::Mutex<()> = std::sync::Mutex::new(());
        let _device_creation = DEVICE_CREATION.lock().unwrap_or_else(|p| p.into_inner());
        let instance =
            wgpu::Instance::new(helio_core::wgpu_instance_descriptor());
        let adapter = pollster::block_on(instance.request_adapter(&Default::default())).ok()?;
        // Ask for block compression where the adapter has it, as the editor does.
        let required_features = adapter.features() & wgpu::Features::TEXTURE_COMPRESSION_BC;
        let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
            required_features,
            ..Default::default()
        }))
        .ok()?;
        Some(EngineGpuContext::new(Arc::new(device), Arc::new(queue)))
    }

    /// A scene's mirror on `ctx`'s device, with a texture store of its own.
    fn scene(ctx: &EngineGpuContext) -> (GpuMirrorHandle, Arc<RwLock<TextureStore>>) {
        let store = Arc::new(SceneGpuStore::new(
            ctx,
            SceneGpuConfig {
                classes: vec![],
                tombstone_headroom: 0,
                max_cells_metadata: 0,
            },
        ));
        let textures = Arc::new(RwLock::new(TextureStore::new(16)));
        let handle = GpuMirrorHandle::new(store, Arc::clone(ctx.queue()))
            .with_texture_store(Arc::clone(&textures))
            .expect("a texture store on the mirror's device");
        (handle, textures)
    }

    /// The detail bias is process-wide; tests that upload hold this so one
    /// test's bias does not change what another uploads.
    static DETAIL: std::sync::Mutex<()> = std::sync::Mutex::new(());

    #[test]
    fn the_same_image_used_as_colour_and_as_data_is_two_gpu_textures_in_two_formats() {
        let _detail = DETAIL.lock().unwrap_or_else(|p| p.into_inner());
        let Some(ctx) = context() else {
            eprintln!("no GPU adapter; skipping");
            return;
        };
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("both.png");
        image::RgbImage::from_pixel(8, 8, image::Rgb([10, 20, 30]))
            .save(&path)
            .unwrap();
        let image = decode_image_file(&path).unwrap();
        let id = AssetId::of(TEXTURE_ASSET_KIND, "colour-and-data-test");
        let (scene, textures) = scene(&ctx);

        let as_colour = resident_slot(id, 1, &image, TextureUse::Color, &scene).unwrap();
        let as_data = resident_slot(id, 1, &image, TextureUse::Data, &scene).unwrap();
        assert_ne!(as_colour, as_data, "two uses, two slots");

        let store = textures.read().unwrap();
        let (colour, data) = (store.texture(as_colour).unwrap(), store.texture(as_data).unwrap());
        assert_ne!(colour, data, "two uses, two GPU textures");
        assert!(colour.format().is_srgb(), "colour is decoded as sRGB: {:?}", colour.format());
        assert!(!data.format().is_srgb(), "data is not: {:?}", data.format());
        assert_eq!(store.upload_count(), 2);
        drop(store);

        // Each use is found again, not uploaded again.
        assert_eq!(resident_slot(id, 1, &image, TextureUse::Color, &scene).unwrap(), as_colour);
        assert_eq!(resident_slot(id, 1, &image, TextureUse::Data, &scene).unwrap(), as_data);
        assert_eq!(textures.read().unwrap().upload_count(), 2);
    }

    #[test]
    fn scenes_on_one_device_share_one_upload_and_a_new_version_breaks_the_share() {
        let _detail = DETAIL.lock().unwrap_or_else(|p| p.into_inner());
        let Some(ctx) = context() else {
            eprintln!("no GPU adapter; skipping");
            return;
        };
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("shared.png");
        image::RgbImage::from_pixel(8, 8, image::Rgb([10, 20, 30]))
            .save(&path)
            .unwrap();
        let image = decode_image_file(&path).unwrap();
        let id = AssetId::of(TEXTURE_ASSET_KIND, "shared-upload-test");

        let (scene_a, textures_a) = scene(&ctx);
        let (scene_b, textures_b) = scene(&ctx);
        let slot_a = resident_slot(id, 1, &image, TextureUse::Color, &scene_a).unwrap();

        // Drop the CPU copy: if the second scene needed the texels it would
        // have to decode again.
        let old_budget = pulsar_scenedb::residency_budget();
        pulsar_scenedb::set_residency_budget(1);
        let filler = decode_image_file(&path).unwrap();
        assert!(!image.is_resident() && filler.is_resident());

        let slot_b = resident_slot(id, 1, &image, TextureUse::Color, &scene_b).unwrap();
        assert!(
            !image.is_resident(),
            "the second scene shared the upload and never needed the texels"
        );
        let (a, b) = (textures_a.read().unwrap(), textures_b.read().unwrap());
        assert_eq!(a.texture(slot_a), b.texture(slot_b), "one GPU texture");
        // What reached the GPU is the cooked form: BC7 where the device can
        // sample it, uncompressed otherwise, and either way the full chain.
        let expected = if ctx.device().features().contains(wgpu::Features::TEXTURE_COMPRESSION_BC) {
            wgpu::TextureFormat::Bc7RgbaUnormSrgb
        } else {
            wgpu::TextureFormat::Rgba8UnormSrgb
        };
        let uploaded = a.texture(slot_a).unwrap();
        assert_eq!(uploaded.format(), expected);
        assert_eq!(uploaded.mip_level_count(), 4, "an 8x8 image has four levels");
        assert_eq!(a.upload_count(), 1);
        assert_eq!(b.upload_count(), 0, "the second store uploaded nothing");
        drop((a, b));

        // A new content version is a new upload, not a stale share.
        pulsar_scenedb::set_residency_budget(old_budget);
        let slot_b2 = resident_slot(id, 2, &image, TextureUse::Color, &scene_b).unwrap();
        let b = textures_b.read().unwrap();
        assert_eq!(b.upload_count(), 1, "version 2 uploaded afresh");
        assert_ne!(
            textures_a.read().unwrap().texture(slot_a),
            b.texture(slot_b2),
            "the scenes no longer show the same texture"
        );
    }

    #[test]
    fn a_detail_bias_uploads_fewer_levels_and_a_changed_bias_uploads_again() {
        let _detail = DETAIL.lock().unwrap_or_else(|p| p.into_inner());
        let Some(ctx) = context() else {
            eprintln!("no GPU adapter; skipping");
            return;
        };
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("detail.png");
        image::RgbImage::from_pixel(16, 16, image::Rgb([10, 20, 30]))
            .save(&path)
            .unwrap();
        let image = decode_image_file(&path).unwrap();
        let id = AssetId::of(TEXTURE_ASSET_KIND, "detail-bias-test");
        let (scene, textures) = scene(&ctx);

        crate::texture_detail::set_detail_bias(0);
        let full = resident_slot(id, 1, &image, TextureUse::Color, &scene).unwrap();
        let (width, levels) = {
            let store = textures.read().unwrap();
            let texture = store.texture(full).unwrap();
            (texture.width(), texture.mip_level_count())
        };
        assert_eq!((width, levels), (16, 5));

        // The same bias finds the texture where it is.
        assert_eq!(resident_slot(id, 1, &image, TextureUse::Color, &scene).unwrap(), full);
        assert_eq!(textures.read().unwrap().upload_count(), 1);

        // Over budget: the next bind uploads it again with the top level off.
        crate::texture_detail::set_detail_bias(1);
        let smaller = resident_slot(id, 1, &image, TextureUse::Color, &scene).unwrap();
        {
            let store = textures.read().unwrap();
            let texture = store.texture(smaller).unwrap();
            assert_eq!((texture.width(), texture.mip_level_count()), (8, 4));
            assert_eq!(store.upload_count(), 2);
        }

        // Room again: the detail comes back.
        crate::texture_detail::set_detail_bias(0);
        let restored = resident_slot(id, 1, &image, TextureUse::Color, &scene).unwrap();
        let store = textures.read().unwrap();
        assert_eq!(store.texture(restored).unwrap().width(), 16);
        assert_eq!(store.upload_count(), 3);
    }
}
