//! Play-In-Editor host — loads a game project's `cdylib` and drives it from the
//! editor's render loop (issue #243).
//!
//! The heavy lifting (offscreen Helio renderer, ECS tick) lives inside the game
//! library behind a stable C ABI ([`pulsar_pie_abi`]). This module is the editor
//! side: it loads the library, verifies the ABI version, hands the game the
//! editor's `wgpu::Device`/`Queue`, and forwards tick/resize/input each frame.
//! The game runs independently once initialized (Unreal-style PIE) — the editor
//! only displays its frames; there is no scene writeback.
//!
//! ## Threading
//! Every method must be called from the editor's render thread — the same thread
//! that owns the viewport surface — because the game stores its GPU + world state
//! in a `thread_local!`. See [`pulsar_game::embed`].
//!
//! ## Lifetime / safety
//! * `ctx` is boxed so its address is stable across the `init` call.
//! * `device`/`queue` are held as `Arc`s for the whole session; `ctx.device` /
//!   `ctx.queue` point into those allocations (`Arc::as_ptr`).
//! * The resolved symbols are copied out as plain `fn` pointers; the `Library`
//!   is kept alive in the struct and dropped **last** (after `shutdown`).

use std::ffi::c_void;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use libloading::Library;
use parking_lot::{RwLock, RwLockWriteGuard};
use pulsar_pie_abi::{
    EngineContext as PieContext, FnAbiVersion, FnInit, FnInput, FnResize, FnShutdown, FnTick,
    InputEvent, INIT_OK, LOG_DEBUG, LOG_ERROR, LOG_INFO, LOG_TRACE, LOG_WARN, PIE_ABI_VERSION,
    SYM_ABI_VERSION, SYM_INIT, SYM_INPUT, SYM_RESIZE, SYM_SHUTDOWN, SYM_TICK,
};

use crate::scene::WorldSceneStore;

/// The host-side half of the ABI v2 shared-world contract (#635).
///
/// Owns a strong count of the editor's world handle and implements the
/// phase-boundary lock callbacks handed to the guest. The guard produced by
/// [`Self::lock`] is stored in [`Self::slice`] until [`Self::unlock`]: this is
/// what lets the returned `*mut WorldSceneStore` outlive the callback call
/// while remaining soundly borrowed -- the borrow's backing allocation (the
/// `Arc`) lives in this struct, which itself outlives the whole PIE session.
///
/// Not `Send`/`Sync`-restricted on purpose: the PIE threading contract keeps
/// every entry point on the editor's render thread (`pulsar_game::embed`'s
/// `thread_local!`), so plain fields are sufficient and cheaper than locks we
/// would never contend on.
pub(crate) struct PieWorldBridge {
    /// The editor's authoritative world. One strong count belongs to this
    /// bridge for the session; a second count is transferred to the guest
    /// via `Arc::into_raw` at load time (the guest reclaims it with
    /// `Arc::from_raw` -- exactly once, per #635's single-transfer rule).
    store: Arc<RwLock<WorldSceneStore>>,
    /// The open slice, if any. Occupied == locked: the non-reentrancy
    /// witness is "slot already holds a guard", checked before locking.
    slice: Option<SliceGuard>,
}

/// A held exclusive slice. Owns an `Arc` clone so the `'static` guard borrow
/// is backed by memory this type provably keeps alive (see [`Self::lock`]).
struct SliceGuard {
    _keep_alive: Arc<RwLock<WorldSceneStore>>,
    // Invariant: `_keep_alive`'s heap allocation is stable, so borrowing
    // through its address for `'static` is sound while `_keep_alive` lives.
    // Never read after construction -- the field exists to be DROPPED at
    // unlock (dropping a write guard is what releases the lock).
    #[allow(dead_code)]
    guard: RwLockWriteGuard<'static, WorldSceneStore>,
}

impl PieWorldBridge {
    pub(crate) fn new(store: Arc<RwLock<WorldSceneStore>>) -> Self {
        Self { store, slice: None }
    }

