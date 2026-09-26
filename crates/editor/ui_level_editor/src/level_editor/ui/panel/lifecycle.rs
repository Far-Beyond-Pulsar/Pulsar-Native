use super::*;

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
    pub(super) fn spawn_level_load(cx: &mut Context<Self>) {
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
    pub(super) fn ensure_default_level_file(&mut self) {
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

        let scene_db = { self.shared_state.read().scene.shared_scene() };

        if default_path.exists() {
            // File already on disk — load it into the shared scene db.
            {
                let mut world = scene_db.write();
                crate::level_editor::scene_edit::objects::clear(&mut world.world);
            }
            let load_result = {
                let mut world = scene_db.write();
                crate::level_editor::scene_edit::level_io::load_from_file_with_editor_camera(
                    &mut world.world,
                    &default_path,
                )
            };
            match load_result {
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

            // The embedded default level references engine meshes (the cathedral
            // batches) by project-relative path; make sure they exist first.
            if let Some(root) = default_path.parent().and_then(|p| p.parent()) {
                engine_backend::services::ensure_engine_primitives(root);
            }

            let seed_result = if let Some(bytes) = embedded {
                // Write the embedded bytes directly — preserves whatever the developer
                // designed as the default scene via "Save as Default Level".
                engine_fs::virtual_fs::write_file(&default_path, &bytes)
                    .map_err(|e| format!("Failed to write embedded default level: {e}"))
            } else {
                // No embedded asset yet — persist the current empty scene so the
                // path is stable for future saves.
                let world = scene_db.read();
                crate::level_editor::scene_edit::level_io::save_to_file_with_editor_camera(
                    &world.world,
                    &default_path,
                    self.current_editor_camera_state(),
                )
            };

            match seed_result {
                Ok(_) => {
                    // Load back what we just wrote so the editor shows the correct scene.
                    {
                        let mut world = scene_db.write();
                        crate::level_editor::scene_edit::objects::clear(&mut world.world);
                    }
                    let load_result = {
                        let mut world = scene_db.write();
                        crate::level_editor::scene_edit::level_io::load_from_file_with_editor_camera(
                            &mut world.world,
                            &default_path,
                        )
                    };
                    match load_result {
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
        let scene_db = { panel.shared_state.read().scene.shared_scene() };
        {
            let mut world = scene_db.write();
            crate::level_editor::scene_edit::objects::clear(&mut world.world);
        }
        let editor_camera = {
            let mut world = scene_db.write();
            crate::level_editor::scene_edit::level_io::load_from_file_with_editor_camera(
                &mut world.world,
                &path,
            )?
        };
        panel.apply_editor_camera_state(editor_camera.as_ref());
        {
            let mut state = panel.shared_state.write();
            state.scene.current_scene = Some(path);
            state.scene.has_unsaved_changes = false;
            state.scene.bump_revision(false);
            if let Some(open_path) = state.scene.current_scene.clone() {
                ai_sessions::register_open_scene(&open_path, &panel.shared_state);
            }
        }
        Ok(panel)
    }

    pub(super) fn new_internal(window_id: Option<u64>, window: &mut Window, cx: &mut Context<Self>) -> Self {
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
        let scene_store = state.scene.shared_scene();

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
                (bool, bool, bool, bool, u64),
                (Option<std::path::PathBuf>, bool),
                (Option<String>, TransformTool),
            )> = None;
            loop {
                cx.background_executor()
                    .timer(std::time::Duration::from_millis(50))
                    .await;
                // A Stop the Game viewport did not process in time still
                // restores the editor world (#925).
                if poll_state.read().play.pie.restore_after_stop {
                    let mut s = poll_state.write();
                    crate::level_editor::ui::panel::pie::finish_stop(&mut s, false);
                }
                let snapshot = {
                    let s = poll_state.read();
                    (
                        (
                            s.play.pie.building,
                            s.play.pie.active,
                            s.play.pie.pending_start.is_some(),
                            s.play.pie.last_error.is_some(),
                            // Class updates rebuilt placed instances (#935).
                            s.scene.class_updates,
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
        let helio_mailbox = gpu_engine.lock().ok().and_then(|engine| engine.editor_mailbox());

        let class_updates =
            crate::level_editor::core::asset_updates::subscribe_class_updates(shared_state.clone());

        Self {
            focus_handle: cx.focus_handle(),
            toolbar,
            status_bar,
            viewport,
            gpu_engine: gpu_engine.clone(),
            helio_mailbox,
            render_enabled,
            _class_updates: class_updates,
            shared_state,
            workspace: None,
            game_panel: None,
            applied_pie_signature: None,
            applied_mode_layout: None,
            mode_right_panels: Vec::new(),
            _root_input_poller: poller,
        }
    }
}
