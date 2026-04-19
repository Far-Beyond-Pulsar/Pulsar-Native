use engine_state::{DropdownOption, FieldType, SettingInfo, ConfigValue};
use gpui::*;
use gpui::prelude::FluentBuilder as _;
use ui::{
    h_flex, v_flex, ActiveTheme as _, Icon, IconName,
    button::{Button, ButtonVariants as _},
    switch::Switch,
    input::{InputState, TextInput},
    menu::popup_menu::PopupMenuExt,
};

/// Render a setting field based on its definition and current value
pub fn render_setting_field<F>(
    definition: &SettingInfo,
    current_value: &ConfigValue,
    on_change: F,
    _window: &mut Window,
    cx: &mut App,
) -> impl IntoElement
where
    F: Fn(ConfigValue) + 'static + Clone,
{
    let theme = cx.theme();
    let key = definition.key.clone();

    match definition.field_type.as_ref() {
        Some(FieldType::Checkbox) => {
            let checked = current_value.as_bool().unwrap_or(false);
            let switch_id = ElementId::Name(key.into());
            let on_change = on_change.clone();

            Switch::new(switch_id)
                .checked(checked)
                .on_click(move |_, _, _| {
                    on_change(ConfigValue::Bool(!checked));
                })
                .into_any_element()
        }

        Some(FieldType::NumberInput { min, max, step: _ }) => {
            let value = current_value.as_float()
                .or_else(|_| current_value.as_int().map(|i| i as f64))
                .unwrap_or(0.0);
            let display_value = format!("{}", value);
            let min_opt = *min;
            let max_opt = *max;

            h_flex()
                .gap_2()
                .items_center()
                .child(
                    div()
                        .px_3()
                        .py_1p5()
                        .min_w(px(80.0))
                        .rounded_md()
                        .bg(theme.background)
                        .border_1()
                        .border_color(theme.border)
                        .child(
                            div()
                                .text_sm()
                                .font_family("monospace")
                                .text_color(theme.foreground)
                                .child(display_value)
                        )
                )
                .when(min_opt.is_some() || max_opt.is_some(), |this| {
                    this.child(
                        div()
                            .text_xs()
                            .text_color(theme.muted_foreground)
                            .child(format!(
                                "({} - {})",
                                min_opt.map(|v| v.to_string()).unwrap_or_else(|| "∞".to_string()),
                                max_opt.map(|v| v.to_string()).unwrap_or_else(|| "∞".to_string())
                            ))
                    )
                })
                .into_any_element()
        }

        Some(FieldType::TextInput { placeholder, multiline }) => {
            let value = current_value.as_str().unwrap_or("").to_string();
            let is_empty = value.is_empty();

            div()
                .px_3()
                .py_1p5()
                .min_w(px(200.0))
                .rounded_md()
                .bg(theme.background)
                .border_1()
                .border_color(theme.border)
                .when(*multiline, |this| this.h(px(100.0)))
                .child(
                    div()
                        .text_sm()
                        .text_color(theme.foreground)
                        .when(is_empty && placeholder.is_some(), |this| {
                            this.text_color(theme.muted_foreground)
                                .child(placeholder.as_ref().unwrap().clone())
                        })
                        .when(!is_empty, |this| this.child(value))
                )
                .into_any_element()
        }

        Some(FieldType::Dropdown { options: _ }) => {
            let current_str = current_value.as_str().unwrap_or("").to_string();

            div()
                .px_3()
                .py_1p5()
                .min_w(px(150.0))
                .rounded_md()
                .bg(theme.background)
                .border_1()
                .border_color(theme.border)
                .child(
                    h_flex()
                        .gap_2()
                        .items_center()
                        .child(
                            div()
                                .text_sm()
                                .text_color(theme.foreground)
                                .child(current_str)
                        )
                        .child(
                            Icon::new(IconName::ChevronDown)
                                .size(px(16.0))
                                .text_color(theme.muted_foreground)
                        )
                )
                .into_any_element()
        }

        Some(FieldType::Slider { min, max, step: _ }) => {
            let value = current_value.as_float()
                .or_else(|_| current_value.as_int().map(|i| i as f64))
                .unwrap_or(*min);
            let percentage = ((value - min) / (max - min) * 100.0).clamp(0.0, 100.0);

            v_flex()
                .gap_2()
                .min_w(px(200.0))
                .child(
                    div()
                        .w_full()
                        .h(px(6.0))
                        .rounded_full()
                        .bg(theme.secondary)
                        .relative()
                        .child(
                            div()
                                .absolute()
                                .left_0()
                                .top_0()
                                .h_full()
                                .w(relative((percentage / 100.0) as f32))
                                .rounded_full()
                                .bg(theme.primary)
                        )
                        .child(
                            div()
                                .absolute()
                                .left(relative((percentage / 100.0) as f32))
                                .top(px(-3.0))
                                .w(px(12.0))
                                .h(px(12.0))
                                .rounded_full()
                                .bg(theme.primary)
                                .border_2()
                                .border_color(theme.background)
                                .shadow_md()
                        )
                )
                .child(
                    h_flex()
                        .justify_between()
                        .child(
                            div()
                                .text_xs()
                                .text_color(theme.muted_foreground)
                                .child(format!("{}", min))
                        )
                        .child(
                            div()
                                .text_xs()
                                .font_weight(FontWeight::MEDIUM)
                                .text_color(theme.foreground)
                                .child(format!("{:.1}", value))
                        )
                        .child(
                            div()
                                .text_xs()
                                .text_color(theme.muted_foreground)
                                .child(format!("{}", max))
                        )
                )
                .into_any_element()
        }

        Some(FieldType::ColorPicker) => {
            let color_str = current_value.as_str().unwrap_or("#000000").to_string();

            h_flex()
                .gap_3()
                .items_center()
                .child(
                    div()
                        .w(px(40.0))
                        .h(px(40.0))
                        .rounded_md()
                        .border_2()
                        .border_color(theme.border)
                        .bg(gpui::rgb(0x000000))
                )
                .child(
                    div()
                        .px_3()
                        .py_1p5()
                        .rounded_md()
                        .bg(theme.background)
                        .border_1()
                        .border_color(theme.border)
                        .child(
                            div()
                                .text_sm()
                                .font_family("monospace")
                                .text_color(theme.foreground)
                                .child(color_str)
                        )
                )
                .into_any_element()
        }

        Some(FieldType::PathSelector { directory }) => {
            let path = current_value.as_str().unwrap_or("").to_string();
            let empty_path = path.is_empty();
            let is_dir = *directory;

            h_flex()
                .gap_2()
                .child(
                    div()
                        .flex_1()
                        .px_3()
                        .py_1p5()
                        .rounded_md()
                        .bg(theme.background)
                        .border_1()
                        .border_color(theme.border)
                        .child(
                            div()
                                .text_sm()
                                .text_color(if empty_path {
                                    theme.muted_foreground
                                } else {
                                    theme.foreground
                                })
                                .child(if empty_path {
                                    if is_dir {
                                        "No directory selected".to_string()
                                    } else {
                                        "No file selected".to_string()
                                    }
                                } else {
                                    path
                                })
                        )
                )
                .child(
                    Button::new("browse")
                        .ghost()
                        .icon(IconName::Folder)
                        .label("Browse")
                )
                .into_any_element()
        }

        None => div().into_any_element(),
    }
}

/// Render a setting row with label, description, and field
pub fn render_setting_row<F>(
    definition: &SettingInfo,
    current_value: &ConfigValue,
    on_change: F,
    window: &mut Window,
    cx: &mut App,
) -> impl IntoElement
where
    F: Fn(ConfigValue) + 'static + Clone,
{
    let theme = cx.theme();

    h_flex()
        .w_full()
        .items_center()
        .justify_between()
        .gap_4()
        .p_4()
        .border_b_1()
        .border_color(theme.border)
        .hover(|style| style.bg(theme.secondary.opacity(0.3)))
        .child(
            v_flex()
                .flex_1()
                .gap_1()
                .child(
                    div()
                        .text_sm()
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(theme.foreground)
                        .child(definition.label.clone().unwrap_or_else(|| definition.key.clone()))
                )
                .when(!definition.description.is_empty(), |this| {
                    this.child(
                        div()
                            .text_xs()
                            .text_color(theme.muted_foreground)
                            .child(definition.description.clone())
                    )
                })
        )
        .child(
            div()
                .flex_shrink_0()
                .child(render_setting_field(definition, current_value, on_change, window, cx))
        )
}
