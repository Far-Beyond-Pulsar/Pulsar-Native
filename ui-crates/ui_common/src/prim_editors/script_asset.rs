//! Property editor for `ScriptAssetPath` — read-only blueprint asset reference.
//!
//! Displays the blueprint directory name.  Clicking opens the blueprint in
//! its default editor via the global `OpenAsset` action.
//!
//! This file's sole job is to register the `ScriptAssetPath` type's editor.

use gpui::{prelude::*, *};
use plugin_editor_api::OpenAsset;
use pulsar_rendering::components::ScriptAssetPath;
use ui::{h_flex, ActiveTheme};

use crate::property_editor_registry::PropertyEditorArgs;

pub(super) fn render(args: &PropertyEditorArgs<'_>, cx: &App) -> AnyElement {
    let path_str = args.current_json.as_str().unwrap_or("").to_string();

    let display = if path_str.is_empty() {
        "No script assigned".to_string()
    } else {
        std::path::Path::new(&path_str)
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or(&path_str)
            .to_string()
    };

    let open_path = std::path::PathBuf::from(&path_str);
    let can_open = !path_str.is_empty();

    h_flex()
        .w_full()
        .justify_between()
        .items_center()
        .gap_2()
        .py_1()
        .child(
            div()
                .text_sm()
                .text_color(cx.theme().muted_foreground)
                .child(args.display_name.to_string()),
        )
        .child(
            div()
                .id(SharedString::from(format!(
                    "script-asset-{}-{}-{}",
                    args.id_prefix, args.class_name, args.prop_name
                )))
                .text_sm()
                .when(can_open, |el| el.cursor_pointer())
                .text_color(if can_open {
                    cx.theme().accent
                } else {
                    cx.theme().muted_foreground
                })
                .hover(|s| if can_open { s.underline() } else { s })
                .child(display)
                .when(can_open, move |el| {
                    el.on_click(move |_event, window, cx| {
                        window.dispatch_action(
                            Box::new(OpenAsset { path: open_path.clone() }),
                            cx,
                        );
                    })
                }),
        )
        .into_any_element()
}

pulsar_reflection::inventory::submit! {
    pulsar_reflection::UiPropertyEditorHint {
        type_id: std::any::TypeId::of::<ScriptAssetPath>(),
        // SAFETY: `render` matches the PropertyEditorRenderFn signature.
        fn_ptr: unsafe { pulsar_reflection::erase_property_editor_fn_ptr(render) },
    }
}
