//! Viewport tools: transform tool, camera view and display toggles.

use super::*;
use crate::{CameraMode, TransformTool};
use tool_registry_macros::tool;

/// Change viewport settings the user controls from the toolbar. Omitted
/// fields are unchanged; the result reports every current setting.
///
/// # Arguments
/// * `transform_tool` - Gizmo for the selection: select, move, rotate, scale.
/// * `camera_mode` - perspective, orthographic, top, front, side.
/// * `show_grid` - Show the ground grid.
/// * `show_wireframe` - Draw wireframes.
/// * `show_lighting` - Lit (true) or unlit (false) view.
/// * `camera_move_speed` - Fly-camera speed, 0.5–100.
#[tool(category = "level_editor")]
pub fn level_editor_set_view_options(
    ctx: &ToolContext,
    transform_tool: Option<String>,
    camera_mode: Option<String>,
    show_grid: Option<bool>,
    show_wireframe: Option<bool>,
    show_lighting: Option<bool>,
    camera_move_speed: Option<f32>,
) -> Result<Value> {
    let tool = transform_tool
        .map(|t| match t.as_str() {
            "select" => Ok(TransformTool::Select),
            "move" => Ok(TransformTool::Move),
            "rotate" => Ok(TransformTool::Rotate),
            "scale" => Ok(TransformTool::Scale),
            other => Err(anyhow!("Unknown transform_tool '{other}'")),
        })
        .transpose()?;
    let mode = camera_mode
        .map(|m| match m.as_str() {
            "perspective" => Ok(CameraMode::Perspective),
            "orthographic" => Ok(CameraMode::Orthographic),
            "top" => Ok(CameraMode::Top),
            "front" => Ok(CameraMode::Front),
            "side" => Ok(CameraMode::Side),
            other => Err(anyhow!("Unknown camera_mode '{other}'")),
        })
        .transpose()?;

    let state_arc = open_scene(ctx)?;
    let mut state = state_arc.write();
    let editor = &mut state.editor;
    if let Some(tool) = tool {
        editor.set_tool(tool);
    }
    if let Some(mode) = mode {
        editor.set_camera_mode(mode);
    }
    if let Some(v) = show_grid {
        editor.show_grid = v;
    }
    if let Some(v) = show_wireframe {
        editor.show_wireframe = v;
    }
    if let Some(v) = show_lighting {
        editor.show_lighting = v;
    }
    if let Some(speed) = camera_move_speed {
        editor.camera_move_speed = speed.clamp(0.5, 100.0);
    }
    let out = json!({
        "transform_tool": format!("{:?}", editor.current_tool).to_lowercase(),
        "camera_mode": format!("{:?}", editor.camera_mode).to_lowercase(),
        "show_grid": editor.show_grid,
        "show_wireframe": editor.show_wireframe,
        "show_lighting": editor.show_lighting,
        "camera_move_speed": editor.camera_move_speed,
    });
    // Toolbar and viewport views watch the revision.
    state.scene.bump_revision(false);
    Ok(out)
}
