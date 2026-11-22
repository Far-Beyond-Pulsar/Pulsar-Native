//! The control of one setting, by its field type: a switch, a dropdown, a
//! text input, a number input, a color picker.
//!
//! Text, number and color controls keep an editor entity; the screen holds
//! it per setting (`SettingsScreen::fields`, keyed by namespace, owner and
//! key), so a search or a scroll never hands one setting's text box to
//! another.

use gpui::{
    px, AnyElement, AppContext as _, Context, Corner, Entity,
    Hsla, IntoElement, SharedString, Styled, Subscription, Window,
};
use ui::{
    button::Button,
    color_picker::{ColorPicker, ColorPickerEvent, ColorPickerState},
    input::{InputEvent, InputState, NumberInput, NumberInputEvent, StepAction, TextInput},
    menu::PopupMenuItem,
    switch::Switch,
    Sizable as _,
};

use engine_state::{Color, ConfigValue, FieldType, SettingInfo};

use super::SettingsScreen;
use crate::utils::model::SettingId;
use crate::utils::store;

/// The editor entity of a text, number or color control.
pub enum FieldState {
    Text(Entity<InputState>, Vec<Subscription>),
    Number(Entity<InputState>, Vec<Subscription>),
    Color(Entity<ColorPickerState>, Vec<Subscription>),
}

/// The text a value shows in a text or dropdown control.
fn value_text(value: &ConfigValue) -> String {
    match value {
        ConfigValue::String(s) => s.clone(),
        ConfigValue::Int(i) => i.to_string(),
        ConfigValue::Float(f) => f.to_string(),
        ConfigValue::Bool(b) => b.to_string(),
        ConfigValue::Color(c) => format!("#{:02x}{:02x}{:02x}{:02x}", c.r, c.g, c.b, c.a),
        ConfigValue::Array(items) => items.iter().map(value_text).collect::<Vec<_>>().join(", "),
    }
}

/// `text` as a value of the same type as `like` (a dropdown over integer
/// values stores integers; a list is comma separated).
fn parse_like(text: &str, like: &ConfigValue) -> Option<ConfigValue> {
    Some(match like {
        ConfigValue::String(_) => ConfigValue::String(text.to_string()),
        ConfigValue::Int(_) => ConfigValue::Int(text.trim().parse().ok()?),
        ConfigValue::Float(_) => ConfigValue::Float(text.trim().parse().ok()?),
        ConfigValue::Bool(_) => ConfigValue::Bool(text.trim().parse().ok()?),
        ConfigValue::Array(_) => ConfigValue::Array(
            text.split(',')
                .map(str::trim)
                .filter(|item| !item.is_empty())
                .map(|item| ConfigValue::String(item.to_string()))
                .collect(),
        ),
        ConfigValue::Color(_) => return None,
    })
}

fn to_hsla(color: &Color) -> Hsla {
    gpui::Rgba {
        r: f32::from(color.r) / 255.0,
        g: f32::from(color.g) / 255.0,
        b: f32::from(color.b) / 255.0,
        a: f32::from(color.a) / 255.0,
    }
    .into()
}

fn to_color(hsla: Hsla) -> Color {
    let rgba = hsla.to_rgb();
    let byte = |x: f32| (x.clamp(0.0, 1.0) * 255.0).round() as u8;
    Color {
        r: byte(rgba.r),
        g: byte(rgba.g),
        b: byte(rgba.b),
        a: byte(rgba.a),
    }
}

/// Write `value` and mark the window changed; log a refused value.
fn commit(
    this: &mut SettingsScreen,
    id: &SettingId,
    value: ConfigValue,
    restart: bool,
    cx: &mut Context<SettingsScreen>,
) -> bool {
    match store::write(id, value) {
        Ok(()) => {
            crate::handlers::mark_changed(this, restart);
            crate::handlers::applied(id, cx);
            cx.notify();
            true
        }
        Err(error) => {
            tracing::warn!("setting {}: {error}", id.key_string());
            false
        }
    }
}

