//! One editor for any property value, whatever its type: used for node
//! properties, slot properties, parameter defaults and the values an instance
//! passes to the widget it uses.

use std::rc::Rc;

use gpui::*;
use serde_json::{json, Value as Json};
use ui::button::{Button, ButtonVariants};
use ui::color_picker::ColorPicker;
use ui::input::{NumberInput, TextInput};
use ui::menu::DropdownMenu;
use ui::switch::Switch;
use ui::{h_flex, ActiveTheme, Disableable, Sizable, Size};

use crate::model::geom::{json_array2, json_array4, num, Color};
use crate::model::schema::{PropType, ValueClass};

use super::fields::{Fields, NumberSpec};

/// How to edit a parameter of `class`.
pub fn type_for_class(class: ValueClass) -> PropType {
    match class {
        ValueClass::Bool => PropType::Bool,
        ValueClass::Float => PropType::Float { min: -1_000_000.0, max: 1_000_000.0, step: 1.0 },
        ValueClass::Int => PropType::Int { min: -1_000_000, max: 1_000_000 },
        ValueClass::Text | ValueClass::Choice => PropType::Text { multiline: false },
        ValueClass::Color => PropType::Color,
        ValueClass::Vec2 => PropType::Vec2,
        ValueClass::Quad => PropType::Insets,
        ValueClass::Image => PropType::Image,
    }
}

fn spec_for(ty: &PropType) -> NumberSpec {
    match *ty {
        PropType::Float { min, max, step } => NumberSpec { step, min, max, decimals: if step < 1.0 { 2 } else { 1 } },
        PropType::Int { min, max } => NumberSpec { step: 1.0, min: min as f32, max: max as f32, decimals: 0 },
        // Vectors and margins: any size, whole-ish steps.
        _ => NumberSpec { step: 1.0, min: -100_000.0, max: 100_000.0, decimals: 1 },
    }
}

fn component_labels(ty: &PropType) -> &'static [&'static str] {
    match ty {
        PropType::Vec2 => &["X", "Y"],
        PropType::Insets => &["L", "T", "R", "B"],
        PropType::Corners => &["TL", "TR", "BR", "BL"],
        PropType::Offsets => &["1", "2", "3", "4"],
        _ => &[],
    }
}

