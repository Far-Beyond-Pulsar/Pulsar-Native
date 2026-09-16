//! Generic enum dropdown editor for all unit enums registered with
//! `#[derive(Reflectable)]`.
//!
//! Uses the reflection system's JSON serialization to extract variant indices
//! from any `&dyn Any` value, and JSON deserialization to construct enum values
//! from selected variant indices. This avoids needing to know the concrete enum
//! type at compile time.

use std::any::Any;

// ── Editor entity ────────────────────────────────────────────────────────────

/// Property editor for any unit enum (C-like enum with no data variants).
///
/// Renders a dropdown listing all variants. The value is stored as the variant
/// index (`u64`) and converted to/from the concrete enum type through the
/// runtime type registry's JSON codec so no per-enum-type code is needed.
#[cfg(feature = "prims-gpui")]
pub struct EnumDropdownEditor {
    label: String,
    id: gpui::SharedString,
    value: u64,
    variants: Option<&'static [&'static str]>,
    write_back: crate::PropertyWriteBack,
    type_info: &'static crate::RuntimeTypeInfo,
}

#[cfg(feature = "prims-gpui")]
impl gpui::Render for EnumDropdownEditor {
    fn render(
        &mut self,
        _window: &mut gpui::Window,
        cx: &mut gpui::Context<Self>,
    ) -> impl gpui::IntoElement {
        use gpui::prelude::*;
        use ui::{ActiveTheme, Sizable, button::Button, menu::PopupMenuItem};

        let Some(variants) = self.variants else {
            return crate::prims::editor_row(
                &self.label,
                gpui::div()
                    .text_sm()
                    .text_color(cx.theme().foreground)
                    .child(self.value.to_string()),
                cx,
            );
        };

        let selected_ix = (self.value as usize).min(variants.len().saturating_sub(1));
        let label = variants.get(selected_ix).copied().unwrap_or("Select");
        let write_back = self.write_back.clone();
        let type_info: &'static crate::RuntimeTypeInfo = self.type_info;

        crate::prims::editor_row(
            &self.label,
            Button::new(self.id.clone())
                .label(label)
                .xsmall()
                .outline()
                .dropdown_caret(true)
                .dropdown_menu_with_anchor(gpui::Corner::BottomRight, move |menu, _window, _cx| {
                    let mut menu = menu;
                    for (ix, _option) in variants.iter().enumerate() {
                        let write_back = write_back.clone();
                        menu = menu.item(
                            PopupMenuItem::new(variants[ix].to_string())
                                .checked(ix == selected_ix)
                                .on_click(move |_event, window, cx| {
                                    // Deserialise the variant index back into the concrete
                                    // enum type through the runtime type registry.
                                    let json = serde_json::json!(ix as u64);
                                    if let Ok(val) = crate::RUNTIME_TYPE_REGISTRY
                                        .deserialize_json_for_type(type_info, json)
                                    {
                                        // SAFETY: All unit enums registered with the reflection
                                        // system implement Send (C-like enums with no heap data).
                                        let val: Box<dyn Any + Send> =
                                            unsafe { std::mem::transmute(val) };
                                        (write_back)(val, window, cx);
                                    }
                                }),
                        );
                    }
                    menu
                }),
            cx,
        )
    }
}

// ── Factory function ─────────────────────────────────────────────────────────

/// Factory function registered as the property editor for ALL unit enums via
/// the `UiPropertyEditorHint` emitted by `#[derive(Reflectable)]` on enum types.
#[cfg(feature = "prims-gpui")]
pub fn enum_dropdown_editor(
    args: &crate::PropertyEditorArgs<'_>,
    _window: &mut gpui::Window,
    cx: &mut gpui::App,
) -> crate::BoundPropertyEditor {
    use gpui::AppContext as _;

    let type_info: &'static crate::RuntimeTypeInfo = args.type_info;
    let variants = type_info.enum_variants();
    let label = args.display_name.to_string();
    let id: gpui::SharedString = format!(
        "enum-{}-{}-{}",
        args.id_prefix, args.class_name, args.prop_name
    )
    .into();
    let value = crate::RUNTIME_TYPE_REGISTRY
        .serialize_json_for_any(args.current_value)
        .ok()
        .and_then(|v| v.as_u64())
        .unwrap_or(0);
    let write_back = args.write_back.clone();

    let entity = cx.new(|_| EnumDropdownEditor {
        label,
        id,
        value,
        variants,
        write_back,
        type_info,
    });

    crate::BoundPropertyEditor {
        view: entity.clone().into(),
        set_value: std::sync::Arc::new(move |value, window, cx| {
            let new_value = crate::RUNTIME_TYPE_REGISTRY
                .serialize_json_for_any(value)
                .ok()
                .and_then(|v| v.as_u64())
                .unwrap_or(0);
            entity.update(cx, |editor, cx| {
                if editor.value != new_value {
                    editor.value = new_value;
                    cx.notify();
                }
            });
        }),
    }
}
