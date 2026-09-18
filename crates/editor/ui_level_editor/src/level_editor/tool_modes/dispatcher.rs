//! Tool Mode Dispatcher
//!
//! The single dispatch point for routing pointer events, camera updates,
//! and toolbar widget edits into the active tool mode.

use std::sync::Mutex;

use engine_backend::services::gpu_renderer::GpuRenderer;
use engine_backend::services::terrain_edit::{
    FlatTerrain, TerrainEditApi, VolumeDefinition, VolumeId,
};

use super::{
    level_edit::LevelEditMode, CameraFrame, ToolModeContext, ToolModeId, ToolPointerEvent,
    ToolPointerResult, ViewportFrame,
};
use crate::level_editor::state::terrain::{SculptMode, TerrainTarget};
use crate::level_editor::state::LevelEditorState;

/// Hierarchy root a flat world is created at.
///
/// LOD 12 covers +/-65 536 canonical cells (+/-6.5 km) — comfortably past the
/// default +/-102.4 m extent, and past the widest `i16` extent too, so the
/// same root serves every flat world the type can express.
const FLAT_WORLD_ROOT_LOD: u8 = 12;

/// Resident-page budget for one flat world.
///
/// Matches the live runtime's own 8192-page (1 GiB) residency cap: the
/// streaming controller keeps far fewer than this actually refined, so this is
/// a ceiling, not an allocation.
const FLAT_WORLD_MAX_RESIDENT_PAGES: usize = 8_192;

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
    /// A [`super::ToolWidget::Action`] button was pressed.
    Invoke {
        id: &'static str,
    },
}

/// Identifier of the Terrain mode's "create flat world" action.
pub const CREATE_FLAT_WORLD: &str = "create_flat_world";

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
    ///
    /// `terrain` is only consulted by actions that create or address terrain
    /// bodies; value-setting widgets never need it, which is why it is
    /// optional rather than threaded everywhere.
    pub fn dispatch_widget_edit_with_terrain(
        state: &mut LevelEditorState,
        terrain: Option<&TerrainEditApi>,
        edit: &ToolWidgetEdit,
    ) {
        if let ToolWidgetEdit::Invoke { id } = edit {
            if *id == CREATE_FLAT_WORLD {
                Self::create_flat_world(state, terrain);
            }
            return;
        }
        Self::dispatch_widget_edit(state, edit);
    }

    /// Create a flat voxel world and make it the terrain mode's target.
    ///
    /// The volume is created through [`TerrainEditApi::create_volume`], which
    /// is the only door: the editor never touches a terrain runtime handle.
    /// It is centred on the world origin, which is where a level's first flat
    /// world belongs and is trivially findable with the camera-frame key.
    fn create_flat_world(state: &mut LevelEditorState, terrain: Option<&TerrainEditApi>) {
        let Some(api) = terrain else {
            tracing::warn!("cannot create a flat world before the renderer is ready");
            return;
        };
        let index = api.authored_volumes().len();
        let definition = VolumeDefinition {
            volume_id: VolumeId::from_stable_name(&format!("flat-world:{index}")),
            flat: FlatTerrain::centered_on([0; 3]),
            material: 1,
            root_lod: FLAT_WORLD_ROOT_LOD,
            max_resident_pages: FLAT_WORLD_MAX_RESIDENT_PAGES,
        };
        match api.create_volume(definition) {
            Ok(volume_id) => {
                state
                    .editor
                    .terrain
                    .set_target(TerrainTarget::Volume(volume_id.to_hex()));
                tracing::info!(volume = %volume_id.to_hex(), "created a flat voxel world");
            }
            Err(error) => tracing::error!(%error, "failed to create a flat world"),
        }
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
            // Actions need the terrain seam; routed by
            // `dispatch_widget_edit_with_terrain`.
            ToolWidgetEdit::Invoke { .. } => {}
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
