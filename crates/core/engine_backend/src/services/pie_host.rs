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
use pulsar_pie_abi::{
    EngineContext as PieContext, FnAbiVersion, FnInit, FnInput, FnResize, FnShutdown, FnTick,
    InputEvent, INIT_OK, LOG_DEBUG, LOG_ERROR, LOG_INFO, LOG_TRACE, LOG_WARN, PIE_ABI_VERSION,
    SYM_ABI_VERSION, SYM_INIT, SYM_INPUT, SYM_RESIZE, SYM_SHUTDOWN, SYM_TICK,
};

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
    /// so it shares the same GPU device. `scene_path` is a `.level` file the
    /// editor wrote from its current `SceneDb`.
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
            return Err(format!(
                "Game was built against PiE ABI v{lib_abi}, editor expects v{PIE_ABI_VERSION}. \
                 Rebuild the project (Build Core)."
            ));
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
            userdata: std::ptr::null_mut(),
            log: log_cb,
            out_texture: std::ptr::null(),
        });

        let ok = init(&mut *ctx as *mut PieContext);
        // The path strings can drop now — the game has copied what it needs.
        drop(project_root_s);
        drop(scene_path_s);

        if ok != INIT_OK {
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
