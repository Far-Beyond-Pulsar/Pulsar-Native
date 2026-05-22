//! Shared reflected properties panel system
//!
//! This module provides a centralized system for rendering reflected component properties
//! that is used by both the level editor and blueprint prefab editor.

use gpui::{prelude::*, *};
use pulsar_reflection::{PropertyType, PropertyValue, REGISTRY};
use serde_json::Value;
use std::collections::HashMap;
use std::sync::Arc;
use ui::button::ButtonVariants as _;
use ui::color_picker::{ColorPickerEvent, ColorPickerState};
use ui::input::{InputEvent, InputState, NumberInputEvent, StepAction};
use ui::popover::Popover;
use ui::{h_flex, v_flex, ActiveTheme, Icon, IconName, Sizable};
use crate::{AssetPickedEvent, AssetQuery, MeshAssetPicker};

// ============================================================================
// Type Aliases and Traits
// ============================================================================

/// Callback for reading a property value from the data source
pub type PropertyReader = Arc<dyn Fn(&str, &str) -> Value + Send + Sync>;

/// Callback for writing a property value to the data source
pub type PropertyWriter = Arc<dyn Fn(&str, &str, Value) + Send + Sync>;

/// Configuration for the reflected properties panel
pub struct ReflectedPropertiesPanelConfig {
    /// ID prefix for UI elements (e.g., "level", "prefab")
    pub id_prefix: String,
    /// Whether to show the component hierarchy list
    pub show_component_list: bool,
    /// Optional component list renderer
    pub component_list_renderer: Option<Box<dyn Fn(&mut App) -> AnyElement>>,
}

impl Default for ReflectedPropertiesPanelConfig {
    fn default() -> Self {
        Self {
            id_prefix: "panel".to_string(),
            show_component_list: false,
            component_list_renderer: None,
        }
    }
}

// ============================================================================
// Property Value Conversion Utilities
// ============================================================================

pub fn property_value_to_json(value: &PropertyValue) -> Value {
    match value {
        PropertyValue::F32(v) => Value::from(*v),
        PropertyValue::I32(v) => Value::from(*v),
        PropertyValue::Bool(v) => Value::from(*v),
        PropertyValue::String(v) => Value::from(v.clone()),
        PropertyValue::Vec3(v) => serde_json::json!([v[0], v[1], v[2]]),
        PropertyValue::Color(v) => serde_json::json!([v[0], v[1], v[2], v[3]]),
        PropertyValue::EnumVariant(v) => Value::from(*v as u64),
        PropertyValue::Vec(v) => {
            Value::Array(v.iter().map(property_value_to_json).collect())
        }
        PropertyValue::Component { class_name, .. } => {
            serde_json::json!({"class_name": class_name})
        }
    }
}

