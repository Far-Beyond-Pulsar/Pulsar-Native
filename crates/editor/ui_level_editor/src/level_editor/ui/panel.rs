use gpui::*;
use rust_i18n::t;
use ui::{
    dock::{DockItem, Panel, PanelEvent},
    resizable::ResizableState,
    v_flex,
    workspace::Workspace,
};
// HelioViewport — GPUI-native Helio 3D viewport
use super::viewport::helio_viewport::HelioViewport;

use engine_backend::services::gpu_renderer::{GpuRenderer, GpuRendererBuilder};
use engine_fs::virtual_fs;
use std::path::Path;
use std::sync::{Arc, Mutex};
use ui::settings::EngineSettings;
use ui::{notification::Notification, ContextModal as _};

use crate::level_editor::state::PieStartRequest;

use super::actions::*;
use super::{toolbar, StatusBarView, ToolbarView, ViewportPanel};
use crate::ai_sessions;
use crate::level_editor::scene_database::{
    LevelEditorCameraState, LightType, MeshType, ObjectType, SceneObjectData, Transform,
};
use crate::level_editor::{request_thumbnail_capture, CameraMode, LevelEditorState, TransformTool};
use engine_backend::subsystems::render::{EditorCameraState, HelioEditorMailbox};
use plugin_manager;

/// Main Level Editor Panel - Orchestrates all sub-components
///
/// Invalidation model: this panel owns almost nothing that other panels need.
/// Every sub-panel (toolbar, status bar, hierarchy, properties, viewport) is
/// its own cached view with its own frame pump watching exactly the state it
/// renders, so scene edits invalidate only the panels that display them.
/// This panel re-renders only for what IT displays: the Play-In-Editor Game
/// tab, and the tab title (scene path / unsaved marker). See
/// `frame_pump`'s doc for why polling beats cross-panel notify cascades here.
pub struct LevelEditorPanel {
    focus_handle: FocusHandle,

    // UI Components. Both are separate entities rendered with `AnyView::cached`
    // so they survive the per-frame invalidation the viewport propagates up the
    // ancestor chain; each owns a frame pump that notifies it only when the
    // state it actually reads changes. See `toolbar::view` / `status_bar_view`.
    toolbar: Entity<ToolbarView>,
    status_bar: Entity<StatusBarView>,

    // Helio viewport rendered via WgpuSurfaceHandle
    viewport: Entity<HelioViewport>,
    gpu_engine: Arc<Mutex<GpuRenderer>>, // Full GPU renderer from backend
    // Cheap, `gpu_engine`-lock-free handle for one-shot editor commands
    // (gizmo mode, deselect, force-full-resync) -- see `HelioEditorMailbox`'s
    // doc. Fetched once at construction, not re-locked per command.
    helio_mailbox: Option<HelioEditorMailbox>,
    /// Voxel terrain edit seam, fetched once alongside `helio_mailbox`.
    /// Undo/redo and the save path use it; see `TerrainEditApi`.
    terrain_api: Option<engine_backend::services::terrain_edit::TerrainEditApi>,
    render_enabled: Arc<std::sync::atomic::AtomicBool>,

    // Shared state for all panels (single source of truth)
    shared_state: Arc<parking_lot::RwLock<LevelEditorState>>,

    // Workspace for draggable panels
    workspace: Option<Entity<Workspace>>,

    // Play In Editor (issue #243): the Game tab is opened when the game starts
    // and removed on stop. `game_panel` is the live tab entity, if open.
    game_panel: Option<Entity<crate::level_editor::ui::viewport::game_viewport::GameViewport>>,

    /// Last `(building, active, pending_start, has_error)` tuple that
    /// [`Self::sync_game_tab`] acted on. Render runs several times per second
    /// (viewport publishes dirty every ancestor); without this guard each of
    /// those renders would take a write lock on the shared state for nothing.
    applied_pie_signature: Option<(bool, bool, bool, bool)>,

    /// Last tool-mode id [`Self::sync_mode_layout`] reconciled the dock area
    /// against, guarding it the same way `applied_pie_signature` guards
    /// `sync_game_tab` — this runs on every render, so a plain `!=` check on
    /// a `Copy` id is what keeps it from touching the dock area for nothing.
    /// Because this only fires on an actual mode switch (a rare, deliberate
    /// user action), `sync_mode_layout` is free to fully rebuild the left
    /// dock's panel set each time rather than caching individual panel
    /// entities — unlike `sync_game_tab`, there is no per-render cost to
    /// avoid here.
    applied_mode_layout: Option<crate::level_editor::tool_modes::ToolModeId>,

    /// Mode-contributed right-dock panels currently inserted, by panel id.
    ///
    /// `sync_mode_layout` rebuilds the right dock only when this set actually
    /// changes between mode switches — the common case (modes contributing no
    /// right panels) shares the empty set, so the Properties/World Settings
    /// tab group and `PropertiesPanelWrapper`'s cached sections survive
    /// untouched. Only a mode that newly contributes right panels (or stops
    /// doing so) pays for a right-dock rebuild, and only on the switch itself.
    /// Left-dock contributions need no such tracking: the left dock is a full
    /// `set_left_dock` rebuild on every mode switch already.
    mode_right_panels: Vec<&'static str>,

    // Keeps the polling task alive for the lifetime of the panel.
    _root_input_poller: gpui::Task<()>,
}

