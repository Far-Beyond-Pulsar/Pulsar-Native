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

use pulsar_scenedb::gpu::shared_textures::{
    self, SharedTextureError, TextureImage, TextureRequest, TextureUpload,
};
use pulsar_scenedb::{AssetId, AssetView, TierLoad};
use std::sync::Arc;

use crate::cooked_texture::{cook_as, CookFormat, CookedTexture, TextureUse, COOK_VERSION};
use crate::texture_options::TextureOptions;

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
    /// The cooked form the loader made, held until the frame thread has
    /// uploaded it ([`Self::take_pinned`]): the residency cache is a budgeted
    /// LRU and a bind that waits behind many others would find it evicted and
    /// read it back from disk on the frame thread.
    pinned: std::sync::Mutex<Vec<(CookFormat, TextureUse, Arc<CookedTexture>)>>,
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

    /// Hold `cooked` (made on a loader thread) for the upload.
    fn pin(&self, format: CookFormat, usage: TextureUse, cooked: Arc<CookedTexture>) {
        let format = self.effective_format(format);
        if let Ok(mut pinned) = self.pinned.lock() {
            pinned.retain(|(f, u, _)| (*f, *u) != (format, usage));
            pinned.push((format, usage, cooked));
        }
    }

    /// Make the form `usage` uploads in the engine's preferred format, and
    /// hold it for the upload: for whoever is on a loader thread and knows the
    /// frame thread will bind it. Nothing is done when it is already held.
    pub fn prepare(&self, usage: TextureUse) -> Result<(), String> {
        let format = preferred_cook_format();
        if self.pinned_for(format, usage).is_none() {
            let cooked = self.cooked_for(format, usage)?;
            self.pin(format, usage, cooked);
        }
        Ok(())
    }

    /// The pinned cooked form when it is the one asked for (`format` as
    /// [`Self::cooked_for`] takes it), else `None`. Leaves it pinned: see
    /// [`Self::unpin`].
    pub fn pinned_for(&self, format: CookFormat, usage: TextureUse) -> Option<Arc<CookedTexture>> {
        let format = self.effective_format(format);
        let pinned = self.pinned.lock().ok()?;
        pinned
            .iter()
            .find(|(f, u, _)| *f == format && *u == usage)
            .map(|(_, _, cooked)| Arc::clone(cooked))
    }

    /// Let go of what is held for `usage` (the GPU holds the texture now); the
    /// residency cache and the derived-data cache still serve a rebuild.
    pub fn unpin(&self, usage: TextureUse) {
        if let Ok(mut pinned) = self.pinned.lock() {
            pinned.retain(|(_, u, _)| *u != usage);
        }
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

// Static atomic: read on every texture cook, from loader threads that cannot reach
// an `EngineContext` (the standalone game sets none before it loads textures).
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
        .get(
            engine_state::settings::NS_PROJECT,
            "streaming",
            "max_texture_size",
        )
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

/// How a plugin's component that shows an image (a sprite, a decal) takes
/// in a loaded texture asset: `register` calls `pulsar_scenedb::register_binding`
/// for the component with [`TEXTURE_ASSET_KIND`]. Submitted by the plugin's
/// component crate; run by [`register_texture_assets`].
pub struct TextureBindingRegistration {
    /// The package that submitted it (`pulsar_plugins::origin!()`): skipped
    /// while that package's plugin is disabled.
    pub origin: &'static str,
    /// Stable id of the registration within its plugin (`"sprites.textures"`): what
    /// the plugin catalog lists it by.
    pub name: &'static str,
    pub register: fn(),
}

impl pulsar_plugins::Contribution for TextureBindingRegistration {
    const CATEGORY: &'static str = "asset.texture_binding";

    fn origin(&self) -> &'static str {
        self.origin
    }

    fn id(&self) -> String {
        self.name.to_string()
    }
}

pulsar_plugins::registration!(TextureBindingRegistration);

