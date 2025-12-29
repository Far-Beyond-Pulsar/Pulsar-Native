//! Workspace panels for Level Editor

use gpui::*;
use ui::{ActiveTheme, StyledExt, dock::{Panel, PanelEvent}, v_flex, input::{TextInput, InputState}};
use super::ui::{WorldSettings, HierarchyPanel, PropertiesPanel, ViewportPanel, LevelEditorState};
use std::sync::Arc;
use std::rc::Rc;
use std::cell::RefCell;
use std::collections::HashSet;
use engine_backend::services::gpu_renderer::GpuRenderer;
use engine_backend::GameThread;

/// World Settings Panel (replaced Scene Browser)
pub struct WorldSettingsPanel {
    world_settings: WorldSettings,
    state: Arc<parking_lot::RwLock<LevelEditorState>>,
    focus_handle: FocusHandle,
    /// Tracks which sections are collapsed (by section name)
    collapsed_sections: HashSet<String>,
}

impl WorldSettingsPanel {
    pub fn new(state: Arc<parking_lot::RwLock<LevelEditorState>>, cx: &mut Context<Self>) -> Self {
        // Default all sections to collapsed
        let mut collapsed_sections = HashSet::new();
        collapsed_sections.insert("Environment".to_string());
        collapsed_sections.insert("Global Illumination".to_string());
        collapsed_sections.insert("Fog & Atmosphere".to_string());
        collapsed_sections.insert("Physics".to_string());
        collapsed_sections.insert("Audio".to_string());
        
        Self {
            world_settings: WorldSettings::new(),
            state,
            focus_handle: cx.focus_handle(),
            collapsed_sections,
        }
    }

    pub fn toggle_section(&mut self, section: String, cx: &mut Context<Self>) {
        if self.collapsed_sections.contains(&section) {
            self.collapsed_sections.remove(&section);
        } else {
            self.collapsed_sections.insert(section);
        }
        cx.notify();
    }

    pub fn is_section_collapsed(&self, section: &str) -> bool {
        self.collapsed_sections.contains(section)
    }
}

impl EventEmitter<PanelEvent> for WorldSettingsPanel {}

impl Render for WorldSettingsPanel {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let state = self.state.read();
        let collapsed_sections = self.collapsed_sections.clone();
        v_flex()
            .size_full()
            .bg(cx.theme().sidebar)
            .child(self.world_settings.render(&*state, self.state.clone(), &collapsed_sections, cx))
    }
}

impl Focusable for WorldSettingsPanel {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Panel for WorldSettingsPanel {
    fn panel_name(&self) -> &'static str {
        "world_settings"
    }

    fn title(&self, _window: &Window, _cx: &App) -> AnyElement {
        "World".into_any_element()
    }
}

/// Hierarchy Panel
pub struct HierarchyPanelWrapper {
    hierarchy: HierarchyPanel,
    state: Arc<parking_lot::RwLock<LevelEditorState>>,
    focus_handle: FocusHandle,
}

impl HierarchyPanelWrapper {
    pub fn new(state: Arc<parking_lot::RwLock<LevelEditorState>>, cx: &mut Context<Self>) -> Self {
        Self {
            hierarchy: HierarchyPanel::new(),
            state,
            focus_handle: cx.focus_handle(),
        }
    }
}

impl EventEmitter<PanelEvent> for HierarchyPanelWrapper {}

impl Render for HierarchyPanelWrapper {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let state = self.state.read();
        v_flex()
            .size_full()
            .bg(cx.theme().sidebar)
            .p_1()
            .child(self.hierarchy.render(&*state, self.state.clone(), cx))
    }
}

impl Focusable for HierarchyPanelWrapper {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Panel for HierarchyPanelWrapper {
    fn panel_name(&self) -> &'static str {
        "hierarchy"
    }

    fn title(&self, _window: &Window, _cx: &App) -> AnyElement {
        "Hierarchy".into_any_element()
    }
}

/// Properties Panel
pub struct PropertiesPanelWrapper {
    properties: PropertiesPanel,
    state: Arc<parking_lot::RwLock<LevelEditorState>>,
    focus_handle: FocusHandle,
    // Input state for property editing
    editing_property: Option<String>,
    property_input: Entity<InputState>,
    /// Tracks which sections are collapsed (by section name)
    collapsed_sections: HashSet<String>,
}

impl PropertiesPanelWrapper {
    pub fn new(state: Arc<parking_lot::RwLock<LevelEditorState>>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let property_input = cx.new(|cx| InputState::new(window, cx));
        // Default all sections to collapsed except Transform (the top section)
        let mut collapsed_sections = HashSet::new();
        collapsed_sections.insert("Camera Settings".to_string());
        collapsed_sections.insert("Light Settings".to_string());
        collapsed_sections.insert("Mesh Settings".to_string());
        collapsed_sections.insert("Folder Settings".to_string());
        collapsed_sections.insert("Empty Object".to_string());
        collapsed_sections.insert("Particle System".to_string());
        collapsed_sections.insert("Audio Source".to_string());
        collapsed_sections.insert("Tags & Layers".to_string());
        collapsed_sections.insert("Components".to_string());
        collapsed_sections.insert("Rendering".to_string());
        collapsed_sections.insert("Physics".to_string());
        
        Self {
            properties: PropertiesPanel::new(),
            state,
            focus_handle: cx.focus_handle(),
            editing_property: None,
            property_input,
            collapsed_sections,
        }
    }

