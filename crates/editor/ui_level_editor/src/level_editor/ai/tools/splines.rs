//! Spline tools: the Spline panel's authoring, driven by data.
//!
//! A spline is an ordinary object whose curve lives in its
//! `editor_spline` property; edits go through `UpdateObject`, so they are
//! undoable like the panel's.

use super::*;
use crate::level_editor::core::splines::{self, SPLINE_PROPERTY};
use crate::level_editor::scene_edit::Transform;
use crate::level_editor::state::spline::{CurveAlgorithm, SplineData, SplinePoint};
use tool_registry_macros::tool;

const ALGORITHMS: &str = "linear, catmull_rom, bezier, hermite, b_spline";

fn parse_algorithm(name: &str) -> Result<CurveAlgorithm> {
    Ok(match name.to_ascii_lowercase().replace(['-', ' '], "_").as_str() {
        "linear" => CurveAlgorithm::Linear,
        "catmull_rom" | "catmullrom" => CurveAlgorithm::CatmullRom,
        "bezier" => CurveAlgorithm::Bezier,
        "hermite" => CurveAlgorithm::Hermite,
        "b_spline" | "bspline" => CurveAlgorithm::BSpline,
        other => bail!("Unknown algorithm '{other}'. Use one of: {ALGORITHMS}"),
    })
}

fn spline_json(object: &SceneObjectData, curve: &SplineData) -> Value {
    json!({
        "id": object.id,
        "name": object.name,
        "position": object.transform.position,
        "point_count": curve.points.len(),
        "points": curve.points,
        "closed": curve.closed,
        "algorithm": format!("{:?}", curve.algorithm),
        "resolution": curve.resolution,
        "tension": curve.tension,
        "length_m": curve.total_length_m(),
    })
}

fn check_curve(curve: &SplineData) -> Result<()> {
    if curve.points.len() > 4096 {
        bail!("A spline holds at most 4096 points");
    }
    let finite = curve
        .points
        .iter()
        .all(|p| p.position.iter().chain(&p.arrive).chain(&p.leave).all(|v| v.is_finite()));
    if !finite || !curve.tension.is_finite() {
        bail!("Spline values must be finite numbers");
    }
    Ok(())
}

/// List the level's splines with their points and settings.
#[tool(category = "level_editor")]
pub fn level_editor_list_splines(ctx: &ToolContext) -> Result<Value> {
    let state_arc = open_scene(ctx)?;
    let state = state_arc.read();
    let rows: Vec<Value> = splines::all(&state)
        .iter()
        .map(|(object, curve)| spline_json(object, curve))
        .collect();
    Ok(json!({ "count": rows.len(), "splines": rows }))
}

/// Create a spline (roads, rails, paths, fences, camera tracks).
///
/// # Arguments
/// * `points` - Control points `[x, y, z]` in metres, relative to `position`.
///   At least 2.
/// * `name` - Display name. Default "Spline N".
/// * `position` - The spline object's `[x, y, z]` in metres. Default origin.
/// * `parent_id` - Parent object id; omit for a root object.
/// * `closed` - Join the last point back to the first. Default false.
/// * `algorithm` - linear, catmull_rom (default), bezier, hermite, b_spline.
/// * `auto_tangents` - Compute smooth tangents for the points. Default true.
#[tool(category = "level_editor")]
pub fn level_editor_create_spline(
    ctx: &ToolContext,
    points: Vec<[f32; 3]>,
    name: Option<String>,
    position: Option<[f32; 3]>,
    parent_id: Option<String>,
    closed: Option<bool>,
    algorithm: Option<String>,
    auto_tangents: Option<bool>,
) -> Result<Value> {
    if points.len() < 2 {
        bail!("A spline needs at least 2 points");
    }
    let mut curve = SplineData {
        points: points.into_iter().map(SplinePoint::new).collect(),
        closed: closed.unwrap_or(false),
        ..SplineData::default()
    };
    if let Some(algorithm) = algorithm {
        curve.algorithm = parse_algorithm(&algorithm)?;
    }
    if auto_tangents.unwrap_or(true) {
        curve.auto_tangents();
    }
    check_curve(&curve)?;

    let state_arc = edit_scene(ctx)?;
    let mut state = state_arc.write();
    if let Some(parent) = &parent_id {
        require_object(&state, parent)?;
    }
    let name = name.unwrap_or_else(|| format!("Spline {}", splines::all(&state).len() + 1));
    let mut object = SceneObjectData {
        id: String::new(),
        name,
        object_type: ObjectType::Empty,
        transform: Transform {
            position: position.unwrap_or_default(),
            ..Transform::default()
        },
        visible: true,
        locked: false,
        parent: parent_id.clone(),
        children: vec![],
        scene_path: String::new(),
        props: Default::default(),
        component_instances: None,
    };
    object
        .props
        .insert(SPLINE_PROPERTY.into(), serde_json::to_value(&curve)?);
    let result = execute_command(&mut state, SceneCommand::AddObject { data: object, parent_id });
    let id = result
        .affected_ids
        .first()
        .cloned()
        .ok_or_else(|| anyhow!("Spline could not be added: {}", result.no_op_reason))?;
    let object = require_object(&state, &id)?;
    Ok(spline_json(&object, &curve))
}

