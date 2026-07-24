//! **Play In Editor** embedding — host-driven, no window of our own (issue #243).
//!
//! When the user presses *Play* in the level editor, the editor compiles the
//! game project as a `cdylib` and loads it. Instead of `main.rs` opening a winit
//! window and owning a GPU device, the editor hands us **its** `wgpu::Device` /
//! `Queue` and drives us one frame at a time. We build an *offscreen* Helio
//! renderer that draws into a texture the editor then samples into its viewport.
//!
//! The generated `lib.rs` is only a thin `extern "C"` shim around the functions
//! here; all the real logic lives in-workspace so it is type- and API-checked
//! against Helio during the normal Pulsar build.
//!
//! ## Threading contract
//!
//! Every `pie_*` entry point **must** be called from the *same* thread — the
//! editor's render thread. The live game is stored in a `thread_local!`, so
//! calling from another thread simply finds no game. This mirrors how a winit
//! app owns all GPU state on the main thread and sidesteps `Send`/`Sync` bounds
//! on Helio's renderer.

use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use helio::{Camera, DebugDrawState, Renderer, RendererConfig, Scene};
use pulsar_pie_abi::{
    EngineContext as PieContext, InputEvent, LogFn, INIT_ERR, INIT_OK, LOG_ERROR, LOG_INFO,
    PIE_ABI_VERSION,
};

use crate::freecam::FreeCam;
use crate::tick::TickLoop;
use pulsar_core::TickMode;

thread_local! {
    /// The single live embedded game for this thread. `None` before init /
    /// after shutdown.
    static GAME: RefCell<Option<EmbeddedGame>> = const { RefCell::new(None) };
}

/// A game running embedded inside the editor: ECS tick loop + an offscreen Helio
/// renderer sharing the editor's GPU device.
pub struct EmbeddedGame {
    tick_loop: TickLoop,
    renderer: Renderer,
    device: Arc<wgpu::Device>,
    #[allow(dead_code)]
    queue: Arc<wgpu::Queue>,
    color_format: wgpu::TextureFormat,
    width: u32,
    height: u32,
    /// The offscreen render target the editor samples. Recreated on resize.
    out_texture: wgpu::Texture,
    out_view: wgpu::TextureView,
    /// Fallback free-look camera (used until an ECS camera drives the view).
    freecam: FreeCam,

    // ── Host log callback ───────────────────────────────────────────────────
    userdata: *mut std::ffi::c_void,
    log: LogFn,
}

impl EmbeddedGame {
    /// Route a log line back to the editor's log viewer.
    fn log(&self, level: u32, msg: &str) {
        (self.log)(self.userdata, level, msg.as_ptr(), msg.len());
    }
}

