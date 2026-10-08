//! Properties dock panel.

use crate::state::LevelEditorState;
use crate::ui::{ObjectHeaderSection, ObjectTypeFieldsSection, PropertiesPanel, TransformSection};
use gpui::*;
use std::collections::HashSet;
use std::sync::Arc;
use ui::{
    dock::{Panel, PanelEvent},
    input::InputState,
    v_flex, ActiveTheme,
};

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
    /// The selected object's subscription: writes made elsewhere (a gizmo
    /// drag, a script, an undo) arrive here with their new values, and the
    /// panel shows them without reading the scene.
    object_feed: Option<pulsar_world_registry::ObjectFeed>,
    /// Wakes the panel when `object_feed` queues an update.
    _object_feed_task: Option<Task<()>>,
    // DEPRECATED: Old manual property editing (will be removed)
    editing_property: Option<String>,
    property_input: Entity<InputState>,
    /// Tracks which sections are collapsed (by section name)
    collapsed_sections: HashSet<String>,
}

/// Scene data shown by the panel (transform, header, component values) only
/// needs to refresh a few times a second. Selection changes and scrolling are
/// not affected: selection is handled immediately, and scrolling invalidates the
/// view through its own path. Gizmo drags bump the store revision at input rate,
/// which without this limit re-read and re-rendered the whole panel every frame.
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

        let mut panel = Self {
            properties: PropertiesPanel::new(),
            state,
            focus_handle: cx.focus_handle(),
            object_header_section: None,
            transform_section: None,
            object_type_fields_section: None,
            current_object_id: None,
            object_feed: None,
            _object_feed_task: None,
            editing_property: None,
            property_input,
            collapsed_sections,
        };
        // Build the initial selection once. Selection changes are explicit
        // editor events; scene data reaches the panel through the selected
        // object's subscription. This panel never polls SceneDB/world
        // revisions.
        panel.sync_sections(window, cx);
        panel
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
        let _scope = gpui::render_stats::scope("properties: sync sections");
        let _read_scope = gpui::render_stats::scope("properties: sync signature read");
        let selected_object_id = {
            let state = self.state.read();
            state.scene.selected_object()
        };
        drop(_read_scope);

        let selection_changed = selected_object_id != self.current_object_id
            || (selected_object_id.is_some() && self.object_type_fields_section.is_none());

        if !selection_changed {
            return false;
        }

        {
            let _scope = gpui::render_stats::scope("properties: rebuild selected sections");
            if let Some(ref object_id) = selected_object_id {
                let scene_db = {
                    let state = self.state.read();
                    state.scene.shared_scene()
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
                self.follow_object(object_id, cx);
            } else {
                self.end_object_feed();
                self.object_header_section = None;
                self.transform_section = None;
                self.object_type_fields_section = None;
                self.current_object_id = None;
            }
        }
        true
    }

    /// Subscribe to `object_id`, replacing the previous selection's feed.
    fn follow_object(&mut self, object_id: &str, cx: &mut Context<Self>) {
        use engine_backend::scene::SceneWorldExt;
        self.end_object_feed();
        let (wake, woken) = smol::channel::unbounded::<()>();
        self.object_feed = {
            let state = self.state.read();
            let mut world = state.scene.world_mut();
            world.entity_for(object_id).and_then(|entity| {
                pulsar_world_registry::ObjectFeed::subscribe(&mut world, entity, move || {
                    let _ = wake.try_send(());
                })
            })
        };
        self._object_feed_task = Some(cx.spawn(async move |this, cx| {
            while woken.recv().await.is_ok() {
                while woken.try_recv().is_ok() {}
                if this.update(cx, |_, cx| cx.notify()).is_err() {
                    break;
                }
            }
        }));
    }

    fn end_object_feed(&mut self) {
        self._object_feed_task = None;
        if let Some(feed) = self.object_feed.take() {
            let state = self.state.read();
            feed.unsubscribe(&mut state.scene.world_mut());
        }
    }

    /// Show what the selected object's subscription delivered since the
    /// last frame: the transform and header from the object itself, every
    /// component value on its card.
    fn apply_object_updates(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        use engine_backend::scene::{Name, Transform, Visibility};
        use pulsar_world_registry::ObjectUpdate;
        let Some(feed) = &self.object_feed else {
            return;
        };
        let object = feed.object();
        let mut header_changed = false;
        for update in feed.take() {
            let ObjectUpdate::Changed(delta) = update else {
                // The object despawned (a full restore respawns it under the
                // same id): rebuild the sections next frame, which follows
                // whatever now carries the selection.
                self.current_object_id = None;
                cx.notify();
                return;
            };
            let header = delta.component == pulsar_scenedb::component_id::<Name>()
                || delta.component == pulsar_scenedb::component_id::<Visibility>();
            if delta.entity != object || !(header || delta.component == pulsar_scenedb::component_id::<Transform>()) {
                if let Some(section) = &self.object_type_fields_section {
                    section.update(cx, |section, cx| section.apply_update(&delta, cx));
                }
                continue;
            }
            if let Some(transform) = delta
                .value
                .as_deref()
                .and_then(|v| v.downcast_ref::<Transform>())
            {
                if let Some(section) = &self.transform_section {
                    section.update(cx, |section, cx| {
                        section.show_transform(transform, window, cx)
                    });
                }
            } else if header {
                header_changed = true;
            }
        }
        if header_changed {
            if let Some(section) = &self.object_header_section {
                section.update(cx, |section, cx| section.refresh(window, cx));
            }
        }
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
        use crate::commands::{execute_command, SceneCommand};
        let selected = self.state.read().scene.selected_object();
        if let Some(object_id) = selected {
            let obj_opt = {
                let state = self.state.read();
                let world = state.scene.world();
                crate::scene_edit::objects::get_object(&world, &object_id)
            };
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

impl Drop for PropertiesPanelWrapper {
    fn drop(&mut self) {
        self.end_object_feed();
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

        // Selection is an explicit invalidation, unlike scene revisions. Read
        // only the selected ID here so a selection notification can replace
        // the three section entities. No polling, timer, or data refresh is
        // allowed through this path.
        self.sync_sections(window, cx);
        self.apply_object_updates(window, cx);
        let _state_scope = gpui::render_stats::scope("properties: render state read");
        let state = self.state.read();
        drop(_state_scope);
        let _element_scope = gpui::render_stats::scope("properties: element build");
        v_flex()
            .size_full()
            .bg(cx.theme().sidebar)
            .child(self.properties.render(
                &state,
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

#[cfg(test)]
#[path = "properties_perf_tests.rs"]
mod perf_tests;
