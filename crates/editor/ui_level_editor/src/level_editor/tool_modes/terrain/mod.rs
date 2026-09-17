//! Terrain Tool Mode
//!
//! Authoring mode for voxel terrain sculpting and foliage painting.
//! Surfaces sculpt mode (Raise/Lower/Flatten/Paint), brush radius, strength,
//! and falloff controls. In Phase 1, pointer events pass through as placeholder.

use super::{
    BrushCursor, StatusReadout, ToolMode, ToolModeContext, ToolModeId, ToolPointerEvent,
    ToolPointerResult, ToolWidget,
};
use crate::level_editor::state::terrain::SculptMode;

/// Tool mode for voxel terrain editing and foliage painting.
#[derive(Clone, Default)]
pub struct TerrainMode;

impl ToolMode for TerrainMode {
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

    fn brush_cursor(&self, _ctx: &ToolModeContext) -> Option<BrushCursor> {
        None
    }

    fn toolbar_controls(&self, ctx: &ToolModeContext) -> Vec<ToolWidget> {
        let sculpt = &ctx.state.editor.terrain.sculpt;
        let selected_mode_str = match sculpt.mode {
            SculptMode::Raise => "raise",
            SculptMode::Lower => "lower",
            SculptMode::Flatten => "flatten",
            SculptMode::Paint => "paint",
        };

        vec![
            ToolWidget::Segmented {
                id: "sculpt_mode",
                options: vec![
                    ("LevelEditor.Terrain.Raise", "raise"),
                    ("LevelEditor.Terrain.Lower", "lower"),
                    ("LevelEditor.Terrain.Flatten", "flatten"),
                    ("LevelEditor.Terrain.Paint", "paint"),
                ],
                selected: selected_mode_str,
            },
            ToolWidget::Divider,
            ToolWidget::Slider {
                id: "radius",
                label_key: "LevelEditor.Terrain.Radius",
                value: sculpt.radius_m,
                min: 1.0,
                max: 64.0,
                step: 0.5,
            },
            ToolWidget::Slider {
                id: "strength",
                label_key: "LevelEditor.Terrain.Strength",
                value: sculpt.strength,
                min: 0.1,
                max: 10.0,
                step: 0.1,
            },
            ToolWidget::Slider {
                id: "falloff",
                label_key: "LevelEditor.Terrain.Falloff",
                value: sculpt.falloff,
                min: 0.0,
                max: 1.0,
                step: 0.05,
            },
        ]
    }

    fn status(&self, ctx: &ToolModeContext) -> Option<StatusReadout> {
        let sculpt = &ctx.state.editor.terrain.sculpt;
        Some(StatusReadout {
            text: format!("Radius: {:.1}m | Strength: {:.1}", sculpt.radius_m, sculpt.strength),
            tooltip: None,
        })
    }

    fn on_pointer(
        &mut self,
        _event: &ToolPointerEvent,
        _ctx: &mut ToolModeContext,
    ) -> ToolPointerResult {
        // Phase 1 placeholder: pass through per design doc
        ToolPointerResult::PassThrough
    }

    fn clone_box(&self) -> Box<dyn ToolMode> {
        Box::new(self.clone())
    }
}
