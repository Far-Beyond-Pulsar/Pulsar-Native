//! Workspace panels for Level Editor

use crate::level_editor::state::LevelEditorState;
use crate::level_editor::ui::{
    HierarchyPanel, ObjectHeaderSection, ObjectTypeFieldsSection, PropertiesPanel,
    TransformSection, ViewportPanel, WorldSettingsReplicated,
};
use engine_backend::services::gpu_renderer::GpuRenderer;
use gpui::{Corner, *};
use std::collections::HashSet;
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
        gpui::render_stats::count("world settings panel: render");
        let _t = gpui::render_stats::scope("world settings panel: render");

        let self_entity_id = cx.entity().entity_id();
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
///
/// Self-refreshing: a frame pump compares a `(store_revision, selected)`
/// signature every platform frame and notifies this view only when something
/// the tree actually displays changed. No other panel needs to know this panel
/// exists — scene mutations from any thread (UI commands, AI tools, the render
/// thread's gizmo-drag release / click-select) all advance the store revision.
pub struct HierarchyPanelWrapper {
    hierarchy: HierarchyPanel,
    state: Arc<parking_lot::RwLock<LevelEditorState>>,
    focus_handle: FocusHandle,
    /// `(store_revision, selected)` last seen by the pump/render pair.
    last_signature: (u64, Option<String>),
    pump_started: bool,
}

impl HierarchyPanelWrapper {
    pub fn new(
        state: Arc<parking_lot::RwLock<LevelEditorState>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let last_signature = {
            let state = state.read();
            (
                state.scene.database.store_revision(),
                state.scene.selected_object(),
            )
        };
        Self {
            hierarchy: HierarchyPanel::new(),
            state,
            focus_handle: cx.focus_handle(),
            last_signature,
            pump_started: false,
        }
    }

    fn signature(&self) -> (u64, Option<String>) {
        let state = self.state.read();
        (
            state.scene.database.store_revision(),
            state.scene.selected_object(),
        )
    }

    fn start_pump(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.pump_started {
            return;
        }
        self.pump_started = true;

        crate::level_editor::ui::frame_pump::spawn_frame_pump(
            &cx.entity(),
            window,
            |this, _window, cx| {
                let signature = this.signature();
                if signature != this.last_signature {
                    this.last_signature = signature;
                    cx.notify();
                }
            },
        );
    }
}

impl EventEmitter<PanelEvent> for HierarchyPanelWrapper {}

ui_common::panel_boilerplate!(HierarchyPanelWrapper);

impl Render for HierarchyPanelWrapper {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        gpui::render_stats::count("hierarchy panel: render");
        let _t = gpui::render_stats::scope("hierarchy panel: render");

        self.start_pump(_window, cx);

        // Record what we are about to paint so the pump doesn't immediately
        // re-notify for a change an explicit notify (e.g. expand toggle, row
        // click) has already picked up.
        self.last_signature = self.signature();

        let state = self.state.read();
        let state_clone = self.state.clone();