/// Create the offscreen color target the game renders into and the editor
/// samples. `TEXTURE_BINDING` lets the editor bind it in a blit; `COPY_SRC`
/// allows thumbnail/readback paths.
fn make_target(
    device: &wgpu::Device,
    format: wgpu::TextureFormat,
    width: u32,
    height: u32,
) -> (wgpu::Texture, wgpu::TextureView) {
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("pie_offscreen_color"),
        size: wgpu::Extent3d {
            width: width.max(1),
            height: height.max(1),
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT
            | wgpu::TextureUsages::TEXTURE_BINDING
            | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
    (texture, view)
}

/// Try to seed the freecam from the `.level` file's saved editor camera so the
/// first embedded frame matches what the editor was showing.
fn read_editor_camera(scene_path: &Path) -> Option<FreeCam> {
    let text = std::fs::read_to_string(scene_path).ok()?;
    let v: serde_json::Value = serde_json::from_str(&text).ok()?;
    let cam = v.get("editor")?.get("camera")?;
    let pos = cam.get("position")?.as_array()?;
    if pos.len() < 3 {
        return None;
    }
    let position = glam::Vec3::new(
        pos[0].as_f64()? as f32,
        pos[1].as_f64()? as f32,
        pos[2].as_f64()? as f32,
    );
    let yaw = cam.get("yaw").and_then(|v| v.as_f64()).unwrap_or(0.0) as f32;
    let pitch = cam.get("pitch").and_then(|v| v.as_f64()).unwrap_or(0.0) as f32;
    Some(FreeCam::default().place(position, yaw, pitch))
}

impl EmbeddedGame {
    /// Build the embedded game from the host context and a project-supplied
    /// `setup` closure (the generated `engine_main::setup`, which registers the
    /// project's actor classes / blueprint dispatcher on the tick loop).
    ///
    /// # Safety
    /// `ctx` must be a valid [`PieContext`] whose `device`/`queue` point at live
    /// `wgpu::Device`/`Queue` of the **same** wgpu version this crate compiled
    /// against (guaranteed by the host's ABI-version gate).
    unsafe fn new<F>(ctx: &PieContext, setup: F) -> Result<Self, String>
    where
        F: FnOnce(&mut TickLoop) -> Result<(), String>,
    {
        if ctx.abi_version != PIE_ABI_VERSION {
            return Err(format!(
                "PiE ABI mismatch: host={}, game={}",
                ctx.abi_version, PIE_ABI_VERSION
            ));
        }

        // Borrow the editor's device/queue and take our own reference-counted
        // handles. `wgpu::Device`/`Queue` are cheap Arc-backed handles, so
        // cloning yields another handle to the *same* GPU device — the same
        // trick the editor's `helio_renderer` uses on GPUI's device.
        let device_ref = &*(ctx.device as *const wgpu::Device);
        let queue_ref = &*(ctx.queue as *const wgpu::Queue);
        let device = Arc::new(device_ref.clone());
        let queue = Arc::new(queue_ref.clone());

        let color_format = wgpu_format_from_u32(ctx.color_format)
            .ok_or_else(|| format!("Unsupported color format id {}", ctx.color_format))?;
        let width = ctx.width.max(1);
        let height = ctx.height.max(1);

        let project_root = read_str(ctx.project_root_ptr, ctx.project_root_len)
            .map(PathBuf::from)
            .ok_or_else(|| "PiE: invalid project_root".to_string())?;
        let scene_path = read_str(ctx.scene_path_ptr, ctx.scene_path_len).map(PathBuf::from);

        // ── Engine state / settings (mirror `TickLoop::run_with_windows`) ────
        // Must happen before scene loading: component sync reads the global
        // EngineContext. This is the dylib's *own* engine_state global, isolated
        // from the editor's copy.
        pulsar_settings::register_all_settings(engine_state::settings::global_config());
        let engine_ctx = engine_state::EngineContext::new();
        engine_ctx.clone().set_global();

        // ── ECS tick loop + project setup ────────────────────────────────────
        let threads = std::thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(4);
        let mut tick_loop = TickLoop::new(TickMode::default(), threads);
        setup(&mut tick_loop).map_err(|e| format!("Project setup failed: {e}"))?;

        // ── Offscreen Helio renderer (external device) ───────────────────────
        let (out_texture, out_view) = make_target(&device, color_format, width, height);

        let config = RendererConfig::new(width, height, color_format);
        let scene = Scene::new(device.clone(), queue.clone());
        let debug_camera_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("pie_debug_camera"),
            size: 64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let cull_stats_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("pie_cull_stats"),
            size: 64,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let debug_state = Arc::new(Mutex::new(DebugDrawState::default()));
        let graph = helio_default_graphs::build_default_graph_external(
            &device,
            &queue,
            &scene,
            config,
            debug_state.clone(),
            &debug_camera_buffer,
            &cull_stats_buffer,
            None,
        );
        let mut renderer = Renderer::new_with_external_device(
            device.clone(),
            queue.clone(),
            color_format,
            width,
            height,
            1.0,
            config,
            scene,
            graph,
            debug_state,
            debug_camera_buffer,
            cull_stats_buffer,
        );
        // Game (not editor) presentation: no editor gizmos, illumination from
        // the scene's own lights only.
        renderer.set_editor_mode(false);
        renderer.set_ambient([0.0, 0.0, 0.0], 0.0);

        // ── Load the scene the editor handed us ──────────────────────────────
        let mut freecam = FreeCam::default();
        if let Some(ref path) = scene_path {
            match pulsar_scene::SceneLoader::load_file(path, &project_root, &mut renderer) {
                Ok(()) => {
                    if let Some(seeded) = read_editor_camera(path) {
                        freecam = seeded;
                    }
                }
                Err(e) => {
                    tracing::warn!(scene = %path.display(), "PiE: failed to load scene: {e}");
                }
            }
        }

        Ok(Self {
            tick_loop,
            renderer,
            device,
            queue,
            color_format,
            width,
            height,
            out_texture,
            out_view,
            freecam,
            userdata: ctx.userdata,
            log: ctx.log,
        })
    }

    /// Advance simulation and render one frame into the offscreen target.
    fn tick(&mut self, dt: f32) {
        // 1. Game logic — one ECS/blueprint tick.
        self.tick_loop.tick_once();

        // 2. Camera. ECS-driven cameras will supersede this once wired; for now
        //    the free-look camera seeded from the editor view drives rendering.
        self.freecam.update(dt);
        let cam = self.freecam.to_render_camera();
        let aspect = self.width as f32 / self.height.max(1) as f32;
        let helio_cam = Camera::perspective_look_at(
            glam::Vec3::from_array(cam.position),
            glam::Vec3::from_array(cam.target),
            glam::Vec3::from_array(cam.up),
            cam.fov_y,
            aspect,
            cam.near,
            cam.far,
        );

        // 3. Render into the offscreen target the editor samples. The game owns
        //    its world from here on — Unreal-style PIE, no writeback to the editor.
        if let Err(e) = self.renderer.render(&helio_cam, &self.out_view) {
            tracing::error!("PiE render error: {:?}", e);
        }
    }

    fn resize(&mut self, width: u32, height: u32) {
        let width = width.max(1);
        let height = height.max(1);
        if (width, height) == (self.width, self.height) {
            return;
        }
        self.width = width;
        self.height = height;
        let (tex, view) = make_target(&self.device, self.color_format, width, height);
        self.out_texture = tex;
        self.out_view = view;
        self.renderer.set_render_size(width, height);
    }

    fn input(&mut self, ev: &InputEvent) {
        use pulsar_pie_abi::input_kind;
        match ev.kind {
            input_kind::MOUSE_MOVE => {
                // Editor forwards look deltas via wheel/move; hook here when the
                // editor drives an in-game cursor. No-op for now.
            }
            input_kind::MOUSE_WHEEL => {
                self.freecam.on_mouse_delta(0.0, ev.delta as f64);
            }
            _ => {}
        }
    }

    fn out_texture_ptr(&self) -> *const std::ffi::c_void {
        &self.out_texture as *const wgpu::Texture as *const std::ffi::c_void
    }
}

// ── Public entry points (called by the generated `lib.rs` shim) ─────────────

/// Initialize the embedded game. Returns [`INIT_OK`]/[`INIT_ERR`]. On success,
/// writes the offscreen texture pointer back into `ctx.out_texture`.
///
/// # Safety
/// `ctx` must point at a valid, host-populated [`PieContext`]. See
/// [`EmbeddedGame::new`].
pub unsafe fn pie_init<F>(ctx: *mut PieContext, setup: F) -> u32
where
    F: FnOnce(&mut TickLoop) -> Result<(), String>,
{
    if ctx.is_null() {
        return INIT_ERR;
    }
    let ctx_ref = &mut *ctx;
    match EmbeddedGame::new(ctx_ref, setup) {
        Ok(game) => {
            game.log(LOG_INFO, "PiE game initialized");
            // Move the game into its final resting place *first*, then take the
            // offscreen-texture pointer from that stable location. Taking it from
            // the pre-move `game` would dangle once it is moved into the cell.
            GAME.with(|g| {
                let mut slot = g.borrow_mut();
                *slot = Some(game);
                ctx_ref.out_texture = slot.as_ref().unwrap().out_texture_ptr();
            });
            INIT_OK
        }
        Err(e) => {
            // Report through both the host log callback and tracing.
            let msg = format!("PiE init failed: {e}");
            (ctx_ref.log)(ctx_ref.userdata, LOG_ERROR, msg.as_ptr(), msg.len());
            tracing::error!("{msg}");
            INIT_ERR
        }
    }
}

/// Advance and render one frame. No-op if not initialized on this thread.
pub fn pie_tick(dt: f32) {
    GAME.with(|g| {
        if let Some(game) = g.borrow_mut().as_mut() {
            game.tick(dt);
        }
    });
}

/// Resize the offscreen render target.
pub fn pie_resize(width: u32, height: u32) {
    GAME.with(|g| {
        if let Some(game) = g.borrow_mut().as_mut() {
            game.resize(width, height);
        }
    });
}

/// Forward one input event.
///
/// # Safety
/// `ev` must be a valid pointer to an [`InputEvent`] for the duration of the call.
pub unsafe fn pie_input(ev: *const InputEvent) {
    if ev.is_null() {
        return;
    }
    let ev = &*ev;
    GAME.with(|g| {
        if let Some(game) = g.borrow_mut().as_mut() {
            game.input(ev);
        }
    });
}

/// Tear down the embedded game, dropping its world + renderer before the host
/// unloads the library.
pub fn pie_shutdown() {
    GAME.with(|g| {
        if let Some(game) = g.borrow_mut().take() {
            game.log(LOG_INFO, "PiE game shutting down");
            drop(game);
        }
    });
}

// ── Helpers ─────────────────────────────────────────────────────────────────

/// Read a host-provided UTF-8 string from a pointer/len pair.
///
/// # Safety
/// `ptr`/`len` must describe a valid UTF-8 byte range or `ptr` be null.
unsafe fn read_str(ptr: *const u8, len: usize) -> Option<String> {
    if ptr.is_null() || len == 0 {
        return None;
    }
    let bytes = std::slice::from_raw_parts(ptr, len);
    std::str::from_utf8(bytes).ok().map(|s| s.to_string())
}

/// Map the `u32` color-format id (a `wgpu::TextureFormat` reinterpreted by the
/// host) back to a `wgpu::TextureFormat`. Only the formats a GPUI/editor
/// viewport surface can present are handled; anything else is rejected so we
/// never build a renderer against a format the editor cannot sample.
fn wgpu_format_from_u32(id: u32) -> Option<wgpu::TextureFormat> {
    use wgpu::TextureFormat as F;
    // The host obtains this via `format_to_u32` (see engine_backend::services::
    // pie_host) which uses the same match, keeping both sides in lockstep.
    Some(match id {
        0 => F::Rgba8Unorm,
        1 => F::Rgba8UnormSrgb,
        2 => F::Bgra8Unorm,
        3 => F::Bgra8UnormSrgb,
        _ => return None,
    })
}

/// The inverse of [`wgpu_format_from_u32`], re-exported so the host encodes the
/// exact same ids. Kept here so the mapping lives in one place.
pub fn format_to_u32(format: wgpu::TextureFormat) -> Option<u32> {
    use wgpu::TextureFormat as F;
    Some(match format {
        F::Rgba8Unorm => 0,
        F::Rgba8UnormSrgb => 1,
        F::Bgra8Unorm => 2,
        F::Bgra8UnormSrgb => 3,
        _ => return None,
    })
}
