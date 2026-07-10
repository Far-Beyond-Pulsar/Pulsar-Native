use gpui::prelude::*;
use gpui::*;
use rust_i18n::t;
use ui::button::{Button, ButtonVariants as _};
use ui::input::TextInput;
use ui::{h_flex, v_flex, ActiveTheme as _, Icon, IconName, Sizable as _, StyledExt};

use crate::components::file_list::FileManagerDrawer;
use crate::utils::tree::FolderNode;
use crate::utils::types::*;

pub fn render_folder_tree(
    d: &mut FileManagerDrawer,
    w: &mut Window,
    cx: &mut Context<FileManagerDrawer>,
) -> impl IntoElement {
    v_flex()
        .size_full()
        .bg(cx.theme().sidebar)
        .border_r_1()
        .border_color(cx.theme().border)
        .child(
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
                        .items_center()
                        .justify_between()
                        .child(
                            h_flex()
                                .gap_2()
                                .items_center()
                                .child(
                                    Icon::new(IconName::Folder)
                                        .size_4()
                                        .text_color(cx.theme().foreground),
                                )
                                .child(
                                    div()
                                        .text_base()
                                        .font_weight(gpui::FontWeight::BOLD)
                                        .text_color(cx.theme().foreground)
                                        .child(t!("FileManager.ProjectFiles").to_string()),
                                ),
                        )
                        .child(
                            h_flex()
                                .gap_1()
                                .child(
                                    Button::new("expand-all")
                                        .icon(IconName::ChevronDown)
                                        .ghost()
                                        .xsmall()
                                        .tooltip(t!("FileManager.ExpandAll").to_string())
                                        .on_click(cx.listener(|d, _e, _w, cx| {
                                            if let Some(ref mut t) = d.folder_tree {
                                                t.expand_all();
                                                cx.notify();
                                            }
                                        })),
                                )
                                .child(
                                    Button::new("collapse-all")
                                        .icon(IconName::ChevronUp)
                                        .ghost()
                                        .xsmall()
                                        .tooltip(t!("FileManager.CollapseAll").to_string())
                                        .on_click(cx.listener(|d, _e, _w, cx| {
                                            if let Some(ref mut t) = d.folder_tree {
                                                t.collapse_all();
                                                cx.notify();
                                            }
                                        })),
                                ),
                        ),
                )
                .child(
                    div().w_full().child(
                        TextInput::new(&d.folder_search_state).w_full().prefix(
                            Icon::new(IconName::Search)
                                .size_3()
                                .text_color(cx.theme().muted_foreground),
                        ),
                    ),
                ),
        )
        .child(
            div()
                .flex_1()
                .overflow_hidden()
                .when_some(d.folder_tree.clone(), |this, tree| {
                    this.child(
                        v_flex()
                            .size_full()
                            .p_2()
                            .gap_px()
                            .scrollable(gpui::Axis::Vertical)
                            .child(render_folder_node(d, &tree, 0, w, cx)),
                    )
                })
                .when(d.folder_tree.is_none(), |this| {
                    this.child(
                        div()
                            .size_full()
                            .flex()
                            .items_center()
                            .justify_center()
                            .p_4()
                            .child(
                                v_flex()
                                    .gap_3()
                                    .items_center()
                                    .max_w(px(200.))
                                    .px_4()
                                    .py_6()
                                    .rounded_lg()
                                    .bg(cx.theme().secondary.opacity(0.2))
                                    .border_1()
                                    .border_color(cx.theme().border.opacity(0.3))
                                    .child(
                                        div()
                                            .w(px(48.))
                                            .h(px(48.))
                                            .rounded_full()
                                            .bg(cx.theme().muted.opacity(0.3))
                                            .flex()
                                            .items_center()
                                            .justify_center()
                                            .child(
                                                Icon::new(IconName::FolderOpen)
                                                    .size(px(24.))
                                                    .text_color(cx.theme().muted_foreground),
                                            ),
                                    )
                                    .child(
                                        div()
                                            .text_sm()
                                            .font_weight(gpui::FontWeight::SEMIBOLD)
                                            .text_color(cx.theme().foreground)
                                            .child("No Project"),
                                    )
                                    .child(
                                        div()
                                            .text_xs()
                                            .text_center()
                                            .text_color(cx.theme().muted_foreground)
                                            .line_height(rems(1.4))
                                            .child("Open a project folder to see files"),
                                    ),
                            ),
                    )
                }),
        )
}

