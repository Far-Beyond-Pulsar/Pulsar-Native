//! Property editor for `ScriptAssetPath` — read-only blueprint asset reference.
//!
//! Shows a Code icon next to the blueprint folder name.  Clicking opens the
//! blueprint in its registered editor via the global `OpenAsset` action.

use gpui::{prelude::*, *};
use plugin_editor_api::OpenAsset;
use pulsar_rendering::components::ScriptAssetPath;
use ui::{
    button::{Button, ButtonVariants as _},
    h_flex, ActiveTheme, Disableable as _, Icon, IconName, Sizable,
};

use crate::property_editor_registry::PropertyEditorArgs;

pub(super) fn render(args: &PropertyEditorArgs<'_>, cx: &App) -> AnyElement {
    let path_str = args.current_json.as_str().unwrap_or("").to_string();

    let file_name = if path_str.is_empty() {
        "None".to_string()
    } else {
        std::path::Path::new(&path_str)
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or(&path_str)
            .to_string()
    };

    let open_path = std::path::PathBuf::from(&path_str);
    let has_asset = !path_str.is_empty();

    let id = format!(
        "script-asset-{}-{}-{}",
        args.id_prefix, args.class_name, args.prop_name
    );

    h_flex()
        .w_full()
        .justify_between()
        .items_center()
        .gap_2()
        .py_1()
        .child(
            // Field label
            div()
                .text_sm()
                .text_color(cx.theme().muted_foreground)
                .child(args.display_name.to_string()),
        )
        .child(
            // [Code icon] [filename] — clickable when an asset is assigned
            Button::new(id)
                .icon(Icon::new(IconName::Code).size(px(12.)))
                .label(file_name)
                .ghost()
                .small()
                .when(!has_asset, |b| b.disabled(true))
                .when(has_asset, move |b| {
                    b.on_click(move |_event, window, cx| {
                        window.dispatch_action(
                            Box::new(OpenAsset {
                                path: open_path.clone(),
                            }),
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
