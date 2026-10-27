//! Terrain mode: create the level's voxel world from a ready-made preset,
//! see its voxel terrains, and manage foliage sets. Voxel worlds are
//! sculpted with brushes (the voxel sculpt mode).

pub mod panels;

use std::sync::Arc;

use gpui::AppContext;
use rust_i18n::t;

use super::{
    ModeLayout, StatusReadout, ToolMode, ToolModeContext, ToolModeId, ToolPointerEvent,
    ToolPointerResult,
};

#[derive(Clone, Copy, Default)]
pub struct TerrainMode;

impl ToolMode for TerrainMode {
    fn build_panels(
        &self,
        ctx: &mut super::ModePanelContext<'_, '_>,
    ) -> Vec<Arc<dyn ui::dock::PanelView>> {
        let state = ctx.state.clone();
        let foliage = {
            let window = &mut *ctx.window;
            ctx.cx
                .new(|cx| panels::FoliageSetsPanel::new(state, window, cx))
        };
        let state = ctx.state.clone();
        let terrain = {
            let window = &mut *ctx.window;
            ctx.cx
                .new(|cx| panels::TerrainPanel::new(state, foliage.clone(), window, cx))
        };
        vec![Arc::new(terrain)]
    }

    fn id(&self) -> ToolModeId {
        ToolModeId::TERRAIN
    }
    fn label_key(&self) -> &'static str {
        "LevelEditor.ToolMode.Terrain"
    }
    fn icon(&self) -> ui::IconName {
        ui::IconName::Globe
    }
    fn description_key(&self) -> &'static str {
        "LevelEditor.ToolMode.TerrainDesc"
    }
    fn on_mode_entered(&mut self, _ctx: &mut ToolModeContext) {}
    fn on_mode_exited(&mut self, _ctx: &mut ToolModeContext) {}
    fn layout(&self) -> ModeLayout {
        ModeLayout {
            show_right_dock: true,
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