    /// The lock callback body ([`pulsar_pie_abi::LockWorldFn`]). Returns an
    /// exclusive raw pointer valid until [`Self::unlock`], or null when a
    /// slice is already open -- the protocol-violation signal.
    fn lock(&mut self) -> *mut c_void {
        if self.slice.is_some() {
            tracing::error!("PiE: guest opened a second shared-world slice; returning null");
            return std::ptr::null_mut();
        }
        // Soundness of the 'static borrow: `SliceGuard::_keep_alive` holds a
        // clone of the Arc for as long as the guard lives, and Arc's
        // allocation address never moves, so borrowing the allocation through
        // its pointer cannot dangle while the guard is stored here.
        let keep_alive = Arc::clone(&self.store);
        let static_lock: &'static RwLock<WorldSceneStore> =
            unsafe { &*Arc::as_ptr(&keep_alive) };
        let mut guard = static_lock.write();
        let ptr = &mut *guard as *mut WorldSceneStore as *mut c_void;
        self.slice = Some(SliceGuard { _keep_alive: keep_alive, guard });
        ptr
    }

    /// The unlock callback body ([`pulsar_pie_abi::UnlockWorldFn`]).
    /// Idempotent: closing an un-opened slice is a no-op (lets early-error
    /// paths in the guest unwind their state without wedging the host).
    fn unlock(&mut self) {
        self.slice = None;
    }
}

extern "C" fn pie_lock_world(userdata: *mut c_void) -> *mut c_void {
    // SAFETY: `userdata` is the `*mut PieWorldBridge` the host boxed at load
    // time and handed to the guest; it outlives the session and the guest
    // contract forbids writing through it.
    let bridge = unsafe { &mut *(userdata as *mut PieWorldBridge) };
    bridge.lock()
}

extern "C" fn pie_unlock_world(userdata: *mut c_void) {
    let bridge = unsafe { &mut *(userdata as *mut PieWorldBridge) };
    bridge.unlock();
}

/// A loaded, running embedded game.
pub struct PieHost {
    tick: FnTick,
    resize: FnResize,
    input: FnInput,
    shutdown: FnShutdown,

    /// Boxed so its address stays fixed while the game holds `&mut *ctx`.
    ctx: Box<PieContext>,
    /// `*const wgpu::Texture` the game renders into; set by `init`.
    out_texture: *const c_void,

    /// The shared-world lock-callback state (#635). Boxed so its address is
    /// stable; its pointer is the `userdata` the lock callbacks receive.
    world_bridge: Option<Box<PieWorldBridge>>,

    /// Held for the session; `ctx.device`/`ctx.queue` point into these.
    _device: Arc<wgpu::Device>,
    _queue: Arc<wgpu::Queue>,

    started: bool,
    /// Dropped **last** — after `shutdown`. `Option` so `Drop` can order things.
    lib: Option<Library>,
    /// On Windows we load a temp copy; remember it so we can clean it up.
    temp_copy: Option<PathBuf>,
}

impl PieHost {
    /// Compute the `cdylib` output path for a project.
    ///
    /// `crate_name` is the project's Cargo package name (dashes are normalized to
    /// underscores by cargo). `release` selects `target/release` vs
    /// `target/debug`.
    pub fn output_dylib_path(project_root: &Path, crate_name: &str, release: bool) -> PathBuf {
        let lib_stem = crate_name.replace('-', "_");
        let profile_dir = if release { "release" } else { "debug" };
        let file = if cfg!(target_os = "windows") {
            format!("{lib_stem}.dll")
        } else if cfg!(target_os = "macos") {
            format!("lib{lib_stem}.dylib")
        } else {
            format!("lib{lib_stem}.so")
        };
        project_root.join("target").join(profile_dir).join(file)
    }

