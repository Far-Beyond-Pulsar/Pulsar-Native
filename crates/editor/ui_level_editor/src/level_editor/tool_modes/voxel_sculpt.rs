//! Exact voxel edits routed through the registered backend into the
//! terrain's edit journal.

use gpui::MouseButton;
use helio_voxel_data::VoxelBrushShape;

use super::{
    PointerKind, StatusReadout, ToolMode, ToolModeContext, ToolModeId, ToolPointerEvent,
    ToolPointerResult, ToolWidget,
};
use crate::level_editor::state::voxel::{VoxelSculptMode as Mode, VoxelStroke, MATERIALS, MAX_RADIUS_M, MIN_RADIUS_M};

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

    fn toolbar_controls(&self, ctx: &ToolModeContext) -> Vec<ToolWidget> {
        let sculpt = ctx.state.editor.voxel;
        vec![
            ToolWidget::Divider,
            ToolWidget::Segmented {
                id: "voxel_mode",
                options: vec![
                    ("LevelEditor.Voxel.Dig", "dig"),
                    ("LevelEditor.Voxel.Build", "build"),
                    ("LevelEditor.Voxel.Paint", "paint"),
                ],
                selected: match sculpt.mode {
                    Mode::Dig => "dig",
                    Mode::Build => "build",
                    Mode::Paint => "paint",
                },
            },
            ToolWidget::Segmented {
                id: "voxel_shape",
                options: vec![("LevelEditor.Voxel.Sphere", "sphere"), ("LevelEditor.Voxel.Cube", "cube")],
                selected: match sculpt.shape {
                    VoxelBrushShape::Sphere => "sphere",
                    VoxelBrushShape::Cube => "cube",
                },
            },
            ToolWidget::Toggle { id: "voxel_block", label_key: "LevelEditor.Voxel.SingleBlock", on: sculpt.single_block },
            ToolWidget::Slider {
                id: "voxel_radius",
                label_key: "LevelEditor.Voxel.Radius",
                value: sculpt.radius_m,
                min: MIN_RADIUS_M,
                max: MAX_RADIUS_M,
                step: if sculpt.radius_m < 2.0 { 0.1 } else { 1.0 },
            },
            ToolWidget::Slider {
                id: "voxel_material",
                label_key: "LevelEditor.Voxel.Material",
                value: sculpt.material as f32,
                min: *MATERIALS.start() as f32,
                max: *MATERIALS.end() as f32,
                step: 1.0,
            },
        ]
    }

    fn status(&self, ctx: &ToolModeContext) -> Option<StatusReadout> {
        let sculpt = &ctx.state.editor.voxel;
        let mode = match sculpt.mode {
            Mode::Dig => "Dig",
            Mode::Build => "Build",
            Mode::Paint => "Paint",
        };
        let size = if sculpt.single_block { "one block".to_string() } else { format!("{:.1} m", sculpt.radius_m) };
        Some(StatusReadout {
            text: format!("Voxel sculpt · {mode} · {size} · {} · Shift swaps dig and build", sculpt.material_name()),
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