/// Build the editor for a value of type `ty`.
///
/// `key` names the fields it creates and must be stable. `current` is the
/// value now; `get` reads it again when a field fires, and `set` stores a
/// new one. `disabled` shows the value without letting it be changed.
#[allow(clippy::too_many_arguments)]
pub fn value_editor<V: 'static>(
    fields: &mut Fields<V>,
    key: &str,
    ty: PropType,
    current: &Json,
    disabled: bool,
    get: impl Fn(&V, &App) -> Option<Json> + Clone + 'static,
    set: impl Fn(&mut V, Json, &mut Context<V>) + Clone + 'static,
    window: &mut Window,
    cx: &mut Context<V>,
) -> AnyElement {
    let theme = cx.theme().clone();
    match ty {
        PropType::Bool => {
            let set = set.clone();
            Switch::new(SharedString::from(format!("{key}-switch")))
                .checked(current.as_bool().unwrap_or(false))
                .disabled(disabled)
                .on_click(cx.listener(move |this, checked: &bool, _, cx| set(this, json!(*checked), cx)))
                .into_any_element()
        }
        PropType::Float { .. } | PropType::Int { .. } => {
            let spec = spec_for(&ty);
            let is_int = matches!(ty, PropType::Int { .. });
            let (g, s) = (get.clone(), set.clone());
            let input = fields.number(
                key.to_string(),
                spec,
                current.as_f64().map(|v| v as f32),
                move |this, cx| g(this, cx).and_then(|j| j.as_f64()).map(|v| v as f32),
                move |this, v, cx| s(this, if is_int { json!(v.round() as i64) } else { num(v) }, cx),
                window,
                cx,
            );
            div().flex_1().min_w_0().child(NumberInput::new(&input).small().disabled(disabled)).into_any_element()
        }
        PropType::Text { multiline } => {
            let (g, s) = (get.clone(), set.clone());
            let input = fields.text(
                key.to_string(),
                multiline,
                "",
                current.as_str().unwrap_or(""),
                move |this, cx| g(this, cx).and_then(|j| j.as_str().map(str::to_string)),
                move |this, text, cx| s(this, json!(text), cx),
                window,
                cx,
            );
            div().flex_1().min_w_0().child(TextInput::new(&input).small().disabled(disabled)).into_any_element()
        }
        PropType::Image => {
            let (g, s) = (get.clone(), set.clone());
            let input = fields.text(
                key.to_string(),
                false,
                "Project path, or drop an image here",
                current.as_str().unwrap_or(""),
                move |this, cx| g(this, cx).and_then(|j| j.as_str().map(str::to_string)),
                move |this, text, cx| s(this, json!(text.replace('\\', "/")), cx),
                window,
                cx,
            );
            let s = set.clone();
            div()
                .flex_1()
                .min_w_0()
                .child(TextInput::new(&input).small().disabled(disabled))
                .on_drop(cx.listener(move |this, drag: &plugin_editor_api::AssetPayload, _, cx| {
                    s(this, json!(drag.engine_path.replace('\\', "/")), cx);
                }))
                .into_any_element()
        }
        PropType::Color => {
            let color = current.as_str().and_then(Color::from_hex).unwrap_or(Color::TRANSPARENT);
            let (g, s) = (get.clone(), set.clone());
            let picker = fields.color(
                format!("{key}-picker"),
                color,
                move |this, cx| g(this, cx).and_then(|j| j.as_str().and_then(Color::from_hex)),
                move |this, c, cx| s(this, json!(c.to_hex()), cx),
                window,
                cx,
            );
            let (g, s) = (get.clone(), set.clone());
            let hex = fields.text(
                format!("{key}-hex"),
                false,
                "#rrggbb",
                &color.to_hex(),
                move |this, cx| g(this, cx).and_then(|j| j.as_str().map(str::to_string)),
                move |this, text, cx| {
                    if let Some(c) = Color::from_hex(&text) {
                        s(this, json!(c.to_hex()), cx);
                    }
                },
                window,
                cx,
            );
            h_flex()
                .flex_1()
                .min_w_0()
                .gap_1p5()
                .items_center()
                .child(div().flex_shrink_0().when_disabled(disabled).child(ColorPicker::new(&picker).size(Size::Small)))
                .child(div().flex_1().min_w_0().child(TextInput::new(&hex).small().disabled(disabled)))
                .into_any_element()
        }
        PropType::Vec2 | PropType::Insets | PropType::Corners | PropType::Offsets => {
            let labels = component_labels(&ty);
            let values: Vec<f32> = match labels.len() {
                2 => json_array2(current).map_or(vec![0.0; 2], |a| a.to_vec()),
                _ => json_array4(current).map_or(vec![0.0; 4], |a| a.to_vec()),
            };
            let spec = spec_for(&ty);
            let n = labels.len();
            let mut row = h_flex().flex_1().min_w_0().gap_1();
            // Four fields wrap to two rows so they stay wide enough to read.
            let mut rows = Vec::new();
            for (i, label) in labels.iter().enumerate() {
                let (g, s) = (get.clone(), set.clone());
                let input = fields.number(
                    format!("{key}:{i}"),
                    spec,
                    Some(values[i]),
                    {
                        let g = g.clone();
                        move |this, cx| g(this, cx).and_then(|j| component(&j, i, n))
                    },
                    move |this, v, cx| {
                        let mut parts = g(this, cx).and_then(|j| components(&j, n)).unwrap_or_else(|| vec![0.0; n]);
                        parts[i] = v;
                        s(this, array_json(&parts), cx);
                    },
                    window,
                    cx,
                );
                let cell = div()
                    .flex_1()
                    .min_w_0()
                    .child(NumberInput::new(&input).small().disabled(disabled).prefix(div().text_xs().text_color(theme.muted_foreground).child(*label)));
                if n == 4 && i == 2 {
                    rows.push(std::mem::replace(&mut row, h_flex().flex_1().min_w_0().gap_1()));
                }
                row = row.child(cell);
            }
            rows.push(row);
            gpui::div().flex_1().min_w_0().flex().flex_col().gap_1().children(rows).into_any_element()
        }
        PropType::Choice(options) => {
            let label = options.iter().find(|(v, _)| Some(*v) == current.as_str()).map_or("", |(_, l)| *l);
            let entity = cx.entity();
            let set = Rc::new(set);
            Button::new(SharedString::from(format!("{key}-choice")))
                .label(label)
                .small()
                .outline()
                .disabled(disabled)
                .w_full()
                .popup_menu(move |menu, _, _| {
                    let mut menu = menu;
                    for (value, label) in options {
                        let (entity, set, value) = (entity.clone(), set.clone(), *value);
                        menu = menu.menu_handler(*label, move |_, cx| {
                            entity.update(cx, |this, cx| set(this, json!(value), cx));
                        });
                    }
                    menu
                })
                .into_any_element()
        }
    }
}

fn components(value: &Json, n: usize) -> Option<Vec<f32>> {
    match n {
        2 => json_array2(value).map(|a| a.to_vec()),
        _ => json_array4(value).map(|a| a.to_vec()),
    }
}

fn component(value: &Json, i: usize, n: usize) -> Option<f32> {
    components(value, n).and_then(|c| c.get(i).copied())
}

fn array_json(parts: &[f32]) -> Json {
    Json::Array(parts.iter().map(|v| num(*v)).collect())
}

/// `when_disabled` dims and blocks a control that cannot be changed.
trait WhenDisabled: Sized {
    fn when_disabled(self, disabled: bool) -> Self;
}

impl WhenDisabled for Div {
    fn when_disabled(self, disabled: bool) -> Self {
        if disabled {
            self.opacity(0.5)
        } else {
            self
        }
    }
}
