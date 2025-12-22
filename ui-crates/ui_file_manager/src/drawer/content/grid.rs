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
    utils::{format_file_size, get_icon_for_file_type},
    context_menus::{folder_context_menu, item_context_menu},
};

// ============================================================================
// GRID VIEW RENDERING
// ============================================================================

/// Render files in a grid layout
pub fn render_grid<V: 'static>(
    items: &[FileItem],
    selected_items: &HashSet<PathBuf>,
    renaming_item: Option<&PathBuf>,
    rename_input_state: &Entity<InputState>,
    has_clipboard: bool,
    window: &mut Window,
    cx: &mut Context<V>,
) -> impl IntoElement {
    let grid_gap = px(8.);
    let item_width = px(100.);

    div()
        .flex()
        .flex_wrap()
        .gap(grid_gap)
        .p_2()
        .children(items.iter().map(|item| {
            render_grid_item(
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

/// Render a single grid item
fn render_grid_item(
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

    v_flex()
        .w(px(100.))
        .gap_1()
        .p_2()
        .rounded_md()
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
            // Icon
            div()
                .flex()
                .justify_center()
                .items_center()
                .h(px(48.))
                .child(
                    Icon::new(get_icon_for_file_type(&item.file_type))
                        .size(px(32.))
                        .when(is_selected, |icon| {
                            icon.text_color(cx.theme().accent_foreground)
                        })
                        .when(!is_selected, |icon| {
                            icon.text_color(cx.theme().foreground)
                        })
                )
        )
        .child(
            // Name (or rename input)
            if is_renaming {
                div()
                    .w_full()
                    .child(
                        TextInput::new(rename_input_state, window, cx)
                            .w_full()
                    )
                    .into_any_element()
            } else {
                div()
                    .text_xs()
                    .text_center()
                    .line_clamp(2)
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
        .when(!is_folder, |this| {
            this.child(
                div()
                    .text_xs()
                    .text_center()
                    .when(is_selected, |this| {
                        this.text_color(cx.theme().accent_foreground).opacity(0.7)
                    })
                    .when(!is_selected, |this| {
                        this.text_color(cx.theme().muted_foreground)
                    })
                    .child(format_file_size(item.size))
            )
        })
}
