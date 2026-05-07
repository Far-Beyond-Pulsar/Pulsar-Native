use gpui::{prelude::*, *};
use pulsar_reflection::{PropertyType, PropertyValue, REGISTRY};
use serde_json::Value;
use std::sync::Arc;
use ui::button::ButtonVariants as _;
use ui::popover::Popover;
use ui::{h_flex, v_flex, ActiveTheme, Icon, IconName, Sizable};

use super::add_component_dialog::AddComponentDialog;
use super::state::LevelEditorState;
use super::ComponentHierarchyPanel;
use crate::level_editor::scene_database::{ObjectType, SceneDatabase};

pub struct ObjectTypeFieldsSection {
    object_id: String,
    scene_db: SceneDatabase,
    /// Selected component index in the component list (for highlighting).
    selected_component: Option<usize>,
    /// Add component dialog entity
    add_component_dialog: Entity<AddComponentDialog>,
    /// Shared state for expand/collapse tracking
    state_arc: Arc<parking_lot::RwLock<LevelEditorState>>,
}

impl ObjectTypeFieldsSection {
    pub fn new(
        object_id: String,
        scene_db: SceneDatabase,
        state_arc: Arc<parking_lot::RwLock<LevelEditorState>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        // Create the add component dialog entity
        let dialog_object_id = object_id.clone();
        let dialog_scene_db = scene_db.clone();
        let add_component_dialog =
            cx.new(|cx| AddComponentDialog::new(dialog_object_id, dialog_scene_db, window, cx));

        // Subscribe to ComponentAddedEvent to refresh the UI
        cx.subscribe(
            &add_component_dialog,
            |this, _dialog, event: &super::add_component_dialog::ComponentAddedEvent, cx| {
                // Refresh the UI when a component is added
                let _ = event; // Event contains class_name but we don't need it
                cx.notify();
            },
        )
        .detach();

        Self {
            object_id,
            scene_db,
            selected_component: None,
            add_component_dialog,
            state_arc,
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
            PropertyValue::Vec(v) => {
                Value::Array(v.iter().map(Self::property_value_to_json).collect())
            }
            PropertyValue::Component { class_name, .. } => {
                serde_json::json!({"class_name": class_name})
            }
        }
    }

    fn json_to_property_value(property_type: &PropertyType, json: &Value) -> Option<PropertyValue> {
        match property_type {
            PropertyType::F32 { .. } => json.as_f64().map(|v| PropertyValue::F32(v as f32)),
            PropertyType::I32 { .. } => json.as_i64().map(|v| PropertyValue::I32(v as i32)),
            PropertyType::Bool => json.as_bool().map(PropertyValue::Bool),
            PropertyType::String { .. } => {
                json.as_str().map(|s| PropertyValue::String(s.to_string()))
            }
            PropertyType::Vec3 => {
                let arr = json.as_array()?;
                if arr.len() != 3 {
                    return None;
                }
                let x = arr.first()?.as_f64()? as f32;
                let y = arr.get(1)?.as_f64()? as f32;
                let z = arr.get(2)?.as_f64()? as f32;
                Some(PropertyValue::Vec3([x, y, z]))
            }
            PropertyType::Color => {
                let arr = json.as_array()?;
                if arr.len() != 4 {
                    return None;
                }
                let r = arr.first()?.as_f64()? as f32;
                let g = arr.get(1)?.as_f64()? as f32;
                let b = arr.get(2)?.as_f64()? as f32;
                let a = arr.get(3)?.as_f64()? as f32;
                Some(PropertyValue::Color([r, g, b, a]))
            }
            PropertyType::Enum { .. } => json
                .as_u64()
                .map(|v| PropertyValue::EnumVariant(v as usize)),
            PropertyType::Vec { .. } => None,
            PropertyType::Component { class_name } => Some(PropertyValue::Component {
                class_name: class_name.to_string(),
            }),
        }
    }

    fn read_property(
        &self,
        class_name: &str,
        property_name: &str,
        property_type: &PropertyType,
        default_value: &PropertyValue,
    ) -> PropertyValue {
        let components = self.scene_db.get_components(&self.object_id);
        let component = components.iter().find(|c| c.class_name == class_name);

        if let Some(component) = component {
            if let Some(value) = component.data.get(property_name) {
                if let Some(parsed) = Self::json_to_property_value(property_type, value) {
                    return parsed;
                }
            }
        }

        default_value.clone()
    }

