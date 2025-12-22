use gpui::prelude::*;
use gpui::*;
use std::collections::HashSet;
use std::path::PathBuf;
use ui::{
    h_flex, v_flex,
    input::{InputState, TextInput},
    context_menu::ContextMenuExt,
    ActiveTheme as _, Icon, IconName, StyledExt,
};

use crate::drawer::{
    types::FileItem,
    utils::{format_file_size, format_modified_time, get_icon_for_file_type},
    context_menus::{folder_context_menu, item_context_menu},
};

// ============================================================================
// LIST VIEW RENDERING
// ============================================================================

/// Render files in a list layout
pub fn render_list(
    items: &[FileItem],
    selected_items: &HashSet<PathBuf>,
    renaming_item: Option<&PathBuf>,
    rename_input_state: &Entity<InputState>,
    has_clipboard: bool,
    on_item_click: impl Fn(PathBuf, MouseButton) -> Box<dyn Action> + 'static + Copy,
    on_item_double_click: impl Fn(PathBuf) -> Box<dyn Action> + 'static + Copy,
    window: &mut Window,
    cx: &mut AppContext,
) -> impl IntoElement {
    v_flex()
        .w_full()
        .gap_px()
        .child(render_list_header(cx))
        .children(items.iter().map(|item| {
            render_list_item(
                item,
                selected_items.contains(&item.path),
                renaming_item == Some(&item.path),
                rename_input_state,
                has_clipboard,
                on_item_click,
                on_item_double_click,
                window,
                cx,
            )
        }))
}

/// Render list header with column labels
fn render_list_header(cx: &mut AppContext) -> impl IntoElement {
    h_flex()
        .w_full()
        .h(px(32.))
        .px_4()
        .items_center()
        .bg(cx.theme().muted)
        .border_b_1()
        .border_color(cx.theme().border)
        .child(
            div()
                .flex_1()
                .text_xs()
                .font_semibold()
                .text_color(cx.theme().muted_foreground)
                .child("Name")
        )
        .child(
            div()
                .w(px(100.))
                .text_xs()
                .font_semibold()
                .text_color(cx.theme().muted_foreground)
                .child("Size")
        )
        .child(
            div()
                .w(px(120.))
                .text_xs()
                .font_semibold()
                .text_color(cx.theme().muted_foreground)
                .child("Type")
        )
        .child(
            div()
                .w(px(160.))
                .text_xs()
                .font_semibold()
                .text_color(cx.theme().muted_foreground)
                .child("Modified")
        )
}

/// Render a single list item
fn render_list_item(
    item: &FileItem,
    is_selected: bool,
    is_renaming: bool,
    rename_input_state: &Entity<InputState>,
    has_clipboard: bool,
    on_item_click: impl Fn(PathBuf, MouseButton) -> Box<dyn Action> + 'static,
    on_item_double_click: impl Fn(PathBuf) -> Box<dyn Action> + 'static,
    window: &mut Window,
    cx: &mut AppContext,
) -> impl IntoElement {
    let item_path = item.path.clone();
    let item_path_for_click = item.path.clone();
    let item_path_for_dbl = item.path.clone();
    let item_path_for_menu = item.path.clone();
    let is_folder = item.is_folder;
    let is_class = item.file_type.is_class();

    h_flex()
        .w_full()
        .h(px(36.))
        .px_4()
        .items_center()
        .border_b_1()
        .border_color(cx.theme().border)
        .when(is_selected, |this| {
            this.bg(cx.theme().accent)
                .text_color(cx.theme().accent_foreground)
        })
        .when(!is_selected, |this| {
            this.hover(|style| {
                style.bg(cx.theme().muted)
            })
        })
        .on_mouse_down(MouseButton::Left, move |event, window, cx| {
            cx.dispatch_action(on_item_click(item_path_for_click.clone(), MouseButton::Left));
        })
        .on_mouse_down(MouseButton::Right, move |event, window, cx| {
            cx.dispatch_action(on_item_click(item_path.clone(), MouseButton::Right));
        })
        .on_double_click(move |event, window, cx| {
            cx.dispatch_action(on_item_double_click(item_path_for_dbl.clone()));
        })
        .context_menu(if is_folder {
            folder_context_menu(item_path_for_menu.clone(), has_clipboard)
        } else {
            item_context_menu(item_path_for_menu.clone(), has_clipboard, is_class)
        })
        .child(
            // Icon and name
            h_flex()
                .flex_1()
                .gap_2()
                .items_center()
                .child(
                    Icon::new(get_icon_for_file_type(&item.file_type))
                        .size_4()
                        .when(is_selected, |icon| {
                            icon.text_color(cx.theme().accent_foreground)
                        })
                        .when(!is_selected, |icon| {
                            icon.text_color(cx.theme().foreground)
                        })
                )
                .child(
                    if is_renaming {
                        div()
                            .flex_1()
                            .child(
                                TextInput::new(rename_input_state, window, cx)
                                    .w_full()
                            )
                            .into_any_element()
                    } else {
                        div()
                            .text_sm()
                            .when(is_selected, |this| {
                                this.text_color(cx.theme().accent_foreground)
                            })
                            .when(!is_selected, |this| {
                                this.text_color(cx.theme().foreground)
                            })
                            .child(&item.name)
                            .into_any_element()
                    }
                )
        )
        .child(
            // Size
            div()
                .w(px(100.))
                .text_sm()
                .when(is_selected, |this| {
                    this.text_color(cx.theme().accent_foreground)
                })
                .when(!is_selected, |this| {
                    this.text_color(cx.theme().muted_foreground)
                })
                .child(if is_folder {
                    "—".to_string()
                } else {
                    format_file_size(item.size)
                })
        )
        .child(
            // Type
            div()
                .w(px(120.))
                .text_sm()
                .when(is_selected, |this| {
                    this.text_color(cx.theme().accent_foreground)
                })
                .when(!is_selected, |this| {
                    this.text_color(cx.theme().muted_foreground)
                })
                .child(item.file_type.display_name())
        )
        .child(
            // Modified
            div()
                .w(px(160.))
                .text_sm()
                .when(is_selected, |this| {
                    this.text_color(cx.theme().accent_foreground)
                })
                .when(!is_selected, |this| {
                    this.text_color(cx.theme().muted_foreground)
                })
                .child(format_modified_time(item.modified))
        )
}