impl LevelEditorPanel {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let panel = Self::new_internal(None, window, cx);
        Self::spawn_level_load(cx);
        panel
    }

    pub fn new_with_window_id(window_id: u64, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let panel = Self::new_internal(Some(window_id), window, cx);
        Self::spawn_level_load(cx);
        panel
    }

    /// Defers `ensure_default_level_file` to run after the editor window has
    /// rendered its first frame.
    ///
    /// By the time the loading screen closes, the scene directory already exists
    /// and the `default.level` file is in the OS page cache (both pre-warmed by
    /// the loading-screen background thread).  Even so, the actual deserialization
    /// happens here on the GPUI main thread — deferring it means the window
    /// becomes visible first, avoiding the "frozen / locked up" appearance on
    /// Windows.
    fn spawn_level_load(cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            // Yield once so the event loop can paint the first frame and make
            // the editor window visible before we touch disk.
            cx.background_executor()
                .timer(std::time::Duration::ZERO)
                .await;

            // Back on the GPUI main thread: load the level file. Panels pick
            // the contents up through their own frame pumps; this panel only
            // needs its title refreshed.
            cx.update(|cx| {
                this.update(cx, |panel, cx| {
                    panel.ensure_default_level_file();
                    cx.notify();
                });
            });
        })
        .detach();
    }

    /// If no project is open, do nothing. Otherwise resolve `<project>/scene/default.level`:
    /// - If the file already exists, load it.
    /// - If it doesn't exist, save the current in-memory default scene to it and
    ///   set `current_scene` so the title bar and save-as shortcuts work correctly.
    fn ensure_default_level_file(&mut self) {
        let Some(project_str) = engine_state::get_project_path() else {
            return;
        };
        let default_path = std::path::PathBuf::from(&project_str)
            .join("scene")
            .join("default.level");

        if let Some(parent) = default_path.parent() {
            if let Err(e) = engine_fs::virtual_fs::create_dir_all(parent) {
                tracing::warn!("Could not create default level directory {:?}: {e}", parent);
                return;
            }
        }

        let scene_db = { self.shared_state.read().scene.database.clone() };

        if default_path.exists() {
            // File already on disk — load it into the shared scene db.
            scene_db.clear();
            match scene_db.load_from_file_with_editor_camera(&default_path) {
                Ok(editor_camera) => {
                    self.apply_editor_camera_state(editor_camera.as_ref());
                    let mut w = self.shared_state.write();
                    w.scene.current_scene = Some(default_path);
                    w.scene.has_unsaved_changes = false;
                    w.scene.bump_revision(false);
                    if let Some(path) = w.scene.current_scene.clone() {
                        ai_sessions::register_open_scene(&path, &self.shared_state);
                    }
                }
                Err(e) => {
                    tracing::warn!("Default level exists but could not be loaded: {e}");
                    self.shared_state.write().scene.bump_revision(false);
                }
            }
        } else {
            // File does not exist — seed from the embedded default.level if available,
            // otherwise save the current in-memory (empty) scene to disk.
            let embedded = engine_state::EngineContext::global()
                .and_then(|ctx| ctx.store.get_or_init::<Option<Vec<u8>>>().read().clone());

            let seed_result = if let Some(bytes) = embedded {
                // Write the embedded bytes directly — preserves whatever the developer
                // designed as the default scene via "Save as Default Level".
                engine_fs::virtual_fs::write_file(&default_path, &bytes)
                    .map_err(|e| format!("Failed to write embedded default level: {e}"))
            } else {
                // No embedded asset yet — persist the current empty scene so the
                // path is stable for future saves.
                scene_db.save_to_file_with_editor_camera(
                    &default_path,
                    self.current_editor_camera_state(),
                    self.terrain_api.as_ref(),
                )
            };

            match seed_result {
                Ok(_) => {
                    // Load back what we just wrote so the editor shows the correct scene.
                    scene_db.clear();
                    match scene_db.load_from_file_with_editor_camera(&default_path) {
                        Ok(editor_camera) => {
                            self.apply_editor_camera_state(editor_camera.as_ref());
                            tracing::info!("Default level seeded at {:?}", default_path)
                        }
                        Err(e) => tracing::warn!("Seeded default level but reload failed: {e}"),
                    }
                    let mut w = self.shared_state.write();
                    w.scene.current_scene = Some(default_path);
                    w.scene.has_unsaved_changes = false;
                    w.scene.bump_revision(false);
                    if let Some(path) = w.scene.current_scene.clone() {
                        ai_sessions::register_open_scene(&path, &self.shared_state);
                    }
                }
                Err(e) => {
                    tracing::warn!("Could not create default level at {:?}: {e}", default_path);
                    self.shared_state.write().scene.bump_revision(false);
                }
            }
        }
    }

    /// Create the editor and immediately load a level file from disk.
    ///
    /// The scene is cleared and reloaded into the existing shared `Arc<SceneDb>`
    /// so the renderer stays in sync. Returns an error string on load failure
    /// (the panel is still valid and shows the default empty scene).
    pub fn new_with_path(
        path: std::path::PathBuf,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Result<Self, String> {
        let mut panel = Self::new_internal(None, window, cx);
        // Clear the default scene that was just populated, then load from file.
        let scene_db = { panel.shared_state.read().scene.database.clone() };
        scene_db.clear();
        let editor_camera = scene_db.load_from_file_with_editor_camera(&path)?;
        panel.apply_editor_camera_state(editor_camera.as_ref());
        {
            let mut state = panel.shared_state.write();
            state.scene.current_scene = Some(path);
            state.scene.has_unsaved_changes = false;
            state.editor.terrain_undo.clear();
            state.scene.bump_revision(false);
            if let Some(open_path) = state.scene.current_scene.clone() {
                ai_sessions::register_open_scene(&open_path, &panel.shared_state);
            }
        }
        Ok(panel)
    }

    fn new_internal(window_id: Option<u64>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let _horizontal_resizable_state = ResizableState::new(cx);
        let _vertical_resizable_state = ResizableState::new(cx);

        // Load engine settings for frame pacing configuration
        let settings = EngineSettings::default_path()
            .and_then(|path| Some(EngineSettings::load(&path)))
            .unwrap_or_default();

        let _max_viewport_fps = settings.advanced.max_viewport_fps;

        // Get the physics query service from the global EngineBackend
        let physics_query = engine_backend::EngineBackend::global()
            .and_then(|backend| backend.read().get_physics_query_service());

        // Construct editor state first: SceneDatabase owns the SceneDB-backed
        // store, and the renderer receives only its shared access handle.
        let state = LevelEditorState::new();
        let scene_store = state.scene.database.shared_store();

        // Create GPU render engine sharing the scene store Arc and physics query service
        let mut renderer_builder = GpuRendererBuilder::new(1600, 900).scene_db(scene_store.clone());
        if let Some(pq) = physics_query {
            renderer_builder = renderer_builder.physics(pq);
        }
        let gpu_engine = Arc::new(Mutex::new(renderer_builder.build()));
        let render_enabled = Arc::new(std::sync::atomic::AtomicBool::new(true));

        // Store GPU renderer in global EngineContext using a marker that the render loop will pick up
        // The render loop will associate it with the correct window when it first renders
        if let Some(engine_context) = engine_state::EngineContext::global() {
            if let Some(wid) = window_id {
                // We have the actual window ID - register directly!
                let handle = engine_state::TypedRendererHandle::helio(wid, gpu_engine.clone());
                engine_context.renderers.register(wid, handle);
            } else {
                // Fallback: Use a sentinel value (0) to mark this renderer as pending association with a window
                // The main render loop will detect windows with viewports and claim this renderer
                let handle = engine_state::TypedRendererHandle::helio(0, gpu_engine.clone());
                engine_context.renderers.register(0, handle);
            }
        }

        // SceneDatabase and HelioRenderer share the same store `Arc`, so every
        // add/remove/update made through SceneDatabase is visible to the
        // renderer's next sync pass without a separate write-through call.

        let shared_state = Arc::new(parking_lot::RwLock::new(state));

        // Temporary debug toggle: replace viewport with a solid yellow panel to
        // verify layout/overlap issues independently of GPU rendering.
        let debug_replace_with_yellow = false;

        // Create HelioViewport — renders via WgpuSurfaceHandle every GPUI frame.
        // It receives shared_state so viewport drop actions mutate SceneDatabase
        // through the same command path as the rest of the editor.
        let viewport = cx.new(|cx| {
            HelioViewport::new(
                gpu_engine.clone(),
                shared_state.clone(),
                debug_replace_with_yellow,
                cx,
            )
        });

        // Poll for changes to the inputs this panel itself displays, at
        // 50 ms intervals. Sub-panels do NOT ride on this loop — each has
        // its own frame pump watching its own signature, so a scene edit
        // invalidates only the views that actually render scene data.
        //
        // This loop covers exactly three things:
        // - Play-In-Editor state (set by the build thread / GameViewport):
        //   `sync_game_tab` opens/closes the Game tab. Applied on the next
        //   render, guarded by `applied_pie_signature`.
        // - Title inputs (`current_scene`, `has_unsaved_changes`): re-render
        //   so the dock's tab title/icon/unsaved marker update.
        // - Selection + tool: pushed to Helio here rather than in `render`,
        //   which used to take a renderer lock several times a second for a
        //   change that happens once per click.
        let poll_state = Arc::clone(&shared_state);
        let poll_gpu = gpu_engine.clone();
        let poller = cx.spawn(async move |this, cx| {
            let mut last: Option<(
                (bool, bool, bool, bool),
                (Option<std::path::PathBuf>, bool),
                (Option<String>, TransformTool),
            )> = None;
            loop {
                cx.background_executor()
                    .timer(std::time::Duration::from_millis(50))
                    .await;
                let snapshot = {
                    let s = poll_state.read();
                    (
                        (
                            s.play.pie.building,
                            s.play.pie.active,
                            s.play.pie.pending_start.is_some(),
                            s.play.pie.last_error.is_some(),
                        ),
                        (s.scene.current_scene.clone(), s.scene.has_unsaved_changes),
                        (s.scene.selected_object(), s.editor.current_tool),
                    )
                };

                let (pie_changed, title_changed, selection_changed) = match &last {
                    Some(last) => (
                        snapshot.0 != last.0,
                        snapshot.1 != last.1,
                        snapshot.2 != last.2,
                    ),
                    None => (true, true, true),
                };

                if selection_changed {
                    // Selection or tool changed: sync both into the renderer.
                    // `.try_lock()`, never a blocking lock — this runs on the
                    // UI thread; a missed tick is corrected by the next one.
                    if let Ok(mut engine) = poll_gpu.try_lock() {
                        let (_, tool) = &snapshot.2;
                        let (_, helio_mode) = Self::tool_to_gizmo(*tool);
                        engine.queue_gizmo_mode(helio_mode);
                        engine.sync_selection_to_helio();
                    }
                }

                if pie_changed || title_changed {
                    cx.update(|cx| {
                        this.update(cx, |_, cx| cx.notify());
                    });
                }

                last = Some(snapshot);
            }
        });

        let toolbar = cx.new(|_| ToolbarView::new(shared_state.clone(), gpu_engine.clone()));
        let status_bar = cx.new(|_| StatusBarView::new(shared_state.clone(), gpu_engine.clone()));

        // Fetched once here, not re-acquired via `gpu_engine.lock()` per
        // command -- see `HelioEditorMailbox`'s doc. `GpuRendererBuilder::build`
        // always sets `helio_renderer: Some(...)` synchronously, so this is
        // `Some` immediately after construction; `None` only if `gpu_engine`
        // somehow arrived pre-torn-down, which the `if let` call sites below
        // degrade out of harmlessly (same "skip this one tick" shape the old
        // `gpu_engine.lock()` sites already had on any lock failure).
        // Same "fetch once, never take `gpu_engine` again" contract as
        // `helio_mailbox` above -- see `TerrainEditApi`'s threading note.
        let (helio_mailbox, terrain_api) = match gpu_engine.lock().ok() {
            Some(engine) => (engine.editor_mailbox(), engine.terrain_edit_api()),
            None => (None, None),
        };

        Self {
            focus_handle: cx.focus_handle(),
            toolbar,
            status_bar,
            viewport,
            gpu_engine: gpu_engine.clone(),
            helio_mailbox,
            terrain_api,
            render_enabled,
            shared_state,
            workspace: None,
            game_panel: None,
            applied_pie_signature: None,
            applied_mode_layout: None,
            mode_right_panels: Vec::new(),
            _root_input_poller: poller,
        }
    }

    fn initialize_workspace(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.workspace.is_some() {
            return;
        }

        let workspace = cx.new(|cx| {
            Workspace::new_with_channel(
                "level-editor-workspace",
                ui::dock::DockChannel(3),
                window,
                cx,
            )
        });

        let shared_state = self.shared_state.clone();
        let gpu = self.gpu_engine.clone();
        let viewport = self.viewport.clone();
        let render_enabled = self.render_enabled.clone();

        workspace.update(cx, |workspace, cx| {
            let dock_area = workspace.dock_area().downgrade();

            // Create viewport in center
            let viewport_panel_inner =
                ViewportPanel::new(viewport.clone(), render_enabled.clone(), window, cx);
            let viewport_panel = cx.new(|cx| {
                use crate::level_editor::ViewportPanelWrapper;
                ViewportPanelWrapper::new(viewport_panel_inner, shared_state.clone(), gpu.clone(), cx)
            });

            // Create right dock panels
            let hierarchy_panel = cx.new(|cx| {
                use crate::level_editor::HierarchyPanelWrapper;
                HierarchyPanelWrapper::new(shared_state.clone(), window, cx)
            });
            let properties_panel = cx.new(|cx| {
                use crate::level_editor::PropertiesPanelWrapper;
                PropertiesPanelWrapper::new(shared_state.clone(), window, cx)
            });
            let world_settings_panel = cx.new(|cx| {
                use crate::level_editor::WorldSettingsPanel;
                WorldSettingsPanel::new(shared_state.clone(), window, cx)
            });

            // NOTE: Panels are self-invalidating — each owns a frame pump
            // that watches the state it renders (`frame_pump`, signatures in
            // `workspace::panels` / `toolbar::view` / `status_bar_view`).
            // There is deliberately no observe/notify wiring between panels
            // or from this panel to them; forwarding notifications here used
            // to turn every scene edit into a whole-tree invalidation.

            // Bottom right: tabs for Properties and World Settings
            let bottom_tabs = DockItem::tabs(
                vec![
                    std::sync::Arc::new(properties_panel)
                        as std::sync::Arc<dyn ui::dock::PanelView>,
                    std::sync::Arc::new(world_settings_panel)
                        as std::sync::Arc<dyn ui::dock::PanelView>,
                ],
                Some(0),
                &dock_area,
                window,
                cx,
            );

            // Top right: hierarchy panel (as a single-tab TabPanel)
            let top_hierarchy = DockItem::tabs(
                vec![std::sync::Arc::new(hierarchy_panel)
                    as std::sync::Arc<dyn ui::dock::PanelView>],
                Some(0),
                &dock_area,
                window,
                cx,
            );

            // Compose right dock as a vertical split: top = hierarchy (25%), bottom = tabs (75%)
            // Hierarchy gets smaller fixed size, Properties/World gets larger
            let right = ui::dock::DockItem::split_with_sizes(
                gpui::Axis::Vertical,
                vec![top_hierarchy, bottom_tabs],
                vec![Some(px(150.0)), Some(px(550.0))], // 150px hierarchy, 550px for Properties/World
                &dock_area,
                window,
                cx,
            );

            // Set center and right dock only (no left dock, matching DAW approach).
            // The Game tab (Play In Editor, issue #243) is added dynamically
            // when the game starts and removed on stop — see `sync_game_tab`.
            let center_tabs = DockItem::tabs(
                vec![std::sync::Arc::new(viewport_panel)
                    as std::sync::Arc<dyn ui::dock::PanelView>],
                Some(0),
                &dock_area,
                window,
                cx,
            );
            let _ = dock_area.update(cx, |dock_area, cx| {
                dock_area.set_center(center_tabs, window, cx);
                dock_area.set_right_dock(right, Some(px(400.0)), true, window, cx);
            });
        });

        self.workspace = Some(workspace);
    }

    /// Open the Play-In-Editor Game tab when the game starts (build finished /
    /// running) — `add_panel` auto-activates it, so it autofocuses — and remove
    /// it on stop. Also surfaces build errors, since the tab may not exist when a
    /// build fails.
    ///
    /// Called from `render`, but guarded: this panel is invalidated several
    /// times a second (every viewport publish dirties the ancestor chain), and
    /// without the check each of those renders would take a write lock on the
    /// shared state for nothing. The lock below is now taken once per actual
    /// PiE state transition.
    fn sync_game_tab(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        use crate::level_editor::ui::viewport::game_viewport::GameViewport;

        // Cheap read-only guard first: no PiE transition since we last acted.
        let current = self.pie_signature();
        if Some(current) == self.applied_pie_signature {
            return;
        }

        let (should_open, error) = {
            let mut st = self.shared_state.write();
            // Open once the build hands off a game to run (or it is already
            // running); close when neither is true.
            let should_open = st.play.pie.active || st.play.pie.pending_start.is_some();
            let error = st.play.pie.last_error.take();
            (should_open, error)
        };

        if let Some(err) = error {
            window.push_notification(
                Notification::error(t!("Notification.Title.PlayInEditorBuildFailed").to_string())
                    .message(err),
                cx,
            );
        }

        let Some(workspace) = self.workspace.clone() else {
            return;
        };

        if should_open && self.game_panel.is_none() {
            let shared = self.shared_state.clone();
            let game_panel = cx.new(|cx| GameViewport::new(shared, cx));
            self.game_panel = Some(game_panel.clone());
            let panel_view: std::sync::Arc<dyn ui::dock::PanelView> =
                std::sync::Arc::new(game_panel);
            workspace.update(cx, |ws, cx| {
                let dock_area = ws.dock_area().clone();
                dock_area.update(cx, |da, cx| {
                    da.add_panel_to_center(panel_view, window, cx);
                });
            });
        } else if !should_open {
            if let Some(game_panel) = self.game_panel.take() {
                let panel_view: std::sync::Arc<dyn ui::dock::PanelView> =
                    std::sync::Arc::new(game_panel);
                workspace.update(cx, |ws, cx| {
                    let dock_area = ws.dock_area().clone();
                    dock_area.update(cx, |da, cx| {
                        da.items().remove_panel(panel_view, window, cx);
                    });
                });
            }
        }

        // Record AFTER acting: `last_error` was taken above, so the fresh
        // tuple differs from the pre-act snapshot whenever an error was
        // consumed, and storing the pre-take value would loop.
        self.applied_pie_signature = Some(self.pie_signature());
    }

    /// Reconcile the dock area against the active tool mode's [`ModeLayout`].
    ///
    /// Same shape as [`Self::sync_game_tab`]: a cheap signature guard first
    /// (render runs several times a second), then act only on a real change
    /// — i.e. an actual mode switch, a rare, deliberate user action.
    ///
    /// The right dock is rebuilt *only* when the set of mode-contributed right
    /// panels changes (`self.mode_right_panels`) — that dock holds
    /// `PropertiesPanelWrapper`'s cached section entities, and tearing them
    /// down on every switch would needlessly discard the cache. The left dock
    /// is the inverse: each [`PanelTab`](crate::level_editor::tool_modes::PanelTab)
    /// the active mode returns becomes its own real dock panel
    /// (`ModeToolsPanel`), grouped into the left dock's native tab strip via
    /// `DockItem::tabs`, and any panels the mode contributes to the left dock
    /// (`contributes_panels` + `build_panel`) join that same strip — the same
    /// mechanism the right dock already uses for Properties/World Settings —
    /// so switching modes rebuilds *which panels exist*, not just whether
    /// they're visible. That's a full `set_left_dock` every time a mode with
    /// `show_mode_panel: true` becomes active, but since this only runs on a
    /// mode switch (not every render), the cost `PropertiesPanelWrapper`'s
    /// caching exists to avoid does not apply here.
    ///
    /// Mode-contributed panels are created here, on the switch, from the
    /// mode's `build_panel` — the "full GPUI in a tool mode" contract — typed
    /// only by their descriptors, placed left or right, and torn down
    /// wholesale when the mode stops contributing them (left: implicit in the
    /// `set_left_dock` rebuild; right: gated on `mode_right_panels`).
    ///
    /// [`ModeLayout`]: crate::level_editor::tool_modes::ModeLayout
    fn sync_mode_layout(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        use crate::level_editor::tool_modes::ModePanelPlacement;
        use ui::dock::{DockItem, DockPlacement};

        let current = self.shared_state.read().editor.tool_mode_registry.selected_id();
        if Some(current) == self.applied_mode_layout {
            return;
        }

        let (layout, tabs, mode) = {
            let state = self.shared_state.read();
            let layout = state.editor.tool_mode_registry.selected().layout();
            let tabs = crate::level_editor::ui::mode_widgets::active_mode_tabs(
                &state,
                &self.gpu_engine,
            );
            // Clone the mode up so the read lock can be dropped before any
            // `build_panel` call — gpui view construction must not happen
            // while the shared state is locked.
            let mode = state.editor.tool_mode_registry.selected().clone_box();
            (layout, tabs, mode)
        };

        // Build the mode's own panels up front, outside the workspace lock.
        // `&mut Context<Self>` derefs to `&mut App`, so it satisfies
        // `build_panel`'s app parameter directly. Right ids are tracked so a
        // subsequent switch can tear the panels down (or keep them) without
        // rebuilding the right dock every time.
        let mut left_contributions: Vec<std::sync::Arc<dyn ui::dock::PanelView>> = Vec::new();
        let mut right_contributions: Vec<std::sync::Arc<dyn ui::dock::PanelView>> = Vec::new();
        let mut right_ids: Vec<&'static str> = Vec::new();
        for desc in mode.contributes_panels() {
            if let Some(view) = mode.build_panel(self.shared_state.clone(), &desc, window, cx) {
                let view: std::sync::Arc<dyn ui::dock::PanelView> = std::sync::Arc::from(view);
                match desc.placement {
                    ModePanelPlacement::Right => {
                        right_ids.push(desc.id);
                        right_contributions.push(view);
                    }
                    ModePanelPlacement::Left => left_contributions.push(view),
                }
            }
        }

        // Rebuild the right dock only when the contributed set actually
        // changed — the common case (both empty) shares the equal vec, so the
        // Properties/World Settings tab group and its cached sections survive
        // untouched. Transitions to *and* from a right-paneled mode both read
        // as inequality, so the teardown-shape rebuild is covered too.
        let right_needs_rebuild = right_ids != self.mode_right_panels;

        let Some(workspace) = self.workspace.clone() else {
            return;
        };
        let shared_state = self.shared_state.clone();
        let gpu_engine = self.gpu_engine.clone();

        workspace.update(cx, |ws, cx| {
            let dock_area = ws.dock_area().clone();
            let dock_area_weak = dock_area.downgrade();

            let right_open = dock_area.read(cx).is_dock_open(DockPlacement::Right, cx);
            if right_open != layout.show_right_dock {
                dock_area.update(cx, |da, cx| {
                    da.toggle_dock(DockPlacement::Right, window, cx);
                });
            }

            // Right dock: rebuild only when the contributed set changed. The
            // rebuild reproduces `initialize_workspace`'s vertical split
            // (hierarchy top, Properties/World Settings bottom) with any
            // right-mode panels folded into the bottom tab group, surfaced
            // first so the mode's own panel is the one that activates. An
            // empty contribution set yields the default layout — i.e. the
            // teardown shape after a right-paneled mode.
            if right_needs_rebuild {
                let hierarchy_panel = cx.new(|cx| {
                    crate::level_editor::HierarchyPanelWrapper::new(
                        shared_state.clone(),
                        window,
                        cx,
                    )
                });
                let properties_panel = cx.new(|cx| {
                    crate::level_editor::PropertiesPanelWrapper::new(
                        shared_state.clone(),
                        window,
                        cx,
                    )
                });
                let world_settings_panel = cx.new(|cx| {
                    crate::level_editor::WorldSettingsPanel::new(
                        shared_state.clone(),
                        window,
                        cx,
                    )
                });

                let mut bottom_views: Vec<std::sync::Arc<dyn ui::dock::PanelView>> =
                    right_contributions; // mode's own panels first
                bottom_views.push(std::sync::Arc::new(properties_panel));
                bottom_views.push(std::sync::Arc::new(world_settings_panel));

                let bottom_tabs = DockItem::tabs(
                    bottom_views,
                    Some(0),
                    &dock_area_weak,
                    window,
                    cx,
                );
                let top_hierarchy = DockItem::tabs(
                    vec![std::sync::Arc::new(hierarchy_panel)
                        as std::sync::Arc<dyn ui::dock::PanelView>],
                    Some(0),
                    &dock_area_weak,
                    window,
                    cx,
                );
                let right = DockItem::split_with_sizes(
                    gpui::Axis::Vertical,
                    vec![top_hierarchy, bottom_tabs],
                    vec![Some(px(150.0)), Some(px(550.0))],
                    &dock_area_weak,
                    window,
                    cx,
                );
                dock_area.update(cx, |da, cx| {
                    da.set_right_dock(right, Some(px(400.0)), true, window, cx);
                });
            }

            // Left dock: contributions join the ModeToolsPanel strips in one
            // native tab group. A full rebuild here is fine — it only happens
            // on a mode switch.
            let show_left = (layout.show_mode_panel && !tabs.is_empty())
                || !left_contributions.is_empty();
            if show_left {
                let panels: Vec<std::sync::Arc<dyn ui::dock::PanelView>> = {
                    let mut panels = tabs
                        .iter()
                        .map(|tab| {
                            let panel = cx.new(|cx| {
                                crate::level_editor::ModeToolsPanel::new(
                                    shared_state.clone(),
                                    gpu_engine.clone(),
                                    tab.id,
                                    tab.label_key,
                                    cx,
                                )
                            });
                            std::sync::Arc::new(panel)
                                as std::sync::Arc<dyn ui::dock::PanelView>
                        })
                        .collect::<Vec<_>>();
                    panels.extend(left_contributions);
                    panels
                };

                let item = DockItem::tabs(panels, Some(0), &dock_area_weak, window, cx);
                dock_area.update(cx, |da, cx| {
                    da.set_left_dock(item, Some(px(280.0)), true, window, cx);
                });
            } else {
                let left_open = dock_area.read(cx).is_dock_open(DockPlacement::Left, cx);
                if left_open {
                    dock_area.update(cx, |da, cx| {
                        da.toggle_dock(DockPlacement::Left, window, cx);
                    });
                }
            }
        });

        self.applied_mode_layout = Some(current);
        self.mode_right_panels = right_ids;
    }

    /// Cheap read-only snapshot of the Play-In-Editor state this panel acts on.
    fn pie_signature(&self) -> (bool, bool, bool, bool) {
        let st = self.shared_state.read();
        (
            st.play.pie.building,
            st.play.pie.active,
            st.play.pie.pending_start.is_some(),
            st.play.pie.last_error.is_some(),
        )
    }

    pub fn toggle_rendering(&mut self) {
        let current = self
            .render_enabled
            .load(std::sync::atomic::Ordering::Relaxed);
        self.render_enabled
            .store(!current, std::sync::atomic::Ordering::Relaxed);
    }

    fn tool_to_gizmo(
        tool: TransformTool,
    ) -> (engine_backend::scene::GizmoType, engine_backend::GizmoMode) {
        use engine_backend::scene::GizmoType as SceneGizmoType;
        use engine_backend::GizmoMode;
        match tool {
            TransformTool::Select => (SceneGizmoType::None, GizmoMode::Translate),
            TransformTool::Move => (SceneGizmoType::Translate, GizmoMode::Translate),
            TransformTool::Rotate => (SceneGizmoType::Rotate, GizmoMode::Rotate),
            TransformTool::Scale => (SceneGizmoType::Scale, GizmoMode::Scale),
        }
    }

    fn sync_gizmo_to_helio(&mut self) {
        let tool = self.shared_state.read().editor.current_tool;
        let (_, helio_mode) = Self::tool_to_gizmo(tool);
        // Mailbox, not `gpu_engine.lock()` -- fires on every tool hotkey
        // press, frequent enough that a dropped tick (the old blocking-lock
        // site's failure mode when contended) would be visibly janky. See
        // `HelioEditorMailbox`'s doc.
        if let Some(mailbox) = &self.helio_mailbox {
            mailbox.queue_gizmo(helio_mode);
        }
    }

    fn queue_gizmo_mode_for_tool(&mut self, tool: TransformTool) {
        let (_, helio_mode) = Self::tool_to_gizmo(tool);
        if let Some(mailbox) = &self.helio_mailbox {
            mailbox.queue_gizmo(helio_mode);
        }
    }

    fn current_editor_camera_state(&self) -> Option<LevelEditorCameraState> {
        // `.try_lock()`, not `.lock()`: camera pose is continuously-mutated
        // state (touched every frame by the render thread's own camera-input
        // handling), not a one-shot command, so it doesn't fit the mailbox
        // shape -- but this call site is load/save/construction only, never
        // per-frame and never on the drag path, so a rare missed read here
        // (falls through to `None`, already handled by every caller) is a
        // fine tradeoff against ever blocking the UI thread on it.
        self.gpu_engine
            .try_lock()
            .ok()
            .and_then(|engine| engine.editor_camera_state())
            .map(|camera| LevelEditorCameraState {
                position: camera.position,
                yaw: camera.yaw,
                pitch: camera.pitch,
            })
    }

    fn apply_editor_camera_state(&mut self, camera: Option<&LevelEditorCameraState>) {
        let Some(camera) = camera else {
            return;
        };

        // Same `.try_lock()` reasoning as `current_editor_camera_state`.
        if let Ok(mut engine) = self.gpu_engine.try_lock() {
            engine.set_editor_camera_state(EditorCameraState {
                position: camera.position,
                yaw: camera.yaw,
                pitch: camera.pitch,
            });
        } else {
            tracing::debug!("[CAMERA_STATE] gpu_engine busy, skipped applying camera state");
        }
    }

    // Action handlers
    fn on_select_tool(&mut self, _: &SelectTool, _: &mut Window, cx: &mut Context<Self>) {
        self.shared_state
            .write()
            .editor
            .set_tool(TransformTool::Select);
        self.queue_gizmo_mode_for_tool(TransformTool::Select);
        cx.notify();
    }

    fn on_move_tool(&mut self, _: &MoveTool, _: &mut Window, cx: &mut Context<Self>) {
        self.shared_state
            .write()
            .editor
            .set_tool(TransformTool::Move);
        self.queue_gizmo_mode_for_tool(TransformTool::Move);
        cx.notify();
    }

    fn on_rotate_tool(&mut self, _: &RotateTool, _: &mut Window, cx: &mut Context<Self>) {
        self.shared_state
            .write()
            .editor
            .set_tool(TransformTool::Rotate);
        self.queue_gizmo_mode_for_tool(TransformTool::Rotate);
        cx.notify();
    }

    fn on_scale_tool(&mut self, _: &ScaleTool, _: &mut Window, cx: &mut Context<Self>) {
        self.shared_state
            .write()
            .editor
            .set_tool(TransformTool::Scale);
        self.queue_gizmo_mode_for_tool(TransformTool::Scale);
        cx.notify();
    }

    // Toolbar action handlers
    fn on_set_time_scale(
        &mut self,
        action: &toolbar::SetTimeScale,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.shared_state.write().play.time_scale = action.0;
        cx.notify();
    }

    fn on_set_multiplayer_mode(
        &mut self,
        action: &toolbar::SetMultiplayerMode,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.shared_state.write().play.multiplayer_mode = action.0;
        cx.notify();
    }

    fn on_set_build_config(
        &mut self,
        action: &toolbar::SetBuildConfig,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.shared_state.write().build.config = action.0;
        cx.notify();
    }

    fn on_set_target_platform(
        &mut self,
        action: &toolbar::SetTargetPlatform,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.shared_state.write().build.target_platform = action.0;
        cx.notify();
    }

    fn on_set_build_mode(
        &mut self,
        action: &toolbar::SetBuildMode,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.shared_state.write().build.mode = action.0;
        cx.notify();
    }

    fn on_set_tool_mode(
        &mut self,
        action: &toolbar::SetToolMode,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // One locked pass for both the camera and the terrain seam: the mode
        // lifecycle hooks need the seam to close any open stroke / clear the
        // brush ring as they hand over.
        let (camera, terrain_api) = {
            let engine = self.gpu_engine.lock().ok();
            match engine {
                Some(engine) => (engine.editor_camera_state(), engine.terrain_edit_api()),
                None => (None, None),
            }
        };
        let camera = camera
            .map(|c| crate::level_editor::tool_modes::CameraFrame {
                position: c.position,
                yaw: c.yaw,
                pitch: c.pitch,
                fov: 60.0,
            })
            .unwrap_or_default();
        let mut state = self.shared_state.write();
        crate::level_editor::tool_modes::ToolModeDispatcher::select_tool_mode(
            &mut state,
            &self.gpu_engine,
            terrain_api.as_ref(),
            action.0,
            camera,
            crate::level_editor::tool_modes::ViewportFrame::default(),
        );
        drop(state);
        cx.notify();
    }

    fn on_add_object(&mut self, _: &AddObject, _: &mut Window, cx: &mut Context<Self>) {
        use crate::level_editor::commands::{execute_command, SceneCommand};
        let mut state = self.shared_state.write();
        execute_command(
            &mut state,
            SceneCommand::AddObject {
                data: SceneObjectData {
                    id: String::new(),
                    name: "New Object".to_string(),
                    object_type: ObjectType::Empty,
                    transform: Transform::default(),
                    visible: true,
                    locked: false,
                    parent: None,
                    children: vec![],
                    scene_path: String::new(),
                    props: Default::default(),
                    component_instances: None,
                },
                parent_id: None,
            },
        );
        drop(state);
        cx.notify();
    }

    fn on_add_object_of_type(
        &mut self,
        action: &AddObjectOfType,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        use crate::level_editor::commands::{execute_command, SceneCommand};
        let mut state = self.shared_state.write();
        execute_command(
            &mut state,
            SceneCommand::AddObject {
                data: SceneObjectData {
                    id: String::new(),
                    name: format!("New {}", action.object_type),
                    object_type: ObjectType::Empty,
                    transform: Transform::default(),
                    visible: true,
                    locked: false,
                    parent: None,
                    children: vec![],
                    scene_path: String::new(),
                    props: Default::default(),
                    component_instances: None,
                },
                parent_id: None,
            },
        );
        drop(state);
        cx.notify();
    }

    fn on_delete_object(&mut self, _: &DeleteObject, _: &mut Window, cx: &mut Context<Self>) {
        use crate::level_editor::commands::{execute_command, SceneCommand};
        let selected = self.shared_state.read().scene.selected_object();
        if let Some(id) = selected {
            let mut state = self.shared_state.write();
            execute_command(&mut state, SceneCommand::RemoveObject { id });
            drop(state);
            self.sync_gizmo_to_helio();
        }
        cx.notify();
    }

    fn on_duplicate_object(&mut self, _: &DuplicateObject, _: &mut Window, cx: &mut Context<Self>) {
        use crate::level_editor::commands::{execute_command, SceneCommand};
        let selected = self.shared_state.read().scene.selected_object();
        if let Some(id) = selected {
            let mut state = self.shared_state.write();
            execute_command(
                &mut state,
                SceneCommand::DuplicateObject {
                    source_id: id,
                    count: 1,
                    position_offset: None,
                },
            );
        }
        cx.notify();
    }

    /// Undo the last mutating scene command (Pulsar-Native#554). Restoring
    /// replaces `WorldSceneStore` wholesale, so the renderer's delta-sync
    /// can't diff against it correctly -- `force_full_resync` is required
    /// here, not optional; see its doc for why.
    fn on_undo(&mut self, _: &super::actions::Undo, _: &mut Window, cx: &mut Context<Self>) {
        // Voxel edits do not live in the scene database, so they have their
        // own history (design doc §5.5). While the Terrain mode is active and
        // that history has something in it, Ctrl+Z means "undo my sculpt
        // stroke" -- one whole stroke, never one stamp.
        if self.undo_terrain_stroke() {
            cx.notify();
            return;
        }
        let mut state = self.shared_state.write();
        if state.scene.undo() {
            state.scene.bump_revision(true);
            drop(state);
            // Mailbox, not `gpu_engine.lock()`: this must never be silently
            // dropped the way a lost `try_lock()`/blocked `.lock()` could --
            // see `pending_force_full_resync`'s doc.
            if let Some(mailbox) = &self.helio_mailbox {
                mailbox.queue_force_full_resync();
            }
            self.sync_gizmo_to_helio();
        }
        cx.notify();
    }

    /// Whether Ctrl+Z/Ctrl+Y should be read as a terrain operation.
    ///
    /// Only while the Terrain tool mode is selected: a user who has switched
    /// back to Level Edit is undoing object edits, and silently rewinding
    /// their voxels instead would be a nasty surprise. Interleaving the two
    /// histories into one ordered timeline is the better long-term answer and
    /// is called out as an open question in the design doc (§9, "Undo
    /// pairing"); this keeps the two unambiguous until that is settled.
    fn terrain_history_owns_undo(&self) -> bool {
        use crate::level_editor::tool_modes::ToolModeId;
        self.shared_state.read().editor.tool_mode_registry.selected_id() == ToolModeId::TERRAIN
    }

    /// Revert one full sculpt stroke. Returns `false` when terrain has no
    /// history to give, so the caller falls through to scene undo.
    fn undo_terrain_stroke(&mut self) -> bool {
        if !self.terrain_history_owns_undo() {
            return false;
        }
        let Some(api) = self.terrain_api.clone() else {
            return false;
        };
        let mut state = self.shared_state.write();
        if !state.editor.terrain_undo.can_undo() {
            return false;
        }
        match state.editor.terrain_undo.undo(&api) {
            Ok(changed) => changed,
            Err(error) => {
                tracing::error!(%error, "terrain undo failed");
                false
            }
        }
    }

    /// Reapply one full sculpt stroke. See [`Self::undo_terrain_stroke`].
    fn redo_terrain_stroke(&mut self) -> bool {
        if !self.terrain_history_owns_undo() {
            return false;
        }
        let Some(api) = self.terrain_api.clone() else {
            return false;
        };
        let mut state = self.shared_state.write();
        if !state.editor.terrain_undo.can_redo() {
            return false;
        }
        match state.editor.terrain_undo.redo(&api) {
            Ok(changed) => changed,
            Err(error) => {
                tracing::error!(%error, "terrain redo failed");
                false
            }
        }
    }

    /// Redo the last undone scene command. See [`Self::on_undo`]'s doc.
    fn on_redo(&mut self, _: &super::actions::Redo, _: &mut Window, cx: &mut Context<Self>) {
        if self.redo_terrain_stroke() {
            cx.notify();
            return;
        }
        let mut state = self.shared_state.write();
        if state.scene.redo() {
            state.scene.bump_revision(true);
            drop(state);
            if let Some(mailbox) = &self.helio_mailbox {
                mailbox.queue_force_full_resync();
            }
            self.sync_gizmo_to_helio();
        }
        cx.notify();
    }

    fn on_select_object(&mut self, action: &SelectObject, _: &mut Window, cx: &mut Context<Self>) {
        self.shared_state
            .write()
            .scene
            .select_object(Some(action.object_id.clone()));
        self.sync_gizmo_to_helio(); // Sync gizmo to follow selected object
        cx.notify();
    }

    fn on_toggle_object_expanded(
        &mut self,
        action: &ToggleObjectExpanded,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.shared_state
            .write()
            .hierarchy
            .toggle_object_expanded(&action.object_id);
        cx.notify();
    }

    fn on_toggle_grid(&mut self, _: &ToggleGrid, _: &mut Window, cx: &mut Context<Self>) {
        self.shared_state.write().editor.toggle_grid();
        cx.notify();
    }

    fn on_toggle_wireframe(&mut self, _: &ToggleWireframe, _: &mut Window, cx: &mut Context<Self>) {
        self.shared_state.write().editor.toggle_wireframe();
        cx.notify();
    }

    fn on_toggle_lighting(&mut self, _: &ToggleLighting, _: &mut Window, cx: &mut Context<Self>) {
        self.shared_state.write().editor.toggle_lighting();
        cx.notify();
    }

    fn on_toggle_performance_overlay(
        &mut self,
        _: &TogglePerformanceOverlay,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.shared_state
            .write()
            .overlays
            .toggle_performance_overlay();
        cx.notify();
    }

    fn on_toggle_camera_mode_selector(
        &mut self,
        _: &ToggleCameraModeSelector,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.shared_state
            .write()
            .overlays
            .toggle_camera_mode_selector();
        cx.notify();
    }

    fn on_toggle_viewport_options(
        &mut self,
        _: &ToggleViewportOptions,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.shared_state.write().overlays.toggle_viewport_options();
        cx.notify();
    }

    fn on_toggle_fps_graph_type(
        &mut self,
        _: &ToggleFpsGraphType,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.shared_state.write().overlays.toggle_fps_graph_type();
        cx.notify();
    }

    // Performance metrics toggles
    fn on_toggle_fps_graph(&mut self, _: &ToggleFpsGraph, _: &mut Window, cx: &mut Context<Self>) {
        self.shared_state.write().overlays.toggle_fps_graph();
        cx.notify();
    }

    fn on_toggle_tps_graph(&mut self, _: &ToggleTpsGraph, _: &mut Window, cx: &mut Context<Self>) {
        self.shared_state.write().overlays.toggle_tps_graph();
        cx.notify();
    }

    fn on_toggle_frame_time_graph(
        &mut self,
        _: &ToggleFrameTimeGraph,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.shared_state.write().overlays.toggle_frame_time_graph();
        cx.notify();
    }

    fn on_toggle_memory_graph(
        &mut self,
        _: &ToggleMemoryGraph,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.shared_state.write().overlays.toggle_memory_graph();
        cx.notify();
    }

    fn on_toggle_draw_calls_graph(
        &mut self,
        _: &ToggleDrawCallsGraph,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.shared_state.write().overlays.toggle_draw_calls_graph();
        cx.notify();
    }

    fn on_toggle_vertices_graph(
        &mut self,
        _: &ToggleVerticesGraph,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.shared_state.write().overlays.toggle_vertices_graph();
        cx.notify();
    }

    fn on_toggle_input_latency_graph(
        &mut self,
        _: &ToggleInputLatencyGraph,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.shared_state
            .write()
            .overlays
            .toggle_input_latency_graph();
        cx.notify();
    }

    fn on_toggle_ui_consistency_graph(
        &mut self,
        _: &ToggleUiConsistencyGraph,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.shared_state
            .write()
            .overlays
            .toggle_ui_consistency_graph();
        cx.notify();
    }

    fn on_play_scene(&mut self, _: &PlayScene, window: &mut Window, cx: &mut Context<Self>) {
        begin_pie(self.shared_state.clone(), window, cx);
        self.sync_gizmo_to_helio();
        cx.notify();
    }

    fn on_stop_scene(&mut self, _: &StopScene, _: &mut Window, cx: &mut Context<Self>) {
        end_pie(self.shared_state.clone());
        self.sync_gizmo_to_helio();
        cx.notify();
    }

    fn on_perspective_view(&mut self, _: &PerspectiveView, _: &mut Window, cx: &mut Context<Self>) {
        self.shared_state
            .write()
            .editor
            .set_camera_mode(CameraMode::Perspective);
        cx.notify();
    }

    fn on_orthographic_view(
        &mut self,
        _: &OrthographicView,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.shared_state
            .write()
            .editor
            .set_camera_mode(CameraMode::Orthographic);
        cx.notify();
    }

    fn on_top_view(&mut self, _: &TopView, _: &mut Window, cx: &mut Context<Self>) {
        self.shared_state
            .write()
            .editor
            .set_camera_mode(CameraMode::Top);
        cx.notify();
    }

    fn on_front_view(&mut self, _: &FrontView, _: &mut Window, cx: &mut Context<Self>) {
        self.shared_state
            .write()
            .editor
            .set_camera_mode(CameraMode::Front);
        cx.notify();
    }

    fn on_side_view(&mut self, _: &SideView, _: &mut Window, cx: &mut Context<Self>) {
        self.shared_state
            .write()
            .editor
            .set_camera_mode(CameraMode::Side);
        cx.notify();
    }

    fn on_save_scene(&mut self, _: &SaveScene, _: &mut Window, cx: &mut Context<Self>) {
        // If no current scene path, do Save As
        if self.shared_state.read().scene.current_scene.is_none() {
            cx.dispatch_action(&SaveSceneAs);
            return;
        }

        let (scene_db, path_opt) = {
            let state = self.shared_state.read();
            (
                state.scene.database.clone(),
                state.scene.current_scene.clone(),
            )
        };

        if let Some(path) = path_opt {
            match scene_db.save_to_file_with_editor_camera(
                &path,
                self.current_editor_camera_state(),
                self.terrain_api.as_ref(),
            ) {
                Ok(_) => {
                    self.shared_state.write().scene.has_unsaved_changes = false;
                    request_thumbnail_capture(&self.shared_state);
                    cx.notify();
                }
                Err(e) => {}
            }
        }
    }

    fn on_save_scene_as(&mut self, _: &SaveSceneAs, _window: &mut Window, cx: &mut Context<Self>) {
        let state_arc = self.shared_state.clone();
        let scene_db = { state_arc.read().scene.database.clone() };
        let editor_camera = self.current_editor_camera_state();
        // Cloned into the async task: `self` is not available once the file
        // dialog await resumes.
        let terrain_api = self.terrain_api.clone();
        let dialog = rfd::AsyncFileDialog::new()
            .set_title("Save Scene As")
            .add_filter("Level file", &["level", "json"])
            .set_file_name("untitled.level");
        cx.spawn(async move |_this, cx| {
            if let Some(handle) = dialog.save_file().await {
                let path = handle.path().to_path_buf();
                let result = scene_db.save_to_file_with_editor_camera(
                    &path,
                    editor_camera,
                    terrain_api.as_ref(),
                );
                cx.update(|cx| {
                    _this.update(cx, |_, cx| {
                        match result {
                            Ok(_) => {
                                let previous = state_arc.write().scene.current_scene.clone();
                                if let Some(prev) = previous {
                                    ai_sessions::unregister_open_scene(&prev);
                                }
                                state_arc.write().scene.current_scene = Some(path);
                                state_arc.write().scene.has_unsaved_changes = false;
                                request_thumbnail_capture(&state_arc);
                                if let Some(open_path) =
                                    state_arc.read().scene.current_scene.clone()
                                {
                                    ai_sessions::register_open_scene(&open_path, &state_arc);
                                }
                            }
                            Err(e) => tracing::error!("Save failed: {}", e),
                        }
                        cx.notify();
                    });
                });
            }
        })
        .detach();
    }

    fn on_open_scene(&mut self, _: &OpenScene, _window: &mut Window, cx: &mut Context<Self>) {
        let state_arc = self.shared_state.clone();
        let scene_db = { state_arc.read().scene.database.clone() };
        let default_dir = state_arc
            .read()
            .scene
            .current_scene
            .as_ref()
            .and_then(|p| p.parent().map(|d| d.to_path_buf()))
            .unwrap_or_else(|| std::env::current_dir().unwrap_or_default());
        let dialog = rfd::AsyncFileDialog::new()
            .set_title("Open Scene")
            .add_filter("Level file", &["level", "json"])
            .set_directory(default_dir);
        cx.spawn(async move |this, cx| {
            if let Some(handle) = dialog.pick_file().await {
                let path = handle.path().to_path_buf();
                // Load into the existing shared SceneDb (renderer keeps its Arc).
                let result = scene_db.load_from_file_with_editor_camera(&path);
                cx.update(|cx| {
                    this.update(cx, |this, cx| {
                        match result {
                            Ok(editor_camera) => {
                                this.apply_editor_camera_state(editor_camera.as_ref());
                                let mut state = state_arc.write();
                                if let Some(prev) = state.scene.current_scene.clone() {
                                    ai_sessions::unregister_open_scene(&prev);
                                }
                                state.scene.current_scene = Some(path);
                                state.scene.has_unsaved_changes = false;
                                // Deselect so properties panel clears stale data.
                                state.scene.select_object(None);
                                // Terrain history anchors snapshots of the
                                // *previous* level's planets; keeping them
                                // would offer undos that can only fail.
                                state.editor.terrain_undo.clear();
                                // Flat worlds created in the previous level
                                // must not follow the user into this one.
                                if let Some(api) = this.terrain_api.as_ref() {
                                    api.clear_authored_volumes();
                                }
                                if let Some(open_path) = state.scene.current_scene.clone() {
                                    ai_sessions::register_open_scene(&open_path, &state_arc);
                                }
                            }
                            Err(e) => tracing::error!("Open scene failed: {}", e),
                        }
                        cx.notify();
                    });
                });
            }
        })
        .detach();
    }

    fn on_new_scene(&mut self, _: &NewScene, _: &mut Window, cx: &mut Context<Self>) {
        // Warn if unsaved changes (TODO: modal dialog)
        // Clear the scene IN-PLACE so the renderer keeps its Arc<SceneDb>.
        let scene_db = { self.shared_state.read().scene.database.clone() };
        let mut editor_camera = None;
        scene_db.clear();

        // Load from the embedded default.level if available, otherwise start empty.
        if let Some(bytes) = engine_state::EngineContext::global()
            .and_then(|ctx| ctx.store.get_or_init::<Option<Vec<u8>>>().read().clone())
        {
            let tmp = std::env::temp_dir().join("pulsar_new_scene_seed.level");
            if engine_fs::virtual_fs::write_file(&tmp, &bytes).is_ok() {
                match scene_db.load_from_file_with_editor_camera(&tmp) {
                    Ok(loaded_camera) => editor_camera = loaded_camera,
                    Err(e) => {
                        tracing::warn!("New scene: could not load embedded default.level: {e}")
                    }
                }
            }
        }
        self.apply_editor_camera_state(editor_camera.as_ref());
        {
            let mut state = self.shared_state.write();
            if let Some(prev) = state.scene.current_scene.clone() {
                ai_sessions::unregister_open_scene(&prev);
            }
            state.scene.current_scene = None;
            state.scene.has_unsaved_changes = false;
            // Deselect so properties panel clears stale data.
            state.scene.select_object(None);
        }
        cx.notify();
    }

    fn on_focus_selected(
        &mut self,
        _: &FocusSelected,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // TODO: Frame selected object in viewport (move camera to focus on selection)
        if let Some(_obj) = self.shared_state.read().scene.get_selected_object() {
            // For now just log - implementing camera movement would require Bevy camera manipulation
        }
        cx.notify();
    }
}

