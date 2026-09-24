//! Exact voxel edits routed through the registered backend and SceneDB recipe.

use gpui::MouseButton;

use super::{
    PointerKind, StatusReadout, ToolMode, ToolModeContext, ToolModeId, ToolPointerEvent,
    ToolPointerResult,
};

#[derive(Clone, Copy, Default)]
pub struct VoxelSculptMode;

impl ToolMode for VoxelSculptMode {
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

    fn status(&self, _ctx: &ToolModeContext) -> Option<StatusReadout> {
        Some(StatusReadout {
            text: "Voxel sculpt · Click or drag to dig · Shift to build".into(),
            tooltip: Some("Edits use exact source voxels and are saved with the terrain".into()),
        })
    }

    fn on_pointer(
        &mut self,
        event: &ToolPointerEvent,
        _ctx: &mut ToolModeContext,
    ) -> ToolPointerResult {
        if !matches!(event.kind, PointerKind::Down | PointerKind::Drag)
            || event.button != Some(MouseButton::Left)
        {
            return ToolPointerResult::PassThrough;
        }
        ToolPointerResult::VoxelBrush {
            radius: 1.5,
            material: if event.holding_mods.shift { 1 } else { 0 },
        }
    }

    fn clone_box(&self) -> Box<dyn ToolMode> {
        Box::new(*self)
    }
}
