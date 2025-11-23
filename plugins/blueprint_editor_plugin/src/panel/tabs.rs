//! Tab management for blueprint editor
//!
//! Handles creating, switching, and closing tabs for different graphs
//! (main event graph, local macros, library macros).

use gpui::*;
use super::super::BlueprintGraph;
use super::core::BlueprintEditorPanel;

/// Tab entry for flat tab system (like Unreal Engine)
#[derive(Clone, Debug)]
pub struct GraphTab {
    pub id: String,
    pub name: String,
    pub graph: BlueprintGraph,
    pub is_main: bool,
    pub is_dirty: bool,
    pub is_library_macro: bool,
    pub library_id: Option<String>,
}

/// Serializable version for persistence
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct SerializedGraphTab {
    pub id: String,
    pub name: String,
    pub is_main: bool,
    pub is_library_macro: bool,
    pub library_id: Option<String>,
}

impl BlueprintEditorPanel {
    /// Sync current graph to active tab
    pub(super) fn sync_graph_to_active_tab(&mut self) {
        let tab_id = if let Some(tab) = self.open_tabs.get(self.active_tab_index) {
            tab.id.clone()
        } else {
            return;
        };

        let is_main = if let Some(tab) = self.open_tabs.get(self.active_tab_index) {
            tab.is_main
        } else {
            return;
        };

        if let Some(tab) = self.open_tabs.get_mut(self.active_tab_index) {
            tab.graph = self.graph.clone();
            tab.is_dirty = true;
        }

        // Sync local macros
        if !is_main && !tab_id.starts_with("🌐") {
            if let Some(macro_def) = self.local_macros.iter_mut().find(|m| m.id == tab_id) {
                // Will need graph conversion here
            }
        }
    }

    /// Load active tab's graph
    pub(super) fn load_active_tab_graph(&mut self) {
        if let Some(tab) = self.open_tabs.get(self.active_tab_index) {
            self.graph = tab.graph.clone();
        }
    }

    /// Switch to a different tab
    pub fn switch_to_tab(&mut self, tab_index: usize, cx: &mut Context<Self>) {
        if tab_index < self.open_tabs.len() && tab_index != self.active_tab_index {
            self.sync_graph_to_active_tab();
            self.active_tab_index = tab_index;
            self.load_active_tab_graph();
            cx.notify();
        }
    }

    /// Close a tab (cannot close main EventGraph)
    pub fn close_tab(&mut self, tab_index: usize, cx: &mut Context<Self>) {
        if tab_index >= self.open_tabs.len() || self.open_tabs[tab_index].is_main {
            println!("⚠️ Cannot close the main EventGraph tab");
            return;
        }

        self.open_tabs.remove(tab_index);

        if self.active_tab_index >= self.open_tabs.len() {
            self.active_tab_index = self.open_tabs.len().saturating_sub(1);
        }
        if self.active_tab_index >= tab_index && self.active_tab_index > 0 {
            self.active_tab_index -= 1;
        }

        self.load_active_tab_graph();
        cx.notify();
    }

    /// Open a local macro in a new tab
    pub fn open_local_macro(&mut self, macro_id: String, macro_name: String, cx: &mut Context<Self>) {
        // Check if already open
        if let Some(index) = self.open_tabs.iter().position(|tab| tab.id == macro_id) {
            self.switch_to_tab(index, cx);
            return;
        }

        // Find and open macro
        if let Some(macro_def) = self.local_macros.iter().find(|m| m.id == macro_id) {
            // Convert to BlueprintGraph (simplified for now)
            let graph = BlueprintGraph {
                nodes: Vec::new(),
                connections: Vec::new(),
                comments: Vec::new(),
                selected_nodes: Vec::new(),
                selected_comments: Vec::new(),
                zoom_level: 1.0,
                pan_offset: Point::new(0.0, 0.0),
                virtualization_stats: super::super::VirtualizationStats::default(),
            };

            self.sync_graph_to_active_tab();

            let new_tab = GraphTab {
                id: macro_id,
                name: macro_name.clone(),
                graph,
                is_main: false,
                is_dirty: false,
                is_library_macro: false,
                library_id: None,
            };

            self.open_tabs.push(new_tab);
            self.active_tab_index = self.open_tabs.len() - 1;
            self.load_active_tab_graph();

            println!("📂 Opened local macro in tab: {}", macro_name);
            cx.notify();
        }
    }

    /// Open a global/engine macro in a new tab
    pub fn open_global_macro(&mut self, macro_id: String, macro_name: String, cx: &mut Context<Self>) {
        // Check if already open
        if let Some(index) = self.open_tabs.iter().position(|tab| tab.id == macro_id) {
            self.switch_to_tab(index, cx);
            return;
        }

        // Request opening library view (app-level navigation)
        let library_id = self.get_macro_library_id(&macro_id);
        
        if let Some(lib_id) = library_id.as_ref() {
            self.request_open_engine_library(
                lib_id.clone(),
                "Engine Library".to_string(),
                Some(macro_id.clone()),
                Some(macro_name.clone()),
                cx,
            );
        }
    }

    /// Get library ID for a macro
    pub fn get_macro_library_id(&self, macro_id: &str) -> Option<String> {
        if self.local_macros.iter().any(|m| m.id == macro_id) {
            return None;
        }

        self.library_manager.get_libraries()
            .iter()
            .find(|(_, lib)| lib.subgraphs.iter().any(|sg| sg.id == macro_id))
            .map(|(id, _)| id.clone())
    }

    /// Request opening engine library (emits event for app-level handling)
    pub fn request_open_engine_library(
        &self,
        library_id: String,
        library_name: String,
        macro_id: Option<String>,
        macro_name: Option<String>,
        cx: &mut Context<Self>,
    ) {
        cx.emit(super::super::OpenEngineLibraryRequest {
            library_id,
            library_name,
            macro_id,
            macro_name,
        });
    }

    /// Create a new local macro from current selection
    pub fn create_new_local_macro(&mut self, cx: &mut Context<Self>) {
        let macro_name = format!("Macro {}", self.local_macros.len() + 1);
        let macro_id = uuid::Uuid::new_v4().to_string();
        
        // Create new empty macro
        let macro_def = ui::graph::SubGraphDefinition {
            id: macro_id.clone(),
            name: macro_name.clone(),
            description: "New macro".to_string(),
            graph: ui::graph::GraphDescription::new(&macro_name),
            interface: ui::graph::SubGraphInterface {
                inputs: Vec::new(),
                outputs: Vec::new(),
            },
            metadata: ui::graph::SubGraphMetadata {
                created_at: chrono::Utc::now().to_rfc3339(),
                modified_at: chrono::Utc::now().to_rfc3339(),
                author: Some(String::new()),
                tags: Vec::new(),
            },
            macro_config: ui::graph::MacroConfiguration::default(),
        };
        
        self.local_macros.push(macro_def);
        self.open_local_macro(macro_id, macro_name, cx);
    }

    /// Sync all tabs back to storage
    pub(super) fn sync_all_tabs_to_storage(&mut self) {
        let mut converted_graphs = Vec::new();

        for tab in self.open_tabs.iter() {
            if !tab.is_main && !tab.is_library_macro {
                // Would convert graph here
                converted_graphs.push((tab.id.clone(), tab.graph.clone()));
            }
        }

        for (tab_id, _graph) in converted_graphs {
            if let Some(_macro_def) = self.local_macros.iter_mut().find(|m| m.id == tab_id) {
                // Update macro storage
            }
        }
    }
}
