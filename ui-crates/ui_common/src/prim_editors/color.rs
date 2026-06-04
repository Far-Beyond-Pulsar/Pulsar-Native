//! Property editor for `[f32; 4]` — RGBA colour picker.
//!
//! This module is deprecated. The actual editor registration now lives in
//! `pulsar_reflection/src/prims/core/color.rs` via the `ui-editors` feature.

use gpui::{prelude::*, Corner, *};
use ui::{color_picker::ColorPicker, h_flex, ActiveTheme};

use crate::property_editor_registry::PropertyEditorArgs;

pub fn render(args: &PropertyEditorArgs<'_>, cx: &App) -> AnyElement {
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
        .child(if let Some(state) = args.color_picker.clone() {
            ColorPicker::new(&state)
                .anchor(Corner::BottomRight)
                .into_any_element()
        } else {
            div()
                .text_sm()
                .text_color(cx.theme().foreground)
                .child(format!("{:?}", args.current_json))
                .into_any_element()
        })
        .into_any_element()
}