impl Drop for LevelEditorPanel {
    fn drop(&mut self) {
        if let Some(path) = self.shared_state.read().scene.current_scene.clone() {
            ai_sessions::unregister_open_scene(&path);
        }
    }
}

impl Panel for LevelEditorPanel {
    fn panel_name(&self) -> &'static str {
        "Level Editor"
    }

    fn title(&self, _window: &Window, _cx: &App) -> AnyElement {
        let state = self.shared_state.read();
        div()
            .child(if let Some(ref scene) = state.scene.current_scene {
                format!(
                    "Level Editor - {}{}",
                    scene
                        .file_stem()
                        .and_then(|s| s.to_str())
                        .unwrap_or("Untitled"),
                    if state.scene.has_unsaved_changes {
                        " *"
                    } else {
                        ""
                    }
                )
            } else {
                "Level Editor".to_string()
            })
            .into_any_element()
    }

    fn dump(&self, _cx: &App) -> ui::dock::PanelState {
        ui::dock::PanelState {
            panel_name: self.panel_name().to_string(),
            ..Default::default()
        }
    }

    fn panel_file_path(&self, _cx: &App) -> Option<std::path::PathBuf> {
        self.shared_state.read().scene.current_scene.clone()
    }

    fn tab_icon(&self, _cx: &App) -> Option<ui::IconName> {
        let state = self.shared_state.read();
        let file_path = state.scene.current_scene.as_ref()?;

        // Get the file type icon from the plugin manager registry
        if let Some(plugin_mgr) = plugin_manager::global() {
            if let Some(file_type_def) = plugin_mgr.read().get_file_type_for_path(file_path) {
                return Some(file_type_def.icon.clone());
            }
        }

        None
    }

    fn tab_unsaved(&self, _cx: &App) -> bool {
        self.shared_state.read().scene.has_unsaved_changes
    }

    fn set_active(&mut self, active: bool, _window: &mut Window, cx: &mut App) {
        if active {
            self.viewport.update(cx, |v, _| v.mark_tab_activated());
        }
    }
}

