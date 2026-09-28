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

    /// Route the terrain mode's declarative controls into its persistent
    /// authoring settings. Keeping this in one place lets both the toolbar
    /// and the contributed dock panels use the same control identifiers.
    pub fn dispatch_widget_edit(state: &mut LevelEditorState, edit: &ToolWidgetEdit) {
        use crate::level_editor::state::terrain::{BrushShape, SculptMode};
        let terrain = &mut state.editor.terrain;
        match edit {
            ToolWidgetEdit::SetSegmented { id, value } => match (*id, *value) {
                ("sculpt_mode" | "mode", "raise") => terrain.activate_sculpt_tool(SculptMode::Raise),
                ("sculpt_mode" | "mode", "lower") => terrain.activate_sculpt_tool(SculptMode::Lower),
                ("sculpt_mode" | "mode", "flatten") => terrain.activate_sculpt_tool(SculptMode::Flatten),
                ("sculpt_mode" | "mode", "paint") => terrain.activate_sculpt_tool(SculptMode::Paint),
                ("brush_shape", "sphere") => terrain.set_brush_shape(BrushShape::Sphere),
                ("brush_shape", "box") => terrain.set_brush_shape(BrushShape::Box),
                _ => {}
            },
            ToolWidgetEdit::SetSlider { id, value } => match *id {
                "radius" => terrain.set_brush_radius(*value),
                "strength" => terrain.set_brush_strength(*value),
                "falloff" => terrain.set_brush_falloff(*value),
                "material" => terrain.set_brush_material(value.round().max(1.0) as u32),
                _ => {}
            },
            ToolWidgetEdit::SetToggle { id, on } if *id == "paint_foliage" => terrain.set_paint_foliage(*on),
            ToolWidgetEdit::Invoke { .. } | ToolWidgetEdit::SetToggle { .. } => {}
        }
    }

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
