use engine_fs::UserTypeInfo as TypeInfo;
use gpui::{prelude::*, *};
use plugin_editor_api::FileTypeId;
use std::collections::HashMap;
use ui::StyledExt;
use ui::{h_flex, scroll::ScrollbarAxis, v_flex, ActiveTheme as _, Icon, IconName};

use crate::components::render_type_item;
use crate::screen::TypeDebuggerDrawer;

pub fn render_flat_view(
    drawer: &TypeDebuggerDrawer,
    types: Vec<TypeInfo>,
    selected_index: Option<usize>,
    cx: &mut Context<TypeDebuggerDrawer>,
) -> impl IntoElement {
    div()
        .id("type-debugger-scroll-container")
        .size_full()
        .scrollable(ScrollbarAxis::Vertical)
        .child(
            v_flex()
                .w_full()
                .p_2()
                .gap_2()
                .children(types.into_iter().enumerate().map(|(index, type_info)| {
                    let is_selected = selected_index == Some(index);
                    render_type_item(drawer, &type_info, is_selected, cx)
                })),
        )
}

pub fn render_grouped_view(
    drawer: &TypeDebuggerDrawer,
    selected_index: Option<usize>,
    cx: &mut Context<TypeDebuggerDrawer>,
) -> impl IntoElement {
    let grouped = drawer.get_grouped_types();
    let mut global_index = 0;

    div()
        .id("type-debugger-scroll-container-grouped")
        .size_full()
        .scrollable(ScrollbarAxis::Vertical)
        .child(v_flex().w_full().p_2().gap_2().children({
            let mut groups = Vec::new();
            let ordered_kinds = vec![
                FileTypeId::new("alias"),
                FileTypeId::new("struct"),
                FileTypeId::new("enum"),
                FileTypeId::new("trait"),
            ];

            for kind in ordered_kinds {
                if let Some(types) = grouped.get(&kind) {
                    if !types.is_empty() {
                        let kind_clone = kind.clone();
                        let types_clone = types.clone();

                        groups.push(
                            v_flex()
                                .w_full()
                                .px_3()
                                .child(
                                    div()
                                        .w_full()
                                        .px_3()
                                        .py_2()
                                        .mb_2()
                                        .rounded_md()
                                        .bg(cx.theme().secondary.opacity(0.3))
                                        .border_1()
                                        .border_color(cx.theme().border.opacity(0.3))
                                        .child(
                                            h_flex()
                                                .w_full()
                                                .gap_3()
                                                .items_center()
                                                .child(
                                                    kind_icon(&kind_clone)
                                                        .size_4()
                                                        .text_color(
                                                            kind_color(&kind_clone, cx),
                                                        ),
                                                )
                                                .child(
                                                    div()
                                                        .flex_1()
                                                        .text_sm()
                                                        .font_weight(
                                                            gpui::FontWeight::SEMIBOLD,
                                                        )
                                                        .text_color(cx.theme().foreground)
                                                        .child(format!(
                                                            "{} ({})",
                                                            kind_label(&kind_clone),
                                                            types_clone.len()
                                                        )),
                                                ),
                                        ),
                                )
                                .children(
                                    types_clone
                                        .iter()
                                        .map(|type_info| {
                                            let is_selected =
                                                selected_index == Some(global_index);
                                            global_index += 1;
                                            render_type_item(
                                                drawer,
                                                type_info,
                                                is_selected,
                                                cx,
                                            )
                                        })
                                        .collect::<Vec<_>>(),
                                ),
                        );
                    }
                }
            }
            groups
        }))
}

pub fn kind_icon(kind: &FileTypeId) -> Icon {
    let icon_name = match kind.as_str() {
        "alias" => IconName::Link,
        "struct" => IconName::Box,
        "enum" => IconName::List,
        "trait" => IconName::Code,
        _ => IconName::Page,
    };
    Icon::new(icon_name)
}

pub fn kind_color(kind: &FileTypeId, _cx: &App) -> Hsla {
    match kind.as_str() {
        "alias" => gpui::rgb(0x607D8B).into(),
        "struct" => gpui::rgb(0x00BCD4).into(),
        "enum" => gpui::rgb(0x673AB7).into(),
        "trait" => gpui::rgb(0x3F51B5).into(),
        _ => gpui::rgb(0x9E9E9E).into(),
    }
}

pub fn kind_label(kind: &FileTypeId) -> String {
    match kind.as_str() {
        "alias" => "Aliases".to_string(),
        "struct" => "Structs".to_string(),
        "enum" => "Enums".to_string(),
        "trait" => "Traits".to_string(),
        _ => format!("{}s", kind.as_str()),
    }
}
