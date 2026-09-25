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
use std::path::PathBuf;
use std::sync::Arc;

use engine_backend::scene::{ensure_gpu_mirror, sync_static_mesh_rows};
use helio::{Camera, Renderer, RendererBuilder, RendererConfig};
use parking_lot::RwLock;
use pulsar_pie_abi::{
    EngineContext as PieContext, InputEvent, LogFn, INIT_ERR, INIT_OK, LOG_ERROR, LOG_INFO,
    PIE_ABI_VERSION,
};

use crate::camera_selection::select_world_camera;
use crate::freecam::FreeCam;
use crate::tick::TickLoop;
use pulsar_core::TickMode;

/// Events the PIE session's debug tap keeps.
const PIE_EVENT_TAP_CAPACITY: usize = 256;

thread_local! {
    /// The single live embedded game for this thread. `None` before init /
    /// after shutdown.
    static GAME: RefCell<Option<EmbeddedGame>> = const { RefCell::new(None) };
}

/// The simulation half of a Play-in-Editor game (#925): the tick loop on
/// the editor's shared world, its script driver (which follows the world's
/// `ClassInstance`s exactly like the standalone game: placed instances,
/// objects placed during Play and runtime spawns), class reloads from the
/// editor, pause / step, and the script problems reported back. It owns no
/// GPU state; [`EmbeddedGame`] adds the offscreen renderer. Tests drive it
/// directly.
pub struct PieSession {
    pub tick_loop: TickLoop,
    /// Queues edited classes for reload by the script driver (#921/#922).
    /// Events come from the host via [`pie_asset_updated`], published on
    /// this dylib's own asset bus.
    _class_reloads: Option<pulsar_events::AssetSubscription>,
    /// Problems not yet taken by the host.
    problems: Vec<pulsar_events::ScriptProblem>,
}

impl PieSession {
    /// A session over `tick_loop`, whose project `setup()` already ran.
    /// `level` names the level `LevelLoaded` reports.
    pub fn new(mut tick_loop: TickLoop, level: Option<String>) -> Self {
        // Scripts follow the shared world (#922): the driver `setup()`
        // enabled finds the level the host already hydrated at its first
        // reconcile, and objects placed during Play the same way. Class
        // edits reach it as reload requests applied at its next frame.
        let class_reloads = tick_loop.scripts.as_ref().map(|driver| {
            driver
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .subscribe_class_reloads()
        });
        if let (Some(level), Some(driver)) = (level, &tick_loop.scripts) {
            driver.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).set_level_name(level);
        }
        // Play-in-Editor is a debugging session: record recently flushed
        // events for the editor's events panel (`pie_events_snapshot`) and
        // keep script problems for the problems panel.
        tick_loop.events.set_tap(true, PIE_EVENT_TAP_CAPACITY);
        tick_loop.collect_script_problems(true);
        Self { tick_loop, _class_reloads: class_reloads, problems: Vec::new() }
    }

    /// Run one simulation frame (nothing while paused, unless a step is
    /// pending).
    pub fn tick(&mut self) {
        self.tick_loop.tick_once();
        let problems = self.tick_loop.take_script_problems();
        if !problems.is_empty() {
            self.problems.extend(problems);
            let excess = self.problems.len().saturating_sub(MAX_PENDING_PROBLEMS);
            self.problems.drain(..excess);
        }
    }

    /// An asset changed in the editor: publish it on this library's asset
    /// bus. A class update reloads the class's module at the next frame,
    /// without rebuilding anything (instances keep their variables; see
    /// `ScriptDriver::reload_class_for_asset`).
    pub fn asset_updated(&self, event: pulsar_events::AssetUpdated) {
        pulsar_events::publish_asset_updated(event);
    }

    /// A [`pulsar_pie_abi::control`] command.
    pub fn control(&mut self, command: u32, arg: u64) -> u64 {
        use pulsar_pie_abi::control;
        match command {
            control::PAUSE => {
                self.tick_loop.set_paused(true);
                1
            }
            control::RESUME => {
                self.tick_loop.set_paused(false);
                1
            }
            control::STEP => {
                if !self.tick_loop.is_paused() {
                    return 0;
                }
                self.tick_loop.step(u32::try_from(arg).unwrap_or(u32::MAX));
                1
            }
            control::IS_PAUSED => u64::from(self.tick_loop.is_paused()),
            control::FRAME => self.tick_loop.ticks(),
            _ => 0,
        }
    }

    /// Problems not yet taken.
    pub fn problems(&self) -> &[pulsar_events::ScriptProblem] {
        &self.problems
    }

    /// Take the problems raised since the last call.
    pub fn take_problems(&mut self) -> Vec<pulsar_events::ScriptProblem> {
        std::mem::take(&mut self.problems)
    }

    /// The session's event hub as a Gamma FFI table for editor plugins
    /// (#942). The caller owns the reference it holds.
    pub fn export_event_bus(&self) -> pulsar_events::gamma::ffi::RawBus {
        self.tick_loop.events.export_raw()
    }

    /// End the session: `end_play` for every script while the world still
    /// holds their objects; script subscriptions, queued handler calls,
    /// timers and queued events are dropped (see `TickLoop::end_scripts`).
    pub fn shutdown(&mut self) {
        self.tick_loop.end_scripts();
        self._class_reloads = None;
    }
}