ui_common::panel_boilerplate!(LevelEditorPanel);

impl EventEmitter<PanelEvent> for LevelEditorPanel {}

impl Render for LevelEditorPanel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // Outermost cached view in the level editor. Its rebuild scope contains
        // every nested one, so if this tracks the draw count the whole tree is
        // being walked every frame no matter how well the panels below cache.
        gpui::render_stats::count("level editor root: render");
        let _t = gpui::render_stats::scope("level editor root: render");

        // Initialize workspace on first render
        self.initialize_workspace(window, cx);

        // Open/close the Play-In-Editor Game tab as the game starts/stops.
        // Guarded: a no-op unless the PiE tuple actually changed since the
        // last render that acted on it (see its doc).
        self.sync_game_tab(window, cx);

        // Reconcile the dock area with the active tool mode's layout.
        // Guarded: a no-op unless the active mode changed since the last
        // render that acted on it (see its doc).
        self.sync_mode_layout(window, cx);

        // NOTE: There is deliberately no per-render engine or panel work left
        // here. Selection/tool sync to Helio runs in the 50ms root-input
        // poller (spawned in `new_internal`), and every sub-panel invalidates
        // itself via its own frame pump. Render only builds this panel's own
        // element tree.

        v_flex()
            .size_full()
            // NO BACKGROUND - allow transparency for viewport
            .key_context("LevelEditor")
            .track_focus(&self.focus_handle)
            // Scene operations
            .on_action(cx.listener(Self::on_new_scene))
            .on_action(cx.listener(Self::on_open_scene))
            .on_action(cx.listener(Self::on_save_scene))
            .on_action(cx.listener(Self::on_save_scene_as))
            // Transform tools - KEYBOARD: Q/W/E/R
            .on_action(cx.listener(Self::on_select_tool))
            .on_action(cx.listener(Self::on_move_tool))
            .on_action(cx.listener(Self::on_rotate_tool))
            .on_action(cx.listener(Self::on_scale_tool))
            // Toolbar actions
            .on_action(cx.listener(Self::on_set_tool_mode))
            .on_action(cx.listener(Self::on_set_time_scale))
            .on_action(cx.listener(Self::on_set_multiplayer_mode))
            .on_action(cx.listener(Self::on_set_build_config))
            .on_action(cx.listener(Self::on_set_target_platform))
            .on_action(cx.listener(Self::on_set_build_mode))
            // Object operations
            .on_action(cx.listener(Self::on_add_object))
            .on_action(cx.listener(Self::on_add_object_of_type))
            .on_action(cx.listener(Self::on_delete_object))
            .on_action(cx.listener(Self::on_duplicate_object))
            .on_action(cx.listener(Self::on_undo))
            .on_action(cx.listener(Self::on_redo))
            .on_action(cx.listener(Self::on_select_object))
            .on_action(cx.listener(Self::on_toggle_object_expanded))
            .on_action(cx.listener(Self::on_focus_selected))
            // View operations
            .on_action(cx.listener(Self::on_toggle_grid))
            .on_action(cx.listener(Self::on_toggle_wireframe))
            .on_action(cx.listener(Self::on_toggle_lighting))
            .on_action(cx.listener(Self::on_toggle_performance_overlay))
            .on_action(cx.listener(Self::on_toggle_camera_mode_selector))
            .on_action(cx.listener(Self::on_toggle_viewport_options))
            .on_action(cx.listener(Self::on_toggle_fps_graph_type))
            // Performance metrics toggles
            .on_action(cx.listener(Self::on_toggle_fps_graph))
            .on_action(cx.listener(Self::on_toggle_tps_graph))
            .on_action(cx.listener(Self::on_toggle_frame_time_graph))
            .on_action(cx.listener(Self::on_toggle_memory_graph))
            .on_action(cx.listener(Self::on_toggle_draw_calls_graph))
            .on_action(cx.listener(Self::on_toggle_vertices_graph))
            .on_action(cx.listener(Self::on_toggle_input_latency_graph))
            .on_action(cx.listener(Self::on_toggle_ui_consistency_graph))
            // Play/Edit mode
            .on_action(cx.listener(Self::on_play_scene))
            .on_action(cx.listener(Self::on_stop_scene))
            // Camera modes
            .on_action(cx.listener(Self::on_perspective_view))
            .on_action(cx.listener(Self::on_orthographic_view))
            .on_action(cx.listener(Self::on_top_view))
            .on_action(cx.listener(Self::on_front_view))
            .on_action(cx.listener(Self::on_side_view))
            // Keyboard shortcuts - LETTER KEYS for fast workflow
            .on_key_down(cx.listener(|this, event: &gpui::KeyDownEvent, window, cx| {
                // Respond if this panel or any child (e.g. viewport) has focus,
                // and no modifier keys are held.
                if !this.focus_handle.contains_focused(window, cx)
                    || event.keystroke.modifiers.control
                    || event.keystroke.modifiers.alt
                    || event.keystroke.modifiers.shift
                    || event.keystroke.modifiers.platform
                    || event.keystroke.modifiers.function
                {
                    return;
                }

                match event.keystroke.key.as_ref() {
                    "escape" => {
                        // Update UI state unconditionally — always clear GPUI selection.
                        this.shared_state.write().scene.select_object(None);
                        // Mailbox, not `gpu_engine.lock()` -- `queue_deselect`
                        // was already just an atomic-flag write internally;
                        // the lock here was pure incidental overhead from the
                        // wrapper shape, not a real dependency.
                        if let Some(mailbox) = &this.helio_mailbox {
                            mailbox.queue_deselect();
                        }
                        cx.notify();
                    }
                    // Tool selection — call handlers directly to avoid action-dispatch drift.
                    "q" => this.on_select_tool(&SelectTool, window, cx),
                    "w" => this.on_move_tool(&MoveTool, window, cx),
                    "g" => this.on_move_tool(&MoveTool, window, cx), // Blender: G = Grab/Move
                    "e" => this.on_rotate_tool(&RotateTool, window, cx),
                    "r" => this.on_rotate_tool(&RotateTool, window, cx), // Blender: R = Rotate
                    "s" => this.on_scale_tool(&ScaleTool, window, cx),   // Blender: S = Scale
                    "l" => {}
                    "f" => cx.dispatch_action(&FocusSelected),
                    _ => {}
                }
            }))
            // Additional keyboard shortcuts for Alt+Up/Down (object reordering)
            .on_key_down(cx.listener(|this, event: &gpui::KeyDownEvent, window, cx| {
                // Only respond if Alt key is pressed and this panel or any child has focus
                if !this.focus_handle.contains_focused(window, cx) || !event.keystroke.modifiers.alt
                {
                    return;
                }

                match event.keystroke.key.as_ref() {
                    "up" => {
                        // Move selected object up in hierarchy
                        if let Some(id) = this.shared_state.read().scene.selected_object() {
                            this.shared_state.read().scene.database.move_object_up(&id);
                            cx.notify();
                        }
                    }
                    "down" => {
                        // Move selected object down in hierarchy
                        if let Some(id) = this.shared_state.read().scene.selected_object() {
                            this.shared_state
                                .read()
                                .scene
                                .database
                                .move_object_down(&id);
                            cx.notify();
                        }
                    }
                    _ => {}
                }
            }))
            .child(
                // Toolbar at the top. Cached: this panel is marked dirty on
                // every frame the viewport publishes, but the toolbar only
                // needs to rebuild when its own state changes.
                AnyView::from(self.toolbar.clone()).cached(ToolbarView::cache_style()),
            )
            .child(
                // Workspace with draggable panels
                if let Some(ref workspace) = self.workspace {
                    workspace.clone().into_any_element()
                } else {
                    div().child("Loading workspace...").into_any_element()
                },
            )
            .child(
                // Status bar at the bottom. Cached for the same reason as the
                // toolbar above.
                AnyView::from(self.status_bar.clone()).cached(StatusBarView::cache_style()),
            )
    }
}

