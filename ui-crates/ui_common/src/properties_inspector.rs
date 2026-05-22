use gpui::{
    prelude::FluentBuilder as _, px, AnyElement, App, Context, Corner, Entity, FontWeight,
    IntoElement, ParentElement, Styled, Window,
};
use pulsar_reflection::{PropertyType, PropertyValue};
use std::sync::Arc;

use ui::{
    button::{Button, ButtonVariants as _},
    color_picker::ColorPicker,
    h_flex,
    input::{InputState, NumberInput, TextInput},
    menu::PopupMenuItem,
    switch::Switch,
    v_flex, ActiveTheme, Icon, IconName, Sizable,
};

pub fn render_header<V>(
    title: impl Into<String>,
    has_selection: bool,
    selected_badge_label: impl Into<String>,
    menu_button_id: impl Into<String>,
    cx: &Context<V>,
) -> impl IntoElement {
    let title = title.into();
    let selected_badge_label = selected_badge_label.into();
    let menu_button_id = menu_button_id.into();

    h_flex()
        .w_full()
        .px_4()
        .py_3()
        .justify_between()
        .items_center()
        .bg(cx.theme().sidebar)
        .border_b_1()
        .border_color(cx.theme().border)
        .child(
            h_flex()
                .gap_3()
                .items_center()
                .child(
                    gpui::div()
                        .text_base()
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(cx.theme().foreground)
                        .child(title),
                )
                .when(has_selection, |this| {
                    this.child(
                        gpui::div()
                            .px_2()
                            .py(px(2.0))
                            .rounded(px(4.0))
                            .bg(cx.theme().accent.opacity(0.15))
                            .text_xs()
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(cx.theme().accent)
                            .child(selected_badge_label),
                    )
                }),
        )
        .child(
            h_flex().gap_1().child(
                Button::new(menu_button_id)
                    .icon(IconName::Ellipsis)
                    .xsmall(),
            ),
        )
}

pub fn render_empty_state<V>(
    icon: IconName,
    title: impl Into<String>,
    description: impl Into<String>,
    cx: &Context<V>,
) -> impl IntoElement {
    let title = title.into();
    let description = description.into();

    gpui::div()
        .size_full()
        .flex()
        .items_center()
        .justify_center()
        .p_8()
        .child(
            v_flex()
                .gap_3()
                .items_center()
                .child(
                    Icon::new(icon)
                        .size(px(48.0))
                        .text_color(cx.theme().muted_foreground.opacity(0.5)),
                )
                .child(
                    gpui::div()
                        .text_base()
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(cx.theme().muted_foreground)
                        .child(title),
                )
                .child(
                    gpui::div()
                        .text_sm()
                        .text_color(cx.theme().muted_foreground.opacity(0.7))
                        .text_center()
                        .child(description),
                ),
        )
}

pub fn render_text_input_property_row<V>(
    name: String,
    kind: String,
    input: Entity<InputState>,
    cx: &Context<V>,
) -> impl IntoElement {
    h_flex()
        .w_full()
        .items_start()
        .gap_2()
        .child(
            v_flex()
                .w(px(140.0))
                .gap_0p5()
                .child(
                    gpui::div()
                        .text_xs()
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(cx.theme().foreground)
                        .child(name),
                )
                .child(
                    gpui::div()
                        .text_xs()
                        .text_color(cx.theme().muted_foreground)
                        .child(kind),
                ),
        )
        .child(gpui::div().flex_1().child(TextInput::new(&input)))
}

fn is_color_field_name(prop_name: &str) -> bool {
    prop_name == "color" || prop_name == "base_color"
}