/// Problems a session keeps for the host at most.
const MAX_PENDING_PROBLEMS: usize = 256;

/// A game running embedded inside the editor: ECS tick loop + an offscreen Helio
/// renderer sharing the editor's GPU device.
pub struct EmbeddedGame {
    session: PieSession,
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
    /// The tick loop's shared world (Pulsar-Native#634): the same store
    /// gameplay mutates, rendered through the per-frame rebuild bridge.
    /// ABI v2 requires the host's shared-world token; this store is adopted
    /// from the host and remains SceneDB-resident
    /// for gameplay and rendering.
    scene_store: engine_backend::scene::SharedScene,
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

        // ABI v2 (#635): when the host hands us its shared-world token, we
        // ADOPT the host's authoritative world as our own via the documented
        // single-count Arc transfer (host did `Arc::into_raw` on a clone; our
        // `from_raw` here reclaims exactly that count, and it drops with the
        // embedded game at shutdown). Everything downstream -- setup()'s
        // actor registration, per-tick schedule/actor phases -- then
        // operates on the editor's live world, so editor edits are visible
        // to gameplay the same frame gameplay runs, and vice versa.
        let tick_loop = if ctx.shared_world.is_null() {
            return Err(
                "PiE: v2 context carries no shared_world token (host too old?)".to_string(),
            );
        } else {
            let host_store = unsafe {
                Arc::from_raw(ctx.shared_world as *const RwLock<pulsar_scenedb::SceneDb>)
            };
            TickLoop::with_scene_store(host_store, TickMode::default(), threads)
        };
        let mut tick_loop = tick_loop;
        // Native hot reload (#653): when the host stopped a still-running
        // game to swap in this fresh build, the shared world carries the
        // previous session's entities. Arm rebinding BEFORE project setup so
        // `TickLoop::register_actor` re-binds script actors to their existing
        // entities instead of spawning duplicates — the native equivalent of
        // D3's `reload_blueprint` for VM instances.
        if ctx.session_flags & pulsar_pie_abi::session_flags::RELOAD != 0 {
            tick_loop.begin_script_reload();
            let msg = "PiE hot reload: actor registrations will re-bind to existing entities";
            (ctx.log)(ctx.userdata, LOG_INFO, msg.as_ptr(), msg.len());
        }
        // NOTE: `setup()` deliberately runs AFTER adoption so project actors
        // register against the host's world. The scene file is NOT loaded:
        // under v2 the shared world already holds the hydrated level.
        setup(&mut tick_loop).map_err(|e| format!("Project setup failed: {e}"))?;

        // ── Offscreen Helio renderer (external device) ───────────────────────
        let (out_texture, out_view) = make_target(&device, color_format, width, height);

        // ── Renderer seam onto the shared world (#637/#634) ──────────────────
        // The world already holds the level (the host hydrated it before Play,
        // and under v2 we adopted that very store). SceneDB's GPU mirror must
        // be attached BEFORE the renderer is constructed: `RendererBuilder::new`
        // requires a `SceneDbHandle` up front (SceneDB is the sole scene
        // authority). When the editor's own viewport already wired the
        // mirror, this is idempotent and just returns that same handle.
        let scene_store = Arc::clone(&tick_loop.scene_store);
        let scene_db_handle =
            ensure_gpu_mirror(&mut scene_store.write(), device.clone(), queue.clone());