/// Edit a spline: replace or append points, change settings, or run one of
/// the Spline panel's operations. Applied in that order.
///
/// # Arguments
/// * `id` - The spline object's id.
/// * `points` - Replace all control points with these `[x, y, z]` (metres,
///   relative to the spline object).
/// * `append_points` - Add these points to the end.
/// * `closed` - Join the last point back to the first.
/// * `algorithm` - linear, catmull_rom, bezier, hermite, b_spline.
/// * `resolution` - Samples drawn per segment (1–256).
/// * `tension` - Curve tension, usually -1..1.
/// * `operation` - One of: auto_tangents, reverse, smooth, resample.
/// * `amount` - For `smooth`: strength 0..1 (default 0.5). For `resample`:
///   the new point count (default: current count).
#[tool(category = "level_editor")]
pub fn level_editor_edit_spline(
    ctx: &ToolContext,
    id: String,
    points: Option<Vec<[f32; 3]>>,
    append_points: Option<Vec<[f32; 3]>>,
    closed: Option<bool>,
    algorithm: Option<String>,
    resolution: Option<u32>,
    tension: Option<f32>,
    operation: Option<String>,
    amount: Option<f32>,
) -> Result<Value> {
    let state_arc = edit_scene(ctx)?;
    let mut state = state_arc.write();
    let mut object = require_object(&state, &id)?;
    if object.locked {
        bail!("Spline '{id}' is locked; unlock it with level_editor_set_object_flags");
    }
    let mut curve = splines::data(&object)
        .ok_or_else(|| anyhow!("'{id}' is not a spline. Use level_editor_list_splines."))?;
    let mut new_points = false;
    if let Some(points) = points {
        curve.points = points.into_iter().map(SplinePoint::new).collect();
        new_points = true;
    }
    if let Some(points) = append_points {
        curve.points.extend(points.into_iter().map(SplinePoint::new));
        new_points = true;
    }
    if let Some(closed) = closed {
        curve.closed = closed;
    }
    if let Some(algorithm) = algorithm {
        curve.algorithm = parse_algorithm(&algorithm)?;
    }
    if let Some(resolution) = resolution {
        curve.resolution = resolution.clamp(1, 256);
    }
    if let Some(tension) = tension {
        curve.tension = tension;
    }
    match operation.as_deref() {
        None if new_points => curve.auto_tangents(),
        None => {}
        Some("auto_tangents") => curve.auto_tangents(),
        Some("reverse") => curve.reverse(),
        Some("smooth") => curve.smooth(amount.unwrap_or(0.5).clamp(0.0, 1.0)),
        Some("resample") => {
            let count = amount.map(|a| a.round() as usize).unwrap_or(curve.points.len());
            curve.resample(count.clamp(2, 4096));
        }
        Some(other) => bail!("Unknown operation '{other}'. Use auto_tangents, reverse, smooth or resample."),
    }
    check_curve(&curve)?;
    object
        .props
        .insert(SPLINE_PROPERTY.into(), serde_json::to_value(&curve)?);
    let result = execute_command(&mut state, SceneCommand::UpdateObject { data: object });
    let object = require_object(&state, &id)?;
    let mut out = spline_json(&object, &curve);
    out["changed"] = json!(result.changed);
    Ok(out)
}
