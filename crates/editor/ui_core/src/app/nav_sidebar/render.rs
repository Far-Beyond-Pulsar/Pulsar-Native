//! Drawing the unified sidebar: the icon rail, and the drawer that opens over
//! the editor on hover (or sits beside it when kept open).

use gpui::{
    div, prelude::*, px, AnyElement, Context, ElementId, Hsla, IntoElement, MouseButton,
    SharedString, Window,
};
use ui::button::{Button, ButtonVariants as _};
use ui::tooltip::Tooltip;
use ui::{h_flex, v_flex, ActiveTheme as _, Icon, IconName, Sizable as _};

use super::model::{content_root, FolderRow, SectionId, SidebarTab, TabSection};
use super::{enabled, pinned_open};
use crate::app::PulsarApp;

/// Width of the collapsed icon rail.
pub(crate) const RAIL_WIDTH: f32 = 48.;
/// Width of the open sidebar.
pub(crate) const DRAWER_WIDTH: f32 = 272.;
const ROW_HEIGHT: f32 = 28.;

fn tab_icon(tab: &SidebarTab) -> IconName {
    tab.icon.clone().unwrap_or(match tab.key {
        super::model::TabKey::Panel(_) => IconName::Globe,
        super::model::TabKey::File(_) => IconName::Page,
    })
}

impl PulsarApp {
    /// `editor_area` (the dock and its overlays) with the sidebar beside it, or
    /// unchanged while the sidebar is off.
    pub(crate) fn with_nav_sidebar(
        &mut self,
        editor_area: gpui::Div,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        if !enabled() {
            return editor_area.into_any_element();
        }
        let tabs = self.sidebar_tabs(cx);
        let sections = self.state.nav_sidebar.model.sections(&tabs);
        let keep_open = pinned_open();
        let overlay_open = !keep_open && self.state.nav_sidebar.hover.is_open();

        h_flex()
            .flex_1()
            .min_h_0()
            .w_full()
            .items_stretch()
            .child(if keep_open {
                self.render_sidebar_drawer(&sections, false, window, cx)
            } else {
                self.render_sidebar_rail(&sections, cx)
            })
            .child(
                editor_area
                    .h_full()
                    .min_w_0()
                    .debug_selector(|| "nav-sidebar-editor-area".into())
                    .when(overlay_open, |area| {
                        area.child(self.render_sidebar_drawer(&sections, true, window, cx))
                    }),
            )
            .into_any_element()
    }

    fn render_sidebar_rail(&self, sections: &[TabSection], cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        let drawer_open = self.state.drawer_open;
        let mut icons = v_flex()
            .id("nav-rail-tabs")
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .items_center()
            .gap_1()
            .py_1();
        for (ix, section) in sections.iter().enumerate() {
            if ix > 0 {
                icons = icons.child(div().w(px(20.)).h(px(1.)).my_1().bg(theme.sidebar_border));
            }
            for tab in &section.tabs {
                icons = icons.child(self.render_rail_tab(tab, cx));
            }
        }

        v_flex()
            .id("nav-rail")
            .debug_selector(|| "nav-sidebar-rail".into())
            .w(px(RAIL_WIDTH))
            .h_full()
            .flex_none()
            .items_center()
            .bg(theme.sidebar)
            .border_r_1()
            .border_color(theme.sidebar_border)
            .on_hover(cx.listener(|app, hovered: &bool, _, cx| {
                let action = app.state.nav_sidebar.hover.set_rail(*hovered);
                app.sidebar_hover(action, cx);
            }))
            .child(
                div().py_2().child(
                    Button::new("nav-rail-expand")
                        .ghost()
                        .small()
                        .icon(IconName::PanelLeftOpen)
                        .tooltip("Keep the sidebar open")
                        .on_click(
                            cx.listener(|app, _, _, cx| app.set_sidebar_pinned_open(true, cx)),
                        ),
                ),
            )
            .child(icons)
            .child(
                div().py_2().child(
                    Button::new("nav-rail-assets")
                        .ghost()
                        .small()
                        .icon(Icon::new(IconName::FolderOpen).text_color(if drawer_open {
                            theme.primary
                        } else {
                            theme.muted_foreground
                        }))
                        .tooltip("Assets of the selected folder (Ctrl+Space)")
                        .on_click(cx.listener(|app, _, window, cx| app.toggle_drawer(window, cx))),
                ),
            )
            .into_any_element()
    }

