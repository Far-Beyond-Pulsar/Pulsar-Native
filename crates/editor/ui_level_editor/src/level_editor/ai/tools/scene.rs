//! Scene-wide tools: overview, save, undo/redo and Play-In-Editor.

use super::*;
use tool_registry_macros::tool;

/// Overview of the open level. Call this first.
///
/// Returns object counts by kind, the selection, unsaved-changes state,
/// undo/redo availability and Play-In-Editor state.
///
/// Working with the level editor:
/// - A level is a hierarchy of objects. An object has a transform (metres,
///   Euler degrees, scale factors) and a list of components; what an object
///   looks like and does comes from its components (StaticMeshComponent,
///   LightComponent, ...), not from its `kind`, which is only a tag.
/// - Find objects with level_editor_list_objects / level_editor_get_hierarchy.
/// - Spawn with level_editor_spawn_object (components can be attached in the
///   same call). Discover component classes with
///   level_editor_list_component_classes and their fields with
///   level_editor_describe_component_class.
/// - Every edit is undoable (level_editor_undo) and stays in memory until
///   level_editor_save_scene writes the level file.
#[tool(category = "level_editor")]
pub fn level_editor_query_scene(ctx: &ToolContext) -> Result<Value> {
    let state_arc = open_scene(ctx)?;
    let state = state_arc.read();
    let world = state.scene.world();
    let objects = scene_edit::objects::get_all_objects(&world);
    let mut counts_by_kind = std::collections::BTreeMap::new();
    for object in &objects {
        *counts_by_kind.entry(object_kind(&object.object_type)).or_insert(0usize) += 1;
    }
    let pie = &state.play.pie;
    Ok(json!({
        "level_file": state.scene.current_scene.as_ref().map(|p| p.display().to_string()),
        "has_unsaved_changes": state.scene.has_unsaved_changes,
        "editor_mode": format!("{:?}", state.scene.editor_mode),
        "object_count": objects.len(),
        "root_object_count": scene_edit::objects::root_count(&world),
        "counts_by_kind": counts_by_kind,
        "selected_object_id": scene_edit::objects::get_selected_object_id(&world),
        "can_undo": state.scene.can_undo(),
        "can_redo": state.scene.can_redo(),
        "play": {
            "building": pie.building,
            "running": pie.active,
            "paused": pie.paused,
            "last_error": pie.last_error,
        },
    }))
}

/// Save the level to its file on disk (the editor's Save).
///
/// Edits made by any tool live only in the editor until this is called.
#[tool(category = "level_editor")]
pub fn level_editor_save_scene(ctx: &ToolContext) -> Result<Value> {
    let state_arc = open_scene(ctx)?;
    let path = state_arc
        .read()
        .scene
        .current_scene
        .clone()
        .ok_or_else(|| anyhow!("This level has no file yet; the user must use Save As first"))?;
    {
        let state = state_arc.read();
        let world = state.scene.world();
        scene_edit::level_io::save_to_file(&world, &path).map_err(|e| anyhow!(e))?;
    }
    state_arc.write().scene.has_unsaved_changes = false;
    crate::level_editor::request_thumbnail_capture(&state_arc);
    Ok(json!({ "saved": path.display().to_string() }))
}

fn step_history(ctx: &ToolContext, steps: Option<u32>, redo: bool) -> Result<Value> {
    let state_arc = open_scene(ctx)?;
    let mut state = state_arc.write();
    let requested = steps.unwrap_or(1).max(1);
    let mut applied = 0;
    while applied < requested {
        let ok = if redo { state.scene.redo() } else { state.scene.undo() };
        if !ok {
            break;
        }
        applied += 1;
    }
    if applied > 0 {
        state.scene.bump_revision(true);
        state.scene.pending_renderer_resync = true;
    }
    Ok(json!({
        "applied": applied,
        "can_undo": state.scene.can_undo(),
        "can_redo": state.scene.can_redo(),
    }))
}

/// Undo the most recent edits (the user's or any tool's), like Ctrl+Z.
///
/// # Arguments
/// * `steps` - How many edits to undo. Default 1.
#[tool(category = "level_editor")]
pub fn level_editor_undo(ctx: &ToolContext, steps: Option<u32>) -> Result<Value> {
    step_history(ctx, steps, false)
}

/// Redo edits undone by level_editor_undo, like Ctrl+Y.
///
/// # Arguments
/// * `steps` - How many edits to redo. Default 1.
#[tool(category = "level_editor")]
pub fn level_editor_redo(ctx: &ToolContext, steps: Option<u32>) -> Result<Value> {
    step_history(ctx, steps, true)
}

/// Start, stop, pause, resume or single-step Play-In-Editor.
///
/// Starting builds the game and runs it against a snapshot of the level;
/// stopping restores the level exactly as it was before Play. Starting is
/// asynchronous: poll level_editor_query_scene (`play.building` /
/// `play.running` / `play.last_error`) to follow it.
///
/// # Arguments
/// * `action` - One of: play, stop, pause, resume, step.
/// * `frames` - For `step`: how many frames to advance while paused. Default 1.
#[tool(category = "level_editor")]
pub fn level_editor_play_control(
    ctx: &ToolContext,
    action: String,
    frames: Option<u32>,
) -> Result<Value> {
    let state_arc = open_scene(ctx)?;
    match action.as_str() {
        "play" => state_arc.write().play.pie.play_requested = true,
        "stop" => {
            if state_arc.read().scene.is_edit_mode() {
                bail!("Play-In-Editor is not running");
            }
            crate::level_editor::ui::panel::pie::end_pie(state_arc.clone())
        }
        "pause" | "resume" => {
            let mut state = state_arc.write();
            if !state.play.pie.active {
                bail!("The game is not running");
            }
            state.play.pie.pause_request = Some(action == "pause");
        }
        "step" => {
            let mut state = state_arc.write();
            if !state.play.pie.active {
                bail!("The game is not running");
            }
            state.play.pie.step_request += frames.unwrap_or(1).max(1);
        }
        other => bail!("Unknown action '{other}'. Use play, stop, pause, resume or step."),
    }
    Ok(json!({ "requested": action }))
}
