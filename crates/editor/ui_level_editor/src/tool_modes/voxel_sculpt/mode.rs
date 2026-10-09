//! Exact voxel edits routed through the registered backend into the
//! terrain's edit journal.

use gpui::{AppContext as _, MouseButton};

use super::super::{
    PointerKind, StatusReadout, ToolMode, ToolModeContext, ToolModeId, ToolPointerEvent,
    ToolPointerResult,
};
use crate::state::voxel::{VoxelSculptMode as Mode, VoxelStroke};

#[derive(Clone, Copy, Default)]
pub struct VoxelSculptMode;

impl ToolMode for VoxelSculptMode {
    fn build_panels(
        &self,
        ctx: &mut super::super::ModePanelContext<'_, '_>,
    ) -> Vec<std::sync::Arc<dyn ui::dock::PanelView>> {
        let state = ctx.state.clone();
        let panel = {
            let window = &mut *ctx.window;
            ctx.cx
                .new(|cx| super::VoxelSculptPanel::new(state, window, cx))
        };
        vec![std::sync::Arc::new(panel)]
    }
    fn id(&self) -> ToolModeId {
        ToolModeId::VOXEL_SCULPT
    }

    fn label_key(&self) -> &'static str {
        "LevelEditor.ToolMode.VoxelSculpt"
    }

    fn icon(&self) -> ui::IconName {
        ui::IconName::Cube
    }

    fn description_key(&self) -> &'static str {
        "LevelEditor.ToolMode.VoxelSculptDesc"
    }

    fn on_mode_entered(&mut self, _ctx: &mut ToolModeContext) {}
    fn on_mode_exited(&mut self, _ctx: &mut ToolModeContext) {}

    fn status(&self, ctx: &ToolModeContext) -> Option<StatusReadout> {
        let sculpt = &ctx.state.editor.voxel;
        let mode = match sculpt.mode {
            Mode::Dig => "Dig",
            Mode::Build => "Build",
            Mode::Paint => "Paint",
        };
        let size = if sculpt.single_block {
            "one block".to_string()
        } else {
            format!("{:.1} m", sculpt.radius_m)
        };
        Some(StatusReadout {
            text: format!(
                "Voxel sculpt · {mode} · {size} · {} · Shift swaps dig and build",
                sculpt.material_name()
            ),
            tooltip: Some("Edits use exact voxels and are saved with the terrain".into()),
        })
    }

    fn on_pointer(
        &mut self,
        event: &ToolPointerEvent,
        ctx: &mut ToolModeContext,
    ) -> ToolPointerResult {
        if event.button != Some(MouseButton::Left) {
            return ToolPointerResult::PassThrough;
        }
        match event.kind {
            PointerKind::Down => VoxelStroke::begin(ctx.state),
            PointerKind::Drag => {}
            PointerKind::Up => {
                VoxelStroke::end(ctx.state);
                return ToolPointerResult::PassThrough;
            }
            _ => return ToolPointerResult::PassThrough,
        }
        ToolPointerResult::VoxelBrush(ctx.state.editor.voxel.request(event.holding_mods.shift))
    }

    fn clone_box(&self) -> Box<dyn ToolMode> {
        Box::new(*self)
    }
}
