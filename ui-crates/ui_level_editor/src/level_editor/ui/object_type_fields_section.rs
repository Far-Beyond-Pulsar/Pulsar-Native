use gpui::{prelude::*, *};
use pulsar_reflection::{PropertyType, PropertyValue, REGISTRY};
use serde_json::Value;
use std::collections::HashMap;
use std::sync::Arc;
use ui::button::ButtonVariants as _;
use ui::color_picker::{ColorPicker, ColorPickerEvent, ColorPickerState};
use ui::input::{InputEvent, InputState, NumberInput, NumberInputEvent, StepAction};
use ui::menu::PopupMenuItem;
use ui::popover::Popover;
use ui::switch::Switch;
use ui::{h_flex, v_flex, ActiveTheme, Icon, IconName, Sizable};
use ui_common::{AssetPickedEvent, AssetQuery, MeshAssetPicker};

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
    /// ColorPickerState per (class_name, prop_name) for Color-typed properties.
    color_pickers: HashMap<(String, String), Entity<ColorPickerState>>,
    /// Number input state per (class_name, prop_name) for numeric properties.
    numeric_inputs: HashMap<(String, String), Entity<InputState>>,
    /// Mesh asset picker state per (class_name, prop_name) for mesh path fields.
    mesh_asset_pickers: HashMap<(String, String), Entity<MeshAssetPicker>>,
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
            color_pickers: HashMap::new(),
            numeric_inputs: HashMap::new(),
            mesh_asset_pickers: HashMap::new(),
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

        self.scene_db
            .update_component(&self.object_id, idx, Value::Object(map));
    }

    fn ensure_f32_input(
        &mut self,
        class_name: &str,
        prop_name: &str,
        current: f32,
        step: f32,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let key = (class_name.to_string(), prop_name.to_string());
        if self.numeric_inputs.contains_key(&key) {
            return;
        }

        let input = cx.new(|cx| InputState::new(window, cx));
        input.update(cx, |state, cx| {
            state.set_value(&format!("{:.3}", current), window, cx);
        });

        let cls = class_name.to_string();
        let prop = prop_name.to_string();
        cx.subscribe_in(
            &input,
            window,
            move |this, state, ev: &InputEvent, _window, cx| {
                if matches!(ev, InputEvent::Change | InputEvent::Blur) {
                    let text = state.read(cx).text().to_string();
                    if let Ok(v) = text.parse::<f32>() {
                        this.write_property(&cls, &prop, PropertyValue::F32(v));
                        cx.notify();
                    }
                }
            },
        )
        .detach();

        let cls = class_name.to_string();
        let prop = prop_name.to_string();
        cx.subscribe_in(
            &input,
            window,
            move |this, state, ev: &NumberInputEvent, window, cx| {
                let NumberInputEvent::Step { action, fine } = ev;
                state.update(cx, |input, cx| {
                    let text = input.text().to_string();
                    if let Ok(mut value) = text.parse::<f32>() {
                        let step_size = if *fine { step * 0.1 } else { step };
                        match action {
                            StepAction::Increment => value += step_size,
                            StepAction::Decrement => value -= step_size,
                        }
                        this.write_property(&cls, &prop, PropertyValue::F32(value));
                        input.set_value(&format!("{value:.3}"), window, cx);
                        cx.notify();
                    }
                });
            },
        )
        .detach();

        self.numeric_inputs.insert(key, input);
    }

    fn ensure_i32_input(
        &mut self,
        class_name: &str,
        prop_name: &str,
        current: i32,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let key = (class_name.to_string(), prop_name.to_string());
        if self.numeric_inputs.contains_key(&key) {
            return;
        }

        let input = cx.new(|cx| InputState::new(window, cx));
        input.update(cx, |state, cx| {
            state.set_value(&current.to_string(), window, cx);
        });

        let cls = class_name.to_string();
        let prop = prop_name.to_string();
        cx.subscribe_in(
            &input,
            window,
            move |this, state, ev: &InputEvent, _window, cx| {
                if matches!(ev, InputEvent::Change | InputEvent::Blur) {
                    let text = state.read(cx).text().to_string();
                    if let Ok(v) = text.parse::<i32>() {
                        this.write_property(&cls, &prop, PropertyValue::I32(v));
                        cx.notify();
                    }
                }
            },
        )
        .detach();

        let cls = class_name.to_string();
        let prop = prop_name.to_string();
        cx.subscribe_in(
            &input,
            window,
            move |this, state, ev: &NumberInputEvent, window, cx| {
                let NumberInputEvent::Step { action, .. } = ev;
                state.update(cx, |input, cx| {
                    let text = input.text().to_string();
                    if let Ok(mut value) = text.parse::<i32>() {
                        match action {
                            StepAction::Increment => value += 1,
                            StepAction::Decrement => value -= 1,
                        }
                        this.write_property(&cls, &prop, PropertyValue::I32(value));
                        input.set_value(value.to_string(), window, cx);
                        cx.notify();
                    }
                });
            },
        )
        .detach();

        self.numeric_inputs.insert(key, input);
    }

    fn ensure_mesh_asset_picker(
        &mut self,
        class_name: &str,
        prop_name: &str,
        current: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let key = (class_name.to_string(), prop_name.to_string());
        if self.mesh_asset_pickers.contains_key(&key) {
            return;
        }

        let builtins = vec![
            "meshes/primitives/SM_Cube.fbx".to_string(),
            "meshes/primitives/SM_Sphere.fbx".to_string(),
            "meshes/primitives/SM_Cylinder.fbx".to_string(),
            "meshes/primitives/SM_Plane.fbx".to_string(),
        ];

        let project_root = engine_state::get_project_path().map(std::path::PathBuf::from);
        let queries = vec![AssetQuery::extension("fbx")];
        let picker = cx.new(|cx| {
            MeshAssetPicker::new(
                current.to_string(),
                builtins,
                project_root,
                queries,
                window,
                cx,
            )
        });

        let cls = class_name.to_string();
        let prop = prop_name.to_string();
        cx.subscribe(
            &picker,
            move |this, picker, _event: &AssetPickedEvent, cx| {
                let selected = picker.read(cx).selected_path().to_string();
                this.write_property(&cls, &prop, PropertyValue::String(selected));
                cx.notify();
            },
        )
        .detach();

        self.mesh_asset_pickers.insert(key, picker);
    }

    fn color_fallback_rgba(&self, class_name: &str, prop_name: &str) -> [f32; 4] {
        let components = self.scene_db.get_components(&self.object_id);
        components
            .iter()
            .find(|c| c.class_name == class_name)
            .and_then(|component| component.data.get(prop_name))
            .and_then(|v| v.as_array())
            .and_then(|arr| {
                if arr.len() == 4 {
                    Some([
                        arr[0].as_f64().unwrap_or(1.0) as f32,
                        arr[1].as_f64().unwrap_or(1.0) as f32,
                        arr[2].as_f64().unwrap_or(1.0) as f32,
                        arr[3].as_f64().unwrap_or(1.0) as f32,
                    ])
                } else {
                    None
                }
            })
            .unwrap_or([1.0, 1.0, 1.0, 1.0])
    }

    fn is_color_field_name(prop_name: &str) -> bool {
        prop_name == "color" || prop_name == "base_color"
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
            (PropertyType::F32 { .. }, PropertyValue::F32(v)) => {
                let input_key = (class_name.to_string(), prop_name.to_string());
                let input_opt = self.numeric_inputs.get(&input_key).cloned();

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
                            .child(if let Some(input) = input_opt {
                                NumberInput::new(&input)
                                    .xsmall()
                                    .w(px(92.0))
                                    .into_any_element()
                            } else {
                                div()
                                    .text_sm()
                                    .text_color(cx.theme().foreground)
                                    .child(format!("{:.3}", v))
                                    .into_any_element()
                            }),
                    )
                    .into_any_element()
            }
            (PropertyType::I32 { .. }, PropertyValue::I32(v)) => {
                let input_key = (class_name.to_string(), prop_name.to_string());
                let input_opt = self.numeric_inputs.get(&input_key).cloned();

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
                            .child(if let Some(input) = input_opt {
                                NumberInput::new(&input)
                                    .xsmall()
                                    .w(px(92.0))
                                    .into_any_element()
                            } else {
                                div()
                                    .text_sm()
                                    .text_color(cx.theme().foreground)
                                    .child(v.to_string())
                                    .into_any_element()
                            }),
                    )
                    .into_any_element()
            }
            (PropertyType::Bool, PropertyValue::Bool(v)) => {
                let class_toggle = class_name.to_string();
                let prop_toggle = prop_name.to_string();
                let scene_db = self.scene_db.clone();
                let object_id = self.object_id.clone();
                let view = cx.entity().downgrade();

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
                        Switch::new(format!("toggle-{}-{}", class_name, prop_name))
                            .checked(*v)
                            .small()
                            .on_click(move |checked, _window, cx| {
                                scene_db.update_component_property(
                                    &object_id,
                                    &class_toggle,
                                    &prop_toggle,
                                    Value::from(*checked),
                                );
                                if let Some(entity) = view.upgrade() {
                                    entity.update(cx, |_this, cx| cx.notify());
                                }
                            }),
                    )
                    .into_any_element()
            }
            (PropertyType::Enum { variants }, PropertyValue::EnumVariant(current_ix)) => {
                let selected_ix = (*current_ix).min(variants.len().saturating_sub(1));
                let label = variants
                    .get(selected_ix)
                    .map(|v| (*v).to_string())
                    .unwrap_or_else(|| "Select".to_string());
                let options = variants
                    .iter()
                    .map(|v| (*v).to_string())
                    .collect::<Vec<_>>();

                let scene_db = self.scene_db.clone();
                let object_id = self.object_id.clone();
                let class_enum = class_name.to_string();
                let prop_enum = prop_name.to_string();
                let view = cx.entity().downgrade();

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
                        ui::button::Button::new(format!("enum-{}-{}", class_name, prop_name))
                            .label(label)
                            .xsmall()
                            .ghost()
                            .dropdown_caret(true)
                            .dropdown_menu_with_anchor(
                                Corner::BottomRight,
                                move |menu, _window, _cx| {
                                    options.iter().enumerate().fold(menu, |menu, (ix, option)| {
                                        let scene_db = scene_db.clone();
                                        let object_id = object_id.clone();
                                        let class_enum = class_enum.clone();
                                        let prop_enum = prop_enum.clone();
                                        let view = view.clone();

                                        menu.item(
                                            PopupMenuItem::new(option.clone())
                                                .checked(ix == selected_ix)
                                                .on_click(move |_event, _window, cx| {
                                                    scene_db.update_component_property(
                                                        &object_id,
                                                        &class_enum,
                                                        &prop_enum,
                                                        Value::from(ix as u64),
                                                    );
                                                    if let Some(entity) = view.upgrade() {
                                                        entity.update(cx, |_this, cx| cx.notify());
                                                    }
                                                }),
                                        )
                                    })
                                },
                            ),
                    )
                    .into_any_element()
            }
            (_, PropertyValue::String(v))
                if v == "unsupported" && Self::is_color_field_name(prop_name) =>
            {
                let key = (class_name.to_string(), prop_name.to_string());
                if let Some(picker_state) = self.color_pickers.get(&key) {
                    v_flex()
                        .w_full()
                        .gap_1()
                        .child(
                            div()
                                .text_sm()
                                .text_color(cx.theme().muted_foreground)
                                .child(display_name.to_string()),
                        )
                        .child(ColorPicker::new(picker_state).label(display_name.to_string()))
                        .into_any_element()
                } else {
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
                            div()
                                .text_sm()
                                .text_color(cx.theme().muted_foreground)
                                .child("Color field unavailable"),
                        )
                        .into_any_element()
                }
            }
            (PropertyType::String { .. }, PropertyValue::String(v)) => {
                if prop_name == "mesh_asset" {
                    let key = (class_name.to_string(), prop_name.to_string());
                    if let Some(picker) = self.mesh_asset_pickers.get(&key).cloned() {
                        let display = if v.is_empty() {
                            "Select mesh asset".to_string()
                        } else {
                            v.clone()
                        };
                        let pop = Popover::<MeshAssetPicker>::new(format!(
                            "mesh-asset-picker-{}-{}",
                            class_name, prop_name
                        ))
                        .anchor(Corner::BottomRight)
                        .trigger(
                            ui::button::Button::new(format!(
                                "mesh-asset-btn-{}-{}",
                                class_name, prop_name
                            ))
                            .label(display)
                            .xsmall()
                            .ghost()
                            .dropdown_caret(true),
                        )
                        .content(move |_window, _cx| picker.clone())
                        .into_any_element();

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
                            .child(pop)
                            .into_any_element()
                    } else {
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
                                div()
                                    .text_sm()
                                    .text_color(cx.theme().foreground)
                                    .child(v.clone()),
                            )
                            .into_any_element()
                    }
                } else {
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
                            div()
                                .text_sm()
                                .text_color(cx.theme().foreground)
                                .child(v.clone()),
                        )
                        .into_any_element()
                }
            }
            (PropertyType::Color, PropertyValue::Color(_)) => {
                let key = (class_name.to_string(), prop_name.to_string());
                if let Some(picker_state) = self.color_pickers.get(&key) {
                    v_flex()
                        .w_full()
                        .gap_1()
                        .child(
                            div()
                                .text_sm()
                                .text_color(cx.theme().muted_foreground)
                                .child(display_name.to_string()),
                        )
                        .child(ColorPicker::new(picker_state).label(display_name.to_string()))
                        .into_any_element()
                } else {
                    // Picker not yet created (pre-pass missed it) — show value as fallback.
                    div()
                        .text_sm()
                        .child(format!("{:?}", value))
                        .into_any_element()
                }
            }
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