// ── Play In Editor build helpers (issue #243) ───────────────────────────────

/// Enter play mode and kick off the Play-In-Editor build.
///
/// Free function (not a panel method) so BOTH the `PlayScene` action handler and
/// the toolbar "Start Simulation" button (`playback_controls`) can share one
/// code path — they previously diverged, and only the action handler had PiE.
pub(crate) fn begin_pie(
    shared_state: Arc<parking_lot::RwLock<LevelEditorState>>,
    window: &mut Window,
    cx: &mut App,
) {
    // Snapshot the scene (also flips to play mode).
    shared_state.write().scene.enter_play_mode();

    let Some(root) = engine_state::get_project_path().map(std::path::PathBuf::from) else {
        window.push_notification(
            Notification::error(t!("Notification.Title.PlayInEditor").to_string())
                .message(t!("Notification.Message.NoProjectOpenForPlay").to_string()),
            cx,
        );
        return;
    };

    tracing::info!(project = %root.display(), "PiE: starting build for Play");

    // Reflect unsaved edits: write the live SceneDb to a temp level file.
    let scene_path = root.join("target").join("pie").join("play.level");
    if let Err(e) = shared_state.read().scene.database.save_to_file(&scene_path) {
        window.push_notification(
            Notification::error(t!("Notification.Title.PlayInEditor").to_string()).message(
                t!(
                    "Notification.Message.FailedToWriteScene",
                    error => e.to_string()
                )
                .to_string(),
            ),
            cx,
        );
        return;
    }

    // Native hot reload (#653): pressing Play while a game runs rebuilds
    // and swaps the library WITHOUT dropping the world — the viewport stops
    // the old host only once the new build is in hand.
    let reload = {
        let mut st = shared_state.write();
        st.play.pie.building = true;
        st.play.pie.stop_requested = false;
        st.play.pie.last_error = None;
        st.play.pie.pending_start = None;
        st.play.pie.active
    };

    if reload {
        tracing::info!("PiE: game already running — this Play is a NATIVE HOT RELOAD");
    }

    window.push_notification(
        Notification::info(t!("Notification.Title.PlayInEditor").to_string())
            .message(t!("Notification.Message.BuildingGame").to_string()),
        cx,
    );

    let shared = shared_state.clone();
    let _ = std::thread::Builder::new()
        .name("pie-build".into())
        .spawn(move || {
            let result = build_pie_dylib(&root, &scene_path, reload);
            let mut st = shared.write();
            st.play.pie.building = false;
            match result {
                Ok(req) => st.play.pie.pending_start = Some(req),
                Err(e) => {
                    tracing::error!("PiE build failed: {e}");
                    st.play.pie.last_error = Some(e);
                }
            }
        });
}

