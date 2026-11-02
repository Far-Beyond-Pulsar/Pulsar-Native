//! Compiling a shader-graph material for an editor preview.
//!
//! Placed meshes get their graph materials through SceneDB's texture store
//! ([`crate::components::StaticMeshDraw`]). A standalone preview (the mesh
//! viewer) has no scene, so it binds the graph's textures itself: this
//! compiles the graph exactly as the renderer does, with texture slots
//! numbered `0..n` in the order of [`GraphPreview::textures`].

use std::path::{Path, PathBuf};

use crate::material_graph::{compile_material_graph, texture_assets};

/// A graph material ready to host in a preview shader.
#[derive(Debug)]
pub struct GraphPreview {
    /// Radiant graph snippet (declarations + surface body) for
    /// `RadiantTemplate::build_shader_source`.
    pub snippet: String,
    /// Image file bound at texture slot `i`.
    pub textures: Vec<PathBuf>,
}

/// The graph file for a material assignment: a folder holding
/// `shader_graph_save.json`, or that file itself. `None` for anything else
/// (a scalar `.mat`).
fn graph_file(path: &Path) -> Option<PathBuf> {
    let file = if path.is_dir() {
        path.join("shader_graph_save.json")
    } else if path
        .file_name()
        .is_some_and(|n| n == "shader_graph_save.json")
    {
        path.to_path_buf()
    } else {
        return None;
    };
    file.is_file().then_some(file)
}

/// Compile the graph material `material_asset` (project-relative). `None`
/// when the assignment is not a graph material.
pub fn compile_graph_preview(
    project_root: &Path,
    material_asset: &str,
) -> Option<Result<GraphPreview, String>> {
    let path = crate::subsystems::resolve_asset_path(project_root, material_asset);
    let file = graph_file(&path)?;
    Some(compile_file(&file, project_root))
}

fn compile_file(file: &Path, project_root: &Path) -> Result<GraphPreview, String> {
    let bytes = std::fs::read(file).map_err(|error| error.to_string())?;
    let text = std::str::from_utf8(&bytes).map_err(|error| error.to_string())?;
    // Shader graph saves may carry a line comment before their JSON body.
    let start = text
        .find('{')
        .ok_or_else(|| "shader graph JSON object is missing".to_string())?;
    let document: serde_json::Value = serde_json::from_str(&text[start..])
        .map_err(|error| format!("invalid shader graph JSON: {error}"))?;
    let graph: psgc::GraphDescription = serde_json::from_value(
        document
            .get("main_graph")
            .ok_or_else(|| "shader graph asset has no main_graph".to_string())?
            .clone(),
    )
    .map_err(|error| format!("invalid main_graph: {error}"))?;

    let assets = texture_assets(&graph)?;
    let bindings = assets
        .iter()
        .enumerate()
        .map(|(slot, asset)| (asset.clone(), slot as u32))
        .collect();
    let textures = assets
        .iter()
        .map(|asset| {
            let path = Path::new(asset);
            if path.is_absolute() {
                path.to_path_buf()
            } else {
                project_root.join(path)
            }
        })
        .collect();
    let mut snippet = compile_material_graph(&graph, &bindings)?;
    // The standalone viewer has no scene material row, so specialize the
    // stable material fields to its own densely bound texture table.
    for (ordinal, field) in crate::material_graph::GRAPH_TEXTURE_FIELDS
        .iter()
        .enumerate()
    {
        snippet = snippet
            .replace(
                &format!("scene_textures[material.{field}]"),
                &format!("scene_textures[{ordinal}u]"),
            )
            .replace(
                &format!("scene_samplers[material.{field}]"),
                &format!("scene_samplers[{ordinal}u]"),
            );
    }
    Ok(GraphPreview { snippet, textures })
}
