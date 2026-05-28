//! Workspace panels for Level Editor

use super::ui::{
    HierarchyPanel, LevelEditorState, ObjectHeaderSection, ObjectTypeFieldsSection,
    PropertiesPanel, TransformSection, ViewportPanel, WorldSettingsReplicated,
};
use engine_backend::services::gpu_renderer::GpuRenderer;
use engine_backend::GameThread;
use gpui::{Corner, *};
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
    state: crate::level_editor::StateEntity,
    focus_handle: FocusHandle,
    /// Tracks which sections are collapsed (by section name)
    collapsed_sections: HashSet<String>,
}

impl WorldSettingsPanel {
    pub fn new(
        state: crate::level_editor::StateEntity,
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
        let self_entity_id = cx.entity().entity_id();
        let collapsed_sections = self.collapsed_sections.clone();
        v_flex()
            .size_full()
            .bg(cx.theme().sidebar)
            .child(
                self.world_settings.render(&collapsed_sections, cx),
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
    state: crate::level_editor::StateEntity,
    focus_handle: FocusHandle,
    last_scene_revision: u64,
}

impl HierarchyPanelWrapper {
    pub fn new(
        state: crate::level_editor::StateEntity,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        // Re-render whenever the shared state is updated.
        cx.observe(&state, |_, _, cx| cx.notify()).detach();
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
        // cx.observe() in new() drives re-renders — no manual revision tracking needed.
        let state_clone = self.state.clone();

        let add_button = Button::new("add_object")
            .icon(IconName::Plus)
            .ghost()
            .xsmall()
            .on_click(move |_, _, cx| {
                use crate::level_editor::commands::{execute_command, SceneCommand};
                use crate::level_editor::scene_database::{ObjectType, SceneObjectData, Transform};
                state_clone.update(cx, |state, cx| {
                    execute_command(state, SceneCommand::AddObject {
                        data: SceneObjectData {
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
                        },
                        parent_id: None,
                    });
                    cx.notify();
                });
            })
            .into_any_element();

        let wrapper_entity = cx.entity().downgrade();

        // Clone state to an owned value so cx borrow is released before render.
        let state = self.state.read(cx).clone();
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
    state: crate::level_editor::StateEntity,
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
        state: crate::level_editor::StateEntity,
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

        // Re-render whenever the shared state is updated.
        cx.observe(&state, |_, _, cx| cx.notify()).detach();
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
                self.update_transform_property(&property_path, value, cx);
            }
        }
        cx.notify();
    }

    fn cancel_property_edit(&mut self, cx: &mut Context<Self>) {
        self.editing_property = None;
        cx.notify();
    }

    fn update_transform_property(&self, property_path: &str, value: f32, cx: &mut Context<Self>) {
        use crate::level_editor::commands::{execute_command, SceneCommand};
        let path = property_path.to_string();
        let (selected, obj) = {
            let s = self.state.read(cx);
            let sel = s.selected_object();
            let obj = sel.as_ref().and_then(|id| s.scene_database.get_object(id));
            (sel, obj)
        };
        if let (Some(_), Some(mut obj)) = (selected, obj) {
            match path.as_str() {
                "position.x" => obj.transform.position[0] = value,
                "position.y" => obj.transform.position[1] = value,
                "position.z" => obj.transform.position[2] = value,
                "rotation.x" => obj.transform.rotation[0] = value,
                "rotation.y" => obj.transform.rotation[1] = value,
                "rotation.z" => obj.transform.rotation[2] = value,
                "scale.x"    => obj.transform.scale[0] = value,
                "scale.y"    => obj.transform.scale[1] = value,
                "scale.z"    => obj.transform.scale[2] = value,
                _ => return,
            }
            self.state.update(cx, |state, cx| {
                execute_command(state, SceneCommand::UpdateObject { data: obj });
                cx.notify();
            });
        }
    }
}

impl EventEmitter<PanelEvent> for PropertiesPanelWrapper {}

ui_common::panel_boilerplate!(PropertiesPanelWrapper);

impl Render for PropertiesPanelWrapper {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // ── Phase 1: extract what we need, release the borrow ────────────────
        // cx.observe() in the constructor already drives re-renders; we never
        // need to call cx.notify() here ourselves.
        let (selected_object_id, scene_revision, scene_db) = {
            let s = self.state.read(cx);
            (s.selected_object(), s.scene_revision, s.scene_database.clone())
        };
        let collapsed_sections = self.collapsed_sections.clone();

        if scene_revision != self.last_scene_revision {
            self.last_scene_revision = scene_revision;
            self.current_object_id = None;
        }

        // ── Phase 2: update sub-sections (may call cx.new) ───────────────────
        // State borrow is released — cx.new() is safe here.
        if selected_object_id != self.current_object_id
            || (selected_object_id.is_some() && self.object_type_fields_section.is_none())
        {
            if let Some(ref object_id) = selected_object_id {
                let oid = object_id.clone();
                let db  = scene_db.clone();
                self.object_header_section = Some(cx.new(|cx| {
                    ObjectHeaderSection::new(oid.clone(), db.clone(), window, cx)
                }));
                self.transform_section = Some(cx.new(|cx| {
                    TransformSection::new(oid.clone(), db.clone(), window, cx)
                }));
                self.object_type_fields_section = Some(cx.new(|cx| {
                    ObjectTypeFieldsSection::new(oid.clone(), db.clone(), self.state.clone(), window, cx)
                }));
                self.current_object_id = Some(object_id.clone());
            } else {
                self.object_header_section = None;
                self.transform_section = None;
                self.object_type_fields_section = None;
                self.current_object_id = None;
            }
        }

        // ── Phase 3: build element tree using an owned state clone.
        // Cloning is cheap: SceneDatabase fields are Arc-wrapped.
        let state = self.state.read(cx).clone();
        v_flex()
            .size_full()
            .bg(cx.theme().sidebar)
            .child(self.properties.render(
                &state,
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
    state: crate::level_editor::StateEntity,
    fps_graph_is_line: Rc<RefCell<bool>>,
    gpu_engine: Arc<std::sync::Mutex<GpuRenderer>>,
    game_thread: Arc<GameThread>,
    focus_handle: FocusHandle,
}

impl ViewportPanelWrapper {
    pub fn new(
        viewport_panel: ViewportPanel,
        state: crate::level_editor::StateEntity,
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
        let state = self.state.read(cx).clone();
        self.viewport_panel.render(
            &state,
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
