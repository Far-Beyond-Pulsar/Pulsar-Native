//! Level Edit Tool Mode
//!
//! Default editing mode: select, move, rotate, and scale scene objects
//! via standard gizmos and scene database interaction.
//! All pointer events pass through to standard scene picking.

use super::super::{
    BrushCursor, StatusReadout, ToolMode, ToolModeContext, ToolModeId, ToolPointerEvent,
    ToolPointerResult, ToolWidget,
};

/// Default mode for standard object transformation and scene hierarchy manipulation.
#[derive(Clone, Default)]
pub struct LevelEditMode;

impl ToolMode for LevelEditMode {
    fn id(&self) -> ToolModeId {
        ToolModeId::LEVEL_EDIT
    }

    fn label(&self) -> String {
        rust_i18n::t!("LevelEditor.ToolMode.LevelEdit").to_string()
    }

    fn icon(&self) -> ui::IconName {
        ui::IconName::BoxIso
    }

    fn description(&self) -> String {
        rust_i18n::t!("LevelEditor.ToolMode.LevelEditDesc").to_string()
    }

    fn on_mode_entered(&mut self, _ctx: &mut ToolModeContext) {}

    fn on_mode_exited(&mut self, _ctx: &mut ToolModeContext) {}

    fn brush_cursor(&self, _ctx: &ToolModeContext) -> Option<BrushCursor> {
        None
    }

    fn toolbar_controls(&self, _ctx: &ToolModeContext) -> Vec<ToolWidget> {
        Vec::new()
    }

    fn status(&self, _ctx: &ToolModeContext) -> Option<StatusReadout> {
        None
    }

    fn on_pointer(
        &mut self,
        _event: &ToolPointerEvent,
        _ctx: &mut ToolModeContext,
    ) -> ToolPointerResult {
        // Exact LevelEdit behavior byte-for-byte: pass through to standard renderer pick + gizmo
        ToolPointerResult::PassThrough
    }

    fn clone_box(&self) -> Box<dyn ToolMode> {
        Box::new(self.clone())
    }
}