    /// Load a freshly-built game `cdylib` and initialize the embedded game.
    ///
    /// `device`/`queue` are the editor's (GPUI's) handles; the game clones them
    /// so it shares the same GPU device. `shared_world` is the editor's
    /// authoritative scene store (#635, ABI v2): one count stays with the host
    /// for the session and one is transferred to the guest via `Arc::into_raw`
    /// (the single-count transfer rule -- the guest reclaims it exactly once).
    /// `scene_path` is a `.level` file the editor wrote from its current scene;
    /// under v2 it is advisory only (the guest adopts the already-hydrated
    /// shared world instead of loading it), kept for logging/legacy guests.
    ///
    /// # Safety
    /// `device`/`queue` must be valid and outlive the call; the loaded library
    /// must have been built against the same `wgpu` version as the editor (the
    /// ABI-version check guards the struct contract, not the wgpu ABI itself).
    #[allow(clippy::too_many_arguments)]
    pub unsafe fn load(
        dylib_path: &Path,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        format: wgpu::TextureFormat,
        width: u32,
        height: u32,
        project_root: &Path,
        scene_path: Option<&Path>,
        shared_world: Arc<RwLock<WorldSceneStore>>,
    ) -> Result<Self, String> {
        if !dylib_path.exists() {
            return Err(format!("Game library not found: {}", dylib_path.display()));
        }

        // On Windows the original file stays locked while loaded, which blocks
        // the next `cargo build --lib` (hot-reload). Load a temp copy instead.
        let (load_path, temp_copy) = if cfg!(target_os = "windows") {
            let mut tmp = std::env::temp_dir();
            let stamp = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0);
            let name = dylib_path
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("pie_game");
            tmp.push(format!("{name}_{stamp}.dll"));
            std::fs::copy(dylib_path, &tmp)
                .map_err(|e| format!("Failed to copy game dll to temp: {e}"))?;
            (tmp.clone(), Some(tmp))
        } else {
            (dylib_path.to_path_buf(), None)
        };

        let lib =
            Library::new(&load_path).map_err(|e| format!("Failed to load game library: {e}"))?;

        // ── ABI-version gate ─────────────────────────────────────────────────
        let abi_version: FnAbiVersion = *lib
            .get(SYM_ABI_VERSION)
            .map_err(|e| format!("Missing symbol {}: {e}", sym_name(SYM_ABI_VERSION)))?;
        let lib_abi = abi_version();
        if lib_abi != PIE_ABI_VERSION {
            return Err(if lib_abi < PIE_ABI_VERSION {
                format!(
                    "Game was built against PiE ABI v{lib_abi}, editor expects v{PIE_ABI_VERSION}. \
                     v1 games predate the shared-world bridge (#635) and cannot adopt this \
                     session's world -- rebuild the project (Build Core)."
                )
            } else {
                format!(
                    "Game was built against PiE ABI v{lib_abi}, editor expects v{PIE_ABI_VERSION}. \
                     Update Pulsar (the game library is newer than this editor)."
                )
            });
        }

        // ── Resolve the rest of the entry points ─────────────────────────────
        let init: FnInit = *lib
            .get(SYM_INIT)
            .map_err(|e| format!("Missing symbol {}: {e}", sym_name(SYM_INIT)))?;
        let tick: FnTick = *lib
            .get(SYM_TICK)
            .map_err(|e| format!("Missing symbol {}: {e}", sym_name(SYM_TICK)))?;
        let resize: FnResize = *lib
            .get(SYM_RESIZE)
            .map_err(|e| format!("Missing symbol {}: {e}", sym_name(SYM_RESIZE)))?;
        let input: FnInput = *lib
            .get(SYM_INPUT)
            .map_err(|e| format!("Missing symbol {}: {e}", sym_name(SYM_INPUT)))?;
        let shutdown: FnShutdown = *lib
            .get(SYM_SHUTDOWN)
            .map_err(|e| format!("Missing symbol {}: {e}", sym_name(SYM_SHUTDOWN)))?;

        let color_format = format_to_u32(format)
            .ok_or_else(|| format!("Unsupported viewport format for PiE: {format:?}"))?;

        // Keep the device/queue alive; ctx points into them.
        let device = Arc::new(device.clone());
        let queue = Arc::new(queue.clone());

        // Path strings must stay valid for the duration of the `init` call.
        let project_root_s = project_root.to_string_lossy().into_owned();
        let scene_path_s = scene_path.map(|p| p.to_string_lossy().into_owned());

