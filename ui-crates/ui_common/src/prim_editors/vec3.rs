//! Property editor for `[f32; 3]` — read-only XYZ preview.
//!
//! This module is deprecated. The actual editor registration now lives in
//! `pulsar_reflection/src/prims/core/vec3.rs` via the `ui-editors` feature.

use gpui::{prelude::*, *};
use ui::{h_flex, ActiveTheme};

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
        .child(
            div()
                .text_sm()
                .text_color(cx.theme().foreground)
                .child(format!("{:?}", args.current_json)),
        )
        .into_any_element()
}
