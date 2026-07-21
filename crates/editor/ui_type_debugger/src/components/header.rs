use engine_fs::UserTypeInfo as TypeInfo;
use gpui::{prelude::*, *};
use plugin_editor_api::FileTypeId;
use rust_i18n::t;
use std::path::PathBuf;
use ui::StyledExt;
use ui::{
    button::{Button, ButtonVariants as _},
    h_flex,
    input::TextInput,
    popup_menu::PopupMenuExt,
    v_flex, ActiveTheme as _, Icon, IconName, Sizable as _,
};

use crate::components::{kind_color, kind_icon, kind_label};
use crate::screen::TypeDebuggerDrawer;
use crate::utils::{FilterAll, FilterAliases, FilterStructs, FilterEnums, FilterTraits};

pub fn render_header(
    drawer: &mut TypeDebuggerDrawer,
    alias_count: usize,
    struct_count: usize,
    enum_count: usize,
    trait_count: usize,
    total_count: usize,
    cx: &mut Context<TypeDebuggerDrawer>,
) -> impl IntoElement {
    let current_filter_label = match &drawer.filtered_kind {
        None => format!("All Types ({})", total_count),
        Some(kind) => match kind.as_str() {
            "alias" => format!("Aliases ({})", alias_count),
            "struct" => format!("Structs ({})", struct_count),
            "enum" => format!("Enums ({})", enum_count),
            "trait" => format!("Traits ({})", trait_count),
            _ => format!("{} ({})", kind_label(kind), total_count),
        },
    };

    v_flex()
        .w_full()
        .gap_3()
        .px_4()
        .py_3()
        .border_b_1()
        .border_color(cx.theme().border)
        .bg(cx.theme().sidebar)
        .child(
            h_flex()
                .w_full()
                .justify_between()
                .items_center()
                .child(
                    h_flex()
                        .gap_3()
                        .items_center()
                        .child(
                            div()
                                .text_base()
                                .font_weight(gpui::FontWeight::BOLD)
                                .text_color(cx.theme().foreground)
                                .child(t!("TypeDebugger.Title").to_string()),
                        )
                        .child(
                            h_flex()
                                .gap_2()
                                .items_center()
                                .when(alias_count > 0, |this| {
                                    this.child(render_type_badge(
                                        drawer,
                                        &FileTypeId::new("alias"),
                                        alias_count,
                                        cx,
                                    ))
                                })
                                .when(struct_count > 0, |this| {
                                    this.child(render_type_badge(
                                        drawer,
                                        &FileTypeId::new("struct"),
                                        struct_count,
                                        cx,
                                    ))
                                })
                                .when(enum_count > 0, |this| {
                                    this.child(render_type_badge(
                                        drawer,
                                        &FileTypeId::new("enum"),
                                        enum_count,
                                        cx,
                                    ))
                                })
                                .when(trait_count > 0, |this| {
                                    this.child(render_type_badge(
                                        drawer,
                                        &FileTypeId::new("trait"),
                                        trait_count,
                                        cx,
                                    ))
                                }),
                        ),
                )
                .child(
                    h_flex()
                        .gap_2()
                        .child(
                            Button::new("toggle-grouping")
                                .ghost()
                                .small()
                                .icon(if drawer.group_by_kind {
                                    IconName::List
                                } else {
                                    IconName::Folder
                                })
                                .tooltip(if drawer.group_by_kind {
                                    t!("TypeDebugger.Action.ShowFlatList").to_string()
                                } else {
                                    t!("TypeDebugger.Action.GroupByKind").to_string()
                                })
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.toggle_grouping(cx);
                                })),
                        )
                        .child(
                            Button::new("clear-all")
                                .ghost()
                                .small()
                                .icon(IconName::Close)
                                .tooltip(t!("TypeDebugger.Action.ClearAll").to_string())
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.clear_types(cx);
                                })),
                        ),
                ),
        )
        .child(
            h_flex()
                .w_full()
                .gap_2()
                .items_center()
                .child(
                    div().flex_1().min_w(px(200.0)).child(
                        TextInput::new(&drawer.search_input).w_full().prefix(
                            ui::Icon::new(IconName::Search)
                                .size_4()
                                .text_color(cx.theme().muted_foreground),
                        ),
                    ),
                )
                .child({
                    let is_all_selected = drawer.filtered_kind.is_none();
                    let is_aliases_selected =
                        drawer.filtered_kind.as_ref().map(|k| k.as_str()) == Some("alias");
                    let is_structs_selected =
                        drawer.filtered_kind.as_ref().map(|k| k.as_str()) == Some("struct");
                    let is_enums_selected =
                        drawer.filtered_kind.as_ref().map(|k| k.as_str()) == Some("enum");
                    let is_traits_selected =
                        drawer.filtered_kind.as_ref().map(|k| k.as_str()) == Some("trait");

                    Button::new("filter-dropdown")
                        .ghost()
                        .small()
                        .icon(IconName::Filter)
                        .label(current_filter_label.clone())
                        .popup_menu_with_anchor(
                            Corner::BottomRight,
                            move |menu, _window, _cx| {
                                menu.menu_with_check(
                                    "All Types",
                                    is_all_selected,
                                    Box::new(FilterAll),
                                )
                                .separator()
                                .menu_with_check(
                                    "Aliases",
                                    is_aliases_selected,
                                    Box::new(FilterAliases),
                                )
                                .menu_with_check(
                                    "Structs",
                                    is_structs_selected,
                                    Box::new(FilterStructs),
                                )
                                .menu_with_check(
                                    "Enums",
                                    is_enums_selected,
                                    Box::new(FilterEnums),
                                )
                                .menu_with_check(
                                    "Traits",
                                    is_traits_selected,
                                    Box::new(FilterTraits),
                                )
                            },
                        )
                }),
        )
}