    fn write_property(&self, class_name: &str, property_name: &str, value: PropertyValue) {
        let components = self.scene_db.get_components(&self.object_id);
        let Some((idx, component)) = components
            .iter()
            .enumerate()
            .find(|(_, c)| c.class_name == class_name)
        else {
            return;
        };

        let mut map = component
            .data
            .as_object()
            .cloned()
            .unwrap_or_else(serde_json::Map::new);
        map.insert(
            property_name.to_string(),
            Self::property_value_to_json(&value),
        );

        let _ = self.scene_db.metadata_db.components().update_component(
            &self.object_id,
            idx,
            Value::Object(map),
        );
    }

    fn nudge_numeric(&self, class_name: &str, prop_name: &str, current: f32, step: f32, sign: f32) {
        self.write_property(
            class_name,
            prop_name,
            PropertyValue::F32(current + step * sign),
        );
    }

    fn nudge_i32(&self, class_name: &str, prop_name: &str, current: i32, sign: i32) {
        self.write_property(class_name, prop_name, PropertyValue::I32(current + sign));
    }

    fn render_property_row(
        &self,
        class_name: &str,
        display_name: &str,
        prop_name: &str,
        property_type: &PropertyType,
        value: &PropertyValue,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        match (property_type, value) {
            (PropertyType::F32 { step, .. }, PropertyValue::F32(v)) => {
                let step = step.unwrap_or(0.1);
                let current = *v;
                let class_dec = class_name.to_string();
                let prop_dec = prop_name.to_string();
                let class_inc = class_name.to_string();
                let prop_inc = prop_name.to_string();

                h_flex()
                    .w_full()
                    .justify_between()
                    .items_center()
                    .gap_2()
                    .child(
                        div()
                            .text_sm()
                            .text_color(cx.theme().muted_foreground)
                            .child(display_name.to_string()),
                    )
                    .child(
                        h_flex()
                            .items_center()
                            .gap_2()
                            .child(
                                ui::button::Button::new(format!(
                                    "dec-{}-{}",
                                    class_name, prop_name
                                ))
                                .icon(IconName::Minus)
                                .xsmall()
                                .ghost()
                                .on_click(cx.listener(
                                    move |this, _event, _window, cx| {
                                        this.nudge_numeric(
                                            &class_dec, &prop_dec, current, step, -1.0,
                                        );
                                        cx.notify();
                                    },
                                )),
                            )
                            .child(
                                div()
                                    .text_sm()
                                    .text_color(cx.theme().foreground)
                                    .child(format!("{:.3}", v)),
                            )
                            .child(
                                ui::button::Button::new(format!(
                                    "inc-{}-{}",
                                    class_name, prop_name
                                ))
                                .icon(IconName::Plus)
                                .xsmall()
                                .ghost()
                                .on_click(cx.listener(
                                    move |this, _event, _window, cx| {
                                        this.nudge_numeric(
                                            &class_inc, &prop_inc, current, step, 1.0,
                                        );
                                        cx.notify();
                                    },
                                )),
                            ),
                    )
                    .into_any_element()
            }
            (PropertyType::I32 { .. }, PropertyValue::I32(v)) => {
                let current = *v;
                let class_dec = class_name.to_string();
                let prop_dec = prop_name.to_string();
                let class_inc = class_name.to_string();
                let prop_inc = prop_name.to_string();

                h_flex()
                    .w_full()
                    .justify_between()
                    .items_center()
                    .gap_2()
                    .child(
                        div()
                            .text_sm()
                            .text_color(cx.theme().muted_foreground)
                            .child(display_name.to_string()),
                    )
                    .child(
                        h_flex()
                            .items_center()
                            .gap_2()
                            .child(
                                ui::button::Button::new(format!(
                                    "dec-{}-{}",
                                    class_name, prop_name
                                ))
                                .icon(IconName::Minus)
                                .xsmall()
                                .ghost()
                                .on_click(cx.listener(
                                    move |this, _event, _window, cx| {
                                        this.nudge_i32(&class_dec, &prop_dec, current, -1);
                                        cx.notify();
                                    },
                                )),
                            )
                            .child(
                                div()
                                    .text_sm()
                                    .text_color(cx.theme().foreground)
                                    .child(v.to_string()),
                            )
                            .child(
                                ui::button::Button::new(format!(
                                    "inc-{}-{}",
                                    class_name, prop_name
                                ))
                                .icon(IconName::Plus)
                                .xsmall()
                                .ghost()
                                .on_click(cx.listener(
                                    move |this, _event, _window, cx| {
                                        this.nudge_i32(&class_inc, &prop_inc, current, 1);
                                        cx.notify();
                                    },
                                )),
                            ),
                    )
                    .into_any_element()
            }
            (PropertyType::Bool, PropertyValue::Bool(v)) => {
                let class_toggle = class_name.to_string();
                let prop_toggle = prop_name.to_string();
                let next = !*v;

                h_flex()
                    .w_full()
                    .justify_between()
                    .items_center()
                    .gap_2()
                    .child(
                        div()
                            .text_sm()
                            .text_color(cx.theme().muted_foreground)
                            .child(display_name.to_string()),
                    )
                    .child(
                        ui::button::Button::new(format!("toggle-{}-{}", class_name, prop_name))
                            .label(if *v { "On" } else { "Off" })
                            .xsmall()
                            .ghost()
                            .on_click(cx.listener(move |this, _event, _window, cx| {
                                this.write_property(
                                    &class_toggle,
                                    &prop_toggle,
                                    PropertyValue::Bool(next),
                                );
                                cx.notify();
                            })),
                    )
                    .into_any_element()
            }
            (PropertyType::String { .. }, PropertyValue::String(v)) => h_flex()
                .w_full()
                .justify_between()
                .items_center()
                .gap_2()
                .child(
                    div()
                        .text_sm()
                        .text_color(cx.theme().muted_foreground)
                        .child(display_name.to_string()),
                )
                .child(
                    div()
                        .text_sm()
                        .text_color(cx.theme().foreground)
                        .child(v.clone()),
                )
                .into_any_element(),
            _ => h_flex()
                .w_full()
                .justify_between()
                .items_center()
                .gap_2()
                .child(
                    div()
                        .text_sm()
                        .text_color(cx.theme().muted_foreground)
                        .child(display_name.to_string()),
                )
                .child(
                    div()
                        .text_sm()
                        .text_color(cx.theme().muted_foreground)
                        .child(format!("{:?}", value)),
                )
                .into_any_element(),
        }
    }
}

