//! Property input widgets for different types
//!
//! This module provides input widgets that automatically render based on
//! PropertyType metadata from the reflection system.

use gpui::{prelude::*, App, *};
use ui::{
    button::{Button, ButtonVariants as _},
    h_flex,
    input::{InputState, TextInput},
    label::Label,
    v_flex, ActiveTheme, Disableable, Icon, IconName, Sizable, StyledExt,
};

/// Render a label for a property
pub fn render_property_label(name: &str, cx: &App) -> impl IntoElement {
    let name = name.to_string();
    div()
        .w(px(120.0))
        .flex_shrink_0()
        .child(Label::new(name).text_color(cx.theme().muted_foreground))
}

/// Render an F32 input with optional constraints
pub fn render_f32_input(
    value: f32,
    min: Option<f32>,
    max: Option<f32>,
    step: Option<f32>,
    _on_change: impl Fn(f32) + 'static,
    cx: &App,
) -> impl IntoElement {
    let value_str = format!("{:.2}", value);
    let _step_size = step.unwrap_or(0.1);

    h_flex()
        .gap_1()
        .items_center()
        .flex_1()
        // Text input field
        .child(
            div()
                .flex_1()
                .h_8()
                .px_2()
                .bg(cx.theme().background)
                .border_1()
                .border_color(cx.theme().border)
                .rounded_md()
                .child(Label::new(value_str).text_color(cx.theme().foreground)),
        )
        // Decrement button
        .child(
            Button::new("dec")
                .icon(IconName::Minus)
                .xsmall()
                .ghost()
                .when_some(min, |button, min_val| button.disabled(value <= min_val)),
        )
        // Increment button
        .child(
            Button::new("inc")
                .icon(IconName::Plus)
                .xsmall()
                .ghost()
                .when_some(max, |button, max_val| button.disabled(value >= max_val)),
        )
        .when_some(min, |this, min_val| {
            this.child(
                Label::new(format!("Min: {}", min_val))
                    .text_xs()
                    .text_color(cx.theme().muted_foreground),
            )
        })
        .when_some(max, |this, max_val| {
            this.child(
                Label::new(format!("Max: {}", max_val))
                    .text_xs()
                    .text_color(cx.theme().muted_foreground),
            )
        })
}

/// Render an I32 input with optional constraints
pub fn render_i32_input(
    value: i32,
    min: Option<i32>,
    max: Option<i32>,
    _on_change: impl Fn(i32) + 'static,
    cx: &App,
) -> impl IntoElement {
    let value_str = value.to_string();

    h_flex()
        .gap_1()
        .items_center()
        .flex_1()
        .child(
            div()
                .flex_1()
                .h_8()
                .px_2()
                .bg(cx.theme().background)
                .border_1()
                .border_color(cx.theme().border)
                .rounded_md()
                .child(Label::new(value_str).text_color(cx.theme().foreground)),
        )
        .child(
            Button::new("dec")
                .icon(IconName::Minus)
                .xsmall()
                .ghost()
                .when_some(min, |button, min_val| button.disabled(value <= min_val)),
        )
        .child(
            Button::new("inc")
                .icon(IconName::Plus)
                .xsmall()
                .ghost()
                .when_some(max, |button, max_val| button.disabled(value >= max_val)),
        )
}

/// Render a boolean checkbox input
pub fn render_bool_input(
    value: bool,
    _on_change: impl Fn(bool) + 'static,
    cx: &App,
) -> impl IntoElement {
    h_flex()
        .gap_2()
        .items_center()
        .flex_1()
        .child(
            div()
                .w_5()
                .h_5()
                .bg(if value {
                    cx.theme().accent
                } else {
                    cx.theme().background
                })
                .border_1()
                .border_color(cx.theme().border)
                .rounded_sm()
                .when(value, |this| {
                    this.child(
                        Icon::new(IconName::Check)
                            .xsmall()
                            .text_color(cx.theme().accent_foreground),
                    )
                }),
        )
        .child(
            Label::new(if value { "Enabled" } else { "Disabled" })
                .text_color(cx.theme().foreground),
        )
}

/// Render a string input
pub fn render_string_input(
    value: &str,
    _max_length: Option<usize>,
    _on_change: impl Fn(String) + 'static,
    cx: &App,
) -> impl IntoElement {
    div()
        .flex_1()
        .h_8()
        .px_2()
        .bg(cx.theme().background)
        .border_1()
        .border_color(cx.theme().border)
        .rounded_md()
        .child(Label::new(value.to_string()).text_color(cx.theme().foreground))
}

