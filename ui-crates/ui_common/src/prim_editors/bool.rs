//! Property editor for `bool` — toggle switch.

use gpui::{prelude::*, *};
use ui::{h_flex, switch::Switch, ActiveTheme, Sizable};

use crate::property_editor_registry::PropertyEditorArgs;

pub(super) fn render(args: &PropertyEditorArgs<'_>, cx: &App) -> AnyElement {
    let value = args.current_json.as_bool().unwrap_or(false);
    let on_toggle = args.on_bool_toggle.clone();
    let id = format!(
        "bool-{}-{}-{}",
        args.id_prefix, args.class_name, args.prop_name
    );
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
            Switch::new(id)
                .checked(value)
                .small()
                .on_click(move |checked, window, cx| {
                    (on_toggle)(*checked, window, cx);
                }),
        )
        .into_any_element()
}

pulsar_reflection::inventory::submit! {
    pulsar_reflection::UiPropertyEditorHint {
        type_id: std::any::TypeId::of::<bool>(),
        // SAFETY: `render` has the required PropertyEditorRenderFn signature.
        fn_ptr: unsafe { pulsar_reflection::erase_property_editor_fn_ptr(render) },
    }
}