    pub fn toggle_section(&mut self, section: String, cx: &mut Context<Self>) {
        if self.collapsed_sections.contains(&section) {
            self.collapsed_sections.remove(&section);
        } else {
            self.collapsed_sections.insert(section);
        }
        cx.notify();
    }

    pub fn is_section_collapsed(&self, section: &str) -> bool {
        self.collapsed_sections.contains(section)
    }

    pub fn start_editing(&mut self, property_path: String, current_value: String, window: &mut Window, cx: &mut Context<Self>) {
        self.editing_property = Some(property_path);
        self.property_input.update(cx, |input, cx| {
            input.set_value(&current_value, window, cx);
            input.focus(window, cx);
        });
        cx.notify();
    }

    fn commit_property_edit(&mut self, cx: &mut Context<Self>) {
        if let Some(property_path) = self.editing_property.take() {
            let new_value = self.property_input.read(cx).text().to_string();

            // Parse and update the property
            if let Ok(value) = new_value.parse::<f32>() {
                self.update_transform_property(&property_path, value);
            }
        }
        cx.notify();
    }

    fn cancel_property_edit(&mut self, cx: &mut Context<Self>) {
        self.editing_property = None;
        cx.notify();
    }

    fn update_transform_property(&self, property_path: &str, value: f32) {
        let state = self.state.read();
        if let Some(object_id) = state.selected_object() {
            if let Some(mut obj) = state.scene_database.get_object(&object_id) {
                // Update the specific transform field
                match property_path {
                    "position.x" => obj.transform.position[0] = value,
                    "position.y" => obj.transform.position[1] = value,
                    "position.z" => obj.transform.position[2] = value,
                    "rotation.x" => obj.transform.rotation[0] = value,
                    "rotation.y" => obj.transform.rotation[1] = value,
                    "rotation.z" => obj.transform.rotation[2] = value,
                    "scale.x" => obj.transform.scale[0] = value,
                    "scale.y" => obj.transform.scale[1] = value,
                    "scale.z" => obj.transform.scale[2] = value,
                    _ => return,
                }

                state.scene_database.update_object(obj);
                drop(state);
                self.state.write().has_unsaved_changes = true;
            }
        }
    }
}

impl EventEmitter<PanelEvent> for PropertiesPanelWrapper {}

impl Render for PropertiesPanelWrapper {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let state = self.state.read();
        let collapsed_sections = self.collapsed_sections.clone();

        v_flex()
            .size_full()
            .bg(cx.theme().sidebar)
            .child(self.properties.render(
                &*state,
                self.state.clone(),
                &self.editing_property,
                &self.property_input,
                &collapsed_sections,
                window,
                cx
            ))
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
                if this.editing_property.is_some() {
                    match event.keystroke.key.as_str() {
                        "enter" => {
                            this.commit_property_edit(cx);
                            cx.stop_propagation();
                        }
                        "escape" => {
                            this.cancel_property_edit(cx);
                            cx.stop_propagation();
                        }
                        _ => {}
                    }
                }
            }))
    }
}

impl Focusable for PropertiesPanelWrapper {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Panel for PropertiesPanelWrapper {
    fn panel_name(&self) -> &'static str {
        "properties"
    }

    fn title(&self, _window: &Window, _cx: &App) -> AnyElement {
        "Properties".into_any_element()
    }
}

/// Viewport Panel Wrapper
pub struct ViewportPanelWrapper {
    viewport_panel: ViewportPanel,
    state: Arc<parking_lot::RwLock<LevelEditorState>>,
    fps_graph_is_line: Rc<RefCell<bool>>,
    gpu_engine: Arc<std::sync::Mutex<GpuRenderer>>,
    game_thread: Arc<GameThread>,
    focus_handle: FocusHandle,
}

impl ViewportPanelWrapper {
    pub fn new(
        viewport_panel: ViewportPanel,
        state: Arc<parking_lot::RwLock<LevelEditorState>>,
        fps_graph_is_line: Rc<RefCell<bool>>,
        gpu_engine: Arc<std::sync::Mutex<GpuRenderer>>,
        game_thread: Arc<GameThread>,
        cx: &mut Context<Self>,
    ) -> Self {
        Self {
            viewport_panel,
            state,
            fps_graph_is_line,
            gpu_engine,
            game_thread,
            focus_handle: cx.focus_handle(),
        }
    }
}

impl EventEmitter<PanelEvent> for ViewportPanelWrapper {}

impl Render for ViewportPanelWrapper {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let mut state = self.state.write();
        div()
            .size_full()
            .p_1()
            .child(
                self.viewport_panel.render(
                    &mut *state,
                    self.state.clone(),
                    self.fps_graph_is_line.clone(),
                    &self.gpu_engine,
                    &self.game_thread,
                    cx
                )
            )
    }
}

impl Focusable for ViewportPanelWrapper {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Panel for ViewportPanelWrapper {
    fn panel_name(&self) -> &'static str {
        "viewport"
    }

    fn title(&self, _window: &Window, _cx: &App) -> AnyElement {
        "Viewport".into_any_element()
    }
}