/// Ask the viewport to tear down the embedded game, then exit play mode.
pub(crate) fn end_pie(shared_state: Arc<parking_lot::RwLock<LevelEditorState>>) {
    {
        let mut st = shared_state.write();
        st.play.pie.stop_requested = true;
        st.play.pie.pending_start = None;
        st.play.pie.building = false;
    }
    shared_state.write().scene.exit_play_mode();
}

/// Regenerate the project scaffolding and build it as a `cdylib`, returning what
/// the viewport needs to load the embedded game. Runs on a background thread.
///
/// `reload` marks a native hot reload (#653): an earlier session is still
/// running and its world state must survive the swap. The flag only rides the
/// request — a failed rebuild leaves the old game untouched either way.
///
/// Fastpath: if a release library already exists and no `.rs`/`.toml` under the
/// project is newer than it, skip regeneration + `cargo build` entirely and reuse
/// the last-built artifact — pressing Play with no source changes is instant.
fn build_pie_dylib(
    root: &Path,
    scene_path: &Path,
    reload: bool,
) -> Result<PieStartRequest, String> {
    // PiE uses the release library (faster at runtime, and matches the artifact
    // `cargo build --release` / `cargo run --release` produce).
    let release = true;

    // Blueprint preflight (#656): validate saved class graphs against the
    // compiled artifacts PIE would load. Bad graphs stop Play here instead of
    // surfacing as runtime failures inside the embedded game.
    if let Err(summary) = blueprint_editor_plugin::validation::validate_project_classes(root) {
        tracing::error!("PiE blocked by blueprint validation:\n{summary}");
        return Err(summary);
    }

    // Fastpath — reuse the existing artifact when nothing changed. Needs the
    // crate name, which needs a manifest; if it's missing we fall through to a
    // full build that generates it.
    if let Ok(crate_name) = read_crate_name(root) {
        let dylib_path =
            engine_backend::services::PieHost::output_dylib_path(root, &crate_name, release);
        if dylib_path.exists() && !any_source_newer(root, &dylib_path) {
            tracing::info!(
                lib = %dylib_path.display(),
                "PiE fastpath: no .rs/.toml changes since last build — reusing artifact"
            );
            return Ok(PieStartRequest {
                dylib_path,
                project_root: root.to_path_buf(),
                scene_path: scene_path.to_path_buf(),
                reload,
            });
        }
    }

    // Slow path — regenerate scaffolding (src/lib.rs + the cdylib manifest) and
    // build.
    engine_backend::services::ensure_core_bootstrap(root)?;

    let output = std::process::Command::new("cargo")
        .arg("build")
        .arg("--lib")
        .arg("--release")
        .current_dir(root)
        .output()
        .map_err(|e| format!("Failed to spawn cargo: {e}"))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        // Keep the message bounded — the full log is on stderr/tracing.
        let tail: String = stderr.lines().rev().take(20).collect::<Vec<_>>().join("\n");
        return Err(format!("cargo build --lib --release failed:\n{tail}"));
    }

    let crate_name = read_crate_name(root)?;
    let dylib_path =
        engine_backend::services::PieHost::output_dylib_path(root, &crate_name, release);
    if !dylib_path.exists() {
        return Err(format!(
            "Build succeeded but library not found at {}",
            dylib_path.display()
        ));
    }

    Ok(PieStartRequest {
        dylib_path,
        project_root: root.to_path_buf(),
        scene_path: scene_path.to_path_buf(),
        reload,
    })
}

