//! Add Component Picker
//!
//! Compact searchable popover listing all engine classes registered via
//! `#[derive(EngineClass)]`. Directly adds the component to the object when clicked.

use gpui::{prelude::*, *};
use pulsar_reflection::{PropertyValue, REGISTRY};
use serde_json::Value;
use ui::{input::{InputState, TextInput}, v_flex, ActiveTheme, Icon, IconName, Sizable};

use crate::level_editor::scene_database::SceneDatabase;

// ── Events ────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct ComponentAddedEvent {
    pub class_name: String,
}

// ── Entity ────────────────────────────────────────────────────────────────────

pub struct AddComponentDialog {
    focus_handle: FocusHandle,
    search_input: Entity<InputState>,
    /// All registered engine class names, captured at construction time.
    engine_classes: Vec<&'static str>,
    /// The object ID to add components to
    object_id: String,
    /// Scene database to modify
    scene_db: SceneDatabase,
}

impl EventEmitter<DismissEvent> for AddComponentDialog {}
impl EventEmitter<ComponentAddedEvent> for AddComponentDialog {}

impl Focusable for AddComponentDialog {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl AddComponentDialog {
    pub fn new(
        object_id: String,
        scene_db: SceneDatabase,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let search_input = cx.new(|cx| InputState::new(window, cx).placeholder("Search components…"));
        cx.observe(&search_input, |_, _, cx| cx.notify()).detach();

        let mut engine_classes = pulsar_reflection::REGISTRY.get_class_names();
        engine_classes.sort();

        Self {
            focus_handle: cx.focus_handle(),
            search_input,
            engine_classes,
            object_id,
            scene_db,
        }
    }

    fn query(&self, cx: &App) -> String {
        self.search_input.read(cx).value().to_lowercase()
    }

    fn add_component(&self, class_name: &str, cx: &mut Context<Self>) {
        // Skip if already attached
        let existing = self.scene_db.get_components(&self.object_id);
        if existing.iter().any(|c| c.class_name == class_name) {
            cx.emit(DismissEvent);
            return;
        }

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
        PropertyValue::Component { class_name, .. } => serde_json::json!({"class_name": class_name}),
    }
}

// ── Render ────────────────────────────────────────────────────────────────────

impl Render for AddComponentDialog {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let query = self.query(cx);

        let classes: Vec<&'static str> = self
            .engine_classes
            .iter()
            .copied()
            .filter(|n| query.is_empty() || n.to_lowercase().contains(&query))
            .collect();

        let row_style = |el: Div| {
            el.flex()
                .flex_row()
                .w_full()
                .h(px(28.0))
                .px_2()
                .gap_2()
                .items_center()
                .cursor_pointer()
                .rounded(px(4.0))
        };

        v_flex()
            .w(px(240.0))
            .max_h(px(320.0))
            .p_1()
            .gap_1()
            .track_focus(&self.focus_handle)
            .child(
                div()
                    .px_1()
                    .pb_1()
                    .border_b_1()
                    .border_color(cx.theme().border)
                    .child(TextInput::new(&self.search_input).w_full().xsmall()),
            )
            .when(classes.is_empty(), |el| {
                el.child(
                    div()
                        .px_2()
                        .py_1()
                        .text_xs()
                        .text_color(cx.theme().muted_foreground)
                        .child("No components found"),
                )
            })
            .when(!classes.is_empty(), |el| {
                el.child(
                    div()
                        .px_2()
                        .pt_1()
                        .text_xs()
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(cx.theme().muted_foreground)
                        .child("Engine Classes"),
                )
                .children(classes.into_iter().map(|name| {
                    let theme = cx.theme().clone();
                    row_style(div())
                        .id(ElementId::Name(name.into()))
                        .hover(move |s| s.bg(theme.accent.opacity(0.12)))
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(move |this, _, _, cx| {
                                this.add_component(name, cx);
                            }),
                        )
                        .child(
                            Icon::new(IconName::Component)
                                .size(px(13.0))
                                .text_color(cx.theme().muted_foreground),
                        )
                        .child(
                            div()
                                .text_sm()
                                .text_color(cx.theme().foreground)
                                .child(name),
                        )
                }))
            })
    }
}