pub fn render_reflected_property_row<V>(
    id_prefix: &str,
    class_name: &str,
    display_name: &str,
    prop_name: &str,
    property_type: &PropertyType,
    value: &PropertyValue,
    numeric_input: Option<Entity<InputState>>,
    color_picker: Option<Entity<ui::color_picker::ColorPickerState>>,
    on_bool_toggle: Arc<dyn Fn(bool, &mut Window, &mut App)>,
    on_enum_select: Arc<dyn Fn(usize, &mut Window, &mut App)>,
    cx: &Context<V>,
) -> AnyElement {
    match (property_type, value) {
        (PropertyType::F32 { .. }, PropertyValue::F32(v)) => h_flex()
            .w_full()
            .justify_between()
            .items_center()
            .gap_2()
            .child(
                gpui::div()
                    .text_sm()
                    .text_color(cx.theme().muted_foreground)
                    .child(display_name.to_string()),
            )
            .child(
                h_flex().items_center().gap_2().child(if let Some(input) = numeric_input {
                    NumberInput::new(&input).xsmall().w(px(92.0)).into_any_element()
                } else {
                    gpui::div()
                        .text_sm()
                        .text_color(cx.theme().foreground)
                        .child(format!("{:.3}", v))
                        .into_any_element()
                }),
            )
            .into_any_element(),
        (PropertyType::I32 { .. }, PropertyValue::I32(v)) => h_flex()
            .w_full()
            .justify_between()
            .items_center()
            .gap_2()
            .child(
                gpui::div()
                    .text_sm()
                    .text_color(cx.theme().muted_foreground)
                    .child(display_name.to_string()),
            )
            .child(
                h_flex().items_center().gap_2().child(if let Some(input) = numeric_input {
                    NumberInput::new(&input).xsmall().w(px(92.0)).into_any_element()
                } else {
                    gpui::div()
                        .text_sm()
                        .text_color(cx.theme().foreground)
                        .child(v.to_string())
                        .into_any_element()
                }),
            )
            .into_any_element(),
        (PropertyType::Bool, PropertyValue::Bool(v)) => {
            let on_bool_toggle = on_bool_toggle.clone();
            h_flex()
                .w_full()
                .justify_between()
                .items_center()
                .gap_2()
                .child(
                    gpui::div()
                        .text_sm()
                        .text_color(cx.theme().muted_foreground)
                        .child(display_name.to_string()),
                )
                .child(
                    Switch::new(format!("toggle-{id_prefix}-{class_name}-{prop_name}"))
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
            let options = variants
                .iter()
                .map(|v| (*v).to_string())
                .collect::<Vec<_>>();

            h_flex()
                .w_full()
                .justify_between()
                .items_center()
                .gap_2()
                .child(
                    gpui::div()
                        .text_sm()
                        .text_color(cx.theme().muted_foreground)
                        .child(display_name.to_string()),
                )
                .child(
                    Button::new(format!("enum-{id_prefix}-{class_name}-{prop_name}"))
                        .label(label)
                        .xsmall()
                        .ghost()
                        .dropdown_caret(true)
                        .dropdown_menu_with_anchor(Corner::BottomRight, move |menu, _window, _cx| {
                            let mut menu = menu;
                            for (ix, option) in options.iter().enumerate() {
                                let on_enum_select = on_enum_select.clone();
                                menu = menu.item(
                                    PopupMenuItem::new(option.clone())
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
                        gpui::div()
                            .text_sm()
                            .text_color(cx.theme().muted_foreground)
                            .child(display_name.to_string()),
                    )
                    .child(ColorPicker::new(&picker_state).anchor(Corner::BottomRight))
                    .into_any_element()
            } else {
                h_flex()
                    .w_full()
                    .justify_between()
                    .items_center()
                    .gap_2()
                    .child(
                        gpui::div()
                            .text_sm()
                            .text_color(cx.theme().muted_foreground)
                            .child(display_name.to_string()),
                    )
                    .child(
                        gpui::div()
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
                gpui::div()
                    .text_sm()
                    .text_color(cx.theme().muted_foreground)
                    .child(display_name.to_string()),
            )
            .child(
                gpui::div()
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
                        gpui::div()
                            .text_sm()
                            .text_color(cx.theme().muted_foreground)
                            .child(display_name.to_string()),
                    )
                    .child(ColorPicker::new(&picker_state).anchor(Corner::BottomRight))
                    .into_any_element()
            } else {
                gpui::div().text_sm().child(format!("{:?}", value)).into_any_element()
            }
        }
        _ => h_flex()
            .w_full()
            .justify_between()
            .items_center()
            .gap_2()
            .child(
                gpui::div()
                    .text_sm()
                    .text_color(cx.theme().muted_foreground)
                    .child(display_name.to_string()),
            )
            .child(
                gpui::div()
                    .text_sm()
                    .text_color(cx.theme().muted_foreground)
                    .child(format!("{:?}", value)),
            )
            .into_any_element(),
    }
}