pub fn render_folder_node(
    d: &mut FileManagerDrawer,
    node: &FolderNode,
    depth: usize,
    w: &mut Window,
    cx: &mut Context<FileManagerDrawer>,
) -> impl IntoElement {
    let sel = d.selected_folder.as_ref() == Some(&node.path);
    let p = node.path.clone();
    let pe = p.clone();
    let pi = p.clone();
    let p_path = p.clone();
    let pd = p.clone();
    let exp = node.expanded;
    let has = !node.children.is_empty();
    let indent = px(depth as f32 * 20.0 + 4.0);
    let icon = if exp {
        IconName::FolderOpen
    } else {
        IconName::Folder
    };
    let ic = d.fs_metadata.get_color_override(&node.path).unwrap_or(ui::hierarchical_tree::tree_colors::FOLDER);
    let tc = if sel {
        cx.theme().accent_foreground
    } else {
        cx.theme().foreground
    };
    let mc = if sel {
        cx.theme().accent_foreground.opacity(0.7)
    } else {
        cx.theme().muted_foreground
    };
    let mut item = h_flex()
        .id(SharedString::from(format!("folder-{}", p.display())))
        .w_full()
        .items_center()
        .gap_1()
        .h_7()
        .pl(indent)
        .pr_2()
        .rounded(px(4.))
        .cursor_pointer();
    if sel {
        item = item.bg(cx.theme().accent).shadow_sm();
    } else {
        item = item.hover(|s| s.bg(cx.theme().muted.opacity(0.3)));
    }
    v_flex()
        .w_full()
        .child(
            item.on_drag_move(cx.listener(
                move |d, _: &DragMoveEvent<gpui::ExternalPaths>, _w, cx| {
                    d.hovered_drop_folder = Some(pd.clone());
                    d.show_drop_hint = true;
                    cx.notify();
                },
            ))
            .drag_over::<DraggedFile>(|s, _, _, cx| {
                s.bg(cx.theme().accent.opacity(0.2))
                    .border_1()
                    .border_color(cx.theme().accent)
            })
            .drag_over::<gpui::ExternalPaths>(|s, _, _, cx| {
                s.bg(cx.theme().accent.opacity(0.2))
                    .border_1()
                    .border_color(cx.theme().accent)
            })
            .on_drop(cx.listener(move |d, drag: &DraggedFile, w, cx| {
                d.handle_drop_on_folder_new(&pi, &drag.paths, w, cx)
            }))
            .on_drop(cx.listener(move |d, ext: &gpui::ExternalPaths, w, cx| {
                d.handle_external_drop_on_folder(&p_path, ext.paths(), w, cx)
            }))
            .on_mouse_down(
                gpui::MouseButton::Left,
                cx.listener(move |d, _: &MouseDownEvent, _w, cx| {
                    crate::handlers::handle_folder_select(d, p.clone(), cx)
                }),
            )
            .child(if has {
                div()
                    .w_4()
                    .h_4()
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded(px(2.))
                    .cursor_pointer()
                    .hover(|s| s.bg(cx.theme().muted.opacity(0.5)))
                    .child(
                        Icon::new(if exp {
                            IconName::ChevronDown
                        } else {
                            IconName::ChevronRight
                        })
                        .size(px(12.))
                        .text_color(mc),
                    )
                    .on_mouse_down(
                        gpui::MouseButton::Left,
                        cx.listener(move |d, _: &MouseDownEvent, _w, cx| {
                            cx.stop_propagation();
                            if let Some(ref mut t) = d.folder_tree {
                                t.toggle_expanded(&pe);
                            }
                            cx.notify();
                        }),
                    )
                    .into_any_element()
            } else {
                div().w_4().into_any_element()
            })
            .child(
                div()
                    .w_5()
                    .h_5()
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded(px(3.))
                    .bg(ic.opacity(0.15))
                    .child(
                        Icon::new(icon)
                            .size(px(14.))
                            .text_color(if sel { tc } else { ic }),
                    ),
            )
            .child(
                div()
                    .flex_1()
                    .text_sm()
                    .text_color(tc)
                    .overflow_hidden()
                    .text_ellipsis()
                    .child(node.name.clone()),
            ),
        )
        .children(if exp {
            node.children
                .iter()
                .map(|c| {
                    render_folder_node(d, c, depth + 1, w, cx)
                        .into_any_element()
                })
                .collect()
        } else {
            Vec::new()
        })
}