/// Render a Vec3 input (3D vector)
pub fn render_vec3_input(
    value: [f32; 3],
    _on_change: impl Fn([f32; 3]) + 'static,
    cx: &App,
) -> impl IntoElement {
    h_flex()
        .gap_1()
        .flex_1()
        .child(
            v_flex()
                .gap_1()
                .flex_1()
                .child(
                    Label::new("X")
                        .text_xs()
                        .text_color(cx.theme().muted_foreground),
                )
                .child(
                    div()
                        .h_8()
                        .px_2()
                        .bg(cx.theme().background)
                        .border_1()
                        .border_color(cx.theme().border)
                        .rounded_md()
                        .child(
                            Label::new(format!("{:.2}", value[0]))
                                .text_color(cx.theme().foreground),
                        ),
                ),
        )
        .child(
            v_flex()
                .gap_1()
                .flex_1()
                .child(
                    Label::new("Y")
                        .text_xs()
                        .text_color(cx.theme().muted_foreground),
                )
                .child(
                    div()
                        .h_8()
                        .px_2()
                        .bg(cx.theme().background)
                        .border_1()
                        .border_color(cx.theme().border)
                        .rounded_md()
                        .child(
                            Label::new(format!("{:.2}", value[1]))
                                .text_color(cx.theme().foreground),
                        ),
                ),
        )
        .child(
            v_flex()
                .gap_1()
                .flex_1()
                .child(
                    Label::new("Z")
                        .text_xs()
                        .text_color(cx.theme().muted_foreground),
                )
                .child(
                    div()
                        .h_8()
                        .px_2()
                        .bg(cx.theme().background)
                        .border_1()
                        .border_color(cx.theme().border)
                        .rounded_md()
                        .child(
                            Label::new(format!("{:.2}", value[2]))
                                .text_color(cx.theme().foreground),
                        ),
                ),
        )
}

/// Render a Color input (RGBA)
pub fn render_color_input(
    value: [f32; 4],
    _on_change: impl Fn([f32; 4]) + 'static,
    cx: &App,
) -> impl IntoElement {
    let rgb = Rgba {
        r: value[0],
        g: value[1],
        b: value[2],
        a: value[3],
    };

    h_flex()
        .gap_2()
        .items_center()
        .flex_1()
        // Color preview swatch
        .child(
            div()
                .w_10()
                .h_8()
                .bg(rgb)
                .border_1()
                .border_color(cx.theme().border)
                .rounded_md(),
        )
        // RGBA values
        .child(
            Label::new(format!(
                "R:{:.0} G:{:.0} B:{:.0} A:{:.2}",
                value[0] * 255.0,
                value[1] * 255.0,
                value[2] * 255.0,
                value[3]
            ))
            .text_xs()
            .text_color(cx.theme().muted_foreground),
        )
}

/// Render a Vec<T> property with +/- buttons
pub fn render_vec_input(
    items: &[String],
    _element_type_name: &str,
    _on_add: impl Fn() + 'static,
    _on_remove: impl Fn(usize) + 'static,
    cx: &App,
) -> impl IntoElement {
    v_flex()
        .gap_2()
        .w_full()
        // Header with count and add button
        .child(
            h_flex()
                .justify_between()
                .items_center()
                .child(
                    Label::new(format!("Array ({} items)", items.len()))
                        .text_color(cx.theme().foreground),
                )
                .child(
                    Button::new("add-item")
                        .icon(IconName::Plus)
                        .xsmall()
                        .ghost(),
                ),
        )
        // Array items
        .children(items.iter().enumerate().map(|(idx, item)| {
            h_flex()
                .gap_2()
                .items_center()
                .w_full()
                .child(
                    Label::new(format!("[{}]", idx))
                        .text_xs()
                        .text_color(cx.theme().muted_foreground)
                        .w(px(30.0)),
                )
                .child(
                    div()
                        .flex_1()
                        .h_8()
                        .px_2()
                        .bg(cx.theme().background)
                        .border_1()
                        .border_color(cx.theme().border)
                        .rounded_md()
                        .child(Label::new(item.clone()).text_color(cx.theme().foreground)),
                )
                .child(
                    Button::new(format!("remove-{}", idx))
                        .icon(IconName::Trash)
                        .xsmall()
                        .ghost(),
                )
        }))
}

/// Render an enum dropdown
pub fn render_enum_input(
    _variants: &[&'static str],
    selected_index: usize,
    selected_name: &str,
    _on_change: impl Fn(usize) + 'static,
    cx: &App,
) -> impl IntoElement {
    h_flex()
        .gap_2()
        .items_center()
        .flex_1()
        .child(
            div()
                .flex_1()
                .h_8()
                .px_2()
                .bg(cx.theme().background)
                .border_1()
                .border_color(cx.theme().border)
                .rounded_md()
                .child(Label::new(selected_name.to_string()).text_color(cx.theme().foreground)),
        )
        .child(
            Icon::new(IconName::ChevronDown)
                .xsmall()
                .text_color(cx.theme().muted_foreground),
        )
}