fn rgba_to_hsla([r, g, b, a]: [f32; 4]) -> Hsla {
    let max = r.max(g).max(b);
    let min = r.min(g).min(b);
    let l = (max + min) / 2.0;
    let s = if max == min {
        0.0
    } else if l < 0.5 {
        (max - min) / (max + min)
    } else {
        (max - min) / (2.0 - max - min)
    };
    let h = if max == min {
        0.0
    } else if max == r {
        ((g - b) / (max - min)).rem_euclid(6.0) / 6.0
    } else if max == g {
        ((b - r) / (max - min) + 2.0) / 6.0
    } else {
        ((r - g) / (max - min) + 4.0) / 6.0
    };
    Hsla { h, s, l, a }
}

fn hsla_to_rgba(Hsla { h, s, l, a }: Hsla) -> [f32; 4] {
    let c = (1.0 - (2.0 * l - 1.0).abs()) * s;
    let x = c * (1.0 - ((h * 6.0).rem_euclid(2.0) - 1.0).abs());
    let m = l - c / 2.0;
    let (r1, g1, b1) = match (h * 6.0) as u32 {
        0 => (c, x, 0.0),
        1 => (x, c, 0.0),
        2 => (0.0, c, x),
        3 => (0.0, x, c),
        4 => (x, 0.0, c),
        _ => (c, 0.0, x),
    };
    [r1 + m, g1 + m, b1 + m, a]
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
        drop(state);

        // ── Pre-populate ColorPickerState for any Color-typed properties ───
        for component in &attached {
            let class_name = component.class_name.as_str();
            if let Some(mut instance) = REGISTRY.create_instance(class_name) {
                for prop in instance.get_properties() {
                    let default = (prop.getter)(instance.as_ref());
                    let current =
                        self.read_property(class_name, prop.name, &prop.property_type, &default);

                    match (&prop.property_type, &current) {
                        (PropertyType::F32 { step, .. }, PropertyValue::F32(v)) => {
                            self.ensure_f32_input(
                                class_name,
                                prop.name,
                                *v,
                                step.unwrap_or(1.0),
                                window,
                                cx,
                            );
                        }
                        (PropertyType::I32 { .. }, PropertyValue::I32(v)) => {
                            self.ensure_i32_input(class_name, prop.name, *v, window, cx);
                        }
                        (PropertyType::String { .. }, PropertyValue::String(v))
                            if prop.name == "mesh_asset" =>
                        {
                            self.ensure_mesh_asset_picker(class_name, prop.name, v, window, cx);
                        }
                        _ => {}
                    }

                    let should_create_picker = matches!(prop.property_type, PropertyType::Color)
                        || (matches!(&default, PropertyValue::String(s) if s == "unsupported")
                            && Self::is_color_field_name(prop.name));

                    if should_create_picker {
                        let key = (class_name.to_string(), prop.name.to_string());
                        if !self.color_pickers.contains_key(&key) {
                            let rgba = if let PropertyValue::Color(c) = current {
                                c
                            } else {
                                self.color_fallback_rgba(class_name, prop.name)
                            };
                            let scene_db = self.scene_db.clone();
                            let object_id = self.object_id.clone();
                            let cn = class_name.to_string();
                            let pn = prop.name.to_string();
                            let state = cx.new(|cx| {
                                let mut s = ColorPickerState::new(window, cx);
                                s.set_value(rgba_to_hsla(rgba), window, cx);
                                s
                            });
                            cx.subscribe_in(&state, window, move |_this, _picker, ev, _w, _cx| {
                                if let ColorPickerEvent::Change(Some(hsla)) = ev {
                                    let json_val = {
                                        let [r, g, b, a] = hsla_to_rgba(*hsla);
                                        serde_json::json!([r, g, b, a])
                                    };
                                    scene_db
                                        .update_component_property(&object_id, &cn, &pn, json_val);
                                }
                            })
                            .detach();
                            self.color_pickers.insert(key, state);
                        }
                    }
                }
            }
        }

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
