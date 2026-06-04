//! Property editor for `f32` — numeric step-input.
//!
//! This module is deprecated. The actual editor registration now lives in
//! `pulsar_reflection/src/prims/core/f32.rs` via the `ui-editors` feature.

use gpui::{prelude::*, *};
use ui::{h_flex, input::NumberInput, ActiveTheme, Sizable};

use crate::property_editor_registry::PropertyEditorArgs;

pub fn render(args: &PropertyEditorArgs<'_>, cx: &App) -> AnyElement {
    let value = args.current_json.as_f64().unwrap_or(0.0) as f32;
    h_flex()
        .w_full()
        .justify_between()
        .items_center()
        .gap_2()
        .child(
            div()
                .text_sm()
                .text_color(cx.theme().muted_foreground)
                .child(args.display_name.to_string()),
        )
        .child(h_flex().items_center().gap_2().child(
            if let Some(input) = args.numeric_input.clone() {
                NumberInput::new(&input)
                    .xsmall()
                    .w(px(92.0))
                    .into_any_element()
            } else {
                div()
                    .text_sm()
                    .text_color(cx.theme().foreground)
                    .child(format!("{:.3}", value))
                    .into_any_element()
            },
        ))
        .into_any_element()
}
