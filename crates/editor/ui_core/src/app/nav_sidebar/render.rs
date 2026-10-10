//! Drawing the unified sidebar: the icon rail, and the drawer that opens over
//! the editor on hover (or sits beside it when kept open). Rows and rail icons
//! drag like tabs; group headers take dropped tabs; a row's context menu pins,
//! groups and closes. The content tree is drawn by [`super::content`].

use gpui::{
    div, prelude::*, px, AnyElement, Context, ElementId, Hsla, IntoElement,
    MouseButton, Pixels, SharedString, Window,
};
use ui::button::{Button, ButtonVariants as _};
use ui::dock::DragPanel;
use ui::input::TextInput;
use ui::menu::context_menu::ContextMenuExt as _;
use ui::menu::PopupMenuItem;
use ui::tooltip::Tooltip;
use ui::{h_flex, v_flex, ActiveTheme as _, Icon, IconName, Sizable as _};

use super::model::{SectionId, SidebarTab, TabSection};
use super::{pinned_open, NavSidebar};

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

/// What the sidebar shows of the editor, read once per render.
pub(super) struct EditorView {
    pub tabs: Vec<SidebarTab>,
    /// A tab drag for each entry of `tabs`, when it is open.
    pub drags: Vec<Option<DragPanel>>,
    pub drawer_open: bool,
    /// How far above the bottom the hover drawer stops: the floating file
    /// drawer's height, so it does not cover the assets.
    pub overlay_bottom: Pixels,
}

impl Render for NavSidebar {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        #[cfg(test)]
        {
            self.renders += 1;
        }
        let Some(view) = self.editor_view(cx) else {
            return div().into_any_element();
        };
        let sections = self.model.sections(&view.tabs);
        if pinned_open() {
            self.render_sidebar_drawer(&sections, &view, false, window, cx)
        } else {
            self.render_sidebar_rail(&sections, &view, cx)
                .into_any_element()
        }
    }
}

/// The drawer that opens over the editor while the rail is hovered.
///
/// A view of its own, drawn in the editor area after the editor so it lies on
/// top, rather than a `deferred` overlay inside the sidebar: a cached view
/// replaying a deferred draw that itself opened deferred draws (a right-click
/// menu) replays the wrong ones, and this keeps the sidebar cacheable.
pub struct NavSidebarOverlay {
    sidebar: gpui::Entity<NavSidebar>,
}

impl NavSidebarOverlay {
    pub fn new(sidebar: gpui::Entity<NavSidebar>) -> Self {
        Self { sidebar }
    }
}

impl Render for NavSidebarOverlay {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let drawer = self.sidebar.update(cx, |sidebar, cx| {
            if pinned_open() || !sidebar.hover.is_open() {
                return None;
            }
            let view = sidebar.editor_view(cx)?;
            let sections = sidebar.model.sections(&view.tabs);
            let drawer = sidebar.render_sidebar_drawer(&sections, &view, true, window, cx);
            Some((drawer, view.overlay_bottom))
        });
        match drawer {
            Some((drawer, bottom)) => div()
                .absolute()
                .top_0()
                .left_0()
                .bottom(bottom)
                .child(drawer)
                .into_any_element(),
            None => div().into_any_element(),
        }
    }
}

impl NavSidebar {
    /// What the sidebar shows of the editor, or `None` once it is gone.
    fn editor_view(&self, cx: &gpui::App) -> Option<EditorView> {
        let app = self.app.upgrade()?;
        let app = app.read(cx);
        let open = app.center_tab_list(cx);
        let drags = open
            .iter()
            .map(|open| ui::dock::TabPanel::tab_drag(&open.tabs, open.local_ix, cx))
            .collect();
        let state = &app.state;
        Some(EditorView {
            tabs: app.sidebar_tabs(cx),
            drags,
            drawer_open: state.drawer_open,
            overlay_bottom: if state.drawer_open && !state.drawer_docked {
                px(state.drawer_height)
            } else {
                px(0.)
            },
        })
    }
}

