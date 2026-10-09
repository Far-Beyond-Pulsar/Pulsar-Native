use super::*;

impl LevelEditorPanel {
    /// Cheap read-only snapshot of the Play-In-Editor state this panel acts on.
    pub(super) fn pie_signature(&self) -> (bool, bool, bool, bool) {
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

    pub(super) fn tool_to_gizmo(
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

    pub(super) fn sync_gizmo_to_helio(&mut self) {
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

    pub(super) fn queue_gizmo_mode_for_tool(&mut self, tool: TransformTool) {
        let (_, helio_mode) = Self::tool_to_gizmo(tool);
        if let Some(mailbox) = &self.helio_mailbox {
            mailbox.queue_gizmo(helio_mode);
        }
    }

    pub(super) fn current_editor_camera_state(&self) -> Option<LevelEditorCameraState> {
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

    pub(super) fn apply_editor_camera_state(&mut self, camera: Option<&LevelEditorCameraState>) {
        let Some(camera) = camera else {
            return;
        };
        if let Some(mailbox) = &self.helio_mailbox {
            mailbox.queue_camera(EditorCameraState {
                position: camera.position,
                yaw: camera.yaw,
                pitch: camera.pitch,
            });
        }
    }
}
