//! Dispatch pointer events and toolbar edits to the active editor mode.

use std::sync::Mutex;

use engine_backend::services::gpu_renderer::GpuRenderer;

use super::{
    CameraFrame, ToolModeContext, ToolModeId, ToolPointerEvent, ToolPointerResult, ViewportFrame,
    level_edit::LevelEditMode,
};
use crate::level_editor::state::LevelEditorState;

/// Edit operation produced by a declarative toolbar widget.
#[derive(Clone, Debug, PartialEq)]
pub enum ToolWidgetEdit {
    SetSegmented {
        id: &'static str,
        value: &'static str,
    },
    SetSlider {
        id: &'static str,
        value: f32,
    },
    SetToggle {
        id: &'static str,
        on: bool,
    },
    Invoke {
        id: &'static str,
    },
}

pub struct ToolModeDispatcher;

impl ToolModeDispatcher {
    pub fn dispatch_pointer(
        state: &mut LevelEditorState,
        gpu_engine: &Mutex<GpuRenderer>,
        event: &ToolPointerEvent,
        camera: CameraFrame,
        viewport: ViewportFrame,
    ) -> ToolPointerResult {
        let (mut active_mode, idx) = state
            .editor
            .tool_mode_registry
            .swap_selected(Box::new(LevelEditMode));
        let result = {
            let mut ctx = ToolModeContext {
                state,
                gpu_engine,
                camera,
                viewport,
            };
            active_mode.on_pointer(event, &mut ctx)
        };
        state
            .editor
            .tool_mode_registry
            .restore_swapped(idx, active_mode);
        result
    }

    /// The remaining built-in mode widgets are read-only status displays.
    pub fn dispatch_widget_edit(_state: &mut LevelEditorState, _edit: &ToolWidgetEdit) {}

    pub fn select_tool_mode(
        state: &mut LevelEditorState,
        gpu_engine: &Mutex<GpuRenderer>,
        id: ToolModeId,
        camera: CameraFrame,
        viewport: ViewportFrame,
    ) {
        if state.editor.tool_mode_registry.selected_id() == id {
            return;
        }

        let (mut old_mode, old_idx) = state
            .editor
            .tool_mode_registry
            .swap_selected(Box::new(LevelEditMode));
        {
            let mut ctx = ToolModeContext {
                state,
                gpu_engine,
                camera,
                viewport,
            };
            old_mode.on_mode_exited(&mut ctx);
        }
        state
            .editor
            .tool_mode_registry
            .restore_swapped(old_idx, old_mode);

        state.editor.tool_mode_registry.select(id, None);

        let (mut new_mode, new_idx) = state
            .editor
            .tool_mode_registry
            .swap_selected(Box::new(LevelEditMode));
        {
            let mut ctx = ToolModeContext {
                state,
                gpu_engine,
                camera,
                viewport,
            };
            new_mode.on_mode_entered(&mut ctx);
        }
        state
            .editor
            .tool_mode_registry
            .restore_swapped(new_idx, new_mode);
    }
}