impl NavSidebar {
    fn render_sidebar_rail(
        &self,
        sections: &[TabSection],
        view: &EditorView,
        cx: &mut Context<Self>,
    ) -> gpui::Stateful<gpui::Div> {
        let theme = cx.theme().clone();
        let drawer_open = view.drawer_open;
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
                icons = icons.child(self.render_rail_tab(tab, drag_of(view, tab), cx));
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
            .on_hover(cx.listener(|sidebar, hovered: &bool, _, cx| {
                sidebar.hover_rail(*hovered, cx)
            }))
            .child(
                div().py_2().child(
                    Button::new("nav-rail-expand")
                        .ghost()
                        .small()
                        .icon(IconName::PanelLeftOpen)
                        .tooltip("Keep the sidebar open")
                        .on_click(
                            cx.listener(|sidebar, _, _, cx| sidebar.set_pinned_open(true, cx)),
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
                        .on_click(cx.listener(|sidebar, _, window, cx| {
                            sidebar.toggle_file_drawer(window, cx)
                        })),
                ),
            )
    }

    fn render_rail_tab(
        &self,
        tab: &SidebarTab,
        drag: Option<DragPanel>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
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
            .when_some(drag, |el, drag| el.on_drag(drag, drag_preview))
            .on_click(
                cx.listener(move |sidebar, _, window, cx| sidebar.activate_tab(&row, window, cx)),
            )
            .into_any_element()
    }

    fn render_sidebar_drawer(
        &self,
        sections: &[TabSection],
        view: &EditorView,
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
                h_flex()
                    .gap_0p5()
                    .child(
                        Button::new("nav-drawer-new-group")
                            .ghost()
                            .xsmall()
                            .icon(IconName::FolderPlus)
                            .tooltip("New group")
                            .on_click(cx.listener(|sidebar, _, window, cx| {
                                sidebar.new_group(None, window, cx)
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
                            .on_click(cx.listener(move |sidebar, _, _, cx| {
                                sidebar.set_pinned_open(!keep_open, cx)
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
            body = body.child(self.render_section(section, view, cx));
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
            .h_full()
            .when(overlay, |el| {
                el.border_b_1()
                    .shadow_xl()
                    .occlude()
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            })
            .on_hover(cx.listener(|sidebar, hovered: &bool, _, cx| {
                sidebar.hover_drawer(*hovered, cx)
            }))
            .child(header)
            .child(body)
            .into_any_element()
    }

    fn render_section(
        &self,
        section: &TabSection,
        view: &EditorView,
        cx: &mut Context<Self>,
    ) -> AnyElement {
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
        let rename_field = custom.and_then(|id| self.rename_field(id).cloned());
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
            .on_drop(cx.listener(move |sidebar, drag: &DragPanel, _, cx| {
                sidebar.drop_on_section(&drop_id, drag, cx)
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
                                .on_click(cx.listener(move |sidebar, _, window, cx| {
                                    cx.stop_propagation();
                                    sidebar.start_group_rename(group, window, cx);
                                })),
                        )
                        .child(
                            Button::new(ElementId::Name(format!("{header_group}-delete").into()))
                                .ghost()
                                .xsmall()
                                .icon(IconName::Trash)
                                .tooltip("Remove group (its tabs stay open)")
                                .on_click(cx.listener(move |sidebar, _, _, cx| {
                                    cx.stop_propagation();
                                    sidebar.delete_group(group, cx);
                                })),
                        ),
                )
            })
            .child(div().px_1().child(count.to_string()))
            .on_click(
                cx.listener(move |sidebar, event: &gpui::ClickEvent, window, cx| {
                    match (custom, event.click_count()) {
                        (Some(group), 2) => sidebar.start_group_rename(group, window, cx),
                        _ => sidebar.toggle_section(&id, cx),
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
                column = column.child(self.render_tab_row(tab, drag_of(view, tab), cx));
            }
        }
        column.into_any_element()
    }

    fn render_tab_row(
        &self,
        tab: &SidebarTab,
        drag: Option<DragPanel>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let theme = cx.theme().clone();
        let pinned = self.model.is_pinned(&tab.key);
        let group: SharedString = format!("nav-tab-{:?}", tab.key).into();
        let activate = tab.clone();
        let pin_key = tab.key.clone();
        let close_index = tab.index;
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
                            .on_click(cx.listener(move |sidebar, _, _, cx| {
                                cx.stop_propagation();
                                sidebar.toggle_pin(&pin_key, cx);
                            })),
                    )
                    .when_some(close_index, |el, index| {
                        el.child(
                            Button::new(ElementId::Name(format!("{group}-close").into()))
                                .ghost()
                                .xsmall()
                                .icon(IconName::Close)
                                .tooltip("Close")
                                .on_click(cx.listener(move |sidebar, _, window, cx| {
                                    cx.stop_propagation();
                                    sidebar.close_tab(index, window, cx);
                                })),
                        )
                    }),
            )
            .when_some(drag, |el, drag| el.on_drag(drag, drag_preview))
            .on_click(cx.listener(move |sidebar, _, window, cx| {
                sidebar.activate_tab(&activate, window, cx)
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
        let sidebar = cx.entity().downgrade();
        let model = &self.model;
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

        move |mut menu, _, cx| {
            hold_while_open(&sidebar, cx);
            let k = key.clone();
            menu = menu.item(
                PopupMenuItem::new(if pinned { "Unpin" } else { "Pin" })
                    .on_click(on(&sidebar, move |sidebar, _, cx| sidebar.toggle_pin(&k, cx))),
            );
            menu = menu.separator();
            let k = key.clone();
            menu = menu.item(
                PopupMenuItem::new("New group with this editor").on_click(on(
                    &sidebar,
                    move |sidebar, window, cx| sidebar.new_group(Some(k.clone()), window, cx),
                )),
            );
            for (id, name) in &groups {
                let k = key.clone();
                let id = *id;
                menu = menu.item(
                    PopupMenuItem::new(format!("Move to {name}"))
                        .on_click(on(&sidebar, move |sidebar, _, cx| {
                            sidebar.move_tab(&k, Some(id), cx)
                        })),
                );
            }
            if current.is_some() {
                let k = key.clone();
                menu = menu.item(
                    PopupMenuItem::new("Remove from group")
                        .on_click(on(&sidebar, move |sidebar, _, cx| sidebar.move_tab(&k, None, cx))),
                );
            }
            if let Some(index) = close_index {
                menu = menu.separator().item(
                    PopupMenuItem::new("Close").on_click(on(&sidebar, move |sidebar, window, cx| {
                        sidebar.close_tab(index, window, cx)
                    })),
                );
            }
            menu
        }
    }
}

/// The tab drag for `tab`, when it is open.
fn drag_of(view: &EditorView, tab: &SidebarTab) -> Option<DragPanel> {
    tab.index
        .and_then(|index| view.drags.get(index).cloned().flatten())
}

/// A menu item's click handler that runs `f` on the sidebar.
pub(super) fn on(
    sidebar: &gpui::WeakEntity<NavSidebar>,
    f: impl Fn(&mut NavSidebar, &mut Window, &mut Context<NavSidebar>) + 'static,
) -> impl Fn(&gpui::ClickEvent, &mut Window, &mut gpui::App) + 'static {
    let sidebar = sidebar.clone();
    move |_, window, cx| {
        _ = sidebar.update(cx, |sidebar, cx| f(sidebar, window, cx));
    }
}

/// Keep the hover drawer open while the menu being built is up: the pointer
/// leaves the drawer for the menu.
pub(super) fn hold_while_open(
    sidebar: &gpui::WeakEntity<NavSidebar>,
    cx: &mut Context<ui::popup_menu::PopupMenu>,
) {
    _ = sidebar.update(cx, |sidebar, cx| sidebar.hold_open(true, cx));
    let sidebar = sidebar.clone();
    cx.subscribe_self(move |_, _: &gpui::DismissEvent, cx| {
        _ = sidebar.update(cx, |sidebar, cx| sidebar.hold_open(false, cx));
    })
    .detach();
}

pub(super) fn section_caption(text: &'static str, cx: &Context<NavSidebar>) -> impl IntoElement {
    div()
        .text_xs()
        .font_weight(gpui::FontWeight::SEMIBOLD)
        .text_color(cx.theme().muted_foreground)
        .child(text.to_uppercase())
}

/// What follows the pointer while a row or rail icon is dragged: the dock's
/// own tab preview, so the drop targets treat it as a dragged tab.
pub(super) fn drag_preview(
    drag: &DragPanel,
    position: gpui::Point<gpui::Pixels>,
    _: &mut Window,
    cx: &mut gpui::App,
) -> gpui::Entity<DragPanel> {
    let drag = drag.clone().with_start_position(position);
    cx.new(|_| drag)
}

pub(super) fn unsaved_dot(color: Hsla) -> gpui::Div {
    div().size(px(6.)).flex_none().rounded_full().bg(color)
}
