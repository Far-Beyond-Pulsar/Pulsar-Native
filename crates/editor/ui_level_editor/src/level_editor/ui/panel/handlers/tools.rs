use super::*;

impl LevelEditorPanel {
    // Action handlers
    pub(in crate::level_editor::ui::panel) fn on_select_tool(&mut self, _: &SelectTool, _: &mut Window, cx: &mut Context<Self>) {
        self.shared_state
            .write()
            .editor
            .set_tool(TransformTool::Select);
        self.queue_gizmo_mode_for_tool(TransformTool::Select);
        cx.notify();
    }

    pub(in crate::level_editor::ui::panel) fn on_move_tool(&mut self, _: &MoveTool, _: &mut Window, cx: &mut Context<Self>) {
        self.shared_state
            .write()
            .editor
            .set_tool(TransformTool::Move);
        self.queue_gizmo_mode_for_tool(TransformTool::Move);
        cx.notify();
    }

    pub(in crate::level_editor::ui::panel) fn on_rotate_tool(&mut self, _: &RotateTool, _: &mut Window, cx: &mut Context<Self>) {
        self.shared_state
            .write()
            .editor
            .set_tool(TransformTool::Rotate);
        self.queue_gizmo_mode_for_tool(TransformTool::Rotate);
        cx.notify();
    }

    pub(in crate::level_editor::ui::panel) fn on_scale_tool(&mut self, _: &ScaleTool, _: &mut Window, cx: &mut Context<Self>) {
        self.shared_state
            .write()
            .editor
            .set_tool(TransformTool::Scale);
        self.queue_gizmo_mode_for_tool(TransformTool::Scale);
        cx.notify();
    }

    // Toolbar action handlers
    pub(in crate::level_editor::ui::panel) fn on_set_time_scale(
        &mut self,
        action: &toolbar::SetTimeScale,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.shared_state.write().play.time_scale = action.0;
        cx.notify();
    }

    pub(in crate::level_editor::ui::panel) fn on_set_multiplayer_mode(
        &mut self,
        action: &toolbar::SetMultiplayerMode,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.shared_state.write().play.multiplayer_mode = action.0;
        cx.notify();
    }

    pub(in crate::level_editor::ui::panel) fn on_set_build_config(
        &mut self,
        action: &toolbar::SetBuildConfig,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.shared_state.write().build.config = action.0;
        cx.notify();
    }

    pub(in crate::level_editor::ui::panel) fn on_set_target_platform(
        &mut self,
        action: &toolbar::SetTargetPlatform,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.shared_state.write().build.target_platform = action.0;
        cx.notify();
    }

    pub(in crate::level_editor::ui::panel) fn on_set_build_mode(
        &mut self,
        action: &toolbar::SetBuildMode,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.shared_state.write().build.mode = action.0;
        cx.notify();
    }

    pub(in crate::level_editor::ui::panel) fn on_set_tool_mode(
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

}
