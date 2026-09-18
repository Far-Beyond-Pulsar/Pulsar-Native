use super::*;

impl LevelEditorPanel {
    pub(in crate::level_editor::ui::panel) fn on_play_scene(&mut self, _: &PlayScene, window: &mut Window, cx: &mut Context<Self>) {
        begin_pie(self.shared_state.clone(), window, cx);
        self.sync_gizmo_to_helio();
        cx.notify();
    }

    pub(in crate::level_editor::ui::panel) fn on_stop_scene(&mut self, _: &StopScene, _: &mut Window, cx: &mut Context<Self>) {
        end_pie(self.shared_state.clone());
        self.sync_gizmo_to_helio();
        cx.notify();
    }

    pub(in crate::level_editor::ui::panel) fn on_perspective_view(&mut self, _: &PerspectiveView, _: &mut Window, cx: &mut Context<Self>) {
        self.shared_state
            .write()
            .editor
            .set_camera_mode(CameraMode::Perspective);
        cx.notify();
    }

    pub(in crate::level_editor::ui::panel) fn on_orthographic_view(
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

    pub(in crate::level_editor::ui::panel) fn on_top_view(&mut self, _: &TopView, _: &mut Window, cx: &mut Context<Self>) {
        self.shared_state
            .write()
            .editor
            .set_camera_mode(CameraMode::Top);
        cx.notify();
    }

    pub(in crate::level_editor::ui::panel) fn on_front_view(&mut self, _: &FrontView, _: &mut Window, cx: &mut Context<Self>) {
        self.shared_state
            .write()
            .editor
            .set_camera_mode(CameraMode::Front);
        cx.notify();
    }

    pub(in crate::level_editor::ui::panel) fn on_side_view(&mut self, _: &SideView, _: &mut Window, cx: &mut Context<Self>) {
        self.shared_state
            .write()
            .editor
            .set_camera_mode(CameraMode::Side);
        cx.notify();
    }

    pub(in crate::level_editor::ui::panel) fn on_save_scene(&mut self, _: &SaveScene, _: &mut Window, cx: &mut Context<Self>) {
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

    pub(in crate::level_editor::ui::panel) fn on_save_scene_as(&mut self, _: &SaveSceneAs, _window: &mut Window, cx: &mut Context<Self>) {
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

}
