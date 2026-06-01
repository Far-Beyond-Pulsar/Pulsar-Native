//! Workspace panels for Level Editor

use super::ui::{
    HierarchyPanel, LevelEditorState, ObjectHeaderSection, ObjectTypeFieldsSection,
    PropertiesPanel, TransformSection, ViewportPanel, WorldSettingsReplicated,
};
use engine_backend::services::gpu_renderer::GpuRenderer;
use engine_backend::GameThread;
use gpui::*;
use std::cell::RefCell;
use std::collections::HashSet;
use std::rc::Rc;
use std::sync::Arc;
use ui::{
    button::{Button, ButtonVariants as _},
    dock::{Panel, PanelEvent},
    input::InputState,
    v_flex, ActiveTheme, IconName, Sizable,
};

/// World Settings Panel (replaced Scene Browser)
pub struct WorldSettingsPanel {
    pub(crate) world_settings: WorldSettingsReplicated,
    state: Arc<parking_lot::RwLock<LevelEditorState>>,
    focus_handle: FocusHandle,
    /// Tracks which sections are collapsed (by section name)
    collapsed_sections: HashSet<String>,
}

impl WorldSettingsPanel {
    pub fn new(
        state: Arc<parking_lot::RwLock<LevelEditorState>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        // Default all sections to collapsed
        let mut collapsed_sections = HashSet::new();
        collapsed_sections.insert("Environment".to_string());
        collapsed_sections.insert("Global Illumination".to_string());
        collapsed_sections.insert("Fog & Atmosphere".to_string());
        collapsed_sections.insert("Physics".to_string());
        collapsed_sections.insert("Audio".to_string());

        Self {
            world_settings: WorldSettingsReplicated::new(window, cx),
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

ui_common::panel_boilerplate!(WorldSettingsPanel);

impl Render for WorldSettingsPanel {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let _self_entity_id = cx.entity().entity_id();
        let state = self.state.read();
        let collapsed_sections = self.collapsed_sections.clone();
        v_flex()
            .size_full()
            .bg(cx.theme().sidebar)
            .child(
                self.world_settings
                    .render(&state, self.state.clone(), &collapsed_sections, cx),
            )
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
    last_scene_revision: u64,
}

impl HierarchyPanelWrapper {
    pub fn new(
        state: Arc<parking_lot::RwLock<LevelEditorState>>,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        Self {
            hierarchy: HierarchyPanel::new(),
            state,
            focus_handle: cx.focus_handle(),
            last_scene_revision: 0,
        }
    }
}

impl EventEmitter<PanelEvent> for HierarchyPanelWrapper {}

ui_common::panel_boilerplate!(HierarchyPanelWrapper);

impl Render for HierarchyPanelWrapper {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let self_entity_id = cx.entity().entity_id();

        let state = self.state.read();
        let current_revision = state.scene_revision;
        if current_revision != self.last_scene_revision {
            self.last_scene_revision = current_revision;
            cx.notify();
        }
        drop(state);

        let state = self.state.read();
        let state_clone = self.state.clone();

        let add_button = Button::new("add_object")
            .icon(IconName::Plus)
            .ghost()
            .xsmall()
            .on_click(move |_, _, cx| {
                use crate::level_editor::commands::{execute_command, SceneCommand};
                use crate::level_editor::scene_database::{ObjectType, SceneObjectData, Transform};

                let mut state = state_clone.write();
                let new_object = SceneObjectData {
                    id: String::new(),
                    name: "New Object".to_string(),
                    object_type: ObjectType::Empty,
                    transform: Transform::default(),
                    visible: true,
                    locked: false,
                    parent: None,
                    children: vec![],
                    scene_path: String::new(),
                    props: Default::default(),
                };
                execute_command(
                    &mut state,
                    SceneCommand::AddObject {
                        data: new_object,
                        parent_id: None,
                    },
                );
                cx.notify(self_entity_id);
            })
            .into_any_element();

        let wrapper_entity = cx.entity().downgrade();

        v_flex()
            .size_full()
            .bg(cx.theme().sidebar)
            .p_1()
            .child(
                self.hierarchy
                    .render(&state, self.state.clone(), wrapper_entity, add_button, cx),
            )
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
    // New field binding system
    object_header_section: Option<Entity<ObjectHeaderSection>>,
    transform_section: Option<Entity<TransformSection>>,
    object_type_fields_section: Option<Entity<ObjectTypeFieldsSection>>,
    current_object_id: Option<String>,
    // DEPRECATED: Old manual property editing (will be removed)
    editing_property: Option<String>,
    property_input: Entity<InputState>,
    /// Tracks which sections are collapsed (by section name)
    collapsed_sections: HashSet<String>,
    /// Last scene revision observed by properties panel.
    last_scene_revision: u64,
}

impl PropertiesPanelWrapper {
    pub fn new(
        state: Arc<parking_lot::RwLock<LevelEditorState>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
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
            object_header_section: None,
            transform_section: None,
            object_type_fields_section: None,
            current_object_id: None,
            editing_property: None,
            property_input,
            collapsed_sections,
            last_scene_revision: 0,
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

    pub fn start_editing(
        &mut self,
        property_path: String,
        current_value: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
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
        use crate::level_editor::commands::{execute_command, SceneCommand};
        let selected = self.state.read().selected_object();
        if let Some(object_id) = selected {
            let obj_opt = self.state.read().scene_database.get_object(&object_id);
            if let Some(mut obj) = obj_opt {
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
                let mut state = self.state.write();
                execute_command(&mut state, SceneCommand::UpdateObject { data: obj });
            }
        }
    }
}

impl EventEmitter<PanelEvent> for PropertiesPanelWrapper {}

ui_common::panel_boilerplate!(PropertiesPanelWrapper);

impl Render for PropertiesPanelWrapper {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let state = self.state.read();
        let collapsed_sections = self.collapsed_sections.clone();
        let selected_object_id = state.selected_object();
        let scene_revision = state.scene_revision;

        if scene_revision != self.last_scene_revision {
            self.last_scene_revision = scene_revision;
            self.current_object_id = None;
        }

        let selection_changed = selected_object_id != self.current_object_id
            || (selected_object_id.is_some() && self.object_type_fields_section.is_none());

        if selection_changed {
            if let Some(ref object_id) = selected_object_id {
                let scene_db = state.scene_database.clone();
                let object_id_clone = object_id.clone();

                self.object_header_section = Some(cx.new(|cx| {
                    ObjectHeaderSection::new(object_id_clone.clone(), scene_db.clone(), window, cx)
                }));
                self.transform_section = Some(cx.new(|cx| {
                    TransformSection::new(object_id_clone.clone(), scene_db.clone(), window, cx)
                }));
                self.object_type_fields_section = Some(cx.new(|cx| {
                    ObjectTypeFieldsSection::new(
                        object_id_clone.clone(),
                        scene_db.clone(),
                        self.state.clone(),
                        window,
                        cx,
                    )
                }));
                self.current_object_id = Some(object_id.clone());
            } else {
                self.object_header_section = None;
                self.transform_section = None;
                self.object_type_fields_section = None;
                self.current_object_id = None;
            }
        }
        // NOTE: Removed the refresh() calls that were running every render frame.
        // The bound fields automatically subscribe to InputEvents and sync changes.
        // External changes (undo/redo, gizmo moves) should explicitly call refresh()
        // when those events occur, not on every render.

        drop(state);

        v_flex()
            .size_full()
            .bg(cx.theme().sidebar)
            .child(self.properties.render(
                &self.state.read(),
                self.state.clone(),
                &self.editing_property,
                &self.property_input,
                &collapsed_sections,
                &self.object_header_section,
                &self.transform_section,
                &self.object_type_fields_section,
                window,
                cx,
            ))
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, _window, cx| {
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

ui_common::panel_boilerplate!(ViewportPanelWrapper);

impl Render for ViewportPanelWrapper {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let mut state = self.state.write();
        self.viewport_panel.render(
            &mut state,
            self.state.clone(),
            self.fps_graph_is_line.clone(),
            &self.gpu_engine,
            &self.game_thread,
            cx,
        )
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
