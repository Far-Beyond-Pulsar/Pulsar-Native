//! Property editor for `MeshAssetPath` — mesh asset browser popover.
//!
//! Companion to `pulsar_rendering::components::static_mesh_component`.
//! The `MeshAssetPath` type is defined and registered with the reflection system
//! in that module; this file registers the GPUI property editor for it.
//!
//! Once `pulsar_rendering` gains a GPUI dependency this registration can move
//! directly into `static_mesh_component.rs` via:
//! ```rust,ignore
//! #[pulsar_reflection::pulsar_type(
//!     primitive,
//!     structure = String,
//!     serialize_json_with   = …,
//!     deserialize_json_with = …,
//!     editor = render,          // ← macro generates the UiPropertyEditorHint submit
//! )]
//! type RegisteredMeshAssetPath = MeshAssetPath;
//! ```

use gpui::{prelude::*, Corner, ImageSource, ObjectFit, *};
use ui::{button::ButtonVariants as _, h_flex, popover::Popover, ActiveTheme, Sizable};

use crate::{property_editor_registry::PropertyEditorArgs, MeshAssetPicker};
// MeshAssetPath is re-exported from pulsar_rendering::components via `pub use static_mesh_component::*`
use pulsar_rendering::components::MeshAssetPath;

pub(super) fn render(args: &PropertyEditorArgs<'_>, cx: &App) -> AnyElement {
    let path_str = args.current_json.as_str().unwrap_or("");

    let Some(picker) = args.mesh_picker.clone() else {
        // Picker state not yet allocated — graceful fallback.
        return h_flex()
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
                    .text_color(cx.theme().muted_foreground)
                    .child(if path_str.is_empty() {
                        "No mesh selected".to_string()
                    } else {
                        path_str.to_string()
                    }),
            )
            .into_any_element();
    };

    let display = if path_str.is_empty() {
        "Select mesh asset…".to_string()
    } else {
        std::path::Path::new(path_str)
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or(path_str)
            .to_string()
    };

    let thumb = picker.read(cx).thumbnail_for_path(path_str);

    let pop = Popover::<MeshAssetPicker>::new(format!(
        "mesh-asset-picker-{}-{}",
        args.class_name, args.prop_name
    ))
    .anchor(Corner::BottomRight)
    .trigger(
        ui::button::Button::new(format!(
            "mesh-asset-btn-{}-{}",
            args.class_name, args.prop_name
        ))
        .label(display)
        .small()
        .ghost()
        .dropdown_caret(true),
    )
    .content(move |_window, _cx| picker.clone())
    .into_any_element();

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
            h_flex()
                .items_center()
                .gap_2()
                .child(pop)
                .map(|el| match thumb {
                    Some(img) => el.child(
                        div()
                            .w(px(40.0))
                            .h(px(40.0))
                            .rounded(px(4.0))
                            .overflow_hidden()
                            .border_1()
                            .border_color(cx.theme().border)
                            .flex_shrink_0()
                            .child(
                                gpui::img(ImageSource::Render(img))
                                    .w(px(40.0))
                                    .h(px(40.0))
                                    .object_fit(ObjectFit::Cover),
                            ),
                    ),
                    None => el,
                }),
        )
        .into_any_element()
}

pulsar_reflection::inventory::submit! {
    pulsar_reflection::UiPropertyEditorHint {
        type_id: std::any::TypeId::of::<MeshAssetPath>(),
        // SAFETY: `render` has the required PropertyEditorRenderFn signature.
        fn_ptr: unsafe { pulsar_reflection::erase_property_editor_fn_ptr(render) },
    }
}