pub fn render_type_badge(
    drawer: &TypeDebuggerDrawer,
    kind: &FileTypeId,
    count: usize,
    cx: &App,
) -> impl IntoElement {
    h_flex()
        .gap_1()
        .items_center()
        .px_2()
        .py_0p5()
        .rounded_md()
        .bg(kind_color(kind, cx).opacity(0.15))
        .child(
            kind_icon(kind)
                .size_3()
                .text_color(kind_color(kind, cx)),
        )
        .child(
            div()
                .text_xs()
                .font_weight(gpui::FontWeight::SEMIBOLD)
                .text_color(kind_color(kind, cx))
                .child(count.to_string()),
        )
}

pub fn render_empty_state(drawer: &TypeDebuggerDrawer, cx: &App) -> Div {
    div().size_full().child(
        div()
            .size_full()
            .flex()
            .items_center()
            .justify_center()
            .p_8()
            .child(
                v_flex()
                    .gap_4()
                    .items_center()
                    .max_w(px(400.0))
                    .px_6()
                    .py_8()
                    .rounded_xl()
                    .bg(cx.theme().secondary.opacity(0.2))
                    .border_1()
                    .border_color(cx.theme().border.opacity(0.3))
                    .child(
                        div()
                            .w(px(64.0))
                            .h(px(64.0))
                            .rounded_full()
                            .bg(cx.theme().accent.opacity(0.15))
                            .flex()
                            .items_center()
                            .justify_center()
                            .child(
                                ui::Icon::new(IconName::Database)
                                    .size(px(32.0)),
                            ),
                    )
                    .child(
                        div()
                            .text_lg()
                            .font_weight(gpui::FontWeight::BOLD)
                            .text_color(cx.theme().foreground)
                            .child(t!("TypeDebugger.Empty.Title").to_string()),
                    )
                    .child(
                        div()
                            .text_sm()
                            .text_center()
                            .text_color(cx.theme().muted_foreground)
                            .line_height(rems(1.5))
                            .child(if !drawer.search_query.is_empty() {
                                "No types match your search. Try a different query."
                            } else {
                                "The type database is empty. Types will appear here once registered."
                            }),
                    ),
            ),
    )
}

