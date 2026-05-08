//! Add Component Picker
//!
//! Compact searchable popover listing all engine classes registered via
//! `#[derive(EngineClass)]`. Directly adds the component to the object when clicked.

use gpui::{prelude::*, *};
use pulsar_reflection::{PropertyValue, REGISTRY};
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
}

// ── Entity ────────────────────────────────────────────────────────────────────

pub struct AddComponentDialog {
    searchable_list: Entity<SearchableList<&'static str>>,
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
        let mut engine_classes = pulsar_reflection::REGISTRY.get_class_names();
        engine_classes.sort();

        let searchable_list = cx.new(|cx| {
            SearchableList::new(window, cx, engine_classes.clone(), |name| name.to_string())
                .with_empty_text("No components found")
                .with_max_width(px(240.0))
                .with_max_height(px(320.0))
                .with_icon_getter(|_| IconName::Component)
        });

        let subscriptions = vec![cx.subscribe(
            &searchable_list,
            |this, _, event: &SearchableListEvent<&'static str>, cx| {
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
        // Allow multiple components of the same type
        if !REGISTRY.has_class(class_name) {
            cx.emit(DismissEvent);
            return;
        }

        // Build default values from reflection metadata
        if let Some(mut instance) = REGISTRY.create_instance(class_name) {
            let props = instance.get_properties();
            let mut map = serde_json::Map::new();
            for prop in &props {
                let v = (prop.getter)(instance.as_ref());
                map.insert(prop.name.to_string(), property_value_to_json(&v));
            }
            self.scene_db.add_component(
                &self.object_id,
                class_name.to_string(),
                Value::Object(map),
            );
        }

        cx.emit(ComponentAddedEvent {
            class_name: class_name.to_string(),
        });
        cx.emit(DismissEvent);
    }
}

fn property_value_to_json(value: &PropertyValue) -> Value {
    match value {
        PropertyValue::F32(v) => Value::from(*v),
        PropertyValue::I32(v) => Value::from(*v),
        PropertyValue::Bool(v) => Value::from(*v),
        PropertyValue::String(v) => Value::from(v.clone()),
        PropertyValue::Vec3(v) => serde_json::json!([v[0], v[1], v[2]]),
        PropertyValue::Color(v) => serde_json::json!([v[0], v[1], v[2], v[3]]),
        PropertyValue::EnumVariant(v) => Value::from(*v as u64),
        PropertyValue::Vec(v) => Value::Array(v.iter().map(property_value_to_json).collect()),
        PropertyValue::Component { class_name, .. } => {
            serde_json::json!({"class_name": class_name})
        }
    }
}

// ── Render ────────────────────────────────────────────────────────────────────

impl Render for AddComponentDialog {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        self.searchable_list.clone()
    }
}
