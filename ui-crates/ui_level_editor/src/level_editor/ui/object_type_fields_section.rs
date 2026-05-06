use gpui::{prelude::*, *};
use pulsar_reflection::{PropertyType, PropertyValue, REGISTRY};
use serde_json::Value;
use ui::{h_flex, v_flex, ActiveTheme, IconName, Sizable};
use ui::button::ButtonVariants as _;

use crate::level_editor::scene_database::{ObjectType, SceneDatabase};

pub struct ObjectTypeFieldsSection {
    object_id: String,
    scene_db: SceneDatabase,
}

impl ObjectTypeFieldsSection {
    pub fn new(
        object_id: String,
        scene_db: SceneDatabase,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Self {
        Self {
            object_id,
            scene_db,
        }
    }

    fn required_classes_for_type(object_type: ObjectType) -> Vec<&'static str> {
        match object_type {
            ObjectType::Light(_) => vec!["LightComponent"],
            ObjectType::Mesh(_) => vec!["MaterialOverride", "LodComponent"],
            ObjectType::Camera => vec![],
            ObjectType::Folder => vec![],
            ObjectType::Empty => vec![],
            ObjectType::ParticleSystem => vec![],
            ObjectType::AudioSource => vec![],
        }
    }

    fn ensure_type_components(&self, object_type: ObjectType) {
        let required = Self::required_classes_for_type(object_type);
        if required.is_empty() {
            return;
        }

        let existing = self.scene_db.get_components(&self.object_id);
        for class_name in required {
            let already_present = existing.iter().any(|c| c.class_name == class_name);
            if already_present || !REGISTRY.has_class(class_name) {
                continue;
            }

            if let Some(default_data) = Self::build_default_component_data(class_name) {
                self.scene_db
                    .add_component(&self.object_id, class_name.to_string(), default_data);
            }
        }
    }

    fn build_default_component_data(class_name: &str) -> Option<Value> {
        let instance = REGISTRY.create_instance(class_name)?;
        let props = instance.get_properties();
        let mut map = serde_json::Map::new();

        for prop in props {
            let value = (prop.getter)(instance.as_ref());
            map.insert(prop.name.to_string(), Self::property_value_to_json(&value));
        }

        Some(Value::Object(map))
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
            PropertyValue::Vec(v) => Value::Array(v.iter().map(Self::property_value_to_json).collect()),
            PropertyValue::Component { class_name, .. } => serde_json::json!({"class_name": class_name}),
        }
    }

    fn json_to_property_value(property_type: &PropertyType, json: &Value) -> Option<PropertyValue> {
        match property_type {
            PropertyType::F32 { .. } => json.as_f64().map(|v| PropertyValue::F32(v as f32)),
            PropertyType::I32 { .. } => json.as_i64().map(|v| PropertyValue::I32(v as i32)),
            PropertyType::Bool => json.as_bool().map(PropertyValue::Bool),
            PropertyType::String { .. } => json.as_str().map(|s| PropertyValue::String(s.to_string())),
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
            PropertyType::Enum { .. } => json.as_u64().map(|v| PropertyValue::EnumVariant(v as usize)),
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
        map.insert(property_name.to_string(), Self::property_value_to_json(&value));

        let _ = self
            .scene_db
            .metadata_db
            .components()
            .update_component(&self.object_id, idx, Value::Object(map));
    }

    fn nudge_numeric(&self, class_name: &str, prop_name: &str, current: f32, step: f32, sign: f32) {
        self.write_property(class_name, prop_name, PropertyValue::F32(current + step * sign));
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
                    .child(div().text_sm().text_color(cx.theme().muted_foreground).child(display_name.to_string()))
                    .child(
                        h_flex()
                            .items_center()
                            .gap_2()
                            .child(
                                ui::button::Button::new(format!("dec-{}-{}", class_name, prop_name))
                                    .icon(IconName::Minus)
                                    .xsmall()
                                    .ghost()
                                    .on_click(cx.listener(move |this, _event, _window, cx| {
                                        this.nudge_numeric(
                                            &class_dec,
                                            &prop_dec,
                                            current,
                                            step,
                                            -1.0,
                                        );
                                        cx.notify();
                                    })),
                            )
                            .child(div().text_sm().text_color(cx.theme().foreground).child(format!("{:.3}", v)))
                            .child(
                                ui::button::Button::new(format!("inc-{}-{}", class_name, prop_name))
                                    .icon(IconName::Plus)
                                    .xsmall()
                                    .ghost()
                                    .on_click(cx.listener(move |this, _event, _window, cx| {
                                        this.nudge_numeric(
                                            &class_inc,
                                            &prop_inc,
                                            current,
                                            step,
                                            1.0,
                                        );
                                        cx.notify();
                                    })),
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
                    .child(div().text_sm().text_color(cx.theme().muted_foreground).child(display_name.to_string()))
                    .child(
                        h_flex()
                            .items_center()
                            .gap_2()
                            .child(
                                ui::button::Button::new(format!("dec-{}-{}", class_name, prop_name))
                                    .icon(IconName::Minus)
                                    .xsmall()
                                    .ghost()
                                    .on_click(cx.listener(move |this, _event, _window, cx| {
                                        this.nudge_i32(&class_dec, &prop_dec, current, -1);
                                        cx.notify();
                                    })),
                            )
                            .child(div().text_sm().text_color(cx.theme().foreground).child(v.to_string()))
                            .child(
                                ui::button::Button::new(format!("inc-{}-{}", class_name, prop_name))
                                    .icon(IconName::Plus)
                                    .xsmall()
                                    .ghost()
                                    .on_click(cx.listener(move |this, _event, _window, cx| {
                                        this.nudge_i32(&class_inc, &prop_inc, current, 1);
                                        cx.notify();
                                    })),
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
                    .child(div().text_sm().text_color(cx.theme().muted_foreground).child(display_name.to_string()))
                    .child(
                        ui::button::Button::new(format!("toggle-{}-{}", class_name, prop_name))
                            .label(if *v { "On" } else { "Off" })
                            .xsmall()
                            .ghost()
                            .on_click(cx.listener(move |this, _event, _window, cx| {
                                this.write_property(&class_toggle, &prop_toggle, PropertyValue::Bool(next));
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
                .child(div().text_sm().text_color(cx.theme().muted_foreground).child(display_name.to_string()))
                .child(div().text_sm().text_color(cx.theme().foreground).child(v.clone()))
                .into_any_element(),
            _ => h_flex()
                .w_full()
                .justify_between()
                .items_center()
                .gap_2()
                .child(div().text_sm().text_color(cx.theme().muted_foreground).child(display_name.to_string()))
                .child(div().text_sm().text_color(cx.theme().muted_foreground).child(format!("{:?}", value)))
                .into_any_element(),
        }
    }
}

impl Render for ObjectTypeFieldsSection {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let Some(obj) = self.scene_db.get_object(&self.object_id) else {
            return div().into_any_element();
        };

        self.ensure_type_components(obj.object_type);
        let required_classes = Self::required_classes_for_type(obj.object_type);

        if required_classes.is_empty() {
            return div().into_any_element();
        }

        let sections = required_classes
            .into_iter()
            .filter_map(|class_name| {
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

        if sections.is_empty() {
            div().into_any_element()
        } else {
            v_flex().w_full().gap_3().children(sections).into_any_element()
        }
    }
}
