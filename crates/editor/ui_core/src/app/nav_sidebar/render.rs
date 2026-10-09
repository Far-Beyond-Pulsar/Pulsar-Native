//! Drawing the unified sidebar: the icon rail, and the drawer that opens over
//! the editor on hover (or sits beside it when kept open). Rows and rail icons
//! drag like tabs; group headers take dropped tabs; a row's context menu pins,
//! groups and closes.

use gpui::{
    div, prelude::*, px, AnyElement, Context, ElementId, Hsla, IntoElement, MouseButton,
    SharedString, Window,
};
use ui::button::{Button, ButtonVariants as _};
use ui::dock::DragPanel;
use ui::input::TextInput;
use ui::menu::context_menu::ContextMenuExt as _;
use ui::menu::PopupMenuItem;
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
        let drag = tab.index.and_then(|ix| self.sidebar_tab_drag(ix, cx));
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
            .when_some(drag, |el, drag| el.on_drag(drag, drag_preview))
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
        // Over the editor, stop above the floating file drawer rather than
        // cover the left of its assets.
        let bottom = if overlay && self.state.drawer_open && !self.state.drawer_docked {
            px(self.state.drawer_height)
        } else {
            px(0.)
        };

        let header = h_flex()
            .h(px(36.))
            .flex_none()
            .px_3()
            .justify_between()
            .border_b_1()
            .border_color(theme.sidebar_border)
            .child(section_caption("Editors", cx))
            .child(
                h_flex()
                    .gap_0p5()
                    .child(
                        Button::new("nav-drawer-new-group")
                            .ghost()
                            .xsmall()
                            .icon(IconName::FolderPlus)
                            .tooltip("New group")
                            .on_click(cx.listener(|app, _, window, cx| {
                                app.new_sidebar_group(None, window, cx)
                            })),
                    )
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
                    ),
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
            .flex_none()
            .bg(theme.sidebar)
            .border_r_1()
            .border_color(theme.sidebar_border)
            .when(!overlay, |el| el.h_full())
            .when(overlay, |el| {
                el.absolute()
                    .top_0()
                    .left_0()
                    .bottom(bottom)
                    .border_b_1()
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
        let drop_id = section.id.clone();
        let label = section.label.clone();
        let count = section.tabs.len();
        let pinned = section.id == SectionId::Pinned;
        let custom = match section.id {
            SectionId::Custom(id) => Some(id),
            _ => None,
        };
        let rename_field = custom.and_then(|id| self.state.nav_sidebar.rename_field(id).cloned());
        let header_group: SharedString = format!("nav-section-{:?}", section.id).into();

        let title = match &rename_field {
            Some(field) => TextInput::new(field).xsmall().into_any_element(),
            None => div()
                .flex_1()
                .truncate()
                .font_weight(gpui::FontWeight::MEDIUM)
                .child(label)
                .into_any_element(),
        };

        let header = h_flex()
            .id(ElementId::Name(header_group.clone()))
            .group(header_group.clone())
            .h(px(24.))
            .mt_1()
            .px_2()
            .gap_1()
            .rounded(theme.radius)
            .cursor_pointer()
            .text_xs()
            .text_color(theme.muted_foreground)
            .hover(|el| el.text_color(theme.sidebar_foreground))
            // Dropping a dragged tab here moves it into this section.
            .drag_over::<DragPanel>({
                let (bg, fg) = (theme.drop_target, theme.sidebar_foreground);
                move |style, _, _, _| style.bg(bg).text_color(fg)
            })
            .on_drop(cx.listener(move |app, drag: &DragPanel, _, cx| {
                app.drop_on_sidebar_section(&drop_id, drag, cx)
            }))
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
            .when(custom.is_some(), |el| {
                el.child(Icon::new(IconName::Group).size(px(11.)))
            })
            .child(title)
            .when_some(custom.filter(|_| rename_field.is_none()), |el, group| {
                el.child(
                    h_flex()
                        .invisible()
                        .group_hover(header_group.clone(), |el| el.visible())
                        .child(
                            Button::new(ElementId::Name(format!("{header_group}-rename").into()))
                                .ghost()
                                .xsmall()
                                .icon(IconName::Edit)
                                .tooltip("Rename group")
                                .on_click(cx.listener(move |app, _, window, cx| {
                                    cx.stop_propagation();
                                    app.start_sidebar_group_rename(group, window, cx);
                                })),
                        )
                        .child(
                            Button::new(ElementId::Name(format!("{header_group}-delete").into()))
                                .ghost()
                                .xsmall()
                                .icon(IconName::Trash)
                                .tooltip("Remove group (its tabs stay open)")
                                .on_click(cx.listener(move |app, _, _, cx| {
                                    cx.stop_propagation();
                                    app.delete_sidebar_group(group, cx);
                                })),
                        ),
                )
            })
            .child(div().px_1().child(count.to_string()))
            .on_click(
                cx.listener(move |app, event: &gpui::ClickEvent, window, cx| {
                    match (custom, event.click_count()) {
                        (Some(group), 2) => app.start_sidebar_group_rename(group, window, cx),
                        _ => app.toggle_sidebar_section(&id, cx),
                    }
                }),
            );

        let mut column = v_flex().px_1().child(header);
        if !section.collapsed {
            if section.tabs.is_empty() && custom.is_some() {
                column = column.child(
                    div()
                        .pl(px(22.))
                        .py_1()
                        .text_xs()
                        .text_color(theme.muted_foreground)
                        .child("Drag editors here"),
                );
            }
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
        let drag = tab.index.and_then(|ix| self.sidebar_tab_drag(ix, cx));
        let menu = self.tab_row_menu(tab, cx);

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
            .when_some(drag, |el, drag| el.on_drag(drag, drag_preview))
            .on_click(cx.listener(move |app, _, window, cx| {
                app.activate_sidebar_tab(&activate, window, cx)
            }))
            .context_menu(menu)
            .into_any_element()
    }

    /// A row's right-click menu: pin, group and close.
    fn tab_row_menu(
        &self,
        tab: &SidebarTab,
        cx: &mut Context<Self>,
    ) -> impl Fn(
        ui::popup_menu::PopupMenu,
        &mut Window,
        &mut Context<ui::popup_menu::PopupMenu>,
    ) -> ui::popup_menu::PopupMenu
           + 'static {
        let app = cx.entity().downgrade();
        let model = &self.state.nav_sidebar.model;
        let key = tab.key.clone();
        let pinned = model.is_pinned(&key);
        let current = model.group_of(&key);
        let groups: Vec<(u32, String)> = model
            .groups()
            .iter()
            .filter(|g| Some(g.id) != current)
            .map(|g| (g.id, g.name.clone()))
            .collect();
        let close_index = tab.index;

        move |mut menu, _, _| {
            let on = |app: &gpui::WeakEntity<PulsarApp>,
                      f: std::rc::Rc<
                dyn Fn(&mut PulsarApp, &mut Window, &mut Context<PulsarApp>),
            >| {
                let app = app.clone();
                move |_: &gpui::ClickEvent, window: &mut Window, cx: &mut gpui::App| {
                    let f = f.clone();
                    _ = app.update(cx, |app, cx| f(app, window, cx));
                }
            };
            let k = key.clone();
            menu = menu.item(
                PopupMenuItem::new(if pinned { "Unpin" } else { "Pin" }).on_click(on(
                    &app,
                    std::rc::Rc::new(move |app, _, cx| app.toggle_sidebar_pin(&k, cx)),
                )),
            );
            menu = menu.separator();
            let k = key.clone();
            menu = menu.item(
                PopupMenuItem::new("New group with this editor").on_click(on(
                    &app,
                    std::rc::Rc::new(move |app, window, cx| {
                        app.new_sidebar_group(Some(k.clone()), window, cx)
                    }),
                )),
            );
            for (id, name) in &groups {
                let k = key.clone();
                let id = *id;
                menu = menu.item(PopupMenuItem::new(format!("Move to {name}")).on_click(on(
                    &app,
                    std::rc::Rc::new(move |app, _, cx| app.move_sidebar_tab(&k, Some(id), cx)),
                )));
            }
            if current.is_some() {
                let k = key.clone();
                menu = menu.item(PopupMenuItem::new("Remove from group").on_click(on(
                    &app,
                    std::rc::Rc::new(move |app, _, cx| app.move_sidebar_tab(&k, None, cx)),
                )));
            }
            if let Some(index) = close_index {
                menu = menu
                    .separator()
                    .item(PopupMenuItem::new("Close").on_click(on(
                        &app,
                        std::rc::Rc::new(move |app, window, cx| {
                            app.close_sidebar_tab(index, window, cx)
                        }),
                    )));
            }
            menu
        }
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

/// What follows the pointer while a row or rail icon is dragged: the dock's
/// own tab preview, so the drop targets treat it as a dragged tab.
fn drag_preview(
    drag: &DragPanel,
    position: gpui::Point<gpui::Pixels>,
    _: &mut Window,
    cx: &mut gpui::App,
) -> gpui::Entity<DragPanel> {
    let drag = drag.clone().with_start_position(position);
    cx.new(|_| drag)
}

fn unsaved_dot(color: Hsla) -> gpui::Div {
    div().size(px(6.)).flex_none().rounded_full().bg(color)
}