    fn render_rail_tab(&self, tab: &SidebarTab, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        let title: SharedString = tab.title.clone().into();
        let row = tab.clone();
        div()
            .id(ElementId::Name(
                format!("nav-rail-tab-{:?}", tab.key).into(),
            ))
            .relative()
            .size(px(32.))
            .flex()
            .items_center()
            .justify_center()
            .rounded(theme.radius)
            .cursor_pointer()
            .when(tab.active, |el| el.bg(theme.sidebar_accent))
            .hover(|el| el.bg(theme.list_hover))
            .when(!tab.is_open(), |el| el.opacity(0.5))
            .child(
                Icon::new(tab_icon(tab))
                    .size(px(16.))
                    .text_color(if tab.active {
                        theme.primary
                    } else {
                        theme.muted_foreground
                    }),
            )
            .when(tab.unsaved, |el| {
                el.child(
                    unsaved_dot(theme.warning)
                        .absolute()
                        .top(px(5.))
                        .right(px(5.)),
                )
            })
            .tooltip(move |window, cx| Tooltip::new(title.clone()).build(window, cx))
            .on_click(
                cx.listener(move |app, _, window, cx| app.activate_sidebar_tab(&row, window, cx)),
            )
            .into_any_element()
    }

    fn render_sidebar_drawer(
        &self,
        sections: &[TabSection],
        overlay: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let theme = cx.theme().clone();
        let keep_open = !overlay;

        let header = h_flex()
            .h(px(36.))
            .flex_none()
            .px_3()
            .justify_between()
            .border_b_1()
            .border_color(theme.sidebar_border)
            .child(section_caption("Editors", cx))
            .child(
                Button::new("nav-drawer-keep-open")
                    .ghost()
                    .xsmall()
                    .icon(if keep_open {
                        IconName::PinSlash
                    } else {
                        IconName::Pin
                    })
                    .tooltip(if keep_open {
                        "Open the sidebar on hover instead"
                    } else {
                        "Keep the sidebar open"
                    })
                    .on_click(cx.listener(move |app, _, _, cx| {
                        app.set_sidebar_pinned_open(!keep_open, cx)
                    })),
            );

        let mut body = v_flex()
            .id("nav-drawer-body")
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .py_1();
        for section in sections {
            body = body.child(self.render_section(section, cx));
        }
        body = body.child(self.render_content_tree(window, cx));

        v_flex()
            .id("nav-drawer")
            .debug_selector(|| "nav-sidebar-drawer".into())
            .w(px(DRAWER_WIDTH))
            .h_full()
            .flex_none()
            .bg(theme.sidebar)
            .border_r_1()
            .border_color(theme.sidebar_border)
            .when(overlay, |el| {
                el.absolute()
                    .top_0()
                    .left_0()
                    .bottom_0()
                    .shadow_xl()
                    .occlude()
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            })
            .on_hover(cx.listener(|app, hovered: &bool, _, cx| {
                let action = app.state.nav_sidebar.hover.set_drawer(*hovered);
                app.sidebar_hover(action, cx);
            }))
            .child(header)
            .child(body)
            .into_any_element()
    }