impl Render for ObjectTypeFieldsSection {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // ── Object type label ─────────────────────────────────────────────
        let type_label = match self.scene_db.get_object(&self.object_id) {
            Some(obj) => match obj.object_type {
                ObjectType::Empty => "Empty".to_string(),
                ObjectType::Folder => "Folder".to_string(),
                ObjectType::Camera => "Camera".to_string(),
                ObjectType::ParticleSystem => "Particle System".to_string(),
                ObjectType::AudioSource => "Audio Source".to_string(),
                ObjectType::Light(lt) => format!("Light ({lt:?})"),
                ObjectType::Mesh(mt) => format!("Mesh ({mt:?})"),
            },
            None => "Unknown".to_string(),
        };

        // ── Type card (always rendered) ───────────────────────────────────
        let type_card = v_flex()
            .w_full()
            .gap_2()
            .p_3()
            .bg(cx.theme().sidebar)
            .rounded(px(8.0))
            .border_1()
            .border_color(cx.theme().border)
            .child(
                h_flex()
                    .w_full()
                    .justify_between()
                    .items_center()
                    .child(
                        div()
                            .text_sm()
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(cx.theme().foreground)
                            .child("Object Type"),
                    )
                    .child(
                        div()
                            .text_sm()
                            .text_color(cx.theme().muted_foreground)
                            .child(type_label),
                    ),
            );

        // ── Attached components ───────────────────────────────────────────
        let attached = self.scene_db.get_components(&self.object_id);

        // Diagnostics (debug log + in-UI card when something is wrong)
        let registry_classes = REGISTRY.get_class_names();
        tracing::debug!(
            "[ObjectTypeFieldsSection] object_id={} attached={} registry={}",
            self.object_id,
            attached.len(),
            registry_classes.len(),
        );
        for c in &attached {
            tracing::debug!(
                "  component='{}' in_registry={} props={}",
                c.class_name,
                REGISTRY.has_class(c.class_name.as_str()),
                REGISTRY
                    .create_instance(c.class_name.as_str())
                    .map(|mut i| i.get_properties().len())
                    .unwrap_or(0),
            );
        }

