use super::*;

impl LevelEditorPanel {
    pub(in crate::ui::panel) fn on_play_scene(
        &mut self,
        _: &PlayScene,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        begin_pie(self.shared_state.clone(), window, cx);
        self.sync_gizmo_to_helio();
        cx.notify();
    }

    pub(in crate::ui::panel) fn on_stop_scene(
        &mut self,
        _: &StopScene,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        end_pie(self.shared_state.clone());
        self.sync_gizmo_to_helio();
        cx.notify();
    }

    pub(in crate::ui::panel) fn on_perspective_view(
        &mut self,
        _: &PerspectiveView,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.shared_state
            .write()
            .editor
            .set_camera_mode(CameraMode::Perspective);
        cx.notify();
    }

    pub(in crate::ui::panel) fn on_orthographic_view(
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

    pub(in crate::ui::panel) fn on_top_view(
        &mut self,
        _: &TopView,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.shared_state
            .write()
            .editor
            .set_camera_mode(CameraMode::Top);
        cx.notify();
    }

    pub(in crate::ui::panel) fn on_front_view(
        &mut self,
        _: &FrontView,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.shared_state
            .write()
            .editor
            .set_camera_mode(CameraMode::Front);
        cx.notify();
    }

    pub(in crate::ui::panel) fn on_side_view(
        &mut self,
        _: &SideView,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.shared_state
            .write()
            .editor
            .set_camera_mode(CameraMode::Side);
        cx.notify();
    }

    pub(in crate::ui::panel) fn on_save_scene(
        &mut self,
        _: &SaveScene,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // If no current scene path, do Save As
        let Some(path) = self.shared_state.read().scene.current_scene.clone() else {
            cx.dispatch_action(&SaveSceneAs);
            return;
        };
        // Runs in the background; the editor stays responsive (#967).
        crate::ui::save::save_level(
            self.shared_state.clone(),
            path,
            self.current_editor_camera_state(),
            window,
            cx,
        );
    }

    pub(in crate::ui::panel) fn on_save_scene_as(
        &mut self,
        _: &SaveSceneAs,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let state_arc = self.shared_state.clone();
        let editor_camera = self.current_editor_camera_state();
        let dialog = rfd::AsyncFileDialog::new()
            .set_title("Save Scene As")
            .add_filter("Level file", &["level", "json"])
            .set_file_name("untitled.level");
        cx.spawn_in(window, async move |_this, cx| {
            if let Some(handle) = dialog.save_file().await {
                let path = handle.path().to_path_buf();
                // Background save; on success the level's path becomes `path`.
                cx.update(|window, cx| {
                    crate::ui::save::save_level(state_arc, path, editor_camera, window, cx)
                })
                .ok();
            }
        })
        .detach();
    }
}