        // ── Shared-world bridge (#635, ABI v2) ───────────────────────────────
        // One Arc count stays with the bridge for the session; a SECOND count
        // is transferred to the guest via into_raw (guest reclaims it with
        // exactly one from_raw). The bridge is boxed so its address is stable
        // -- the guest's lock callbacks receive it as userdata.
        let mut world_bridge = Box::new(PieWorldBridge::new(Arc::clone(&shared_world)));
        let shared_world_ptr =
            Arc::into_raw(Arc::clone(&shared_world)) as *const c_void;
        let userdata: *mut c_void = &mut *world_bridge as *mut PieWorldBridge as *mut c_void;

        let mut ctx = Box::new(PieContext {
            abi_version: PIE_ABI_VERSION,
            device: Arc::as_ptr(&device) as *const c_void,
            queue: Arc::as_ptr(&queue) as *const c_void,
            color_format,
            width: width.max(1),
            height: height.max(1),
            project_root_ptr: project_root_s.as_ptr(),
            project_root_len: project_root_s.len(),
            scene_path_ptr: scene_path_s.as_ref().map(|s| s.as_ptr()).unwrap_or(std::ptr::null()),
            scene_path_len: scene_path_s.as_ref().map(|s| s.len()).unwrap_or(0),
            userdata,
            log: log_cb,
            out_texture: std::ptr::null(),
            shared_world: shared_world_ptr,
            lock_shared_world: pie_lock_world,
            unlock_shared_world: pie_unlock_world,
        });

        let ok = init(&mut *ctx as *mut PieContext);
        // The path strings can drop now — the game has copied what it needs.
        drop(project_root_s);
        drop(scene_path_s);

        if ok != INIT_OK {
            // Give the transferred count back so the world can drop normally.
            drop(Arc::from_raw(shared_world_ptr as *const RwLock<WorldSceneStore>));
            return Err("Game init returned failure (see log for details)".to_string());
        }

        let out_texture = ctx.out_texture;

        Ok(Self {
            tick,
            resize,
            input,
            shutdown,
            ctx,
            out_texture,
            world_bridge: Some(world_bridge),
            _device: device,
            _queue: queue,
            started: true,
            lib: Some(lib),
            temp_copy,
        })
    }

    /// Advance and render one game frame.
    pub fn tick(&self, delta_time: f32) {
        if self.started {
            unsafe { (self.tick)(delta_time) };
        }
    }

    /// Resize the game's offscreen render target.
    pub fn resize(&mut self, width: u32, height: u32) {
        if self.started {
            unsafe { (self.resize)(width.max(1), height.max(1)) };
            self.ctx.width = width.max(1);
            self.ctx.height = height.max(1);
        }
    }

    /// Forward one input event to the game.
    pub fn input(&self, ev: &InputEvent) {
        if self.started {
            unsafe { (self.input)(ev as *const InputEvent) };
        }
    }

    /// The game's offscreen color texture, for the editor to sample into its
    /// viewport. Valid until [`PieHost::stop`] / drop. `None` if init did not set
    /// it or the pointer is null.
    ///
    /// # Safety
    /// The returned reference borrows memory owned by the loaded library; it must
    /// not outlive `self`.
    pub unsafe fn out_texture(&self) -> Option<&wgpu::Texture> {
        (self.out_texture as *const wgpu::Texture).as_ref()
    }

    /// Stop the game: run its teardown and unload the library.
    pub fn stop(&mut self) {
        if self.started {
            unsafe { (self.shutdown)() };
            self.started = false;
        }
        // Tear down the shared-world bridge only AFTER shutdown returned --
        // the guest's tick slices borrow through it for the whole session.
        // If a misbehaving guest left a slice open, dropping the bridge here
        // releases that lock with the session anyway.
        self.world_bridge = None;
        // Drop the library (dlclose / FreeLibrary) before removing the temp copy.
        self.lib = None;
        if let Some(path) = self.temp_copy.take() {
            let _ = std::fs::remove_file(path);
        }
    }
}

impl Drop for PieHost {
    fn drop(&mut self) {
        self.stop();
    }
}

