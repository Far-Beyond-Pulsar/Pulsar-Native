use super::*;

impl LevelEditorPanel {
    /// Redraw the views that show the transform tool: the toolbar and the
    /// viewport's options bar. Notifying this panel instead would rebuild the
    /// whole editor for a tool switch (#812).
    fn notify_tool_views(&self, cx: &mut Context<Self>) {
        self.toolbar.update(cx, |_, cx| cx.notify());
        if let Some(viewport) = self.viewport_panel.as_ref().and_then(|v| v.upgrade()) {
            viewport.update(cx, |_, cx| cx.notify());
        }
    }

    pub(in crate::ui::panel) fn on_set_transform_snap(
        &mut self,
        action: &toolbar::SetTransformSnap,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let mut state = self.shared_state.write();
        let target = match action.0 {
            0 => &mut state.editor.location_snap,
            1 => &mut state.editor.rotation_snap,
            _ => &mut state.editor.scale_snap,
        };
        if action.1.is_finite() && action.1 > 0.0 {
            *target = action.1;
        }
        let (location, rotation, scale) = (
            state.editor.location_snap,
            state.editor.rotation_snap,
            state.editor.scale_snap,
        );
        if let Some(mailbox) = &self.helio_mailbox {
            mailbox.set_gizmo_snap_settings(location, rotation, scale);
        }
        drop(state);
        let key = match action.0 {
            0 => "location_snap",
            1 => "rotation_snap",
            _ => "scale_snap",
        };
        let value = match action.0 {
            0 => location,
            1 => rotation,
            _ => scale,
        };
        if let Err(error) = engine_state::GlobalSettings::new().set_and_save(
            "viewport",
            key,
            engine_state::ConfigValue::Float(value as f64),
        ) {
            tracing::warn!(%error, "Could not persist viewport snap setting");
        }
        cx.notify();
    }

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
        self.notify_tool_views(cx);
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
        self.notify_tool_views(cx);
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
        self.notify_tool_views(cx);
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
        self.notify_tool_views(cx);
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