pub fn json_to_property_value(property_type: &PropertyType, json: &Value) -> Option<PropertyValue> {
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

// ============================================================================
// Color Utilities
// ============================================================================

pub fn rgba_to_hsla([r, g, b, a]: [f32; 4]) -> Hsla {
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

pub fn hsla_to_rgba(Hsla { h, s, l, a }: Hsla) -> [f32; 4] {
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

pub fn is_color_field_name(prop_name: &str) -> bool {
    prop_name == "color" || prop_name == "base_color"
}

/// Extract RGBA from JSON value with fallback
pub fn json_to_rgba_fallback(json: &Value) -> [f32; 4] {
    json.as_array()
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

// ============================================================================
// Property State Management
// ============================================================================

/// State manager for property input fields, color pickers, and mesh pickers
pub struct PropertyStateManager {
    /// ColorPickerState per (class_name, prop_name) for Color-typed properties
    pub color_pickers: HashMap<(String, String), Entity<ColorPickerState>>,
    /// Number input state per (class_name, prop_name) for numeric properties
    pub numeric_inputs: HashMap<(String, String), Entity<InputState>>,
    /// Mesh asset picker state per (class_name, prop_name) for mesh path fields
    pub mesh_asset_pickers: HashMap<(String, String), Entity<MeshAssetPicker>>,
}

impl PropertyStateManager {
    pub fn new() -> Self {
        Self {
            color_pickers: HashMap::new(),
            numeric_inputs: HashMap::new(),
            mesh_asset_pickers: HashMap::new(),
        }
    }

    pub fn clear(&mut self) {
        self.color_pickers.clear();
        self.numeric_inputs.clear();
        self.mesh_asset_pickers.clear();
    }

    /// Ensure an F32 input exists and is up to date
    pub fn ensure_f32_input<V: 'static>(
        &mut self,
        class_name: &str,
        prop_name: &str,
        current: f32,
        step: f32,
        on_change: impl Fn(f32) + 'static + Send + Sync,
        window: &mut Window,
        cx: &mut Context<V>,
    ) -> Entity<InputState> {
        let key = (class_name.to_string(), prop_name.to_string());
        if let Some(input) = self.numeric_inputs.get(&key) {
            return input.clone();
        }

        let input = cx.new(|cx| InputState::new(window, cx));
        input.update(cx, |state, cx| {
            state.set_value(&format!("{:.3}", current), window, cx);
        });

        let on_change = Arc::new(on_change);
        let on_change_clone = on_change.clone();
        cx.subscribe_in(
            &input,
            window,
            move |_this, state, ev: &InputEvent, _window, _cx| {
                if matches!(ev, InputEvent::Change | InputEvent::Blur) {
                    let text = state.read(_cx).text().to_string();
                    if let Ok(v) = text.parse::<f32>() {
                        (on_change)(v);
                    }
                }
            },
        )
        .detach();

        cx.subscribe_in(
            &input,
            window,
            move |_this, state, ev: &NumberInputEvent, window, cx| {
                let NumberInputEvent::Step { action, fine } = ev;
                state.update(cx, |input, cx| {
                    let text = input.text().to_string();
                    if let Ok(mut value) = text.parse::<f32>() {
                        let step_size = if *fine { step * 0.1 } else { step };
                        match action {
                            StepAction::Increment => value += step_size,
                            StepAction::Decrement => value -= step_size,
                        }
                        (on_change_clone)(value);
                        input.set_value(&format!("{value:.3}"), window, cx);
                    }
                });
            },
        )
        .detach();

        self.numeric_inputs.insert(key, input.clone());
        input
    }

    /// Ensure an I32 input exists and is up to date
    pub fn ensure_i32_input<V: 'static>(
        &mut self,
        class_name: &str,
        prop_name: &str,
        current: i32,
        on_change: impl Fn(i32) + 'static + Send + Sync,
        window: &mut Window,
        cx: &mut Context<V>,
    ) -> Entity<InputState> {
        let key = (class_name.to_string(), prop_name.to_string());
        if let Some(input) = self.numeric_inputs.get(&key) {
            return input.clone();
        }

        let input = cx.new(|cx| InputState::new(window, cx));
        input.update(cx, |state, cx| {
            state.set_value(&current.to_string(), window, cx);
        });

        let on_change = Arc::new(on_change);
        let on_change_clone = on_change.clone();
        cx.subscribe_in(
            &input,
            window,
            move |_this, state, ev: &InputEvent, _window, _cx| {
                if matches!(ev, InputEvent::Change | InputEvent::Blur) {
                    let text = state.read(_cx).text().to_string();
                    if let Ok(v) = text.parse::<i32>() {
                        (on_change)(v);
                    }
                }
            },
        )
        .detach();

        cx.subscribe_in(
            &input,
            window,
            move |_this, state, ev: &NumberInputEvent, window, cx| {
                let NumberInputEvent::Step { action, .. } = ev;
                state.update(cx, |input, cx| {
                    let text = input.text().to_string();
                    if let Ok(mut value) = text.parse::<i32>() {
                        match action {
                            StepAction::Increment => value += 1,
                            StepAction::Decrement => value -= 1,
                        }
                        (on_change_clone)(value);
                        input.set_value(value.to_string(), window, cx);
                    }
                });
            },
        )
        .detach();

        self.numeric_inputs.insert(key, input.clone());
        input
    }

    /// Ensure a mesh asset picker exists
    pub fn ensure_mesh_asset_picker<V: 'static>(
        &mut self,
        class_name: &str,
        prop_name: &str,
        current: &str,
        on_change: impl Fn(String) + 'static + Send + Sync,
        window: &mut Window,
        cx: &mut Context<V>,
    ) -> Entity<MeshAssetPicker> {
        let key = (class_name.to_string(), prop_name.to_string());
        if let Some(picker) = self.mesh_asset_pickers.get(&key) {
            return picker.clone();
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

        let on_change = Arc::new(on_change);
        cx.subscribe(&picker, move |_this, picker, _event: &AssetPickedEvent, cx| {
            let selected = picker.read(cx).selected_path().to_string();
            (on_change)(selected);
        })
        .detach();

        self.mesh_asset_pickers.insert(key, picker.clone());
        picker
    }

    /// Ensure a color picker exists
    pub fn ensure_color_picker<V: 'static>(
        &mut self,
        class_name: &str,
        prop_name: &str,
        rgba: [f32; 4],
        on_change: impl Fn([f32; 4]) + 'static + Send + Sync,
        window: &mut Window,
        cx: &mut Context<V>,
    ) -> Entity<ColorPickerState> {
        let key = (class_name.to_string(), prop_name.to_string());
        if let Some(picker) = self.color_pickers.get(&key) {
            return picker.clone();
        }

        let state = cx.new(|cx| {
            let mut s = ColorPickerState::new(window, cx);
            s.set_value(rgba_to_hsla(rgba), window, cx);
            s
        });

        let on_change = Arc::new(on_change);
        cx.subscribe_in(&state, window, move |_this, _picker, ev, _w, _cx| {
            if let ColorPickerEvent::Change(Some(hsla)) = ev {
                (on_change)(hsla_to_rgba(*hsla));
            }
        })
        .detach();

        self.color_pickers.insert(key, state.clone());
        state
    }
}

impl Default for PropertyStateManager {
    fn default() -> Self {
        Self::new()
    }
}

// ============================================================================
// Property Row Rendering
// ============================================================================

/// Render a single property row with full type support including mesh fields
pub fn render_property_row<V: 'static>(
    id_prefix: &str,
    class_name: &str,
    display_name: &str,
    prop_name: &str,
    property_type: &PropertyType,
    value: &PropertyValue,
    numeric_input: Option<Entity<InputState>>,
    color_picker: Option<Entity<ColorPickerState>>,
    mesh_picker: Option<Entity<MeshAssetPicker>>,
    on_bool_toggle: Arc<dyn Fn(bool, &mut Window, &mut App)>,
    on_enum_select: Arc<dyn Fn(usize, &mut Window, &mut App)>,
    cx: &Context<V>,
) -> AnyElement {
    // ── Mesh Asset Field Handling ──
    if matches!((property_type, value), (PropertyType::String { .. }, PropertyValue::String(v)) if prop_name == "mesh_asset")
    {
        if let (PropertyValue::String(v), Some(picker)) = (value, mesh_picker) {
            let display = if v.is_empty() {
                "Select mesh asset…".to_string()
            } else {
                std::path::Path::new(v)
                    .file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or(v)
                    .to_string()
            };

            let thumb = picker.read(cx).thumbnail_for_path(v);

            let pop = Popover::<MeshAssetPicker>::new(format!(
                "mesh-asset-picker-{}-{}",
                class_name, prop_name
            ))
            .anchor(Corner::BottomRight)
            .trigger(
                ui::button::Button::new(format!("mesh-asset-btn-{}-{}", class_name, prop_name))
                    .label(display)
                    .small()
                    .ghost()
                    .dropdown_caret(true),
            )
            .content(move |_window, _cx| picker.clone())
            .into_any_element();

            return h_flex()
                .w_full()
                .justify_between()
                .items_center()
                .gap_2()
                .py_1()
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
                        .child(pop)
                        .map(|el| match thumb {
                            Some(render_img) => el.child(
                                div()
                                    .w(px(40.0))
                                    .h(px(40.0))
                                    .rounded(px(4.0))
                                    .overflow_hidden()
                                    .border_1()
                                    .border_color(cx.theme().border)
                                    .flex_shrink_0()
                                    .child(
                                        gpui::img(gpui::ImageSource::Render(render_img))
                                            .w(px(40.0))
                                            .h(px(40.0))
                                            .object_fit(gpui::ObjectFit::Cover),
                                    ),
                            ),
                            None => el,
                        }),
                )
                .into_any_element();
        }
    }

    // ── Standard Property Type Handling ──
    match (property_type, value) {
        (PropertyType::F32 { .. }, PropertyValue::F32(v)) => h_flex()
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
            .child(h_flex().items_center().gap_2().child(
                if let Some(input) = numeric_input {
                    ui::input::NumberInput::new(&input)
                        .xsmall()
                        .w(px(92.0))
                        .into_any_element()
                } else {
                    div()
                        .text_sm()
                        .text_color(cx.theme().foreground)
                        .child(format!("{:.3}", v))
                        .into_any_element()
                },
            ))
            .into_any_element(),
        (PropertyType::I32 { .. }, PropertyValue::I32(v)) => h_flex()
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
            .child(h_flex().items_center().gap_2().child(
                if let Some(input) = numeric_input {
                    ui::input::NumberInput::new(&input)
                        .xsmall()
                        .w(px(92.0))
                        .into_any_element()
                } else {
                    div()
                        .text_sm()
                        .text_color(cx.theme().foreground)
                        .child(v.to_string())
                        .into_any_element()
                },
            ))
            .into_any_element(),
        (PropertyType::Bool, PropertyValue::Bool(v)) => {
            let on_bool_toggle = on_bool_toggle.clone();
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
                    ui::switch::Switch::new(format!("toggle-{id_prefix}-{class_name}-{prop_name}"))
                        .checked(*v)
                        .small()
                        .on_click(move |checked, window, cx| {
                            (on_bool_toggle)(*checked, window, cx);
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
            let options = variants.iter().map(|v| (*v).to_string()).collect::<Vec<_>>();

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
                    ui::button::Button::new(format!("enum-{id_prefix}-{class_name}-{prop_name}"))
                        .label(label)
                        .xsmall()
                        .ghost()
                        .dropdown_caret(true)
                        .dropdown_menu_with_anchor(Corner::BottomRight, move |menu, _window, _cx| {
                            let mut menu = menu;
                            for (ix, option) in options.iter().enumerate() {
                                let on_enum_select = on_enum_select.clone();
                                menu = menu.item(
                                    ui::menu::PopupMenuItem::new(option.clone())
                                        .checked(ix == selected_ix)
                                        .on_click(move |_event, window, cx| {
                                            (on_enum_select)(ix, window, cx);
                                        }),
                                );
                            }
                            menu
                        }),
                )
                .into_any_element()
        }
        (_, PropertyValue::String(v)) if v == "unsupported" && is_color_field_name(prop_name) => {
            if let Some(picker_state) = color_picker {
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
                    .child(ui::color_picker::ColorPicker::new(&picker_state).anchor(Corner::BottomRight))
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
        (PropertyType::Color, PropertyValue::Color(_)) => {
            if let Some(picker_state) = color_picker {
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
                    .child(ui::color_picker::ColorPicker::new(&picker_state).anchor(Corner::BottomRight))
                    .into_any_element()
            } else {
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

// ============================================================================
// Component Section Rendering
// ============================================================================

/// Render a complete component section with all its properties
pub fn render_component_section<V: 'static>(
    id_prefix: &str,
    class_name: &str,
    properties: Vec<(
        String,      // display_name
        String,      // prop_name
        PropertyType,
        PropertyValue,
        Option<Entity<InputState>>,         // numeric_input
        Option<Entity<ColorPickerState>>,   // color_picker
        Option<Entity<MeshAssetPicker>>,    // mesh_picker
    )>,
    on_bool_toggle: Arc<dyn Fn(&str, bool, &mut Window, &mut App) + Send + Sync>,
    on_enum_select: Arc<dyn Fn(&str, usize, &mut Window, &mut App) + Send + Sync>,
    cx: &Context<V>,
) -> impl IntoElement {
    let rows = properties
        .into_iter()
        .map(
            |(display_name, prop_name, property_type, value, numeric_input, color_picker, mesh_picker)| {
                let prop_bool = prop_name.clone();
                let on_bool_toggle = on_bool_toggle.clone();
                let bool_callback = Arc::new(move |checked: bool, window: &mut Window, cx: &mut App| {
                    (on_bool_toggle)(&prop_bool, checked, window, cx);
                });

                let prop_enum = prop_name.clone();
                let on_enum_select = on_enum_select.clone();
                let enum_callback = Arc::new(move |ix: usize, window: &mut Window, cx: &mut App| {
                    (on_enum_select)(&prop_enum, ix, window, cx);
                });

                render_property_row(
                    id_prefix,
                    class_name,
                    &display_name,
                    &prop_name,
                    &property_type,
                    &value,
                    numeric_input,
                    color_picker,
                    mesh_picker,
                    bool_callback,
                    enum_callback,
                    cx,
                )
            },
        )
        .collect::<Vec<_>>();

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
        .children(rows)
}
