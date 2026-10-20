use super::*;

impl LevelEditorPanel {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let panel = Self::new_internal(None, window, cx);
        Self::spawn_level_load(window, cx);
        panel
    }

    pub fn new_with_window_id(window_id: u64, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let panel = Self::new_internal(Some(window_id), window, cx);
        Self::spawn_level_load(window, cx);
        panel
    }

    /// Defers opening the default level until the editor window has painted its
    /// first frame, so the loading bar is on screen while the level loads (a
    /// load that starts and ends before that frame never shows it). File
    /// preparation and scene hydration then run in the background.
    pub(super) fn spawn_level_load(window: &mut Window, cx: &mut Context<Self>) {
        let this = cx.entity().downgrade();
        window.on_next_frame(move |_, cx| {
            _ = this.update(cx, |panel, cx| panel.begin_default_level_load(cx));
        });
    }

    /// If no project is open, do nothing. Otherwise prepare
    /// `<project>/scene/default.level` in the background and start normal scene
    /// hydration once the file exists.
    pub(super) fn begin_default_level_load(&self, cx: &mut Context<Self>) {
        let Some(project_str) = engine_state::get_project_path() else {
            return;
        };
        let default_path = std::path::PathBuf::from(&project_str)
            .join("scene")
            .join("default.level");
        let scene_db = self.shared_state.read().scene.shared_scene();
        let camera_state = self.current_editor_camera_state();
        let embedded = engine_state::EngineContext::global()
            .and_then(|ctx| ctx.store.get_or_init::<Option<Vec<u8>>>().read().clone());

        cx.spawn(async move |this, cx| {
            let prepare_path = default_path.clone();
            let result = cx
                .background_executor()
                .spawn(async move {
                    if let Some(parent) = prepare_path.parent() {
                        engine_fs::virtual_fs::create_dir_all(parent).map_err(|e| {
                            format!("Could not create default level directory: {e}")
                        })?;
                    }

                    if !prepare_path.exists() {
                        if let Some(root) = prepare_path.parent().and_then(|p| p.parent()) {
                            engine_backend::services::ensure_engine_primitives(root);
                        }
                        if let Some(bytes) = embedded {
                            engine_fs::virtual_fs::write_file(&prepare_path, &bytes).map_err(
                                |e| format!("Failed to write embedded default level: {e}"),
                            )?;
                        } else {
                            let world = scene_db.read();
                            crate::scene_edit::level_io::save_to_file_with_editor_camera(
                                &world.world,
                                &prepare_path,
                                camera_state,
                            )?;
                        }
                        tracing::info!("Default level seeded at {:?}", prepare_path);
                    }
                    Ok::<_, String>(prepare_path)
                })
                .await;

            cx.update(|cx| {
                let _ = this.update(cx, |panel, cx| match result {
                    Ok(path) => panel.begin_scene_load(path, cx),
                    Err(error) => {
                        tracing::warn!("Could not prepare default level: {error}");
                        cx.notify();
                    }
                });
            });
        })
        .detach();
    }

    /// Create the editor and start loading a level file in the background.
    ///
    /// The scene is cleared and reloaded into the existing shared `Arc<SceneDb>`
    /// so the renderer stays in sync. The panel appears immediately with a
    /// viewport hydration overlay while the file is loaded and applied. Load
    /// failures are logged after panel creation.
    pub fn new_with_path(
        path: std::path::PathBuf,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Result<Self, String> {
        let panel = Self::new_internal(None, window, cx);
        // Not now: the caller may still have synchronous construction to do
        // before the window paints, and a load that starts (and can finish)
        // before the first frame never shows its progress bar.
        let this = cx.entity().downgrade();
        window.on_next_frame(move |_, cx| {
            _ = this.update(cx, |panel, cx| panel.begin_scene_load(path, cx));
        });
        Ok(panel)
    }

    /// Start a level load without blocking panel creation or the UI executor.
    pub(super) fn begin_scene_load(&self, path: std::path::PathBuf, cx: &mut Context<Self>) {
        self.begin_scene_load_from(Some(path), None, cx);
    }

    /// File opens and untitled scenes share the same worker and loading overlay.
    pub(super) fn begin_scene_load_from(
        &self,
        path: Option<std::path::PathBuf>,
        embedded: Option<Vec<u8>>,
        cx: &mut Context<Self>,
    ) {
        let state_arc = self.shared_state.clone();
        let scene_db = { state_arc.read().scene.shared_scene() };
        let loading = state_arc.read().editor_loading.begin("Loading scene…");
        cx.notify();

        cx.spawn(async move |this, cx| {
            let load_path = path.clone();
            let executor = cx.background_executor().clone();
            let result: Result<
                (
                    crate::scene_edit::LevelEditorFileState,
                    crate::world_settings_data::WorldSettingsData,
                ),
                String,
            > = cx
                .background_executor()
                .spawn(async move {
                    use crate::scene_edit::level_io;

                    // Parse and migrate outside SceneDB's lock, then apply one
                    // object/component group per write so readers can proceed
                    // between batches.
                    let mut prepared = match load_path {
                        Some(path) => level_io::prepare_level_for_hydration(path)?,
                        None => level_io::prepare_new_level_for_hydration(embedded)?,
                    };
                    let total = prepared.total_work() as f32;
                    let mut completed = 0usize;
                    let set_progress = |value: f32| loading.set_progress(value);
                    set_progress(0.02);
                    {
                        let mut scene = scene_db.write();
                        level_io::clear_for_hydration(&mut scene.world);
                    }

                    for object in std::mem::take(&mut prepared.objects) {
                        let mut scene = scene_db.write();
                        level_io::add_hydrated_object(&mut scene.world, object);
                        drop(scene);
                        completed += 1;
                        set_progress(0.05 + 0.78 * completed as f32 / total);
                    }
                    {
                        let mut scene = scene_db.write();
                        level_io::convert_legacy_transforms(&mut scene.world, &prepared.version);
                    }
                    for (object_id, components) in std::mem::take(&mut prepared.components) {
                        let count = components.len();
                        let mut scene = scene_db.write();
                        level_io::add_hydrated_components(
                            &mut scene.world,
                            &object_id,
                            &components,
                        );
                        drop(scene);
                        completed += count;
                        set_progress(0.05 + 0.78 * completed as f32 / total);
                    }
                    set_progress(0.86);
                    {
                        let mut scene = scene_db.write();
                        level_io::finish_prepared_hydration(&mut scene.world, &prepared);
                    }
                    // Asset workers compile shaders and decode meshes/textures.
                    // Waiting for them while holding SceneDB's write lock also
                    // freezes every UI panel that reads the world. Only bind
                    // completed work under the lock, then release it before
                    // yielding. Keep the loading token alive across the wait.
                    let deadline = std::time::Instant::now()
                        + engine_backend::scene::LEVEL_ASSET_WAIT;
                    loop {
                        let pending = {
                            let mut scene = scene_db.write();
                            scene.world.pump_assets();
                            scene.world.asset_work_pending()
                        };
                        if !pending {
                            break;
                        }
                        if std::time::Instant::now() >= deadline {
                            tracing::warn!(
                                "Scene assets are still loading; the renderer will continue loading them"
                            );
                            break;
                        }
                        executor.timer(std::time::Duration::from_millis(16)).await;
                    }
                    set_progress(0.98);
                    tracing::info!(
                        "Scene loaded from: {} (version: {})",
                        prepared.path.display(),
                        prepared.version
                    );
                    Ok((prepared.editor, prepared.world_settings))
                })
                .await;

            cx.update(|cx| {
                let _ = this.update(cx, |panel, cx| {
                    match result {
                        Ok((editor_state, world_settings)) => {
                            panel.apply_editor_camera_state(editor_state.camera.as_ref());
                            let mut state = state_arc.write();
                            state.scene.set_current_scene(path, false);
                            state.scene.world_settings = world_settings;
                            state.editor.terrain.foliage_sets = editor_state.foliage_sets;
                            state.scene.has_unsaved_changes = false;
                            state.scene.select_object(None);
                            state.scene.bump_revision(false);
                        }
                        Err(error) => tracing::error!("Open scene failed: {}", error),
                    }
                    cx.notify();
                });
            });
        })
        .detach();
    }

    pub(super) fn new_internal(
        window_id: Option<u64>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
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

        // Construct editor state first: its scene owns the SceneDB-backed
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

        // The scene and HelioRenderer share the same store `Arc`, so every
        // add/remove/update made through the scene is visible to the
        // renderer's next sync pass without a separate write-through call.

        let shared_state = Arc::new(parking_lot::RwLock::new(state));
        // Reachable by the AI tools for as long as this panel lives (unregistered
        // in `Drop`), whatever level it shows or whether that was ever saved.
        crate::ai_sessions::register_editor(&shared_state);

        // Temporary debug toggle: replace viewport with a solid yellow panel to
        // verify layout/overlap issues independently of GPU rendering.
        let debug_replace_with_yellow = false;

        // Create HelioViewport — renders via WgpuSurfaceHandle every GPUI frame.
        // It receives shared_state so viewport drop actions mutate the scene
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
        // Fetched once here, not re-acquired via `gpu_engine.lock()` per
        // command -- see `HelioEditorMailbox`'s doc. `GpuRendererBuilder::build`
        // always sets `helio_renderer: Some(...)` synchronously, so this is
        // `Some` immediately after construction; `None` only if `gpu_engine`
        // somehow arrived pre-torn-down, which the `if let` call sites below
        // degrade out of harmlessly (same "skip this one tick" shape the old
        // `gpu_engine.lock()` sites already had on any lock failure).
        let helio_mailbox = gpu_engine
            .lock()
            .ok()
            .and_then(|engine| engine.editor_mailbox());
        if let Some(mailbox) = &helio_mailbox {
            shared_state.write().scene.voxel_chunk_saves = Some(mailbox.voxel_chunk_saves());
        }

        let playback_host_id = engine_state::playback::new_playback_host_id();
        let poll_state = Arc::clone(&shared_state);
        let poll_gpu = gpu_engine.clone();
        let poll_mailbox = helio_mailbox.clone();
        let extra_viewports = split_viewports::ExtraViewports::default();
        let poll_extras = extra_viewports.clone();
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
                // Report Play-In-Editor status to the engine-global playback state.
                super::playback_host::publish_playback_status(playback_host_id, &poll_state.read());
                // A released sculpt stroke becomes one undo step once the
                // render thread applied its last samples.
                if poll_state.read().editor.voxel_stroke.is_some() {
                    let mut s = poll_state.write();
                    crate::state::voxel::VoxelStroke::finish(&mut s, false);
                }
                // A Stop the Game viewport did not process in time still
                // restores the editor world (#925).
                if poll_state.read().play.pie.restore_after_stop {
                    let mut s = poll_state.write();
                    crate::ui::panel::pie::finish_stop(&mut s, false);
                }
                // Undo/redo from a caller without a renderer handle (the AI
                // tools) may have changed the selection. Cleared only once
                // the gizmo follows it.
                // Viewports added by the split buttons (dropped ones pruned).
                let extras = poll_extras.live();
                if poll_state.read().scene.pending_selection_sync {
                    if let Ok(mut engine) = poll_gpu.try_lock() {
                        poll_state.write().scene.pending_selection_sync = false;
                        engine.sync_selection_to_helio();
                        for extra in &extras {
                            if let Ok(mut engine) = extra.gpu.try_lock() {
                                engine.sync_selection_to_helio();
                            }
                        }
                    }
                }
                // Finished gizmo drags become undo steps (#931), whichever
                // viewport made them.
                let drags: Vec<_> = poll_mailbox
                    .iter()
                    .chain(extras.iter().map(|extra| &extra.mailbox))
                    .flat_map(|mailbox| mailbox.take_completed_drags())
                    .collect();
                if !drags.is_empty() {
                    let mut s = poll_state.write();
                    for drag in drags {
                        s.scene.record_gizmo_drag(drag.entity, drag.before);
                    }
                }
                // A gizmo drag on a Static/Stationary object (#837); the
                // notification needs a window, so render shows it.
                if let Some(warning) = poll_mailbox
                    .iter()
                    .chain(extras.iter().map(|extra| &extra.mailbox))
                    .find_map(|mailbox| mailbox.take_static_drag_warning())
                {
                    cx.update(|cx| {
                        let _ = this.update(cx, |panel, cx| {
                            panel.pending_static_drag_warning = Some(warning);
                            cx.notify();
                        });
                    });
                }
                // Play needs a window; render starts it.
                if poll_state.read().play.pie.play_requested {
                    cx.update(|cx| {
                        let _ = this.update(cx, |_, cx| cx.notify());
                    });
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
                    for extra in &extras {
                        if let Ok(mut engine) = extra.gpu.try_lock() {
                            let (_, tool) = &snapshot.2;
                            let (_, helio_mode) = Self::tool_to_gizmo(*tool);
                            engine.queue_gizmo_mode(helio_mode);
                            engine.sync_selection_to_helio();
                        }
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

        let playback_host = Self::bind_playback_host(playback_host_id, window, cx);

        let viewport_input_state = Arc::new(InputState::new());
        let toolbar = cx.new(|cx| {
            ToolbarView::new(
                window,
                cx,
                shared_state.clone(),
                gpu_engine.clone(),
                helio_mailbox.clone(),
                viewport_input_state.clone(),
            )
        });
        let class_updates =
            crate::core::asset_updates::subscribe_class_updates(shared_state.clone());
        let asset_reloads =
            crate::core::asset_updates::subscribe_asset_reloads(shared_state.clone());

        Self {
            focus_handle: cx.focus_handle(),
            toolbar,
            viewport_input_state,
            viewport,
            gpu_engine: gpu_engine.clone(),
            helio_mailbox,
            render_enabled,
            extra_viewports,
            _class_updates: class_updates,
            _asset_reloads: asset_reloads,
            shared_state,
            workspace: None,
            viewport_panel: None,
            game_panel: None,
            playback_host,
            applied_pie_signature: None,
            applied_mode_layout: None,
            mode_panels: std::collections::HashMap::new(),
            _root_input_poller: poller,
            pending_static_drag_warning: None,
            warned_static_drags: Default::default(),
        }
    }
}
