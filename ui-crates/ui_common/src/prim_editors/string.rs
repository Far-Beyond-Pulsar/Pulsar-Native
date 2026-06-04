//! Property editor for `String` — read-only text display.
//!
//! This module is deprecated. The actual editor registration now lives in
//! `pulsar_reflection/src/prims/core/string.rs` via the `ui-editors` feature.

use gpui::{prelude::*, *};
use ui::{h_flex, ActiveTheme};

use crate::property_editor_registry::PropertyEditorArgs;

pub fn render(args: &PropertyEditorArgs<'_>, cx: &App) -> AnyElement {
    let value = args.current_json.as_str().unwrap_or("").to_string();
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
        .child(
            div()
                .text_sm()
                .text_color(cx.theme().foreground)
                .child(value),
        )
        .into_any_element()
}