    fn render_section(&self, section: &TabSection, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        let id = section.id.clone();
        let label = section.label.clone();
        let count = section.tabs.len();
        let pinned = section.id == SectionId::Pinned;

        let header = h_flex()
            .id(ElementId::Name(
                format!("nav-section-{:?}", section.id).into(),
            ))
            .h(px(24.))
            .mt_1()
            .px_2()
            .gap_1()
            .cursor_pointer()
            .text_xs()
            .text_color(theme.muted_foreground)
            .hover(|el| el.text_color(theme.sidebar_foreground))
            .child(
                Icon::new(if section.collapsed {
                    IconName::ChevronRight
                } else {
                    IconName::ChevronDown
                })
                .size(px(12.)),
            )
            .when(pinned, |el| {
                el.child(Icon::new(IconName::Pin).size(px(11.)))
            })
            .child(
                div()
                    .flex_1()
                    .truncate()
                    .font_weight(gpui::FontWeight::MEDIUM)
                    .child(label),
            )
            .child(div().px_1().child(count.to_string()))
            .on_click(cx.listener(move |app, _, _, cx| app.toggle_sidebar_section(&id, cx)));

        let mut column = v_flex().px_1().child(header);
        if !section.collapsed {
            for tab in &section.tabs {
                column = column.child(self.render_tab_row(tab, cx));
            }
        }
        column.into_any_element()
    }

    fn render_tab_row(&self, tab: &SidebarTab, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        let pinned = self.state.nav_sidebar.model.is_pinned(&tab.key);
        let group: SharedString = format!("nav-tab-{:?}", tab.key).into();
        let activate = tab.clone();
        let pin_key = tab.key.clone();
        let close_index = tab.index;

        h_flex()
            .id(ElementId::Name(group.clone()))
            .group(group.clone())
            .h(px(ROW_HEIGHT))
            .pl(px(22.))
            .pr_1()
            .gap_2()
            .rounded(theme.radius)
            .cursor_pointer()
            .text_sm()
            .text_color(theme.sidebar_foreground)
            .when(tab.active, |el| {
                el.bg(theme.sidebar_accent)
                    .text_color(theme.sidebar_accent_foreground)
            })
            .when(!tab.active, |el| el.hover(|el| el.bg(theme.list_hover)))
            .when(!tab.is_open(), |el| el.text_color(theme.muted_foreground))
            .child(
                Icon::new(tab_icon(tab))
                    .size(px(14.))
                    .text_color(if tab.active {
                        theme.primary
                    } else {
                        theme.muted_foreground
                    }),
            )
            .child(div().flex_1().min_w_0().truncate().child(tab.title.clone()))
            .when(tab.unsaved, |el| el.child(unsaved_dot(theme.warning)))
            .child(
                h_flex()
                    .gap_0p5()
                    .invisible()
                    .group_hover(group.clone(), |el| el.visible())
                    .child(
                        Button::new(ElementId::Name(format!("{group}-pin").into()))
                            .ghost()
                            .xsmall()
                            .icon(if pinned {
                                IconName::PinSlash
                            } else {
                                IconName::Pin
                            })
                            .tooltip(if pinned { "Unpin" } else { "Pin" })
                            .on_click(cx.listener(move |app, _, _, cx| {
                                cx.stop_propagation();
                                app.toggle_sidebar_pin(&pin_key, cx);
                            })),
                    )
                    .when_some(close_index, |el, index| {
                        el.child(
                            Button::new(ElementId::Name(format!("{group}-close").into()))
                                .ghost()
                                .xsmall()
                                .icon(IconName::Close)
                                .tooltip("Close")
                                .on_click(cx.listener(move |app, _, window, cx| {
                                    cx.stop_propagation();
                                    app.close_sidebar_tab(index, window, cx);
                                })),
                        )
                    }),
            )
            .on_click(cx.listener(move |app, _, window, cx| {
                app.activate_sidebar_tab(&activate, window, cx)
            }))
            .into_any_element()
    }

