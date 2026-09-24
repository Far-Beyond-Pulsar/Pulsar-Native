use super::*;

impl LevelEditorPanel {
    pub(in crate::level_editor::ui::panel) fn on_open_scene(&mut self, _: &OpenScene, _window: &mut Window, cx: &mut Context<Self>) {
        let state_arc = self.shared_state.clone();
        let scene_db = { state_arc.read().scene.shared_scene() };
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
                let result = {
                    let mut world = scene_db.write();
                    crate::level_editor::scene_edit::level_io::load_from_file_with_editor_camera(
                        &mut world.world,
                        &path,
                    )
                };
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

    pub(in crate::level_editor::ui::panel) fn on_new_scene(&mut self, _: &NewScene, _: &mut Window, cx: &mut Context<Self>) {
        // Warn if unsaved changes (TODO: modal dialog)
        // Clear the scene IN-PLACE so the renderer keeps its Arc<SceneDb>.
        let scene_db = { self.shared_state.read().scene.shared_scene() };
        let mut editor_camera = None;
        {
            let mut world = scene_db.write();
            crate::level_editor::scene_edit::objects::clear(&mut world.world);
        }

        // Load from the embedded default.level if available, otherwise start empty.
        if let Some(bytes) = engine_state::EngineContext::global()
            .and_then(|ctx| ctx.store.get_or_init::<Option<Vec<u8>>>().read().clone())
        {
            let tmp = std::env::temp_dir().join("pulsar_new_scene_seed.level");
            if engine_fs::virtual_fs::write_file(&tmp, &bytes).is_ok() {
                let load_result = {
                    let mut world = scene_db.write();
                    crate::level_editor::scene_edit::level_io::load_from_file_with_editor_camera(
                        &mut world.world,
                        &tmp,
                    )
                };
                match load_result {
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

    pub(in crate::level_editor::ui::panel) fn on_focus_selected(
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
