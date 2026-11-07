//! Sculpt strokes: each pointer sample is a ray the render thread casts with
//! the frame's camera; the terrain component's `sculpt` method stamps where
//! it hits the terrain as drawn (the same method scripts call).

use gpui::{AppContext as _, MouseButton};

use super::super::state::{VoxelSculptDomain, VoxelSculptMode as Mode, VoxelStroke};
use ui_level_editor::{
    ModeLayout, PointerKind, StatusReadout, ToolMode, ToolModeContext, ToolModeId,
    ToolPointerEvent, ToolPointerResult,
};

#[derive(Clone, Copy, Default)]
pub struct VoxelSculptMode;

impl ToolMode for VoxelSculptMode {
    fn build_panels(
        &self,
        ctx: &mut ui_level_editor::ModePanelContext<'_, '_>,
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
        super::super::SCULPT_MODE
    }

    fn label(&self) -> String {
        rust_i18n::t!("LevelEditor.ToolMode.VoxelSculpt").to_string()
    }

    fn icon(&self) -> ui::IconName {
        ui::IconName::Cube
    }

    fn description(&self) -> String {
        rust_i18n::t!("LevelEditor.ToolMode.VoxelSculptDesc").to_string()
    }

    fn layout(&self) -> ModeLayout {
        ModeLayout {
            show_right_dock: true,
            dock_width: Some(360.0),
        }
    }

    fn on_mode_entered(&mut self, _ctx: &mut ToolModeContext) {}
    fn on_mode_exited(&mut self, _ctx: &mut ToolModeContext) {}

    fn status(&self, ctx: &ToolModeContext) -> Option<StatusReadout> {
        let sculpt = &super::super::state::brush(ctx.state);
        let mode = match sculpt.mode {
            Mode::Dig => "Dig",
            Mode::Build => "Build",
            Mode::Paint => "Paint",
            Mode::Flatten => "Flatten",
            Mode::Smooth => "Smooth",
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
        ToolPointerResult::Ray(sculpt_ray(
            ctx.state
                .editor
                .plugins
                .get_mut::<VoxelSculptDomain>()
                .request(event.holding_mods.shift),
        ))
    }

    fn clone_box(&self) -> Box<dyn ToolMode> {
        Box::new(*self)
    }
}

/// A stroke sample of `request` along the pointer ray, on every enabled,
/// editable voxel terrain: through `VoxelTerrainComponent`'s `sculpt`.
fn sculpt_ray(
    request: voxel_component::sculpt::VoxelBrushRequest,
) -> engine_backend::subsystems::render::ToolRay {
    engine_backend::subsystems::render::ToolRay(std::sync::Arc::new(
        move |world, origin, direction, start| {
            let terrains: Vec<_> = world
                .query::<&voxel_component::VoxelTerrainComponent>()
                .filter(|(entity, terrain)| {
                    terrain.enabled
                        && terrain.editable
                        && engine_backend::scene::attachments::is_enabled(world, *entity)
                })
                .map(|(entity, _)| entity)
                .collect();
            for terrain in terrains {
                if let Err(error) = voxel_component::sculpt::sculpt(
                    world, terrain, origin, direction, request, start,
                ) {
                    tracing::warn!("Voxel sculpt: {error}");
                }
            }
        },
    ))
}
