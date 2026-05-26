//! Property editor for `[f32; 3]` — read-only XYZ preview.
//!
//! Companion to `pulsar_reflection/src/prims/core/vec3.rs`.

use gpui::{prelude::*, *};
use ui::{h_flex, ActiveTheme};

use crate::property_editor_registry::PropertyEditorArgs;

pub(super) fn render(args: &PropertyEditorArgs<'_>, cx: &App) -> AnyElement {
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

pulsar_reflection::inventory::submit! {
    pulsar_reflection::UiPropertyEditorHint {
        type_id: std::any::TypeId::of::<[f32; 3]>(),
        // SAFETY: `render` has the required PropertyEditorRenderFn signature.
        fn_ptr: unsafe { pulsar_reflection::erase_property_editor_fn_ptr(render) },
    }
}