pub fn render_type_item(
    drawer: &TypeDebuggerDrawer,
    type_info: &TypeInfo,
    is_selected: bool,
    cx: &mut Context<TypeDebuggerDrawer>,
) -> impl IntoElement {
    let type_info_clone = type_info.clone();

    div().w_full().px_3().py_2().child(
        div()
            .w_full()
            .px_4()
            .py_3()
            .rounded_lg()
            .border_1()
            .border_color(if is_selected {
                cx.theme().accent
            } else {
                cx.theme().border.opacity(0.5)
            })
            .bg(if is_selected {
                cx.theme().accent.opacity(0.08)
            } else {
                cx.theme().sidebar.opacity(0.5)
            })
            .shadow_sm()
            .when(is_selected, |this| {
                this.border_l_3().border_color(cx.theme().accent)
            })
            .hover(|this| {
                this.bg(cx.theme().secondary.opacity(0.7))
                    .border_color(cx.theme().accent.opacity(0.5))
            })
            .cursor_pointer()
            .on_mouse_down(
                gpui::MouseButton::Left,
                cx.listener(move |this, _, _window, cx| {
                    this.navigate_to_type(&type_info_clone, cx);
                }),
            )
            .child(
                v_flex()
                    .gap_2()
                    .w_full()
                    .child(
                        h_flex()
                            .gap_3()
                            .items_center()
                            .w_full()
                            .child(
                                h_flex()
                                    .gap_1p5()
                                    .items_center()
                                    .child(
                                        kind_icon(&type_info.file_type_id)
                                            .size_4()
                                            .text_color(
                                                kind_color(&type_info.file_type_id, cx),
                                            ),
                                    )
                                    .child(
                                        div()
                                            .text_xs()
                                            .font_weight(gpui::FontWeight::SEMIBOLD)
                                            .text_color(
                                                kind_color(&type_info.file_type_id, cx),
                                            )
                                            .child(kind_label(&type_info.file_type_id)),
                                    ),
                            )
                            .child(
                                div()
                                    .flex_1()
                                    .text_xs()
                                    .font_family("monospace")
                                    .text_color(cx.theme().muted_foreground)
                                    .child(format!("ID: {}", type_info.uuid)),
                            ),
                    )
                    .child(
                        div()
                            .w_full()
                            .text_sm()
                            .font_weight(gpui::FontWeight::SEMIBOLD)
                            .text_color(cx.theme().foreground)
                            .line_height(rems(1.4))
                            .child(type_info.display_name.clone()),
                    )
                    .when_some(type_info.description.as_ref(), |container, desc| {
                        container.child(
                            div()
                                .w_full()
                                .text_xs()
                                .text_color(cx.theme().muted_foreground)
                                .line_height(rems(1.4))
                                .child(desc.clone()),
                        )
                    })
                    .when_some(
                        Some(type_info.file_path.as_path()),
                        |container, file_path| {
                            let display_path = drawer.get_display_path(file_path);
                            container.child(
                                div()
                                    .w_full()
                                    .px_2()
                                    .py_1()
                                    .mt_1()
                                    .rounded_md()
                                    .bg(cx.theme().background.opacity(0.5))
                                    .border_1()
                                    .border_color(cx.theme().border.opacity(0.3))
                                    .child(
                                        h_flex()
                                            .gap_2()
                                            .items_center()
                                            .child(
                                                ui::Icon::new(IconName::Folder)
                                                    .size_3()
                                                    .text_color(cx.theme().muted_foreground),
                                            )
                                            .child(
                                                div()
                                                    .text_xs()
                                                    .font_family("monospace")
                                                    .text_color(cx.theme().muted_foreground)
                                                    .child(display_path),
                                            ),
                                    ),
                            )
                        },
                    ),
            ),
    )
}