        let diag_card: Option<AnyElement> = if attached.is_empty() {
            Some(
                v_flex()
                    .w_full()
                    .gap_1()
                    .p_3()
                    .bg(cx.theme().sidebar)
                    .rounded(px(8.0))
                    .border_1()
                    .border_color(cx.theme().border)
                    .child(
                        div()
                            .text_xs()
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(cx.theme().muted_foreground)
                            .child("⚠ No components attached"),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(cx.theme().muted_foreground)
                            .child(format!("object_id = {}", self.object_id)),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(cx.theme().muted_foreground)
                            .child(format!(
                                "registry ({} classes): {}",
                                registry_classes.len(),
                                registry_classes.join(", ")
                            )),
                    )
                    .into_any_element(),
            )
        } else if attached
            .iter()
            .all(|c| !REGISTRY.has_class(c.class_name.as_str()))
        {
            Some(
                v_flex()
                    .w_full()
                    .gap_1()
                    .p_3()
                    .bg(cx.theme().sidebar)
                    .rounded(px(8.0))
                    .border_1()
                    .border_color(cx.theme().border)
                    .child(
                        div()
                            .text_xs()
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(cx.theme().muted_foreground)
                            .child("⚠ Components not found in registry"),
                    )
                    .children(attached.iter().map(|c| {
                        div()
                            .text_xs()
                            .text_color(cx.theme().muted_foreground)
                            .child(format!("  '{}' → missing", c.class_name))
                            .into_any_element()
                    }))
                    .child(
                        div()
                            .text_xs()
                            .text_color(cx.theme().muted_foreground)
                            .child(format!("registry: {}", registry_classes.join(", "))),
                    )
                    .into_any_element(),
            )
        } else {
            None
        };

        // ── Component hierarchy panel ─────────────────────────────────────
        // Shows components in a tree structure similar to the scene hierarchy
        let dialog = self.add_component_dialog.clone();
        let add_popover = Popover::<AddComponentDialog>::new("add-component-picker")
            .anchor(Corner::TopRight)
            .trigger(
                ui::button::Button::new("add-component-btn")
                    .icon(IconName::Plus)
                    .xsmall()
                    .ghost(),
            )
            .content(move |_window, _cx| dialog.clone())
            .into_any_element();

        let component_hierarchy =
            ComponentHierarchyPanel::new(self.object_id.clone(), self.scene_db.clone());
        let state = self.state_arc.read();
        let component_panel = component_hierarchy
            .render(&state, self.state_arc.clone(), add_popover, cx)
            .into_any_element();

        // ── Property sections for every attached component ─────────────────
        let component_sections = attached
            .iter()
            .filter_map(|component| {
                let class_name = component.class_name.as_str();
                let mut instance = REGISTRY.create_instance(class_name)?;
                let properties = instance.get_properties();
                if properties.is_empty() {
                    return None;
                }

                let rows = properties
                    .iter()
                    .map(|prop| {
                        let default = (prop.getter)(instance.as_ref());
                        let value = self.read_property(
                            class_name,
                            prop.name,
                            &prop.property_type,
                            &default,
                        );
                        self.render_property_row(
                            class_name,
                            &prop.display_name,
                            prop.name,
                            &prop.property_type,
                            &value,
                            cx,
                        )
                    })
                    .collect::<Vec<_>>();

                Some(
                    v_flex()
                        .w_full()
                        .gap_2()
                        .p_3()
                        .bg(cx.theme().sidebar)
                        .rounded(px(8.0))
                        .border_1()
                        .border_color(cx.theme().border)
                        .child(
                            h_flex()
                                .w_full()
                                .items_center()
                                .gap_2()
                                .child(ui::Icon::new(IconName::Component).small())
                                .child(
                                    div()
                                        .text_sm()
                                        .font_weight(FontWeight::SEMIBOLD)
                                        .text_color(cx.theme().foreground)
                                        .child(class_name.to_string()),
                                ),
                        )
                        .children(rows),
                )
            })
            .collect::<Vec<_>>();

        v_flex()
            .w_full()
            .gap_3()
            .child(type_card)
            .child(component_panel)
            .children(diag_card)
            .children(component_sections)
            .into_any_element()
    }
}