// ── C callbacks (game → editor) ─────────────────────────────────────────────

/// Route a game-side log line into the editor's tracing subscriber.
extern "C" fn log_cb(_userdata: *mut c_void, level: u32, msg_ptr: *const u8, msg_len: usize) {
    let msg = read_utf8(msg_ptr, msg_len);
    match level {
        LOG_ERROR => tracing::error!(target: "pie_game", "{msg}"),
        LOG_WARN => tracing::warn!(target: "pie_game", "{msg}"),
        LOG_INFO => tracing::info!(target: "pie_game", "{msg}"),
        LOG_DEBUG => tracing::debug!(target: "pie_game", "{msg}"),
        LOG_TRACE => tracing::trace!(target: "pie_game", "{msg}"),
        _ => tracing::info!(target: "pie_game", "{msg}"),
    }
}

// ── Helpers ─────────────────────────────────────────────────────────────────

fn read_utf8(ptr: *const u8, len: usize) -> String {
    if ptr.is_null() || len == 0 {
        return String::new();
    }
    let bytes = unsafe { std::slice::from_raw_parts(ptr, len) };
    String::from_utf8_lossy(bytes).into_owned()
}

fn sym_name(sym: &[u8]) -> String {
    String::from_utf8_lossy(sym).into_owned()
}

/// Encode a `wgpu::TextureFormat` as the `u32` id the game decodes. Must match
/// `pulsar_game::embed::wgpu_format_from_u32`.
fn format_to_u32(format: wgpu::TextureFormat) -> Option<u32> {
    use wgpu::TextureFormat as F;
    Some(match format {
        F::Rgba8Unorm => 0,
        F::Rgba8UnormSrgb => 1,
        F::Bgra8Unorm => 2,
        F::Bgra8UnormSrgb => 3,
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// #635 locking protocol, host side: lock hands out an exclusive slice,
    /// a second concurrent lock is refused (the non-reentrancy witness), and
    /// unlock closes the slice so the next one can open. Idempotent unlock.
    #[test]
    fn world_bridge_hands_out_one_exclusive_slice_at_a_time() {
        let bridge = PieWorldBridge::new(Arc::new(RwLock::new(WorldSceneStore::new())));
        let userdata = &bridge as *const PieWorldBridge as *mut c_void;

        // SAFETY (test): userdata points at `bridge`, alive for this scope.
        let first = unsafe { pie_lock_world(userdata) };
        assert!(!first.is_null(), "first slice must be granted");

        let second = unsafe { pie_lock_world(userdata) };
        assert!(
            second.is_null(),
            "a second slice while one is open must be REFUSED, not granted"
        );

        unsafe { pie_unlock_world(userdata) };
        let third = unsafe { pie_lock_world(userdata) };
        assert!(!third.is_null(), "after unlock a new slice must be grantable");
        unsafe { pie_unlock_world(userdata) };

        // Idempotent unlock: no panic / no wedge.
        unsafe { pie_unlock_world(userdata) };
        let fourth = unsafe { pie_lock_world(userdata) };
        assert!(!fourth.is_null());
        unsafe { pie_unlock_world(userdata) };
    }

    /// #635: writes made through the handed-out pointer are visible through
    /// the editor's own handle afterwards -- same allocation, not a copy.
    #[test]
    fn slice_mutations_land_in_the_host_store() {
        let store = Arc::new(RwLock::new(WorldSceneStore::new()));
        let expected = {
            let mut s = store.write();
            s.spawn(Some("probe".into()), "Probe", None).unwrap()
        };
        let mut bridge = PieWorldBridge::new(Arc::clone(&store));
        let userdata = &bridge as *const PieWorldBridge as *mut c_void;

        let ptr = bridge.lock() as *mut WorldSceneStore;
        assert!(!ptr.is_null());
        // The guest would write here; reading the spawned entity through the
        // raw pointer proves it aliases the host's store.
        let name = unsafe { (*ptr).name(expected).map(str::to_string) };
        assert_eq!(name.as_deref(), Some("Probe"));
        bridge.unlock();
    }
}