        let config = RendererConfig::new(width, height, color_format);
        let renderer = RendererBuilder::new(config, scene_db_handle)
            .with_external_device()
            .with_editor_mode(false)
            .with_ambient([0.0, 0.0, 0.0], 0.0)
            .with_pass_build_context(Box::new(
                helio_default_graphs::build_default_graph_external_with_context,
            ))
            .build(device.clone(), queue.clone(), width, height, color_format);

        // Under v2 the world comes pre-hydrated by the host, so the old
        // editor-camera file seeding is gone too -- camera selection prefers
        // Camera-typed entities from the shared world instead.
        let freecam = FreeCam::default();
        engine_state::set_project_path(project_root.display().to_string());
        // Advisory only under v2 (the world comes pre-hydrated); it names
        // the level `LevelLoaded` reports.
        let session = PieSession::new(tick_loop, scene_path.map(|p| p.display().to_string()));

        Ok(Self {
            session,
            renderer,
            device,
            queue,
            color_format,
            width,
            height,
            out_texture,
            out_view,
            scene_store,
            freecam,
            userdata: ctx.userdata,
            log: ctx.log,
        })
    }

    /// Advance simulation and render one frame into the offscreen target.
    fn tick(&mut self) {
        // 1. Game logic — one ECS/blueprint tick.
        //
        // Locking note (ABI v2, #635): under PIE this whole call IS the
        // guest's tick slice -- it runs on the editor's render thread inside
        // SYM_TICK, and `TickLoop::tick_once` acquires the shared world's
        // write lock once per phase, dropping it between phases. The
        // reference guest locks through the same parking_lot instance via
        // the transferred Arc (identical by single-workspace builds); the
        // host's lock callbacks remain the policy/witness surface for
        // guests that don't share that universe.
        self.session.tick();

        // 2. Advance the shared world's authoritative SceneDB state and flush
        //    its GPU mirror. World content is read by Helio passes directly
        //    from that mirror -- there is no renderer-owned frame projection
        //    or CPU object cache to rebuild here (same zero-copy seam the
        //    editor viewport renderer uses). A runtime-spawned entity or a
        //    moved object therefore shows up on the very next frame.
        {
            let mut store = self.scene_store.write();
            sync_static_mesh_rows(&mut store, None);
            store.step();
        }

        // 3. Camera. A Camera-typed entity in the shared world drives the
        //    view when present (#637 -- no more unconditional freecam); the
        //    freecam seeded from the editor view remains the fallback.
        let cam = select_world_camera(&self.scene_store.read())
            .unwrap_or_else(|| self.freecam.to_render_camera());
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

        // 4. Render into the offscreen target the editor samples. ABI v2
        //    requires the host's authoritative SceneDB-backed store above, so
        //    simulation and rendering observe the same hydrated world.
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
            // Keys and buttons become input events on the session's hub,
            // delivered at the next tick's after-input flush.
            input_kind::KEY => {
                let key = i64::from(ev.button_or_key);
                if ev.pressed != 0 {
                    self.session.tick_loop.publish_input(pulsar_events::builtin::KeyDown { key });
                } else {
                    self.session.tick_loop.publish_input(pulsar_events::builtin::KeyUp { key });
                }
            }
            input_kind::MOUSE_BUTTON => {
                let button = i64::from(ev.button_or_key);
                if ev.pressed != 0 {
                    self.session.tick_loop.publish_input(pulsar_events::builtin::MouseButtonDown { button });
                } else {
                    self.session.tick_loop.publish_input(pulsar_events::builtin::MouseButtonUp { button });
                }
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
/// `dt` is retained for ABI stability; the frame's simulation step is the
/// tick loop's own clocked tick.
pub fn pie_tick(dt: f32) {
    let _ = dt;
    GAME.with(|g| {
        if let Some(game) = g.borrow_mut().as_mut() {
            game.tick();
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

/// Deliver an asset-update notification from the host (#921): published on
/// this game's own asset bus, where the running game's subscribers (the
/// class reloader) pick it up. Strings are UTF-8 pointer/length pairs; an
/// empty or null id/path means "not given".
///
/// # Safety
/// Each pointer/length pair must describe a valid UTF-8 range or be null.
pub unsafe fn pie_asset_updated(
    kind_ptr: *const u8,
    kind_len: usize,
    id_ptr: *const u8,
    id_len: usize,
    path_ptr: *const u8,
    path_len: usize,
) {
    let Some(kind) = read_str(kind_ptr, kind_len)
        .and_then(|k| serde_json::from_str::<pulsar_events::AssetKind>(&k).ok())
    else {
        tracing::warn!("PiE: asset update with an unreadable kind; ignored");
        return;
    };
    let mut event = pulsar_events::AssetUpdated::new(kind);
    event.id = read_str(id_ptr, id_len);
    event.path = read_str(path_ptr, path_len).map(PathBuf::from);
    GAME.with(|g| match g.borrow().as_ref() {
        Some(game) => game.session.asset_updated(event),
        None => pulsar_events::publish_asset_updated(event),
    });
}

/// A simulation control command ([`pulsar_pie_abi::control`]); 0 when no
/// game runs.
pub fn pie_control(command: u32, arg: u64) -> u64 {
    GAME.with(|g| g.borrow_mut().as_mut().map_or(0, |game| game.session.control(command, arg)))
}

/// Write the script problems raised since the last call (JSON array) into
/// `out` if they fit in `capacity` bytes, forgetting them; returns their
/// length (0 when there are none or no game runs).
///
/// # Safety
/// `out` must be valid for `capacity` bytes of writes, or `capacity` 0.
pub unsafe fn pie_take_problems(out: *mut u8, capacity: usize) -> usize {
    GAME.with(|g| {
        let mut game = g.borrow_mut();
        let Some(game) = game.as_mut() else { return 0 };
        if game.session.problems().is_empty() {
            return 0;
        }
        let Ok(json) = serde_json::to_vec(game.session.problems()) else { return 0 };
        if !out.is_null() && json.len() <= capacity {
            // SAFETY: `out` is valid for `capacity >= len` bytes (caller).
            unsafe { std::ptr::copy_nonoverlapping(json.as_ptr(), out, json.len()) };
            game.session.take_problems();
        }
        json.len()
    })
}

/// Write a `RawBus` exporting the session's event hub to `out` (#942).
/// Returns 1 on success, 0 when no game runs or `out_size` is not the size
/// of a `RawBus`.
///
/// # Safety
/// `out` must be valid for `out_size` bytes of writes.
pub unsafe fn pie_event_bus(out: *mut std::ffi::c_void, out_size: usize) -> u32 {
    use pulsar_events::gamma::ffi::RawBus;
    if out.is_null() || out_size != std::mem::size_of::<RawBus>() {
        return 0;
    }
    GAME.with(|g| {
        let game = g.borrow();
        let Some(game) = game.as_ref() else { return 0 };
        let raw = game.session.export_event_bus();
        // SAFETY: `out` holds a `RawBus` (size checked; caller-aligned).
        unsafe { std::ptr::write_unaligned(out as *mut RawBus, raw) };
        1
    })
}

/// Write the session's event hub debug snapshot (JSON) into `out` if it
/// fits in `capacity` bytes; returns its length (0 when no game runs).
///
/// # Safety
/// `out` must be valid for `capacity` bytes of writes, or `capacity` 0.
pub unsafe fn pie_events_snapshot(out: *mut u8, capacity: usize) -> usize {
    let json = GAME.with(|g| {
        g.borrow()
            .as_ref()
            .and_then(|game| serde_json::to_vec(&game.session.tick_loop.events.snapshot()).ok())
    });
    let Some(json) = json else { return 0 };
    if !out.is_null() && json.len() <= capacity {
        std::ptr::copy_nonoverlapping(json.as_ptr(), out, json.len());
    }
    json.len()
}

/// Tear down the embedded game, dropping its world + renderer before the host
/// unloads the library.
pub fn pie_shutdown() {
    GAME.with(|g| {
        if let Some(mut game) = g.borrow_mut().take() {
            game.log(LOG_INFO, "PiE game shutting down");
            // Scripts get end_play while the world still holds their
            // objects; subscriptions, queued events and timers go with them.
            game.session.shutdown();
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
