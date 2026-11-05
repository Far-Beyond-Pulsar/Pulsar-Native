//! Terrain mode: create the level's voxel world from a ready-made preset and
//! see its voxel terrains. Voxel worlds are sculpted with brushes (the voxel
//! sculpt mode); foliage is the foliage plugin's Foliage mode.

pub mod panel;

use std::sync::Arc;

use gpui::AppContext;
use rust_i18n::t;

use ui_level_editor::{
    ModeLayout, StatusReadout, ToolMode, ToolModeContext, ToolModeId, ToolPointerEvent,
    ToolPointerResult,
};

#[derive(Clone, Copy, Default)]
pub struct TerrainMode;

impl ToolMode for TerrainMode {
    fn build_panels(
        &self,
        ctx: &mut ui_level_editor::ModePanelContext<'_, '_>,
    ) -> Vec<Arc<dyn ui::dock::PanelView>> {
        let state = ctx.state.clone();
        let window = &mut *ctx.window;
        let terrain = ctx.cx.new(|cx| panel::TerrainPanel::new(state, window, cx));
        vec![Arc::new(terrain)]
    }

    fn id(&self) -> ToolModeId {
        super::TERRAIN_MODE
    }
    fn label(&self) -> String {
        rust_i18n::t!("LevelEditor.ToolMode.Terrain").to_string()
    }
    fn icon(&self) -> ui::IconName {
        ui::IconName::Globe
    }
    fn description(&self) -> String {
        rust_i18n::t!("LevelEditor.ToolMode.TerrainDesc").to_string()
    }
    fn on_mode_entered(&mut self, _ctx: &mut ToolModeContext) {}
    fn on_mode_exited(&mut self, _ctx: &mut ToolModeContext) {}
    fn layout(&self) -> ModeLayout {
        ModeLayout {
            show_right_dock: true,
            dock_width: Some(380.0),
        }
    }
    fn status(&self, _ctx: &ToolModeContext) -> Option<StatusReadout> {
        Some(StatusReadout {
            text: t!("LevelEditor.Terrain.Status").to_string(),
            tooltip: Some(t!("LevelEditor.Terrain.NavigationHint").to_string()),
        })
    }
    fn on_pointer(
        &mut self,
        _event: &ToolPointerEvent,
        _ctx: &mut ToolModeContext,
    ) -> ToolPointerResult {
        ToolPointerResult::PassThrough
    }
    fn clone_box(&self) -> Box<dyn ToolMode> {
        Box::new(*self)
    }
}