/// Whether any `.rs` or `.toml` file under `root` is newer than `artifact`.
/// Skips `target/` and `.git/`. A missing/unreadable artifact counts as "newer"
/// so the caller rebuilds.
fn any_source_newer(root: &Path, artifact: &Path) -> bool {
    let Ok(artifact_mtime) = std::fs::metadata(artifact).and_then(|m| m.modified()) else {
        return true;
    };
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let Ok(file_type) = entry.file_type() else {
                continue;
            };
            if file_type.is_dir() {
                let name = entry.file_name();
                if name == "target" || name == ".git" {
                    continue;
                }
                stack.push(entry.path());
            } else if file_type.is_file() {
                let path = entry.path();
                let is_source = path
                    .extension()
                    .map(|e| e == "rs" || e == "toml")
                    .unwrap_or(false);
                if is_source {
                    if let Ok(mtime) = entry.metadata().and_then(|m| m.modified()) {
                        if mtime > artifact_mtime {
                            return true;
                        }
                    }
                }
            }
        }
    }
    false
}

/// Read the `[package] name` from the project's `Cargo.toml`.
fn read_crate_name(root: &Path) -> Result<String, String> {
    let toml = std::fs::read_to_string(root.join("Cargo.toml"))
        .map_err(|e| format!("Failed to read Cargo.toml: {e}"))?;
    for line in toml.lines() {
        let line = line.trim();
        if let Some(rest) = line.strip_prefix("name") {
            let rest = rest.trim_start();
            if let Some(value) = rest.strip_prefix('=') {
                let name = value.trim().trim_matches('"').trim();
                if !name.is_empty() {
                    return Ok(name.to_string());
                }
            }
        }
    }
    Err("Could not find package name in Cargo.toml".to_string())
}
