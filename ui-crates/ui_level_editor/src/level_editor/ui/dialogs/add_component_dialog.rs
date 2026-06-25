//! Add Component Picker
//!
//! Compact searchable popover listing all engine classes registered via
//! `#[derive(EngineClass)]` and any plugin-provided components.
//! Directly adds the component to the object when clicked.

use gpui::{prelude::*, *};
use plugin_editor_api::ComponentDefinition;
use pulsar_reflection::{REGISTRY, RUNTIME_TYPE_REGISTRY};
use serde_json::Value;
use ui::{
    dropdown::{SearchableList, SearchableListEvent},
    IconName,
};

use crate::level_editor::scene_database::SceneDatabase;

// ── Events ────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct ComponentAddedEvent {
    pub class_name: String,
    pub is_plugin: bool,
}

// ── Entity ────────────────────────────────────────────────────────────────────

pub struct AddComponentDialog {
    searchable_list: Entity<SearchableList<String>>,
    _subscriptions: Vec<Subscription>,
    /// The object ID to add components to
    object_id: String,
    /// Scene database to modify
    scene_db: SceneDatabase,
}

impl EventEmitter<DismissEvent> for AddComponentDialog {}
impl EventEmitter<ComponentAddedEvent> for AddComponentDialog {}

impl Focusable for AddComponentDialog {
    fn focus_handle(&self, cx: &App) -> FocusHandle {
        self.searchable_list.read(cx).focus_handle(cx)
    }
}

impl AddComponentDialog {
    pub fn new(
        object_id: String,
        scene_db: SceneDatabase,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let mut items: Vec<String> = REGISTRY
            .get_class_names()
            .into_iter()
            .map(|s| s.to_string())
            .collect();

        // Also include plugin-provided component names
        if let Some(pm) = plugin_manager::global() {
            let pm = pm.read();
            let plugin_defs = pm.get_all_component_definitions();
            for def in &plugin_defs {
                if !items.contains(&def.id) {
                    items.push(def.id.clone());
                }
            }
        }

        items.sort();

        let searchable_list = cx.new(|cx| {
            SearchableList::new(window, cx, items, |name| name.clone())
                .with_empty_text("No components found")
                .with_max_width(px(240.0))
                .with_max_height(px(320.0))
                .with_icon_getter(|_| IconName::Component)
        });

        let subscriptions = vec![cx.subscribe(
            &searchable_list,
            |this, _, event: &SearchableListEvent<String>, cx| {
                if let SearchableListEvent::Select(class_name) = event {
                    this.add_component(class_name, cx);
                }
            },
        )];

        Self {
            searchable_list,
            _subscriptions: subscriptions,
            object_id,
            scene_db,
        }
    }

    fn add_component(&self, class_name: &str, cx: &mut Context<Self>) {
        if REGISTRY.has_class(class_name) {
            self.add_builtin_component(class_name, cx);
        } else {
            self.add_plugin_component(class_name, cx);
        }
    }

    fn add_builtin_component(&self, class_name: &str, cx: &mut Context<Self>) {
        if let Some(mut instance) = REGISTRY.create_instance(class_name) {
            let props = instance.get_properties();
            let mut map = serde_json::Map::new();
            for prop in &props {
                let v = (prop.getter)(instance.as_ref());
                let json_value = RUNTIME_TYPE_REGISTRY
                    .serialize_json_for_any(v.as_ref())
                    .unwrap_or(serde_json::json!(null));
                map.insert(prop.name.to_string(), json_value);
            }
            self.scene_db.add_component(
                &self.object_id,
                class_name.to_string(),
                Value::Object(map),
            );
        }

        cx.emit(ComponentAddedEvent {
            class_name: class_name.to_string(),
            is_plugin: false,
        });
        cx.emit(DismissEvent);
    }

    fn add_plugin_component(&self, class_name: &str, cx: &mut Context<Self>) {
        let default_data = engine_backend::EngineBackend::global()
            .map(|b| {
                let guard = b.read();
                guard
                    .plugin_components()
                    .get_default_data(class_name)
                    .cloned()
            })
            .flatten()
            .unwrap_or(serde_json::Value::Object(serde_json::Map::new()));

        self.scene_db
            .add_component(&self.object_id, class_name.to_string(), default_data);

        cx.emit(ComponentAddedEvent {
            class_name: class_name.to_string(),
            is_plugin: true,
        });
        cx.emit(DismissEvent);
    }
}

// ── Render ────────────────────────────────────────────────────────────────────

impl Render for AddComponentDialog {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        self.searchable_list.clone()
    }
}
