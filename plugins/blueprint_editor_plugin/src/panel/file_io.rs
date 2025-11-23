//! File I/O - save and load blueprint files

use gpui::*;
use super::core::BlueprintEditorPanel;
use super::tabs::{GraphTab, SerializedGraphTab};
use ui::graph::{BlueprintAsset, GraphDescription, SubGraphDefinition};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

// Legacy format structures for backward compatibility
#[derive(Debug, Clone, Serialize, Deserialize)]
struct LegacyGraphDescription {
    pub nodes: HashMap<String, ui::graph::NodeInstance>,
    pub connections: Vec<LegacyConnection>,
    pub metadata: ui::graph::GraphMetadata,
    #[serde(default)]
    pub comments: Vec<LegacyBlueprintComment>,
}

// Legacy connection format - actually matches current format exactly
#[derive(Debug, Clone, Serialize, Deserialize)]
struct LegacyConnection {
    pub id: String,
    pub source_node: String,
    pub source_pin: String,
    pub target_node: String,
    pub target_pin: String,
    pub connection_type: ui::graph::ConnectionType,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct LegacyBlueprintComment {
    pub id: String,
    pub text: String,
    pub position: LegacyPosition,
    pub size: LegacySize,
    pub color: LegacyColor,
    pub contained_node_ids: Vec<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct LegacyPosition {
    pub x: f32,
    pub y: f32,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct LegacySize {
    pub width: f32,
    pub height: f32,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct LegacyColor {
    pub h: f32,
    pub s: f32,
    pub l: f32,
    pub a: f32,
}

impl From<LegacyConnection> for ui::graph::Connection {
    fn from(legacy: LegacyConnection) -> Self {
        ui::graph::Connection {
            id: legacy.id,
            source_node: legacy.source_node,
            source_pin: legacy.source_pin,
            target_node: legacy.target_node,
            target_pin: legacy.target_pin,
            connection_type: legacy.connection_type,
        }
    }
}

impl From<LegacyGraphDescription> for GraphDescription {
    fn from(legacy: LegacyGraphDescription) -> Self {
        GraphDescription {
            nodes: legacy.nodes,
            connections: legacy.connections.into_iter().map(|c| c.into()).collect(),
            metadata: legacy.metadata,
            comments: legacy.comments.into_iter().map(|c| c.into()).collect(),
        }
    }
}

impl From<LegacyBlueprintComment> for ui::graph::BlueprintComment {
    fn from(legacy: LegacyBlueprintComment) -> Self {
        // Convert HSL to RGB for the color array
        let (r, g, b) = hsl_to_rgb(legacy.color.h, legacy.color.s, legacy.color.l);
        ui::graph::BlueprintComment {
            id: legacy.id,
            text: legacy.text,
            position: (legacy.position.x, legacy.position.y),
            size: (legacy.size.width, legacy.size.height),
            color: [r, g, b, legacy.color.a],
            contained_node_ids: legacy.contained_node_ids,
        }
    }
}

fn hsl_to_rgb(h: f32, s: f32, l: f32) -> (f32, f32, f32) {
    if s == 0.0 {
        return (l, l, l);
    }
    
    let q = if l < 0.5 {
        l * (1.0 + s)
    } else {
        l + s - l * s
    };
    let p = 2.0 * l - q;
    
    let hue_to_rgb = |p: f32, q: f32, mut t: f32| -> f32 {
        if t < 0.0 { t += 1.0; }
        if t > 1.0 { t -= 1.0; }
        if t < 1.0 / 6.0 { return p + (q - p) * 6.0 * t; }
        if t < 1.0 / 2.0 { return q; }
        if t < 2.0 / 3.0 { return p + (q - p) * (2.0 / 3.0 - t) * 6.0; }
        p
    };
    
    (
        hue_to_rgb(p, q, h + 1.0 / 3.0),
        hue_to_rgb(p, q, h),
        hue_to_rgb(p, q, h - 1.0 / 3.0),
    )
}

impl BlueprintEditorPanel {
    /// Save complete blueprint to unified JSON file
    pub fn save_blueprint(&mut self, file_path: &str) -> Result<(), String> {
        // Sync all open tabs to storage before saving
        self.sync_all_tabs_to_storage();

        let main_tab = self.open_tabs.iter()
            .find(|tab| tab.is_main)
            .ok_or("Main event graph tab not found")?;

        // Convert main graph
        let main_graph = self.convert_graph_to_description(&main_tab.graph)?;

        // Convert variables
        let variables: Vec<ui::graph::ClassVariable> = self.class_variables.iter()
            .map(|v| ui::graph::ClassVariable {
                id: uuid::Uuid::new_v4().to_string(),
                name: v.name.clone(),
                data_type: ui::graph::DataType::from_type_str(&v.var_type),
                default_value: v.default_value.clone(),
                description: String::new(),
            })
            .collect();

        // Build editor state
        let open_tab_ids: Vec<String> = self.open_tabs.iter().map(|tab| tab.id.clone()).collect();
        
        let mut graph_view_states = std::collections::HashMap::new();
        for tab in &self.open_tabs {
            graph_view_states.insert(
                tab.id.clone(),
                ui::graph::GraphViewState {
                    pan_offset_x: tab.graph.pan_offset.x,
                    pan_offset_y: tab.graph.pan_offset.y,
                    zoom: tab.graph.zoom_level,
                }
            );
        }
        
        let editor_state = ui::graph::BlueprintEditorState {
            open_tab_ids,
            active_tab_index: self.active_tab_index,
            graph_view_states,
        };
        
        // Create unified blueprint asset
        let blueprint_asset = BlueprintAsset {
            format_version: 1,
            main_graph,
            local_macros: self.local_macros.clone(),
            variables,
            editor_state: Some(editor_state),
            blueprint_metadata: ui::graph::BlueprintMetadata::default(),
        };

        let json = serde_json::to_string_pretty(&blueprint_asset)
            .map_err(|e| format!("Failed to serialize: {}", e))?;

        // Add header
        let now = chrono::Local::now();
        let version = ui::ENGINE_VERSION;
        let header = format!(
            "// Auto Generated by the Pulsar Blueprint Editor\n\
             // DO NOT EDIT MANUALLY - YOUR CHANGES WILL BE OVERWRITTEN\n\
             // Generated on {} - Engine version {}\n\
             //\n\
             // This file contains the COMPLETE blueprint for this class including:\n\
             //   - Main event graph\n\
             //   - All local macro graphs\n\
             //   - Class variables\n\
             //   - Editor state (open tabs, camera positions, etc.)\n\
             //\n\
             // You can modify the graph by opening this class in the Pulsar Blueprint Editor.\n\
             // The graph is saved in JSON format for human readability and version control.\n\
             //\n\
             // EDITING THE JSON STRUCTURE COULD BREAK THE EDITOR\n\
             // AND PREVENT THE GUI FROM OPENING THIS CLASS - BE CAREFUL\n\n",
            now.format("%Y-%m-%d %H:%M:%S"),
            version
        );

        let content = format!("{}{}", header, json);
        std::fs::write(file_path, content).map_err(|e| format!("Failed to write file: {}", e))?;

        println!("💾 ═══════════════════════════════════════════════════════════════");
        println!("💾 BLUEPRINT SAVED SUCCESSFULLY");
        println!("💾 ═══════════════════════════════════════════════════════════════");
        println!("💾 File: {}", file_path);
        println!("💾");
        println!("💾 📊 Content Summary:");
        println!("💾   ✓ Main Event Graph: {} nodes, {} connections",
            main_tab.graph.nodes.len(),
            main_tab.graph.connections.len());
        println!("💾   ✓ Local Macros: {}", self.local_macros.len());
        for macro_def in &self.local_macros {
            println!("💾     - {} ({})", macro_def.name, macro_def.id);
        }
        println!("💾   ✓ Class Variables: {}", self.class_variables.len());
        println!("💾   ✓ Open Tabs: {}", self.open_tabs.len());
        println!("💾   ✓ Active Tab: {} ({})",
            self.open_tabs.get(self.active_tab_index).map(|t| t.name.as_str()).unwrap_or("Unknown"),
            self.active_tab_index);
        println!("💾 ═══════════════════════════════════════════════════════════════");

        // Also save separate files for legacy support
        self.save_local_macros()?;
        self.save_tabs_state()?;

        Ok(())
    }

    /// Save local macros to macros.json
    fn save_local_macros(&self) -> Result<(), String> {
        if let Some(class_path) = &self.current_class_path {
            let macros_file = class_path.join("macros.json");
            let json = serde_json::to_string_pretty(&self.local_macros)
                .map_err(|e| format!("Failed to serialize local macros: {}", e))?;
            std::fs::write(&macros_file, json)
                .map_err(|e| format!("Failed to write macros.json: {}", e))?;
            println!("💾 Saved {} local macros to macros.json", self.local_macros.len());
        }
        Ok(())
    }

    /// Save tabs state to tabs.json
    fn save_tabs_state(&self) -> Result<(), String> {
        if let Some(class_path) = &self.current_class_path {
            let tabs_file = class_path.join("tabs.json");
            let serialized_tabs: Vec<SerializedGraphTab> = self.open_tabs.iter().map(|tab| {
                SerializedGraphTab {
                    id: tab.id.clone(),
                    name: tab.name.clone(),
                    is_main: tab.is_main,
                    is_library_macro: tab.is_library_macro,
                    library_id: tab.library_id.clone(),
                }
            }).collect();

            let json = serde_json::to_string_pretty(&serialized_tabs)
                .map_err(|e| format!("Failed to serialize tabs: {}", e))?;
            std::fs::write(&tabs_file, json)
                .map_err(|e| format!("Failed to write tabs.json: {}", e))?;
            println!("💾 Saved {} tab states to tabs.json", serialized_tabs.len());
        }
        Ok(())
    }

    /// Load blueprint from file
    pub fn load_blueprint(
        &mut self,
        file_path: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Result<(), String> {
        println!("📂 ═══════════════════════════════════════════════════════════════");
        println!("📂 LOADING BLUEPRINT FROM FILE");
        println!("📂 ═══════════════════════════════════════════════════════════════");
        println!("📂 File: {}", file_path);
        
        let content = std::fs::read_to_string(file_path)
            .map_err(|e| {
                let error_msg = format!("Failed to read file: {}", e);
                eprintln!("❌ {}", error_msg);
                error_msg
            })?;

        println!("📂 ✓ File read successfully ({} bytes)", content.len());

        // Strip header comments
        let json = content.lines()
            .skip_while(|line| line.trim().starts_with("//"))
            .collect::<Vec<_>>()
            .join("\n");

        // Try new unified format first
        match serde_json::from_str::<BlueprintAsset>(&json) {
            Ok(blueprint_asset) => {
                println!("📂 ✓ Detected unified blueprint format");
                self.load_from_blueprint_asset(blueprint_asset, file_path, window, cx)?;
            },
            Err(unified_err) => {
                println!("📂 ⚠️  Unified format parse failed:");
                println!("📂    Error: {}", unified_err);
                println!("📂    Line: {}, Column: {}", unified_err.line(), unified_err.column());
                
                // Show context around the error location
                let lines: Vec<&str> = json.lines().collect();
                let error_line = unified_err.line().saturating_sub(1);
                if error_line < lines.len() {
                    println!("📂    Context:");
                    for i in error_line.saturating_sub(2)..=error_line.saturating_add(2).min(lines.len().saturating_sub(1)) {
                        println!("📂      {}{}: {}", 
                            if i == error_line { ">>> " } else { "    " },
                            i + 1,
                            lines.get(i).unwrap_or(&"")
                        );
                    }
                }
                
                println!("📂 ✓ Trying legacy format...");
                
                // Try parsing as legacy format first, then convert to new format
                let legacy_graph: LegacyGraphDescription = serde_json::from_str(&json)
                    .map_err(|e| {
                        let error_msg = format!("Failed to parse as both unified and legacy format.\nUnified error: {}\nLegacy error: {}", unified_err, e);
                        eprintln!("❌ {}", error_msg);
                        error_msg
                    })?;
                
                println!("📂 ✓ Legacy format parsed successfully");
                let graph_description: GraphDescription = legacy_graph.into();
                self.graph = self.convert_graph_description_to_blueprint(&graph_description, window, cx)?;

                // Reset to main tab
                self.open_tabs = vec![GraphTab {
                    id: "main".to_string(),
                    name: "EventGraph".to_string(),
                    graph: self.graph.clone(),
                    is_main: true,
                    is_dirty: false,
                    is_library_macro: false,
                    library_id: None,
                }];
                self.active_tab_index = 0;

                // Load separate legacy files
                let file_path_buf = std::path::Path::new(file_path);
                if let Some(parent) = file_path_buf.parent() {
                    self.current_class_path = Some(parent.to_path_buf());
                    let _ = self.load_local_macros(parent);
                    let _ = self.restore_tabs_state(parent, window, cx);
                    let _ = self.load_variables_from_class(parent);
                }

                println!("📂 Loaded blueprint in legacy format");
            }
        }

        // Reload library manager
        self.library_manager = ui::graph::LibraryManager::default();
        if let Err(e) = self.library_manager.load_all_libraries() {
            eprintln!("Failed to reload sub-graph libraries: {}", e);
        }

        cx.notify();
        Ok(())
    }

    /// Load from unified blueprint asset
    fn load_from_blueprint_asset(
        &mut self,
        asset: BlueprintAsset,
        file_path: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Result<(), String> {
        let file_path_buf = std::path::Path::new(file_path);
        if let Some(parent) = file_path_buf.parent() {
            self.current_class_path = Some(parent.to_path_buf());
        }

        // Load main graph
        self.graph = self.convert_graph_description_to_blueprint(&asset.main_graph, window, cx)?;

        // Load local macros
        self.local_macros = asset.local_macros;

        // Load variables
        self.class_variables = asset.variables.iter().map(|v| {
            super::super::variables::ClassVariable {
                name: v.name.clone(),
                var_type: format!("{:?}", v.data_type),
                default_value: v.default_value.clone(),
            }
        }).collect();

        // Restore main tab
        self.open_tabs = vec![GraphTab {
            id: "main".to_string(),
            name: "EventGraph".to_string(),
            graph: self.graph.clone(),
            is_main: true,
            is_dirty: false,
            is_library_macro: false,
            library_id: None,
        }];
        self.active_tab_index = 0;

        // Restore editor state (open tabs, active tab, view states)
        if let Some(editor_state) = asset.editor_state {
            // Restore open tabs
            for tab_id in &editor_state.open_tab_ids {
                if tab_id == "main" {
                    continue; // Already added
                }
                
                // Check if this is a local macro
                let macro_data = self.local_macros.iter()
                    .find(|m| &m.id == tab_id)
                    .map(|m| (m.name.clone(), m.graph.clone()));
                    
                if let Some((macro_name, macro_graph)) = macro_data {
                    if let Ok(mut blueprint_graph) = self.convert_graph_description_to_blueprint(&macro_graph, window, cx) {
                        // Restore view state for this tab if available
                        if let Some(view_state) = editor_state.graph_view_states.get(tab_id) {
                            blueprint_graph.pan_offset = Point {
                                x: view_state.pan_offset_x,
                                y: view_state.pan_offset_y,
                            };
                            blueprint_graph.zoom_level = view_state.zoom;
                        }
                        
                        self.open_tabs.push(GraphTab {
                            id: tab_id.clone(),
                            name: macro_name,
                            graph: blueprint_graph,
                            is_main: false,
                            is_dirty: false,
                            is_library_macro: false,
                            library_id: None,
                        });
                    }
                }
            }
            
            // Restore view state for main tab
            if let Some(view_state) = editor_state.graph_view_states.get("main") {
                if let Some(main_tab) = self.open_tabs.iter_mut().find(|t| t.is_main) {
                    main_tab.graph.pan_offset = Point {
                        x: view_state.pan_offset_x,
                        y: view_state.pan_offset_y,
                    };
                    main_tab.graph.zoom_level = view_state.zoom;
                }
                
                self.graph.pan_offset = Point {
                    x: view_state.pan_offset_x,
                    y: view_state.pan_offset_y,
                };
                self.graph.zoom_level = view_state.zoom;
            }
            
            // Restore active tab index (with bounds check)
            self.active_tab_index = editor_state.active_tab_index.min(self.open_tabs.len().saturating_sub(1));
            
            // Load the active tab's graph into self.graph
            if let Some(active_tab) = self.open_tabs.get(self.active_tab_index) {
                self.graph = active_tab.graph.clone();
            }
        }

        println!("📂 Loaded unified blueprint format");
        println!("📂   ✓ Main Graph: {} nodes", self.graph.nodes.len());
        println!("📂   ✓ Local Macros: {}", self.local_macros.len());
        println!("📂   ✓ Variables: {}", self.class_variables.len());
        println!("📂   ✓ Open Tabs: {}", self.open_tabs.len());
        println!("📂   ✓ Active Tab Index: {}", self.active_tab_index);
        println!("📂 ═══════════════════════════════════════════════════════════════");

        Ok(())
    }

    /// Load local macros from macros.json
    fn load_local_macros(&mut self, class_path: &std::path::Path) -> Result<(), String> {
        let macros_file = class_path.join("macros.json");
        if !macros_file.exists() {
            self.local_macros.clear();
            return Ok(());
        }

        let content = std::fs::read_to_string(&macros_file)
            .map_err(|e| format!("Failed to read macros.json: {}", e))?;
        let macros: Vec<SubGraphDefinition> = serde_json::from_str(&content)
            .map_err(|e| format!("Failed to parse macros.json: {}", e))?;

        self.local_macros = macros;
        println!("📂 Loaded {} local macros from macros.json", self.local_macros.len());
        Ok(())
    }

    /// Restore tabs from tabs.json
    fn restore_tabs_state(
        &mut self,
        class_path: &std::path::Path,
        window: &mut Window,
        cx: &mut Context<Self>
    ) -> Result<(), String> {
        let tabs_file = class_path.join("tabs.json");
        if !tabs_file.exists() {
            return Ok(());
        }

        let content = std::fs::read_to_string(&tabs_file)
            .map_err(|e| format!("Failed to read tabs.json: {}", e))?;
        let serialized_tabs: Vec<SerializedGraphTab> = serde_json::from_str(&content)
            .map_err(|e| format!("Failed to parse tabs.json: {}", e))?;

        self.open_tabs.retain(|tab| tab.is_main);
        self.active_tab_index = 0;

        for ser_tab in serialized_tabs {
            if ser_tab.is_main {
                continue;
            }

            if ser_tab.is_library_macro {
                let macro_graph = self.library_manager.get_subgraph(&ser_tab.id)
                    .map(|m| m.graph.clone());
                    
                if let Some(graph) = macro_graph {
                    if let Ok(blueprint_graph) = self.convert_graph_description_to_blueprint(&graph, window, cx) {
                        self.open_tabs.push(GraphTab {
                            id: ser_tab.id.clone(),
                            name: ser_tab.name.clone(),
                            graph: blueprint_graph,
                            is_main: false,
                            is_dirty: false,
                            is_library_macro: true,
                            library_id: ser_tab.library_id.clone(),
                        });
                    }
                }
            } else {
                let macro_graph = self.local_macros.iter()
                    .find(|m| m.id == ser_tab.id)
                    .map(|m| m.graph.clone());
                    
                if let Some(graph) = macro_graph {
                    if let Ok(blueprint_graph) = self.convert_graph_description_to_blueprint(&graph, window, cx) {
                        self.open_tabs.push(GraphTab {
                            id: ser_tab.id.clone(),
                            name: ser_tab.name.clone(),
                            graph: blueprint_graph,
                            is_main: false,
                            is_dirty: false,
                            is_library_macro: false,
                            library_id: None,
                        });
                    }
                }
            }
        }

        println!("📂 Restored {} tabs from tabs.json", self.open_tabs.len());
        Ok(())
    }
}