        let add_button = Button::new("add_object")
            .icon(IconName::Plus)
            .ghost()
            .xsmall()
            .on_click(move |_, _, _cx| {
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
                    component_instances: None,
                };
                execute_command(
                    &mut state,
                    SceneCommand::AddObject {
                        data: new_object,
                        parent_id: None,
                    },
                );
                // No cx.notify() here: the command advanced the store revision,
                // which this panel's frame pump observes and turns into exactly
                // one invalidate.
            })
            .into_any_element();

        let wrapper_entity = cx.entity().downgrade();

        v_flex()
            .size_full()
            .bg(cx.theme().sidebar)
            .p_1()
            .child(self.hierarchy.render(
                &state,
                self.state.clone(),
                wrapper_entity,
                add_button,
                cx,
            ))
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
///
/// Like [`HierarchyPanelWrapper`], self-refreshing via its own frame pump: the
/// pump owns ALL section lifecycle work (building editors on selection change,
/// pushing refreshed values through them on scene edits) so that `render()` is
/// a pure function of already-built state. Mutating child entities mid-render
/// used to be how this panel worked — that both did the work repeatedly on
/// every spurious invalidate and re-entrantly touched other entities while
/// GPUI was mid-walk.
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
    /// Store revision the section editors were last synced against.
    last_store_revision: u64,
    pump_started: bool,
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
            last_store_revision: 0,
            pump_started: false,
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

    fn start_pump(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.pump_started {
            return;
        }
        self.pump_started = true;

        crate::level_editor::ui::frame_pump::spawn_frame_pump(
            &cx.entity(),
            window,
            |this, window, cx| {
                if this.sync_sections(window, cx) {
                    cx.notify();
                }
            },
        );
    }

    /// Bring the section entities in line with the current scene state.
    ///
    /// A revision bump means the scene data changed — not that the user
    /// selected something else. These are deliberately kept apart:
    ///
    /// Rebuilding on every revision change used to null `current_object_id`,
    /// tearing down and recreating all three sections per bump.
    /// `TransformSection` alone owns 9 `F32BoundField`s, each with its own
    /// `Entity<InputState>`, so a dozen-odd entities were being destroyed and
    /// recreated every bump — and gizmo drags bump at input rate. It also
    /// wiped the user's expanded/collapsed property categories, which live on
    /// `ObjectTypeFieldsSection`.
    ///
    /// Same object with new data only needs the existing editors to re-read
    /// their values, which is exactly what `refresh()` does — a value push per
    /// field instead of a rebuild.
    ///
    /// Returns `true` when anything changed and the view needs invalidating.
    fn sync_sections(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let (store_revision, selected_object_id) = {
            let state = self.state.read();
            (
                state.scene.database.store_revision(),
                state.scene.selected_object(),
            )
        };

        let revision_changed = store_revision != self.last_store_revision;
        let selection_changed = selected_object_id != self.current_object_id
            || (selected_object_id.is_some() && self.object_type_fields_section.is_none());

        if !revision_changed && !selection_changed {
            return false;
        }
        self.last_store_revision = store_revision;

        if selection_changed {
            if let Some(ref object_id) = selected_object_id {
                let scene_db = {
                    let state = self.state.read();
                    state.scene.database.clone()
                };
                let object_id_clone = object_id.clone();

                self.object_header_section = Some(cx.new(|cx| {
                    ObjectHeaderSection::new(
                        object_id_clone.clone(),
                        scene_db.clone(),
                        self.state.clone(),
                        window,
                        cx,
                    )
                }));
                self.transform_section = Some(cx.new(|cx| {
                    TransformSection::new(
                        object_id_clone.clone(),
                        scene_db.clone(),
                        self.state.clone(),
                        window,
                        cx,
                    )
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
        } else if revision_changed {
            // Scene changed under an unchanged selection — undo/redo, a gizmo
            // drag, an AI tool edit. Push values into the cached editors
            // rather than rebuilding them. Header/transform refreshes are
            // targeted component reads (cheap at bump rate); the component
            // card list is only re-rendered when a change actually touched
            // this object's components — transform edits, gizmo drags and
            // edits to other objects must not rebuild it, or panel complexity
            // would set the editor's framerate.
            if let Some(section) = self.object_header_section.clone() {
                section.update(cx, |section, cx| section.refresh(window, cx));
            }
            if let Some(section) = self.transform_section.clone() {
                section.update(cx, |section, cx| section.refresh(window, cx));
            }
            let components_touched = match &self.current_object_id {
                Some(id) => {
                    let state = self.state.read();
                    state.scene.database.has_property_changes_for(id)
                }
                None => false,
            };
            // `render_property_row_runtime` pushes the current value into each
            // cached editor as it renders, so this section only needs to be
            // told to render again.
            if components_touched {
                if let Some(section) = self.object_type_fields_section.clone() {
                    section.update(cx, |_, cx| cx.notify());
                }
            }
        }
        true
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
        let selected = self.state.read().scene.selected_object();
        if let Some(object_id) = selected {
            let obj_opt = self.state.read().scene.database.get_object(&object_id);
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
        // If this count tracks the window's full-draw count under
        // `WGPUI_RENDER_STATS=1`, the panel's cache is missing every frame.
        gpui::render_stats::count("properties panel: render");
        let _t = gpui::render_stats::scope("properties panel: render");

        // Section lifecycle (create on selection change, refresh on scene
        // edit) lives in the frame pump via `sync_sections`, never here:
        // by the time a dirty render runs, the pump has already brought the
        // sections up to date. Render only lays out what exists.
        self.start_pump(window, cx);

        v_flex()
            .size_full()
            .bg(cx.theme().sidebar)
            .child(self.properties.render(
                &self.state.read(),
                self.state.clone(),
                &self.editing_property,
                &self.property_input,
                &self.collapsed_sections.clone(),
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
    state: Arc<parking_lot::RwLock<LevelEditorState>>,
    gpu_engine: Arc<std::sync::Mutex<GpuRenderer>>,
    focus_handle: FocusHandle,
}

impl ViewportPanelWrapper {
    pub fn new(
        viewport_panel: ViewportPanel,
        state: Arc<parking_lot::RwLock<LevelEditorState>>,
        gpu_engine: Arc<std::sync::Mutex<GpuRenderer>>,
        cx: &mut Context<Self>,
    ) -> Self {
        Self {
            viewport_panel,
            state,
            gpu_engine,
            focus_handle: cx.focus_handle(),
        }
    }
}

impl EventEmitter<PanelEvent> for ViewportPanelWrapper {}

ui_common::panel_boilerplate!(ViewportPanelWrapper);

impl Render for ViewportPanelWrapper {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        gpui::render_stats::count("viewport panel: render");
        let _t = gpui::render_stats::scope("viewport panel: render");

        let state = self.state.read();
        self.viewport_panel
            .render(&state, self.state.clone(), &self.gpu_engine, cx)
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
