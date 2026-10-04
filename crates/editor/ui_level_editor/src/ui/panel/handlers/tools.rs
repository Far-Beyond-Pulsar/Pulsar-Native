use super::*;

impl LevelEditorPanel {
    // Action handlers
    pub(in crate::ui::panel) fn on_select_tool(
        &mut self,
        _: &SelectTool,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.shared_state
            .write()
            .editor
            .set_tool(TransformTool::Select);
        self.queue_gizmo_mode_for_tool(TransformTool::Select);
        cx.notify();
    }

    pub(in crate::ui::panel) fn on_move_tool(
        &mut self,
        _: &MoveTool,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.shared_state
            .write()
            .editor
            .set_tool(TransformTool::Move);
        self.queue_gizmo_mode_for_tool(TransformTool::Move);
        cx.notify();
    }

    pub(in crate::ui::panel) fn on_rotate_tool(
        &mut self,
        _: &RotateTool,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.shared_state
            .write()
            .editor
            .set_tool(TransformTool::Rotate);
        self.queue_gizmo_mode_for_tool(TransformTool::Rotate);
        cx.notify();
    }

    pub(in crate::ui::panel) fn on_scale_tool(
        &mut self,
        _: &ScaleTool,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.shared_state
            .write()
            .editor
            .set_tool(TransformTool::Scale);
        self.queue_gizmo_mode_for_tool(TransformTool::Scale);
        cx.notify();
    }

    // Toolbar action handlers
    pub(in crate::ui::panel) fn on_set_tool_mode(
        &mut self,
        action: &toolbar::SetToolMode,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let camera = self
            .gpu_engine
            .lock()
            .ok()
            .and_then(|engine| engine.editor_camera_state());
        let camera = camera
            .map(|c| crate::tool_modes::CameraFrame {
                position: c.position.map(|coordinate| coordinate as f32),
                yaw: c.yaw,
                pitch: c.pitch,
                fov: 60.0,
            })
            .unwrap_or_default();
        let mut state = self.shared_state.write();
        crate::tool_modes::ToolModeDispatcher::select_tool_mode(
            &mut state,
            &self.gpu_engine,
            action.0,
            camera,
            crate::tool_modes::ViewportFrame::default(),
        );
        drop(state);
        cx.notify();
    }
}