/// Registers the texture asset kind with SceneDB: its loader and payload
/// size, and how the registered components take in a loaded image
/// ([`TextureBindingRegistration`]). Idempotent, and incremental: each
/// binding runs once, also one a plugin library brings after an earlier call
/// (`pulsar_plugins::Handled`). Done on each [`texture_asset_id`].
pub fn register_texture_assets() {
    static LOADER: std::sync::Once = std::sync::Once::new();
    static HANDLED: pulsar_plugins::Handled = pulsar_plugins::Handled::new();
    // The loader and size are this crate's own and need no registration
    // from anywhere else: once per copy is right (SceneDB replaces).
    LOADER.call_once(|| {
        pulsar_scenedb::register_tiered_loader(TEXTURE_ASSET_KIND, load_texture_tier);
        pulsar_scenedb::register_payload_size(TEXTURE_ASSET_KIND, |payload| {
            // The texels are budgeted by SceneDB's residency cache; the
            // payload itself is a handle.
            payload
                .downcast_ref::<LoadedTexture>()
                .map_or(0, |_| std::mem::size_of::<LoadedTexture>())
        });
    });
    for registration in HANDLED.take_new::<TextureBindingRegistration>() {
        (registration.register)();
    }
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

// A changed image reloads its texture asset.
inventory::submit! {
    crate::asset_component::AssetReloadRegistration {
        origin: pulsar_plugins::engine_origin!(),
        name: "textures",
        kind: ui_types_common::AssetKind::Texture,
        sources_for_file: |path| vec![texture_source_for_file(path)],
    }
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

/// The largest side, in texels, of the first tier of a texture that loads
/// coarse first: project setting `streaming.coarse_texture_size`. An image no
/// larger than this loads in one step; `0` turns progressive loading off.
pub const DEFAULT_COARSE_TEXTURE_SIZE: u32 = 128;

pub fn coarse_texture_size() -> u32 {
    match engine_state::settings::global_config().get(
        engine_state::settings::NS_PROJECT,
        "streaming",
        "coarse_texture_size",
    ) {
        Ok(engine_state::settings::ConfigValue::Int(size)) => size.clamp(0, 4096) as u32,
        _ => DEFAULT_COARSE_TEXTURE_SIZE,
    }
}

/// The texture kind's loader, a tiered one: tier 0 is a small cooked image
/// (cheap to decode, upload and hold, so the level is textured within moments
/// of its first load), tier 1 the full-size one the world then upgrades to,
/// nearest first. An image already within the coarse size, or with progressive
/// loading off, is complete at tier 0.
fn load_texture_tier(source: &str, tier: u8) -> Result<TierLoad, String> {
    let path = std::path::Path::new(source);
    let full_cap = max_texture_size();
    let coarse = coarse_texture_size();
    if tier == 0 && coarse > 0 {
        let (width, height) = image_dimensions(path)?;
        let (full_w, full_h) = capped_dimensions(width, height, full_cap);
        if full_w.max(full_h) > coarse {
            let image = decode_for_wanted_uses(source, path, coarse)?;
            return Ok(TierLoad {
                payload: Arc::new(image),
                last: false,
            });
        }
    }
    Ok(TierLoad {
        payload: Arc::new(decode_for_wanted_uses(source, path, full_cap)?),
        last: true,
    })
}

/// The uses each source has been asked for, so the loader prepares those and
/// no others: a map that is only a normal map is not also encoded as a
/// colour. A use missing here costs nothing but time: whoever binds it
/// prepares it ([`LoadedTexture::prepare`]).
fn wanted_uses() -> &'static std::sync::Mutex<std::collections::HashMap<String, Vec<TextureUse>>> {
    // Static: noted from loader threads and writers with no context, and read
    // by loaders.
    static WANTED: std::sync::OnceLock<
        std::sync::Mutex<std::collections::HashMap<String, Vec<TextureUse>>>,
    > = std::sync::OnceLock::new();
    WANTED.get_or_init(Default::default)
}

/// Note that the texture at `source` will be sampled for `usage`.
fn want_use(source: &str, usage: TextureUse) {
    if let Ok(mut wanted) = wanted_uses().lock() {
        match wanted.get_mut(source) {
            Some(uses) if uses.contains(&usage) => {}
            Some(uses) => uses.push(usage),
            None => {
                wanted.insert(source.to_string(), vec![usage]);
            }
        }
    }
}

/// Decode the image at `path` capped at `cap` and prepare the uses `source`
/// is wanted for; a colour when nothing has said.
fn decode_for_wanted_uses(
    source: &str,
    path: &std::path::Path,
    cap: u32,
) -> Result<LoadedTexture, String> {
    let uses = wanted_uses()
        .lock()
        .ok()
        .and_then(|wanted| wanted.get(source).cloned())
        .unwrap_or_else(|| vec![TextureUse::Color]);
    let image = decode_image_file_at(path, cap, uses[0])?;
    for &usage in &uses[1..] {
        image.prepare(usage)?;
    }
    Ok(image)
}

/// The texture asset of the file at `source` (a [`texture_source`]), declared
/// so a world can load it, for an asset that needs it sampled as `usage`.
pub fn needed_texture(source: &str, usage: TextureUse) -> AssetId {
    register_texture_assets();
    want_use(source, usage);
    AssetId::declare(TEXTURE_ASSET_KIND, source)
}

/// Load the image file at `path`: read its size, and cook it now, on this
/// loader thread, in the format the engine prefers. A file that cannot be
/// decoded fails the load here, and the derived-data cache is warm before any
/// scene asks for the texture.
pub fn decode_image_file(path: &std::path::Path) -> Result<LoadedTexture, String> {
    decode_image_file_at(path, max_texture_size(), TextureUse::Color)
}

/// [`decode_image_file`] cooked for `usage`: a roughness or normal map is
/// cooked as data, not as colour, and is cooked that way here, on the loader
/// thread, so the draw that binds it finds the cooked form ready instead of
/// cooking it on the frame thread.
pub fn decode_image_file_for(
    path: &std::path::Path,
    usage: TextureUse,
) -> Result<LoadedTexture, String> {
    decode_image_file_at(path, max_texture_size(), usage)
}

/// The source size of the image at `path`, read from its header.
fn image_dimensions(path: &std::path::Path) -> Result<(u32, u32), String> {
    let shown = path.display();
    let (width, height) = image::ImageReader::open(path)
        .map_err(|error| format!("could not read '{shown}': {error}"))?
        .with_guessed_format()
        .map_err(|error| format!("could not read '{shown}': {error}"))?
        .into_dimensions()
        .map_err(|error| format!("could not decode '{shown}': {error}"))?;
    if width == 0 || height == 0 {
        return Err(format!("'{shown}' has empty dimensions"));
    }
    Ok((width, height))
}

/// [`decode_image_file`] with the larger side capped at `cap` (0 = no cap).
fn decode_image_file_at(
    path: &std::path::Path,
    cap: u32,
    usage: TextureUse,
) -> Result<LoadedTexture, String> {
    let (source_width, source_height) = image_dimensions(path)?;
    let (width, height) = capped_dimensions(source_width, source_height, cap);
    let texture = LoadedTexture {
        width,
        height,
        source: path.to_path_buf(),
        cap,
        cooked: Default::default(),
        pinned: Default::default(),
    };
    texture.prepare(usage)?;
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
    profiling::profile_scope!("Texture::cook_from_source");
    let settings = format!("cap={cap};format={};use={}", format.label(), usage.label());
    let request = pulsar_scenedb::DerivedRequest {
        kind: TEXTURE_ASSET_KIND,
        source: path,
        settings: &settings,
        version: COOK_VERSION,
    };
    let cook_now = || -> Result<CookedTexture, String> {
        let (width, height, rgba) = {
            profiling::profile_scope!("Texture::decode");
            decode_rgba(path, cap)?
        };
        profiling::profile_scope!("Texture::cook (mips and compression)");
        cook_as(&rgba, width, height, format, usage)
    };
    let bytes =
        pulsar_scenedb::derive_cached(&request, || cook_now().map(|cooked| cooked.to_bytes()))?;
    match CookedTexture::from_bytes(&bytes) {
        Ok(cooked) => Ok(cooked),
        Err(error) => {
            tracing::warn!(
                "derived texture for '{}' is unusable ({error}); cooking it again",
                path.display()
            );
            // Replace the entry through the cache, so the repair is stored and
            // the next load is a read.
            pulsar_scenedb::forget_derived(&request);
            let bytes = pulsar_scenedb::derive_cached(&request, || {
                cook_now().map(|cooked| cooked.to_bytes())
            })?;
            CookedTexture::from_bytes(&bytes)
        }
    }
}

/// Decode `path` to RGBA8 with its larger side at most `cap` (0 = no cap).
/// The scale-down happens before the RGBA conversion so a 4K RGB source never
/// becomes a 64 MiB RGBA buffer on the way to a 16 MiB one.
fn decode_rgba(path: &std::path::Path, cap: u32) -> Result<(u32, u32, Vec<u8>), String> {
    let shown = path.display();
    let bytes = engine_fs::virtual_fs::read_file(path)
        .map_err(|error| format!("could not read '{shown}': {error}"))?;
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
pub fn bind_texture(bound: &mut BoundTexture, path: &str, view: &AssetView, what: &str) {
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
pub fn collect_texture(path: &str, out: &mut Vec<pulsar_scenedb::handle_ledger::HandleId>) {
    // A component shows its image as a colour.
    if let Some(source) = texture_source(path) {
        want_use(&source, TextureUse::Color);
    }
    out.push(texture_asset_id(path).handle());
}

/// The scene texture store slot of `bound`'s image when it is the one
/// `path` names, uploading it on first use and again after a reload;
/// `u32::MAX` for none (the tint alone). The GPU copy is made as `options`,
/// the component's own choice, asks.
pub fn texture_slot(
    bound: &BoundTexture,
    path: &str,
    options: TextureOptions,
    mirror: &pulsar_scenedb::gpu::GpuMirrorHandle,
    what: &str,
) -> u32 {
    let Some(image) = bound.image_for(texture_asset_id_of(path)) else {
        return u32::MAX;
    };
    let current = bound.0.as_ref().expect("image_for found it");
    match resident_slot_with(
        current.id,
        current.version,
        image,
        TextureUse::Color,
        options,
        mirror,
        TextureUpload::Whole,
    ) {
        Ok(slot) => slot,
        Err(error) => {
            tracing::warn!("{what} texture '{path}': {error}");
            u32::MAX
        }
    }
}

/// The slot `image` (content `version` of the texture `id`) holds in
/// `mirror`'s texture store, uploading it when the store does not hold that
/// version yet. A device that already holds that version for another scene
/// shares it: no decode, no second upload, no second GPU allocation.
///
/// Where textures live and who shares them is SceneDB's
/// ([`shared_textures`]); what their texels are is this module's.
pub fn resident_slot(
    id: AssetId,
    version: u64,
    image: &LoadedTexture,
    usage: TextureUse,
    mirror: &pulsar_scenedb::gpu::GpuMirrorHandle,
) -> Result<u32, String> {
    let options = TextureOptions::default();
    resident_slot_with(
        id,
        version,
        image,
        usage,
        options,
        mirror,
        TextureUpload::Whole,
    )
}

/// [`resident_slot`] for a frame thread: the GPU copy is staged and written a
/// few block rows a frame under the world's upload budget
/// (`World::advance_texture_uploads`), not in this call. The slot is returned at
/// once; it shows nothing (a material's fallback) until its first upload is
/// whole, and keeps showing its earlier quality (the coarse tier) until the
/// finer texture is whole, which then replaces it in one step.
pub fn resident_slot_staged(
    id: AssetId,
    version: u64,
    image: &LoadedTexture,
    usage: TextureUse,
    mirror: &pulsar_scenedb::gpu::GpuMirrorHandle,
) -> Result<u32, String> {
    let options = TextureOptions::default();
    resident_slot_with(
        id,
        version,
        image,
        usage,
        options,
        mirror,
        TextureUpload::Staged,
    )
}

/// What SceneDB keys the GPU copy by: the image, its use (an image sampled
/// as a colour and as data are different texels) and the choice it was asked
/// with, at the content version and the detail bias ([`crate::texture_detail`])
/// it is uploaded with.
fn texture_request(
    id: AssetId,
    version: u64,
    usage: TextureUse,
    options: TextureOptions,
    bias: u32,
) -> TextureRequest {
    let usage = match usage {
        TextureUse::Color => 0,
        TextureUse::Data => 1,
        TextureUse::Normal => 2,
    };
    TextureRequest {
        asset: id,
        variant: usage | (options.variant_bits() << 2),
        revision: (version << 8) | u64::from(bias.min(0xff)),
    }
}

fn resident_slot_with(
    id: AssetId,
    version: u64,
    image: &LoadedTexture,
    usage: TextureUse,
    options: TextureOptions,
    mirror: &pulsar_scenedb::gpu::GpuMirrorHandle,
    upload: TextureUpload,
) -> Result<u32, String> {
    profiling::profile_scope!("Texture::resident_slot");
    let bias = crate::texture_detail::detail_bias();
    let mut made = false;
    let slot = shared_textures::acquire(
        mirror,
        texture_request(id, version, usage, options, bias),
        upload,
        |device| {
            made = true;
            // A texture whose cook is not resident or in the derived cache is
            // cooked here, on the caller's thread: this scope is that cost.
            profiling::profile_scope!("Texture::cooked_for (cook, read or join)");
            let format = if options.compressed {
                CookFormat::best_for(device.features(), image.width, image.height)
            } else {
                CookFormat::Rgba8
            };
            let cooked = match image.pinned_for(format, usage) {
                Some(cooked) => cooked,
                None => image.cooked_for(format, usage)?,
            };
            let native_bytes = cooked.byte_size() as u64;
            let drop = bias + options.levels_to_drop(cooked.width, cooked.height);
            let cooked = if drop == 0 {
                cooked
            } else {
                Arc::new(cooked.without_top_mips(drop))
            };
            Ok(TextureImage {
                descriptor: cooked_descriptor(&cooked),
                texels: cooked,
                native_bytes,
                // Fit the detail of what scenes show to the video memory
                // budget; the next texture bound (or this one, next time)
                // uses the new bias.
                published: Some(Box::new(|native_total| {
                    crate::texture_detail::rebalance(native_total);
                })),
            })
        },
    )
    .map_err(|error| {
        let what = match (made, upload) {
            (false, _) => "could not share a texture",
            (true, TextureUpload::Whole) => "could not register texture in SceneDB",
            (true, TextureUpload::Staged) => "could not stage a texture in SceneDB",
        };
        texture_error(error, what)
    })?;
    image.unpin(usage);
    Ok(slot)
}

/// What a texture that got no slot means to whoever shows it. A full store
/// is the scene's texture table at its capacity (the device's,
/// [`helio_core::texture_table_capacity`], in the editor): the texture gets
/// no slot and the caller draws without it (a material its fallback
/// surface), so the message says so plainly.
fn texture_error(error: SharedTextureError, what: &str) -> String {
    use pulsar_scenedb::gpu::TextureError;
    match error {
        SharedTextureError::NoTextureStore => "SceneDB has no texture store".to_string(),
        SharedTextureError::Poisoned => "SceneDB texture lock poisoned".to_string(),
        SharedTextureError::Produce(error) => error,
        SharedTextureError::Store {
            error: TextureError::SlotsExhausted,
            slots,
        } => format!("{what}: the scene's texture table is full (all {slots} slots are in use)"),
        SharedTextureError::Store { error, .. } => format!("{what}: {error:?}"),
    }
}

/// The GPU texture a cooked texture is uploaded as.
fn cooked_descriptor(cooked: &CookedTexture) -> wgpu::TextureDescriptor<'static> {
    wgpu::TextureDescriptor {
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
    }
}

/// Start cooking, on the asset loader threads, every texture in `images` (with
/// its content version) that `mirror`'s store would have to upload, so the cooks run in
/// parallel instead of one after another on the thread that is about to need
/// them. A texture the store already holds, or that another scene on the same
/// device has uploaded, is left alone: nothing about it would be used. The
/// `resident_slot` calls that follow join the work in flight.
pub fn prefetch_missing<'a>(
    images: impl IntoIterator<Item = (AssetId, u64, TextureUse, &'a LoadedTexture)>,
    mirror: &pulsar_scenedb::gpu::GpuMirrorHandle,
) {
    profiling::profile_scope!("Texture::prefetch_missing");
    if mirror.texture_store().is_none() {
        return;
    }
    let features = mirror.store().device_arc().features();
    let bias = crate::texture_detail::detail_bias();
    for (id, version, usage, image) in images {
        let request = texture_request(id, version, usage, TextureOptions::default(), bias);
        if !shared_textures::holds(mirror, request) {
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
    fn a_large_image_loads_coarse_first_and_a_small_one_in_one_step() {
        let dir = tempfile::tempdir().unwrap();
        let big = write_png(dir.path(), "big.png", 512, 256);
        let small = write_png(dir.path(), "small.png", 64, 64);
        let (big, small) = (big.to_str().unwrap(), small.to_str().unwrap());
        let coarse = coarse_texture_size();
        assert!(coarse > 0 && coarse < 512, "the default is progressive");

        let first = load_texture_tier(big, 0).unwrap();
        assert!(!first.last, "a finer tier follows");
        let first = first.payload.downcast::<LoadedTexture>().unwrap();
        assert_eq!(first.width.max(first.height), coarse);
        assert_eq!(
            (first.width, first.height),
            (coarse, coarse / 2),
            "aspect kept"
        );
        // The coarse tier is cooked with its full mip chain, at its own size.
        assert_eq!(first.cooked(CookFormat::Rgba8).unwrap().width, coarse);

        let second = load_texture_tier(big, 1).unwrap();
        assert!(second.last);
        let second = second.payload.downcast::<LoadedTexture>().unwrap();
        assert_eq!((second.width, second.height), (512, 256));

        let only = load_texture_tier(small, 0).unwrap();
        assert!(only.last, "within the coarse size: nothing to upgrade");
        let only = only.payload.downcast::<LoadedTexture>().unwrap();
        assert_eq!((only.width, only.height), (64, 64));
    }
    /// A loader thread cooks the texture for the use that will bind it and
    /// pins the result, so the frame thread that uploads it neither cooks nor
    /// reads it back from disk; unpinning lets it go.
    #[test]
    fn a_loaded_texture_is_pinned_for_its_use_until_it_is_uploaded() {
        let dir = tempfile::tempdir().unwrap();
        let path = write_png(dir.path(), "normal.png", 64, 64);
        let image = decode_image_file_for(&path, TextureUse::Normal).unwrap();
        let format = preferred_cook_format();
        assert!(image.pinned_for(format, TextureUse::Normal).is_some());
        assert!(
            image.pinned_for(format, TextureUse::Color).is_none(),
            "another use is a different cook"
        );
        image.unpin(TextureUse::Normal);
        assert!(image.pinned_for(format, TextureUse::Normal).is_none());
        // Unpinned, the form is still served (the residency cache or a rebuild).
        assert!(image.cooked_for(format, TextureUse::Normal).is_ok());
    }

    #[test]
    fn dimensions_fit_the_cap_and_keep_their_aspect() {
        assert_eq!(capped_dimensions(4096, 4096, 2048), (2048, 2048));
        assert_eq!(capped_dimensions(8192, 4096, 2048), (2048, 1024));
        assert_eq!(
            capped_dimensions(1000, 500, 2048),
            (1000, 500),
            "small images are untouched"
        );
        assert_eq!(
            capped_dimensions(4096, 4096, 0),
            (4096, 4096),
            "0 disables the cap"
        );
        assert_eq!(
            capped_dimensions(4096, 1, 64),
            (64, 1),
            "never collapses to zero"
        );
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
        let instance = wgpu::Instance::new(helio_core::wgpu_instance_descriptor());
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
        scene_with(ctx, 16)
    }

    /// A scene's mirror whose texture store holds `slots` textures.
    fn scene_with(
        ctx: &EngineGpuContext,
        slots: u32,
    ) -> (GpuMirrorHandle, Arc<RwLock<TextureStore>>) {
        let store = Arc::new(SceneGpuStore::new(
            ctx,
            SceneGpuConfig {
                classes: vec![],
                tombstone_headroom: 0,
                max_cells_metadata: 0,
            },
        ));
        let textures = Arc::new(RwLock::new(TextureStore::new(slots)));
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
        let (colour, data) = (
            store.texture(as_colour).unwrap(),
            store.texture(as_data).unwrap(),
        );
        assert_ne!(colour, data, "two uses, two GPU textures");
        assert!(
            colour.format().is_srgb(),
            "colour is decoded as sRGB: {:?}",
            colour.format()
        );
        assert!(!data.format().is_srgb(), "data is not: {:?}", data.format());
        assert_eq!(store.upload_count(), 2);
        drop(store);

        // Each use is found again, not uploaded again.
        assert_eq!(
            resident_slot(id, 1, &image, TextureUse::Color, &scene).unwrap(),
            as_colour
        );
        assert_eq!(
            resident_slot(id, 1, &image, TextureUse::Data, &scene).unwrap(),
            as_data
        );
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
        let expected = if ctx
            .device()
            .features()
            .contains(wgpu::Features::TEXTURE_COMPRESSION_BC)
        {
            wgpu::TextureFormat::Bc7RgbaUnormSrgb
        } else {
            wgpu::TextureFormat::Rgba8UnormSrgb
        };
        let uploaded = a.texture(slot_a).unwrap();
        assert_eq!(uploaded.format(), expected);
        assert_eq!(
            uploaded.mip_level_count(),
            4,
            "an 8x8 image has four levels"
        );
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
        assert_eq!(
            resident_slot(id, 1, &image, TextureUse::Color, &scene).unwrap(),
            full
        );
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

    #[test]
    fn a_full_texture_table_is_an_error_that_says_so() {
        let _detail = DETAIL.lock().unwrap_or_else(|p| p.into_inner());
        let Some(ctx) = context() else {
            eprintln!("no GPU adapter; skipping");
            return;
        };
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("full.png");
        image::RgbImage::from_pixel(4, 4, image::Rgb([1, 2, 3]))
            .save(&path)
            .unwrap();
        let image = decode_image_file(&path).unwrap();
        let (scene, textures) = scene(&ctx);
        for index in 0..16 {
            let id = AssetId::of(TEXTURE_ASSET_KIND, &format!("full-table-{index}"));
            resident_slot(id, 1, &image, TextureUse::Color, &scene).unwrap();
        }

        // A seventeenth texture has no slot: uploaded or shared, the error
        // names the full table rather than a store enum.
        let id = AssetId::of(TEXTURE_ASSET_KIND, "full-table-overflow");
        let error = resident_slot(id, 1, &image, TextureUse::Color, &scene).unwrap_err();
        assert!(
            error.contains("the scene's texture table is full (all 16 slots are in use)"),
            "{error}"
        );
        let (other_scene, _) = scene_with(&ctx, 0);
        let shared = AssetId::of(TEXTURE_ASSET_KIND, "full-table-0");
        let error = resident_slot(shared, 1, &image, TextureUse::Color, &other_scene).unwrap_err();
        assert!(
            error.contains("could not share a texture: the scene's texture table is full"),
            "{error}"
        );
        assert_eq!(textures.read().unwrap().slot_count(), 16);
    }
}
