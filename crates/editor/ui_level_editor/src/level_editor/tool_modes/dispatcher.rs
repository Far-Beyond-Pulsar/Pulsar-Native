//! Tool Mode Dispatcher
//!
//! The single dispatch point for routing pointer events, camera updates,
//! and toolbar widget edits into the active tool mode.

use std::sync::Mutex;

use engine_backend::services::gpu_renderer::GpuRenderer;
use engine_backend::services::terrain_edit::TerrainEditApi;

use super::{
    level_edit::LevelEditMode, CameraFrame, ToolModeContext, ToolModeId, ToolPointerEvent,
    ToolPointerResult, ViewportFrame,
};
use crate::level_editor::state::terrain::SculptMode;
use crate::level_editor::state::LevelEditorState;

// ── Tool Widget Edit ───────────────────────────────────────────────────────

/// Represents an edit operation triggered from a declarative [`super::ToolWidget`].
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
}

// ── ToolModeDispatcher ─────────────────────────────────────────────────────

/// Centralized dispatcher for tool mode operations.
pub struct ToolModeDispatcher;

impl ToolModeDispatcher {
    /// Dispatches a pointer event to the active tool mode.
    pub fn dispatch_pointer(
        state: &mut LevelEditorState,
        gpu_engine: &Mutex<GpuRenderer>,
        terrain: Option<&TerrainEditApi>,
        event: &ToolPointerEvent,
        camera: CameraFrame,
        viewport: ViewportFrame,
    ) -> ToolPointerResult {
        // Temporarily swap selected mode out to allow constructing ToolModeContext with &mut state
        let (mut active_mode, idx) = state
            .editor
            .tool_mode_registry
            .swap_selected(Box::new(LevelEditMode));

        let result = {
            let mut ctx = ToolModeContext {
                state,
                gpu_engine,
                terrain,
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

    /// Dispatches a widget edit from a toolbar control into editor state.
    pub fn dispatch_widget_edit(state: &mut LevelEditorState, edit: &ToolWidgetEdit) {
        match edit {
            ToolWidgetEdit::SetSegmented { id, value } => {
                if *id == "sculpt_mode" || *id == "mode" {
                    match *value {
                        "raise" => state.editor.terrain.set_sculpt_mode(SculptMode::Raise),
                        "lower" => state.editor.terrain.set_sculpt_mode(SculptMode::Lower),
                        "flatten" => state.editor.terrain.set_sculpt_mode(SculptMode::Flatten),
                        "paint" => state.editor.terrain.set_sculpt_mode(SculptMode::Paint),
                        _ => {}
                    }
                }
            }
            ToolWidgetEdit::SetSlider { id, value } => match *id {
                "radius" => state.editor.terrain.set_brush_radius(*value),
                "strength" => state.editor.terrain.set_brush_strength(*value),
                "falloff" => state.editor.terrain.set_brush_falloff(*value),
                _ => {}
            },
            ToolWidgetEdit::SetToggle { .. } => {}
        }
    }

    /// Switches tool mode with full context lifecycle notifications (on_mode_exited/entered).
    pub fn select_tool_mode(
        state: &mut LevelEditorState,
        gpu_engine: &Mutex<GpuRenderer>,
        terrain: Option<&TerrainEditApi>,
        id: ToolModeId,
        camera: CameraFrame,
        viewport: ViewportFrame,
    ) {
        if state.editor.tool_mode_registry.selected_id() == id {
            return;
        }

        // Notify old mode
        let (mut old_mode, old_idx) = state
            .editor
            .tool_mode_registry
            .swap_selected(Box::new(LevelEditMode));
        {
            let mut ctx = ToolModeContext {
                state,
                gpu_engine,
                terrain,
                camera,
                viewport,
            };
            old_mode.on_mode_exited(&mut ctx);
        }
        state
            .editor
            .tool_mode_registry
            .restore_swapped(old_idx, old_mode);

        // Update selected ID
        state.editor.tool_mode_registry.select(id, None);

        // Notify new mode
        let (mut new_mode, new_idx) = state
            .editor
            .tool_mode_registry
            .swap_selected(Box::new(LevelEditMode));
        {
            let mut ctx = ToolModeContext {
                state,
                gpu_engine,
                terrain,
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