    fn render_content_tree(&self, _window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        let drawer = self.state.file_manager_drawer.read(cx);
        let selected = drawer.selected_folder().map(|p| p.to_path_buf());
        let rows: Vec<FolderRow> = drawer
            .folder_tree()
            .map(|tree| {
                let root = content_root(tree);
                self.state.nav_sidebar.model.folder_rows(root)
            })
            .unwrap_or_default();
        let root = drawer.folder_tree().map(|tree| {
            (
                content_root(tree).path.clone(),
                content_root(tree).name.clone(),
            )
        });

        let mut column = v_flex()
            .px_1()
            .mt_2()
            .pt_1()
            .border_t_1()
            .border_color(theme.sidebar_border)
            .child(
                h_flex()
                    .h(px(24.))
                    .px_2()
                    .child(section_caption("Content", cx)),
            );

        if let Some((root_path, root_name)) = root {
            let is_selected = selected.as_deref() == Some(root_path.as_path());
            column = column.child(folder_row(
                &FolderRow {
                    path: root_path,
                    name: root_name,
                    depth: 0,
                    has_children: false,
                    expanded: true,
                },
                is_selected,
                true,
                cx,
            ));
        }
        for row in &rows {
            let is_selected = selected.as_deref() == Some(row.path.as_path());
            column = column.child(folder_row(row, is_selected, false, cx));
        }
        if rows.is_empty() {
            column = column.child(
                div()
                    .px_3()
                    .py_1()
                    .text_xs()
                    .text_color(theme.muted_foreground)
                    .child("No folders yet"),
            );
        }
        column.into_any_element()
    }
}

/// A folder in the content tree. Clicking lists its assets; the chevron
/// expands it.
fn folder_row(
    row: &FolderRow,
    selected: bool,
    is_root: bool,
    cx: &mut Context<PulsarApp>,
) -> AnyElement {
    let theme = cx.theme().clone();
    let path = row.path.clone();
    let toggle_path = row.path.clone();
    // The root row sits level with the top-level folders' chevrons.
    let indent = if is_root {
        0.
    } else {
        12. * (row.depth as f32 + 1.)
    };

    h_flex()
        .id(ElementId::Name(
            format!("nav-folder-{}", row.path.display()).into(),
        ))
        .h(px(ROW_HEIGHT - 2.))
        .pl(px(8. + indent))
        .pr_2()
        .gap_1()
        .rounded(theme.radius)
        .cursor_pointer()
        .text_sm()
        .text_color(theme.sidebar_foreground)
        .when(selected, |el| {
            el.bg(theme.sidebar_accent)
                .text_color(theme.sidebar_accent_foreground)
        })
        .when(!selected, |el| el.hover(|el| el.bg(theme.list_hover)))
        .child(
            div()
                .id(ElementId::Name(
                    format!("nav-folder-toggle-{}", row.path.display()).into(),
                ))
                .size(px(14.))
                .flex()
                .items_center()
                .justify_center()
                .when(row.has_children, |el| {
                    el.child(
                        Icon::new(if row.expanded {
                            IconName::ChevronDown
                        } else {
                            IconName::ChevronRight
                        })
                        .size(px(12.))
                        .text_color(theme.muted_foreground),
                    )
                    .on_click(cx.listener(move |app, _, _, cx| {
                        cx.stop_propagation();
                        app.toggle_sidebar_folder(&toggle_path, cx);
                    }))
                }),
        )
        .child(
            Icon::new(if selected || (row.expanded && row.has_children) {
                IconName::FolderOpen
            } else {
                IconName::Folder
            })
            .size(px(14.))
            .text_color(if selected {
                theme.primary
            } else {
                theme.muted_foreground
            }),
        )
        .child(div().flex_1().min_w_0().truncate().child(row.name.clone()))
        .on_click(cx.listener(move |app, _, _, cx| app.show_sidebar_folder(path.clone(), cx)))
        .into_any_element()
}

fn section_caption(text: &'static str, cx: &Context<PulsarApp>) -> impl IntoElement {
    div()
        .text_xs()
        .font_weight(gpui::FontWeight::SEMIBOLD)
        .text_color(cx.theme().muted_foreground)
        .child(text.to_uppercase())
}

fn unsaved_dot(color: Hsla) -> gpui::Div {
    div().size(px(6.)).flex_none().rounded_full().bg(color)
}
