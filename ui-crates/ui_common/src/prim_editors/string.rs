//! Property editor for `String` — read-only text display.
//!
//! Companion to `pulsar_reflection/src/prims/std/string.rs`.
//! Types that want a richer editor (e.g. `MeshAssetPath`) register their own
//! hint under their specific [`TypeId`] and shadow this fallback.

use gpui::{prelude::*, *};
use ui::{h_flex, ActiveTheme};

use crate::property_editor_registry::PropertyEditorArgs;

pub(super) fn render(args: &PropertyEditorArgs<'_>, cx: &App) -> AnyElement {
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

pulsar_reflection::inventory::submit! {
    pulsar_reflection::UiPropertyEditorHint {
        type_id: std::any::TypeId::of::<String>(),
        // SAFETY: `render` has the required PropertyEditorRenderFn signature.
        fn_ptr: unsafe { pulsar_reflection::erase_property_editor_fn_ptr(render) },
    }
}
