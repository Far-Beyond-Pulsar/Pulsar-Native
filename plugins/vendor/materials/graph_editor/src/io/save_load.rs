//! Save and load for shader files.
//!
//! This is the boundary between the `.material` file and the editing session: a
//! file is read once into a [`ShaderAsset`] whose graph the canvas takes over as it
//! is, and saving writes an asset made of the canvas's graph. The graph is never
//! converted, so node and connection ids, comments and property values survive a
//! round trip unchanged.

use super::formats::{self, ShaderAsset, ShaderEditorState};
use crate::editor::panel::{DocMeta, ShaderEditorPanel};
use crate::features::compilation::CompilationState;
use gpui::*;
use graphy::GraphDescription;
use graphy_blueprint::GraphViewState;
use graphy_render::Vec2;
use std::path::{Path, PathBuf};

pub const GRAPH_SAVE_FILE_NAME: &str = "shader_graph_save.json";

/// Reads shader text: the asset format, or the legacy blueprint format.
///
/// Legacy blueprint files used a node model that cannot be mapped onto the shader
/// graph, so they are recognised (by Graphy's codec, the one owner of that format)
/// and open as an empty shader graph.
pub fn decode(content: &str) -> Result<ShaderAsset, String> {
    match formats::deserialize_shader(content) {
        Ok(asset) => Ok(asset),
        Err(error) => {
            if graphy_blueprint::legacy::is_legacy_format(&formats::strip_header_comments(content)) {
                Ok(ShaderAsset::with_graph(GraphDescription::new("Shader Graph")))
            } else {
                Err(error)
            }
        }
    }
}

/// The file text of `asset`.
pub fn encode(asset: &ShaderAsset) -> Result<String, String> {
    formats::serialize_shader_with_header(asset)
}

/// Where the graph file of a material is: the material folder holds
/// `shader_graph_save.json`.
pub fn resolve_shader_path(path: &Path) -> PathBuf {
    if path
        .file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name.eq_ignore_ascii_case(GRAPH_SAVE_FILE_NAME))
    {
        return path.to_path_buf();
    }

    if path.extension().is_none() {
        return path.join(GRAPH_SAVE_FILE_NAME);
    }

    path.to_path_buf()
}

impl ShaderEditorPanel {
    /// Save the material to its file path
    pub fn plugin_save(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(path) = self.get_graph_file_path() else {
            tracing::warn!("plugin_save: no save path set - cannot save shader");
            self.compilation_status.state = CompilationState::Error;
            self.compilation_status.message = "Save failed: no file path set".to_string();
            cx.notify();
            return;
        };
        if let Some(error) = &self.load_error {
            self.compilation_status.state = CompilationState::Error;
            self.compilation_status.message =
                format!("Save blocked because the file failed to load: {error}");
            cx.notify();
            return;
        }
        match self.save_to_path(&path, window, cx) {
            Ok(()) => {
                plugin_editor_api::undo::lock(&self.history).mark_saved();
                cx.notify();
            }
            Err(e) => {
                tracing::error!("plugin_save: failed to save shader: {}", e);
                self.compilation_status.state = CompilationState::Error;
                self.compilation_status.message = format!("Save failed: {}", e);
                cx.notify();
            }
        }
    }

    /// Reload the material from its file path
    pub fn plugin_reload(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(path) = self.get_graph_file_path() else {
            tracing::warn!("plugin_reload: no file path set");
            return;
        };
        match self.load_from_path(&path, window, cx) {
            Ok(()) => cx.notify(),
            Err(e) => {
                tracing::error!("plugin_reload: failed: {}", e);
                self.compilation_status.state = CompilationState::Error;
                self.compilation_status.message = format!("Reload failed: {}", e);
                cx.notify();
            }
        }
    }