impl SettingsScreen {
    pub(crate) fn render_field(
        &mut self,
        info: &SettingInfo,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let id = SettingId::of(info);
        let restart = info.requires_restart();
        let current = store::read(&id).unwrap_or_else(|| info.default_value.clone());
        let element_id = SharedString::from(format!("field-{}", id.key_string()));

        match info.field_type.clone() {
            Some(FieldType::Checkbox) => {
                let checked = current.as_bool().unwrap_or(false);
                Switch::new(element_id)
                    .checked(checked)
                    .on_click(cx.listener(move |this, checked: &bool, _, cx| {
                        commit(this, &id, ConfigValue::Bool(*checked), restart, cx);
                    }))
                    .into_any_element()
            }
            Some(FieldType::Dropdown { options }) => {
                let selected = value_text(&current);
                let label = options
                    .iter()
                    .find(|option| option.value == selected)
                    .map(|option| option.label.clone())
                    .unwrap_or_else(|| selected.clone());
                let like = info.default_value.clone();
                let view = cx.entity();
                Button::new(element_id)
                    .label(label)
                    .dropdown_caret(true)
                    .outline()
                    .small()
                    .min_w(px(160.0))
                    .dropdown_menu_with_anchor(Corner::TopRight, move |menu, _, _| {
                        options.iter().fold(menu, |menu, option| {
                            let (id, like, view) = (id.clone(), like.clone(), view.clone());
                            let value = option.value.clone();
                            menu.item(
                                PopupMenuItem::new(option.label.clone())
                                    .checked(option.value == selected)
                                    .on_click(move |_, _, cx| {
                                        let Some(value) = parse_like(&value, &like) else {
                                            return;
                                        };
                                        view.update(cx, |this, cx| {
                                            commit(this, &id, value, restart, cx);
                                        });
                                    }),
                            )
                        })
                    })
                    .into_any_element()
            }
            Some(FieldType::NumberInput { min, max, step }) => {
                let (min, max, step) =
                    (min.unwrap_or(f64::MIN), max.unwrap_or(f64::MAX), step.unwrap_or(1.0));
                let input = self.number_state(&id, &current, min, max, step, restart, window, cx);
                NumberInput::new(&input).small().w(px(160.0)).into_any_element()
            }
            Some(FieldType::Slider { min, max, step }) => {
                let input = self.number_state(&id, &current, min, max, step, restart, window, cx);
                NumberInput::new(&input).small().w(px(160.0)).into_any_element()
            }
            Some(FieldType::TextInput { placeholder, multiline }) => {
                let input = self.text_state(&id, &current, placeholder, multiline, restart, window, cx);
                TextInput::new(&input)
                    .small()
                    .w(px(if multiline { 360.0 } else { 260.0 }))
                    .into_any_element()
            }
            Some(FieldType::PathSelector { .. }) => {
                let input = self.text_state(&id, &current, None, false, restart, window, cx);
                TextInput::new(&input).small().w(px(260.0)).into_any_element()
            }
            Some(FieldType::ColorPicker) => {
                let state = self.color_state(&id, &current, restart, window, cx);
                ColorPicker::new(&state).small().into_any_element()
            }
            None => gpui::div().into_any_element(),
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn number_state(
        &mut self,
        id: &SettingId,
        current: &ConfigValue,
        min: f64,
        max: f64,
        step: f64,
        restart: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Entity<InputState> {
        if let Some(FieldState::Number(input, _)) = self.fields.get(id) {
            return input.clone();
        }
        let text = value_text(current);
        let input = cx.new(|cx| InputState::new(window, cx).default_value(text));
        let step_sub = cx.subscribe_in(&input, window, move |_, input, event: &NumberInputEvent, window, cx| {
            let NumberInputEvent::Step { action, fine } = event;
            input.update(cx, |input, cx| {
                let Ok(value) = input.value().parse::<f64>() else {
                    return;
                };
                let step = if *fine { step * 0.1 } else { step };
                let next = match action {
                    StepAction::Increment => value + step,
                    StepAction::Decrement => value - step,
                };
                input.set_value(next.clamp(min, max).to_string(), window, cx);
            });
        });
        let like = current.clone();
        let setting = id.clone();
        let change_sub = cx.subscribe_in(&input, window, move |this, input, event: &InputEvent, window, cx| {
            if !matches!(event, InputEvent::Change) {
                return;
            }
            let raw = input.read(cx).value().to_string();
            let Ok(typed) = raw.trim().parse::<f64>() else {
                return; // mid-edit ("-", "1.", ""): wait for a number
            };
            let shown = store::read(&setting).map(|v| value_text(&v));
            if shown.as_deref() == Some(raw.trim()) {
                return;
            }
            let clamped = typed.clamp(min, max);
            let written = store::number_value(clamped, &like)
                .map_err(str::to_owned)
                .is_ok_and(|value| commit(this, &setting, value, restart, cx));
            // Show what the setting holds now when the entry was clamped or refused.
            if let Some(actual) = store::read(&setting) {
                let actual_text = value_text(&actual);
                if !written || clamped != typed {
                    input.update(cx, |input, cx| input.set_value(actual_text, window, cx));
                }
            }
        });
        self.fields
            .insert(id.clone(), FieldState::Number(input.clone(), vec![step_sub, change_sub]));
        input
    }

    #[allow(clippy::too_many_arguments)]
    fn text_state(
        &mut self,
        id: &SettingId,
        current: &ConfigValue,
        placeholder: Option<String>,
        multiline: bool,
        restart: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Entity<InputState> {
        if let Some(FieldState::Text(input, _)) = self.fields.get(id) {
            return input.clone();
        }
        let text = value_text(current);
        let input = cx.new(|cx| {
            let mut state = InputState::new(window, cx);
            if multiline {
                state = state.multi_line().auto_grow(3, 12);
            }
            let state = state.default_value(text);
            match placeholder {
                Some(placeholder) => state.placeholder(placeholder),
                None => state,
            }
        });
        let like = current.clone();
        let setting = id.clone();
        let sub = cx.subscribe(&input, move |this, input, event: &InputEvent, cx| {
            if !matches!(event, InputEvent::Change) {
                return;
            }
            let raw = input.read(cx).value().to_string();
            if store::read(&setting).map(|v| value_text(&v)).as_deref() == Some(raw.as_str()) {
                return;
            }
            if let Some(value) = parse_like(&raw, &like) {
                commit(this, &setting, value, restart, cx);
            }
        });
        self.fields.insert(id.clone(), FieldState::Text(input.clone(), vec![sub]));
        input
    }

    fn color_state(
        &mut self,
        id: &SettingId,
        current: &ConfigValue,
        restart: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Entity<ColorPickerState> {
        if let Some(FieldState::Color(state, _)) = self.fields.get(id) {
            return state.clone();
        }
        let initial = match current {
            ConfigValue::Color(color) => to_hsla(color),
            _ => gpui::black(),
        };
        let state = cx.new(|cx| ColorPickerState::new(window, cx).default_value(initial));
        let setting = id.clone();
        let sub = cx.subscribe(&state, move |this, _, event: &ColorPickerEvent, cx| {
            let ColorPickerEvent::Change(Some(hsla)) = event else {
                return;
            };
            commit(this, &setting, ConfigValue::Color(to_color(*hsla)), restart, cx);
        });
        self.fields.insert(id.clone(), FieldState::Color(state.clone(), vec![sub]));
        state
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_dropdown_over_integers_stores_integers() {
        assert_eq!(parse_like("8", &ConfigValue::Int(6)), Some(ConfigValue::Int(8)));
        assert_eq!(value_text(&ConfigValue::Int(6)), "6");
        assert_eq!(parse_like("x", &ConfigValue::Int(6)), None);
    }

    #[test]
    fn a_list_is_edited_as_comma_separated_text() {
        let list = parse_like("a, b,,c", &ConfigValue::Array(vec![])).unwrap();
        assert_eq!(value_text(&list), "a, b, c");
    }

    #[test]
    fn colors_round_trip_through_the_picker() {
        let color = Color { r: 10, g: 200, b: 30, a: 255 };
        assert_eq!(to_color(to_hsla(&color)), color);
    }
}