    /// The asset the session would save now: the canvas's graph with the file's
    /// other fields as loaded and the current view and shader model.
    pub fn to_shader_asset(&self, cx: &App) -> ShaderAsset {
        let canvas = self.canvas.read(cx);
        let mut graph = canvas.graph().clone();
        // Comments say which nodes they hold, by where the nodes are laid out.
        let contained = contained_nodes(canvas);
        for (comment, nodes) in graph.comments.iter_mut().zip(contained) {
            comment.contained_node_ids = nodes;
        }
        let (pan, zoom) = canvas.view();
        let mut editor_state = self
            .meta
            .editor_state
            .clone()
            .unwrap_or_else(ShaderEditorState::new);
        editor_state.graph_view_states.insert(
            "main".to_string(),
            GraphViewState {
                pan_offset_x: pan.x,
                pan_offset_y: pan.y,
                zoom,
            },
        );
        let mut shader_metadata = self.meta.shader_metadata.clone();
        shader_metadata.shader_model = self.shader_model.clone();
        ShaderAsset {
            format_version: self.meta.format_version,
            main_graph: graph,
            editor_state: Some(editor_state),
            shader_metadata,
            extra: self.meta.extra.clone(),
        }
    }

    /// Save shader to a specific path
    pub fn save_to_path(
        &mut self,
        path: &Path,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Result<(), String> {
        let target_path = resolve_shader_path(path);
        let content = encode(&self.to_shader_asset(cx))?;

        if let Some(parent) = target_path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| format!("Failed to create directory: {}", e))?;
        }
        std::fs::write(&target_path, &content).map_err(|e| format!("Failed to write file: {}", e))?;
        tracing::info!("saved {} bytes to {:?}", content.len(), target_path);
        Ok(())
    }

    /// Load shader from a specific path
    pub fn load_from_path(
        &mut self,
        path: &Path,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Result<(), String> {
        let (source_path, asset) = Self::parse_asset_from_path(path)?;
        self.apply_loaded_asset(source_path, asset, window, cx)
    }

    /// Load a shader file (kept for callers that open by path string).
    pub fn load_blueprint(
        &mut self,
        file_path: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Result<(), String> {
        self.load_from_path(&PathBuf::from(file_path), window, cx)
    }

    /// Read and deserialize a shader without touching UI state. Editor startup
    /// uses this on the background executor, then applies the result on the UI
    /// thread once the restored workspace is visible.
    pub(crate) fn parse_asset_from_path(path: &Path) -> Result<(PathBuf, ShaderAsset), String> {
        let source_path = resolve_shader_path(path);
        let content = std::fs::read_to_string(&source_path)
            .map_err(|e| format!("Failed to read file: {}", e))?;
        let asset = decode(&content)?;
        Ok((source_path, asset))
    }

    /// Apply a parsed asset to the live editor. This must run on the UI thread.
    pub(crate) fn apply_loaded_asset(
        &mut self,
        source_path: PathBuf,
        asset: ShaderAsset,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Result<(), String> {
        if !formats::is_version_supported(asset.format_version) {
            return Err(format!(
                "Unsupported shader format version: {}",
                asset.format_version
            ));
        }
        let ShaderAsset {
            format_version,
            main_graph,
            editor_state,
            shader_metadata,
            extra,
        } = asset;
        self.shader_model = shader_metadata.shader_model.clone();
        let view = editor_state
            .as_ref()
            .and_then(|state| state.graph_view_states.get("main"))
            .cloned();
        self.meta = DocMeta {
            format_version,
            editor_state,
            shader_metadata,
            extra,
        };
        self.install_graph(main_graph, cx);
        if let Some(view) = view {
            self.canvas.update(cx, |canvas, cx| {
                canvas.set_view(Vec2::new(view.pan_offset_x, view.pan_offset_y), view.zoom, cx)
            });
        } else {
            self.canvas.update(cx, |canvas, cx| canvas.frame_all(cx));
        }
        self.set_path(source_path);
        if let Some(service) = self.undo_service.clone() {
            self.adopt_history(&service);
        }
        Ok(())
    }

    /// Autosave - called periodically to save work in progress
    pub fn autosave(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.is_dirty() {
            return; // No changes to save
        }
        if let Some(path) = self.get_graph_file_path() {
            // Create autosave path (same location with .autosave extension)
            let autosave_path = path.with_extension("material.autosave");
            match self.save_to_path(&autosave_path, window, cx) {
                Ok(()) => tracing::info!("autosave: saved to {:?}", autosave_path),
                Err(e) => tracing::error!("autosave: failed: {}", e),
            }
        } else {
            tracing::warn!("autosave: no file path set");
        }
    }

    /// Check if an autosave file exists for the current path
    pub fn has_autosave(&self) -> bool {
        self.get_graph_file_path()
            .is_some_and(|path| path.with_extension("material.autosave").exists())
    }

    /// Load from autosave file (recovery)
    pub fn load_autosave(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Result<(), String> {
        let path = self
            .get_graph_file_path()
            .ok_or_else(|| "No file path set - cannot load autosave".to_string())?;
        let autosave_path = path.with_extension("material.autosave");
        self.load_from_path(&autosave_path, window, cx)?;
        // Delete autosave after successful recovery
        std::fs::remove_file(&autosave_path)
            .map_err(|e| format!("Failed to delete autosave file: {}", e))
    }

    /// Export shader to a different location
    pub fn export_shader(
        &mut self,
        export_path: &Path,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Result<(), String> {
        let content = encode(&self.to_shader_asset(cx))?;
        std::fs::write(export_path, &content)
            .map_err(|e| format!("Failed to export shader: {}", e))
    }

    fn get_graph_file_path(&self) -> Option<PathBuf> {
        self.current_material_path
            .as_ref()
            .map(|material_path| material_path.join(GRAPH_SAVE_FILE_NAME))
    }

    /// Get the display name for the current shader
    pub fn get_display_name(&self) -> String {
        if let Some(title) = &self.tab_title {
            return title.clone();
        }

        if let Some(path) = &self.current_material_path {
            path.file_name()
                .and_then(|s| s.to_str())
                .unwrap_or("Untitled")
                .to_string()
        } else {
            "Untitled Shader".to_string()
        }
    }

    /// Get the full path as a string
    pub fn get_path_string(&self) -> Option<String> {
        self.get_graph_file_path()
            .as_deref()
            .and_then(|p| p.to_str())
            .map(|s| s.to_string())
    }

    /// Set the current file path
    pub fn set_path(&mut self, path: PathBuf) {
        let material_path = if path
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| name.eq_ignore_ascii_case(GRAPH_SAVE_FILE_NAME))
        {
            path.parent().unwrap_or(&path).to_path_buf()
        } else {
            path
        };

        self.tab_title = material_path
            .file_name()
            .and_then(|name| name.to_str())
            .map(|name| name.to_string());
        self.current_material_path = Some(material_path);
    }
}

/// What each comment contains, by comment index, from the laid-out geometry.
fn contained_nodes(canvas: &graphy_gpui::canvas::GraphCanvas) -> Vec<Vec<String>> {
    let scene = canvas.scene();
    scene
        .comments
        .iter()
        .map(|placed| {
            let mut ids: Vec<String> = scene
                .nodes
                .iter()
                .filter(|n| {
                    let r = n.rect();
                    r.min.x >= placed.rect.min.x
                        && r.min.y >= placed.rect.min.y
                        && r.max.x <= placed.rect.max.x
                        && r.max.y <= placed.rect.max.y
                })
                .map(|n| n.id.clone())
                .collect();
            ids.sort();
            ids
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::{decode, encode, formats, resolve_shader_path, ShaderAsset, ShaderEditorState, GRAPH_SAVE_FILE_NAME};
    use graphy_blueprint::GraphViewState;
    use graphy::GraphDescription;
    use std::path::Path;
    use graphy::{Connection, ConnectionType, DataType, NodeInstance, Pin, PinInstance, PinType, Position};
    use graphy::core::GraphComment;

    fn v(text: &str) -> serde_json::Value {
        serde_json::from_str(text).unwrap()
    }

    /// A material using what real ones do: constants, a vector default, a texture
    /// handle, a comment with a colour and members, a reroute with its own pin names.
    fn fixture_asset() -> ShaderAsset {
        let mut graph = GraphDescription::new("Wood");
        let mut constant = NodeInstance::new("0b1e", "float_constant", Position::new(-300.0, 20.0));
        constant.inputs.clear();
        constant.outputs.push(PinInstance::new(
            "result",
            Pin::new("result", "", DataType::typed("f32"), PinType::Output),
        ));
        constant.set_property("value", 0.25);
        graph.add_node(constant);

        let mut tint = NodeInstance::new("9c42", "vec4_constant", Position::new(-300.0, 140.0));
        tint.outputs.push(PinInstance::new(
            "result",
            Pin::new("result", "", DataType::typed("vec4<f32>"), PinType::Output),
        ));
        tint.set_property("value", v("[0.8, 0.5, 0.2, 1.0]"));
        graph.add_node(tint);

        let mut sample = NodeInstance::new("a7d0", "texture_sample", Position::new(-60.0, 60.0));
        sample.inputs.push(PinInstance::new(
            "texture",
            Pin::new("texture", "texture", DataType::typed("TextureSrc"), PinType::Input),
        ));
        sample.set_property("texture", "guid:tex-1234");
        graph.add_node(sample);

        let mut reroute = NodeInstance::new("e5a1", "reroute", Position::new(60.0, 60.0));
        reroute.add_input_pin("input", DataType::typed("vec4<f32>"));
        reroute.add_output_pin("output", DataType::typed("vec4<f32>"));
        graph.add_node(reroute);

        let mut out = NodeInstance::new("f003", "fragment_output", Position::new(200.0, 0.0));
        out.inputs.push(PinInstance::new(
            "color",
            Pin::new("color", "color", DataType::typed("vec4<f32>"), PinType::Input),
        ));
        graph.add_node(out);

        graph.add_connection(Connection::new("9c42", "result", "e5a1", "input", ConnectionType::Data));
        graph.add_connection(Connection::new("e5a1", "output", "f003", "color", ConnectionType::Data));

        let mut comment = GraphComment::new("Base colour", Position::new(-340.0, 100.0), (240.0, 140.0));
        comment.id = "c-1".into();
        comment.color = [0.2, 0.4, 0.8, 0.35];
        comment.contained_node_ids = vec!["9c42".into()];
        graph.comments.push(comment);

        let mut asset = ShaderAsset::with_graph(graph);
        asset.shader_metadata.description = "wood".into();
        asset.shader_metadata.extra.insert("tags".into(), v(r#"["pbr"]"#));
        asset.extra.insert("future_field".into(), v(r#"{"keep": true}"#));
        let mut state = ShaderEditorState::new();
        state.graph_view_states.insert(
            "main".into(),
            GraphViewState {
                pan_offset_x: 12.5,
                pan_offset_y: -3.0,
                zoom: 0.75,
            },
        );
        asset.editor_state = Some(state);
        asset
    }

    fn as_json(asset: &ShaderAsset) -> serde_json::Value {
        serde_json::from_str(&formats::serialize_shader(asset).unwrap()).unwrap()
    }

    #[test]
    fn a_material_round_trips_without_losing_a_field() {
        let asset = fixture_asset();
        let text = encode(&asset).unwrap();
        let back = decode(&text).unwrap();
        assert_eq!(as_json(&asset), as_json(&back));
        assert_eq!(back.main_graph.nodes.len(), 5);
        assert_eq!(back.main_graph.connections.len(), 2);
        assert_eq!(back.main_graph.nodes["9c42"].properties["value"], v("[0.8, 0.5, 0.2, 1.0]"));
        assert_eq!(back.main_graph.comments[0].color, [0.2, 0.4, 0.8, 0.35]);
        assert_eq!(back.main_graph.comments[0].contained_node_ids, vec!["9c42".to_string()]);
        assert_eq!(back.extra["future_field"], v(r#"{"keep": true}"#));
        assert_eq!(back.shader_metadata.extra["tags"], v(r#"["pbr"]"#));
        assert_eq!(back.editor_state.unwrap().graph_view_states["main"].zoom, 0.75);
    }

    #[test]
    fn ids_survive_an_edit_and_a_second_round_trip() {
        let asset = fixture_asset();
        let mut graph = asset.main_graph.clone();
        // An edit the canvas makes: move a node, change a property.
        let changes = graphy_gpui::apply_transaction(
            &mut graph,
            &[
                graphy::GraphCommand::SetProperty {
                    node: "0b1e".into(),
                    key: "value".into(),
                    value: Some(v("0.5")),
                },
                graphy::GraphCommand::MoveNode {
                    id: "a7d0".into(),
                    x: -50.0,
                    y: 70.0,
                },
            ],
        );
        assert!(changes.is_ok());
        let edited = ShaderAsset {
            main_graph: graph,
            ..asset.clone()
        };
        let again = decode(&encode(&edited).unwrap()).unwrap();
        let mut before: Vec<_> = asset.main_graph.nodes.keys().cloned().collect();
        let mut after: Vec<_> = again.main_graph.nodes.keys().cloned().collect();
        before.sort();
        after.sort();
        assert_eq!(before, after);
        assert_eq!(again.main_graph.nodes["0b1e"].properties["value"], v("0.5"));
        assert_eq!(as_json(&again)["main_graph"]["connections"], as_json(&asset)["main_graph"]["connections"]);
        assert_eq!(again.main_graph.comments[0].id, "c-1");
    }

    #[test]
    fn files_written_by_the_old_editor_open_and_keep_their_graph() {
        // The shape the previous editor wrote: a graph, view state, and the
        // blueprint-style metadata block.
        let old = r#"// Auto Generated
{
  "format_version": 1,
  "main_graph": {
    "nodes": {},
    "connections": [],
    "metadata": {"name": "Shader Graph", "description": "", "version": "1.0.0",
                 "created_at": "2024-01-01T00:00:00+00:00", "modified_at": "2024-01-01T00:00:00+00:00"},
    "comments": []
  },
  "editor_state": {"open_tab_ids": ["main"], "active_tab_index": 0,
                   "graph_view_states": {"main": {"pan_offset_x": 1.0, "pan_offset_y": 2.0, "zoom": 1.0}}},
  "shader_metadata": {"blueprint_type": "Generic", "description": "", "category": "", "tags": []}
}"#;
        let asset = decode(old).unwrap();
        assert_eq!(asset.main_graph.metadata.name, "Shader Graph");
        assert_eq!(asset.shader_metadata.extra["blueprint_type"], v(r#""Generic""#));
        // Saving it back keeps the legacy block.
        let json = as_json(&asset);
        assert_eq!(json["shader_metadata"]["blueprint_type"], v(r#""Generic""#));
    }

    #[test]
    fn the_default_material_content_is_a_valid_asset() {
        let default = r#"{            "format_version": 1,            "main_graph": {                "nodes": {}, "connections": [],                "metadata": {"name": "MainMaterial", "description": "", "version": "1.0.0",                             "created_at": "2024-01-01T00:00:00+00:00",                             "modified_at": "2024-01-01T00:00:00+00:00"},                "comments": []            },            "shader_metadata": {"shader_model": "Standard_unlit", "stage": "Fragment",                                "description": "", "category": "Uncategorized"}        }"#;
        let asset = decode(default).unwrap();
        assert_eq!(asset.shader_metadata.shader_model, "Standard_unlit");
        assert!(asset.shader_metadata.extra.is_empty());
    }

    /// Legacy blueprint bytes open as an empty shader graph, and Graphy's codec is
    /// the only one that recognises them.
    #[test]
    fn legacy_blueprint_files_are_recognised_by_graphys_codec() {
        let legacy = r#"{"nodes":{},"connections":[],"metadata":{"name":"Old","description":"","version":"1.0.0","created_at":"2024-01-01T00:00:00+00:00","modified_at":"2024-01-01T00:00:00+00:00"},
            "comments":[{"id":"c","text":"t","position":{"x":0.0,"y":0.0},"size":{"width":10.0,"height":10.0},
            "color":{"h":0.5,"s":0.3,"l":0.2,"a":0.3},"contained_node_ids":[]}]}"#;
        assert!(graphy_blueprint::legacy::is_legacy_format(legacy));
        let asset = decode(legacy).unwrap();
        assert!(asset.main_graph.nodes.is_empty());
        assert!(decode("not json at all").is_err());
    }

    #[test]
    fn bare_folders_resolve_to_their_graph_file_and_files_stay() {
        // A path without an extension is a material folder; a file (the graph file
        // itself, or any path with an extension) is taken as it is.
        let folder = Path::new("proj/Wood");
        let file = resolve_shader_path(folder);
        assert_eq!(file, folder.join(GRAPH_SAVE_FILE_NAME));
        assert_eq!(resolve_shader_path(&file), file);
        let named = Path::new("proj/Wood.material");
        assert_eq!(resolve_shader_path(named), named);
    }
}
