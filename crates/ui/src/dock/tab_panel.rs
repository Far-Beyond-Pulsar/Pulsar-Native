use std::sync::Arc;

use gpui::{
    div, prelude::FluentBuilder, px, relative, rems, size, App, AppContext, Bounds, Context, Corner,
    DismissEvent, Div, DragMoveEvent, Empty, Entity, EventEmitter, FocusHandle, Focusable,
    InteractiveElement as _, IntoElement, ParentElement, Pixels, Point, Render, ScrollHandle,
    SharedString, StatefulInteractiveElement, StyleRefinement, Styled, WeakEntity, Window,
    WindowBounds, WindowKind, WindowOptions,
};
use rust_i18n::t;

use crate::{
    button::{Button, ButtonVariants as _},
    context_menu::ContextMenu,
    dock::PanelInfo,
    h_flex,
    popup_menu::{PopupMenu, PopupMenuExt},
    tab::{Tab, TabBar},
    v_flex, ActiveTheme, AxisExt, IconName, Placement, Selectable, Sizable,
};

use super::{
    ClosePanel, Dock, DockArea, DockItem, DockPlacement, Panel, PanelControl, PanelEvent, PanelState,
    PanelStyle, PanelView, StackPanel, ToggleZoom,
};

#[derive(Clone)]
struct TabState {
    closable: bool,
    zoomable: Option<PanelControl>,
    draggable: bool,
    droppable: bool,
    active_panel: Option<Arc<dyn PanelView>>,
    channel: DockChannel,
}

/// Drag channel identifier - used to isolate different dock systems
/// Each DockArea should use a unique channel to prevent interference
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct DockChannel(pub u32);

impl Default for DockChannel {
    fn default() -> Self {
        DockChannel(0)
    }
}

#[derive(Clone)]
pub(crate) struct DragPanel {
    pub(crate) panel: Arc<dyn PanelView>,
    pub(crate) tab_panel: Entity<TabPanel>,
    pub(crate) source_index: usize,
    pub(crate) drag_start_position: Option<Point<Pixels>>,
    pub(crate) channel: DockChannel,
}

impl DragPanel {
    pub(crate) fn new(panel: Arc<dyn PanelView>, tab_panel: Entity<TabPanel>, channel: DockChannel) -> Self {
        Self {
            panel,
            tab_panel,
            source_index: 0,
            drag_start_position: None,
            channel,
        }
    }

    pub(crate) fn with_index(mut self, index: usize) -> Self {
        self.source_index = index;
        self
    }

    pub(crate) fn with_start_position(mut self, position: Point<Pixels>) -> Self {
        self.drag_start_position = Some(position);
        self
    }
}

impl Render for DragPanel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // Check if we should show "new window" visual feedback
        let is_outside_bounds = self.tab_panel.read(cx).dragging_outside_window;

        div()
            .id("drag-panel")
            .cursor_grab()
            .py_1()
            .px_3()
            .min_w_24()
            .max_w_64()
            .overflow_hidden()
            .whitespace_nowrap()
            .rounded(cx.theme().radius)
            .when(is_outside_bounds, |this| {
                // Visual feedback for creating new window - make it look like a floating window
                this.border_2()
                    .border_color(cx.theme().accent)
                    .text_color(cx.theme().primary_foreground)
                    .bg(cx.theme().accent)
                    .opacity(0.95)
                    .shadow_2xl()
            })
            .when(!is_outside_bounds, |this| {
                // Normal drag appearance - subtle ghost-like
                this.border_1()
                    .border_color(cx.theme().border)
                    .text_color(cx.theme().tab_foreground)
                    .bg(cx.theme().tab_active)
                    .opacity(0.8)
                    .shadow_md()
            })
            .child(self.panel.title(window, cx))
    }
}

pub struct TabPanel {
    focus_handle: FocusHandle,
    dock_area: WeakEntity<DockArea>,
    /// The stock_panel can be None, if is None, that means the panels can't be split or move
    stack_panel: Option<WeakEntity<StackPanel>>,
    pub(crate) panels: Vec<Arc<dyn PanelView>>,
    pub(crate) active_ix: usize,
    /// If this is true, the Panel closable will follow the active panel's closable,
    /// otherwise this TabPanel will not able to close
    ///
    /// This is used for Dock to limit the last TabPanel not able to close, see [`super::Dock::new`].
    pub(crate) closable: bool,

    tab_bar_scroll_handle: ScrollHandle,
    zoomed: bool,
    collapsed: bool,
    /// When drag move, will get the placement of the panel to be split
    will_split_placement: Option<Placement>,
    /// Is TabPanel used in Tiles.
    in_tiles: bool,
    /// Track the index where a dragged tab should be inserted for reordering
    pending_reorder_index: Option<usize>,
    /// Track if we're currently dragging outside window bounds
    dragging_outside_window: bool,
    /// Dock channel - isolates this TabPanel to only interact with same-channel drags
    pub(crate) channel: DockChannel,
    /// Track if we're in a valid same-channel drag (set by drag_over predicate)
    in_valid_drag: bool,
}

impl Panel for TabPanel {
    fn panel_name(&self) -> &'static str {
        "TabPanel"
    }

    fn title(&self, window: &Window, cx: &App) -> gpui::AnyElement {
        self.active_panel(cx)
            .map(|panel| panel.title(window, cx))
            .unwrap_or("Empty Tab".into_any_element())
    }

    fn closable(&self, cx: &App) -> bool {
        if !self.closable {
            return false;
        }

        self.active_panel(cx)
            .map(|panel| panel.closable(cx))
            .unwrap_or(false)
    }

    fn zoomable(&self, cx: &App) -> Option<PanelControl> {
        self.active_panel(cx).and_then(|panel| panel.zoomable(cx))
    }

    fn visible(&self, cx: &App) -> bool {
        self.visible_panels(cx).next().is_some()
    }

    fn popup_menu(&self, menu: PopupMenu, window: &Window, cx: &App) -> PopupMenu {
        if let Some(panel) = self.active_panel(cx) {
            panel.popup_menu(menu, window, cx)
        } else {
            menu
        }
    }

    fn toolbar_buttons(&self, window: &mut Window, cx: &mut App) -> Option<Vec<Button>> {
        self.active_panel(cx)
            .and_then(|panel| panel.toolbar_buttons(window, cx))
    }

    fn dump(&self, cx: &App) -> PanelState {
        let mut state = PanelState::new(self);
        for panel in self.panels.iter() {
            state.add_child(panel.dump(cx));
            state.info = PanelInfo::tabs(self.active_ix);
        }
        state
    }

    fn inner_padding(&self, cx: &App) -> bool {
        self.active_panel(cx)
            .map_or(true, |panel| panel.inner_padding(cx))
    }
}

impl TabPanel {
    pub fn new(
        stack_panel: Option<WeakEntity<StackPanel>>,
        dock_area: WeakEntity<DockArea>,
        channel: DockChannel,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        Self {
            focus_handle: cx.focus_handle(),
            dock_area,
            stack_panel,
            panels: Vec::new(),
            active_ix: 0,
            tab_bar_scroll_handle: ScrollHandle::new(),
            will_split_placement: None,
            zoomed: false,
            collapsed: false,
            closable: true,
            in_tiles: false,
            pending_reorder_index: None,
            dragging_outside_window: false,
            channel,
            in_valid_drag: false,
        }
    }

    /// Returns the index of the panel with the given entity_id, or None if not found.
    pub fn index_of_panel_by_entity_id(&self, entity_id: gpui::EntityId) -> Option<usize> {
        self.panels
            .iter()
            .position(|p| p.view().entity_id() == entity_id)
    }

    /// Mark the TabPanel as being used in Tiles.
    pub(super) fn set_in_tiles(&mut self, in_tiles: bool) {
        self.in_tiles = in_tiles;
    }

    pub(super) fn set_parent(&mut self, view: WeakEntity<StackPanel>) {
        self.stack_panel = Some(view);
    }

    /// Return current active_panel View
    pub fn active_panel(&self, cx: &App) -> Option<Arc<dyn PanelView>> {
        let panel = self.panels.get(self.active_ix);

        if let Some(panel) = panel {
            if panel.visible(cx) {
                Some(panel.clone())
            } else {
                // Return the first visible panel
                self.visible_panels(cx).next()
            }
        } else {
            None
        }
    }

    /// Public method to set the active tab by index.
    pub fn set_active_tab(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) {
        if ix == self.active_ix {
            return;
        }

        let last_active_ix = self.active_ix;

        self.active_ix = ix;
        self.tab_bar_scroll_handle.scroll_to_item(ix);
        self.focus_active_panel(window, cx);

        // Sync the active state to all panels
        cx.spawn_in(window, async move |view, cx| {
            _ = cx.update(|window, cx| {
                _ = view.update(cx, |view, cx| {
                    if let Some(last_active) = view.panels.get(last_active_ix) {
                        last_active.set_active(false, window, cx);
                    }
                    if let Some(active) = view.panels.get(view.active_ix) {
                        active.set_active(true, window, cx);
                    }
                });
            });
        })
        .detach();

        cx.emit(PanelEvent::LayoutChanged);
        cx.emit(PanelEvent::TabChanged { active_index: ix });
        cx.notify();
    }

    /// Add a panel to the end of the tabs
    pub fn add_panel(
        &mut self,
        panel: Arc<dyn PanelView>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.add_panel_with_active(panel, true, window, cx);
    }

    fn add_panel_with_active(
        &mut self,
        panel: Arc<dyn PanelView>,
        active: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        assert_ne!(
            panel.panel_name(cx),
            "StackPanel",
            "can not allows add `StackPanel` to `TabPanel`"
        );

        if self
            .panels
            .iter()
            .any(|p| p.view().entity_id() == panel.view().entity_id())
        {
            return;
        }

        self.panels.push(panel);
        // set the active panel to the new panel
        if active {
            self.set_active_tab(self.panels.len() - 1, window, cx);
        }
        cx.emit(PanelEvent::LayoutChanged);
        cx.notify();
    }

    /// Add panel to try to split
    pub fn add_panel_at(
        &mut self,
        panel: Arc<dyn PanelView>,
        placement: Placement,
        size: Option<Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        cx.spawn_in(window, async move |view, cx| {
            cx.update(|window, cx| {
                view.update(cx, |view, cx| {
                    view.will_split_placement = Some(placement);
                    view.split_panel(panel, placement, size, window, cx)
                })
                .ok()
            })
            .ok()
        })
        .detach();
        cx.emit(PanelEvent::LayoutChanged);
        cx.notify();
    }

    fn insert_panel_at(
        &mut self,
        panel: Arc<dyn PanelView>,
        ix: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self
            .panels
            .iter()
            .any(|p| p.view().entity_id() == panel.view().entity_id())
        {
            return;
        }

        self.panels.insert(ix, panel);
        self.set_active_tab(ix, window, cx);
        cx.emit(PanelEvent::LayoutChanged);
        cx.notify();
    }

    /// Remove a panel from the tab panel
    pub fn remove_panel(
        &mut self,
        panel: Arc<dyn PanelView>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let entity_id = panel.view().entity_id();
        self.detach_panel(panel, window, cx);
        self.remove_self_if_empty(window, cx);
        cx.emit(PanelEvent::TabClosed(entity_id));
        cx.emit(PanelEvent::ZoomOut);
        cx.emit(PanelEvent::LayoutChanged);
    }

    fn detach_panel(
        &mut self,
        panel: Arc<dyn PanelView>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let panel_view = panel.view();
        self.panels.retain(|p| p.view() != panel_view);
        if self.active_ix >= self.panels.len() {
            self.set_active_tab(self.panels.len().saturating_sub(1), window, cx)
        }
    }

    /// Check to remove self from the parent StackPanel, if there is no panel left
    fn remove_self_if_empty(&self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.panels.is_empty() {
            return;
        }

        let tab_view = cx.entity().clone();
        if let Some(stack_panel) = self.stack_panel.as_ref() {
            _ = stack_panel.update(cx, |view, cx| {
                view.remove_panel(Arc::new(tab_view), window, cx);
            });
        }
    }

    pub(super) fn set_collapsed(
        &mut self,
        collapsed: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.collapsed = collapsed;
        if let Some(panel) = self.panels.get(self.active_ix) {
            panel.set_active(!collapsed, window, cx);
        }
        cx.notify();
    }

    fn is_locked(&self, cx: &App) -> bool {
        let Some(dock_area) = self.dock_area.upgrade() else {
            return true;
        };

        if dock_area.read(cx).is_locked() {
            return true;
        }

        if self.zoomed {
            return true;
        }

        self.stack_panel.is_none()
    }

    /// Return true if self or parent only have last panel.
    fn is_last_panel(&self, cx: &App) -> bool {
        if let Some(parent) = &self.stack_panel {
            if let Some(stack_panel) = parent.upgrade() {
                if !stack_panel.read(cx).is_last_panel(cx) {
                    return false;
                }
            }
        }

        self.panels.len() <= 1
    }

    /// Return all visible panels
    fn visible_panels<'a>(&'a self, cx: &'a App) -> impl Iterator<Item = Arc<dyn PanelView>> + 'a {
        self.panels.iter().filter_map(|panel| {
            if panel.visible(cx) {
                Some(panel.clone())
            } else {
                None
            }
        })
    }

    /// Return true if the tab panel is draggable.
    ///
    /// Tabs are draggable unless:
    /// - The dock area is locked
    /// - The panel is zoomed
    /// - It's the last panel in a stack with a parent (prevents breaking the layout)
    ///
    /// Note: Single tabs without a parent stack are draggable to allow creating new windows.
    fn draggable(&self, cx: &App) -> bool {
        let Some(dock_area) = self.dock_area.upgrade() else {
            return false;
        };

        if dock_area.read(cx).is_locked() {
            return false;
        }

        if self.zoomed {
            return false;
        }

        // If we have a stack panel parent, check if we're the last panel
        // (don't allow dragging the last panel as it would break the layout)
        if let Some(parent) = &self.stack_panel {
            if let Some(stack_panel) = parent.upgrade() {
                if stack_panel.read(cx).is_last_panel(cx) && self.panels.len() <= 1 {
                    return false;
                }
            }
        }

        // Allow dragging for all other cases (including single tabs without parent)
        true
    }

    /// Return true if the tab panel is droppable.
    ///
    /// E.g. if the tab panel is locked, it is not droppable.
    fn droppable(&self, cx: &App) -> bool {
        let Some(dock_area) = self.dock_area.upgrade() else {
            return false;
        };

        if dock_area.read(cx).is_locked() {
            return false;
        }

        if self.zoomed {
            return false;
        }

        // Allow dropping on top-level tabs (those without a stack_panel parent)
        // as they are the main dockable areas
        true
    }

    fn render_toolbar(
        &self,
        state: &TabState,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        if self.collapsed {
            return div();
        }

        let zoomed = self.zoomed;
        let view = cx.entity().clone();
        let zoomable_toolbar_visible = state.zoomable.map_or(false, |v| v.toolbar_visible());

        h_flex()
            .gap_1()
            .occlude()
            .when_some(self.toolbar_buttons(window, cx), |this, buttons| {
                this.children(
                    buttons
                        .into_iter()
                        .map(|btn| btn.xsmall().ghost().tab_stop(false)),
                )
            })
            .map(|this| {
                let value = if zoomed {
                    Some(("zoom-out", IconName::Minimize, t!("Dock.Zoom Out")))
                } else if zoomable_toolbar_visible {
                    Some(("zoom-in", IconName::Maximize, t!("Dock.Zoom In")))
                } else {
                    None
                };

                if let Some((id, icon, tooltip)) = value {
                    this.child(
                        Button::new(id)
                            .icon(icon)
                            .xsmall()
                            .ghost()
                            .tab_stop(false)
                            .tooltip_with_action(tooltip, &ToggleZoom, None)
                            .when(zoomed, |this| this.selected(true))
                            .on_click(cx.listener(|view, _, window, cx| {
                                view.on_action_toggle_zoom(&ToggleZoom, window, cx)
                            })),
                    )
                } else {
                    this
                }
            })
            .child(
                Button::new("menu")
                    .icon(IconName::Ellipsis)
                    .xsmall()
                    .ghost()
                    .tab_stop(false)
                    .popup_menu({
                        let zoomable = state.zoomable.map_or(false, |v| v.menu_visible());
                        let closable = state.closable;

                        move |this, window, cx| {
                            view.read(cx)
                                .popup_menu(this, window, cx)
                                .separator()
                                .menu_with_disabled(
                                    if zoomed {
                                        t!("Dock.Zoom Out")
                                    } else {
                                        t!("Dock.Zoom In")
                                    },
                                    Box::new(ToggleZoom),
                                    !zoomable,
                                )
                                .when(closable, |this| {
                                    this.separator()
                                        .menu(t!("Dock.Close"), Box::new(ClosePanel))
                                })
                        }
                    })
                    .anchor(Corner::TopRight),
            )
    }

    fn render_dock_toggle_button(
        &self,
        placement: DockPlacement,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<impl IntoElement> {
        if self.zoomed {
            return None;
        }

        let dock_area = self.dock_area.upgrade()?.read(cx);
        if !dock_area.toggle_button_visible {
            return None;
        }
        if !dock_area.is_dock_collapsible(placement, cx) {
            return None;
        }

        let view_entity_id = cx.entity().entity_id();
        let toggle_button_panels = dock_area.toggle_button_panels;

        // Check if current TabPanel's entity_id matches the one stored in DockArea for this placement
        if !match placement {
            DockPlacement::Left => {
                dock_area.left_dock.is_some() && toggle_button_panels.left == Some(view_entity_id)
            }
            DockPlacement::Right => {
                dock_area.right_dock.is_some() && toggle_button_panels.right == Some(view_entity_id)
            }
            DockPlacement::Bottom => {
                dock_area.bottom_dock.is_some()
                    && toggle_button_panels.bottom == Some(view_entity_id)
            }
            DockPlacement::Center => unreachable!(),
        } {
            return None;
        }

        let is_open = dock_area.is_dock_open(placement, cx);

        let icon = match placement {
            DockPlacement::Left => {
                if is_open {
                    IconName::PanelLeft
                } else {
                    IconName::PanelLeftOpen
                }
            }
            DockPlacement::Right => {
                if is_open {
                    IconName::PanelRight
                } else {
                    IconName::PanelRightOpen
                }
            }
            DockPlacement::Bottom => {
                if is_open {
                    IconName::PanelBottom
                } else {
                    IconName::PanelBottomOpen
                }
            }
            DockPlacement::Center => unreachable!(),
        };

        Some(
            Button::new(SharedString::from(format!("toggle-dock:{:?}", placement)))
                .icon(icon)
                .xsmall()
                .ghost()
                .tab_stop(false)
                .tooltip(match is_open {
                    true => t!("Dock.Collapse"),
                    false => t!("Dock.Expand"),
                })
                .on_click(cx.listener({
                    let dock_area = self.dock_area.clone();
                    move |_, _, window, cx| {
                        _ = dock_area.update(cx, |dock_area, cx| {
                            dock_area.toggle_dock(placement, window, cx);
                        });
                    }
                })),
        )
    }

    fn render_title_bar(
        &self,
        state: &TabState,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let view = cx.entity().clone();

        let Some(dock_area) = self.dock_area.upgrade() else {
            return div().into_any_element();
        };
        let panel_style = dock_area.read(cx).panel_style;

        let left_dock_button = self.render_dock_toggle_button(DockPlacement::Left, window, cx);
        let bottom_dock_button = self.render_dock_toggle_button(DockPlacement::Bottom, window, cx);
        let right_dock_button = self.render_dock_toggle_button(DockPlacement::Right, window, cx);

        let is_bottom_dock = bottom_dock_button.is_some();

        if self.panels.len() == 1 && panel_style == PanelStyle::Default {
            let panel = self.panels.get(0).unwrap();

            if !panel.visible(cx) {
                return div().into_any_element();
            }

            let title_style = panel.title_style(cx);

            return h_flex()
                .justify_between()
                .line_height(rems(1.0))
                .h(px(30.))
                .py_2()
                .pl_3()
                .pr_2()
                .when(left_dock_button.is_some(), |this| this.pl_2())
                .when(right_dock_button.is_some(), |this| this.pr_2())
                .when_some(title_style, |this, theme| {
                    this.bg(theme.background).text_color(theme.foreground)
                })
                .when(title_style.is_none(), |this| {
                    this.bg(cx.theme().tab_bar)
                })
                .when(
                    left_dock_button.is_some() || bottom_dock_button.is_some(),
                    |this| {
                        this.child(
                            h_flex()
                                .flex_shrink_0()
                                .mr_1()
                                .gap_1()
                                .children(left_dock_button)
                                .children(bottom_dock_button),
                        )
                    },
                )
                .child(
                    div()
                        .id("tab")
                        .flex_1()
                        .min_w_16()
                        .overflow_hidden()
                        .text_ellipsis()
                        .whitespace_nowrap()
                        .child(panel.title(window, cx))
                        .when(state.draggable, |this| {
                            let channel = state.channel;
                            this.on_drag(
                                DragPanel::new(panel.clone(), view.clone(), channel)
                                    .with_index(0),
                                move |drag, position, _, cx| {
                                    // Capture the drag start position for window creation
                                    let mut drag_with_pos = drag.clone();
                                    drag_with_pos.drag_start_position = Some(position);
                                    cx.stop_propagation();
                                    cx.new(|_| drag_with_pos)
                                },
                            )
                            .on_drag_move(cx.listener(|this, event: &DragMoveEvent<DragPanel>, window, cx| {
                                // Track mouse position to detect when dragging outside window
                                this.check_drag_outside_window(event.event.position, window, cx);
                            }))
                        }),
                )
                .children(panel.title_suffix(window, cx))
                .child(
                    h_flex()
                        .flex_shrink_0()
                        .ml_1()
                        .gap_1()
                        .child(self.render_toolbar(&state, window, cx))
                        .children(right_dock_button),
                )
                .into_any_element();
        }

        let tabs_count = self.panels.len();

        TabBar::new("tab-bar")
            .tab_item_top_offset(-px(1.))
            .track_scroll(&self.tab_bar_scroll_handle)
            .when(
                left_dock_button.is_some() || bottom_dock_button.is_some(),
                |this| {
                    this.prefix(
                        h_flex()
                            .items_center()
                            .top_0()
                            // Right -1 for avoid border overlap with the first tab
                            .right(-px(1.))
                            .border_r_1()
                            .border_b_1()
                            .h_full()
                            .border_color(cx.theme().border)
                            .bg(cx.theme().tab_bar)
                            .px_2()
                            .children(left_dock_button)
                            .children(bottom_dock_button),
                    )
                },
            )
            .children(self.panels.iter().enumerate().filter_map(|(ix, panel)| {
                let mut active = state.active_panel.as_ref() == Some(panel);
                let droppable = self.collapsed;

                if !panel.visible(cx) {
                    return None;
                }

                // Always not show active tab style, if the panel is collapsed
                if self.collapsed {
                    active = false;
                }

                Some({
                    // Add close button to all tabs except Level Editor
                    let is_level_editor = panel.panel_name(cx) == "Level Editor";
                    let panel_for_menu = panel.clone();
                    let view_for_menu = view.clone();

                    let tab = Tab::empty()
                        .map(|this| {
                            if let Some(tab_name) = panel.tab_name(cx) {
                                this.child(tab_name)
                            } else {
                                this.child(panel.title(window, cx))
                            }
                        })
                        .selected(active)
                        .on_click(cx.listener({
                            let is_collapsed = self.collapsed;
                            let dock_area = self.dock_area.clone();
                            move |view, _, window, cx| {
                                view.set_active_tab(ix, window, cx);

                                // Open dock if clicked on the collapsed bottom dock
                                if is_bottom_dock && is_collapsed {
                                    _ = dock_area.update(cx, |dock_area, cx| {
                                        dock_area.toggle_dock(DockPlacement::Bottom, window, cx);
                                    });
                                }
                            }
                        }))
                        .when(state.draggable, |this| {
                            let channel = state.channel;
                            this.on_drag(
                                DragPanel::new(panel.clone(), view.clone(), channel)
                                    .with_index(ix),
                                move |drag, position, _, cx| {
                                    // Capture the drag start position for window creation
                                    let mut drag_with_pos = drag.clone();
                                    drag_with_pos.drag_start_position = Some(position);
                                    cx.stop_propagation();
                                    cx.new(|_| drag_with_pos)
                                },
                            )
                            .on_drag_move(cx.listener(|this, event: &DragMoveEvent<DragPanel>, window, cx| {
                                // Track mouse position to detect when dragging outside window
                                let is_outside = this.check_drag_outside_window(event.event.position, window, cx);

                                // Clear split placement when dragging outside window
                                if is_outside {
                                    this.will_split_placement = None;
                                }
                            }))
                        })
                        .when(state.droppable, |this| {
                            let channel = state.channel;
                            let view = view.clone();
                            this.drag_over::<DragPanel>(move |this, drag, window, cx| {
                                // Only show drop visual if same channel
                                if drag.channel == channel {
                                    // Mark that we're in a valid drag
                                    view.update(cx, |v, cx| {
                                        v.in_valid_drag = true;
                                        cx.notify();
                                    });
                                    this.rounded_l_none()
                                        .border_l_2()
                                        .border_r_0()
                                        .border_color(cx.theme().drag_border)
                                } else {
                                    this
                                }
                            })
                            .on_drop(cx.listener(
                                move |this, drag: &DragPanel, window, cx| {
                                    this.will_split_placement = None;
                                    this.on_drop(drag, Some(ix), true, window, cx)
                                },
                            ))
                        })
                        .suffix(h_flex().gap_1().when(!is_level_editor, |this| {
                            let panel = panel_for_menu.clone();
                            let view = view_for_menu.clone();
                            let dock = self.dock_area.clone();

                            this.child(
                                Button::new(("move-to-window", ix))
                                    .icon(IconName::ExternalLink)
                                    .ghost()
                                    .xsmall()
                                    .tooltip("Move to New Window")
                                    .on_click(cx.listener(move |tab_panel, _, window, cx| {
                                        let panel_to_move = panel.clone();
                                        let dock_area = dock.clone();
                                        let mouse_pos = window.mouse_position();

                                        println!("[TAB_PANEL] Popout button clicked");
                                        println!("[TAB_PANEL] TabPanel entity ID: {:?}", cx.entity_id());
                                        println!("[TAB_PANEL] Dock area: {:?}", dock_area);

                                        // Emit event FIRST, before deferring
                                        println!("[TAB_PANEL] Emitting PanelEvent::MoveToNewWindow");
                                        cx.emit(PanelEvent::MoveToNewWindow(panel_to_move.clone(), mouse_pos));

                                        // Then detach panel immediately (not deferred)
                                        // The window creation will happen asynchronously via the event
                                        println!("[TAB_PANEL] Detaching panel from current tab panel");
                                        tab_panel.detach_panel(panel_to_move, window, cx);
                                        tab_panel.remove_self_if_empty(window, cx);
                                        println!("[TAB_PANEL] Panel detached");
                                    }))
                            )
                        }).when(!is_level_editor, |this| {
                            this.child(
                                Button::new(("close-tab", ix))
                                    .icon(IconName::Close)
                                    .ghost()
                                    .xsmall()
                                    .on_click(cx.listener({
                                        let panel = panel.clone();
                                        move |this, _, window, cx| {
                                            this.remove_panel(panel.clone(), window, cx);
                                        }
                                    }))
                            )
                        }).into_any_element());
                    tab
                })
            }))
            .last_empty_space(
                // empty space to allow move to last tab right
                div()
                    .id("tab-bar-empty-space")
                    .h_full()
                    .flex_grow()
                    .min_w_16()
                    .when(state.droppable, |this| {
                        let channel = state.channel;
                        let view_entity = view.clone();
                        this.drag_over::<DragPanel>(move |this, drag, window, cx| {
                            // Only show drop visual if same channel
                            if drag.channel == channel {
                                // Mark that we're in a valid drag
                                view_entity.update(cx, |v, cx| {
                                    v.in_valid_drag = true;
                                    cx.notify();
                                });
                                this.bg(cx.theme().drop_target)
                            } else {
                                this
                            }
                        })
                        .on_drop(cx.listener(
                            move |this, drag: &DragPanel, window, cx| {
                                this.will_split_placement = None;

                                let ix = if drag.tab_panel == view {
                                    Some(tabs_count - 1)
                                } else {
                                    None
                                };

                                this.on_drop(drag, ix, false, window, cx)
                            },
                        ))
                    }),
            )
            .when(!self.collapsed, |this| {
                this.suffix(
                    h_flex()
                        .items_center()
                        .top_0()
                        .right_0()
                        .border_l_1()
                        .border_b_1()
                        .h_full()
                        .border_color(cx.theme().border)
                        .bg(cx.theme().tab_bar)
                        .px_2()
                        .gap_1()
                        .children(
                            self.active_panel(cx)
                                .and_then(|panel| panel.title_suffix(window, cx)),
                        )
                        .child(self.render_toolbar(state, window, cx))
                        .when_some(right_dock_button, |this, btn| this.child(btn)),
                )
            })
            .into_any_element()
    }

    fn render_active_panel(
        &self,
        state: &TabState,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        if self.collapsed {
            return Empty {}.into_any_element();
        }

        let Some(active_panel) = state.active_panel.as_ref() else {
            return Empty {}.into_any_element();
        };

        let is_render_in_tabs = self.panels.len() > 1 && self.inner_padding(cx);

        v_flex()
            .id("active-panel")
            .group("")
            .flex_1()
            .child(
                div()
                    .id("tab-content")
                    .overflow_y_scroll()
                    .overflow_x_hidden()
                    .flex_1()
                    .child(
                        active_panel
                            .view()
                            .cached(StyleRefinement::default().absolute().size_full()),
                    ),
            )
            .when(state.droppable, |this| {
                let channel = state.channel;
                let view = cx.entity().clone();
                this.on_drag_move(cx.listener(Self::on_panel_drag_move))
                    .child(
                        div()
                            .invisible()
                            .absolute()
                            .bg(cx.theme().drop_target)
                            .map(|this| match self.will_split_placement {
                                Some(placement) => {
                                    let size = relative(0.5);
                                    match placement {
                                        Placement::Left => this.left_0().top_0().bottom_0().w(size),
                                        Placement::Right => {
                                            this.right_0().top_0().bottom_0().w(size)
                                        }
                                        Placement::Top => this.top_0().left_0().right_0().h(size),
                                        Placement::Bottom => {
                                            this.bottom_0().left_0().right_0().h(size)
                                        }
                                    }
                                }
                                None => this.top_0().left_0().size_full(),
                            })
                            .drag_over::<DragPanel>(move |this, drag, _window, cx| {
                                // Only show drop visual if same channel
                                if drag.channel == channel {
                                    // Mark that we're in a valid drag
                                    view.update(cx, |v, cx| {
                                        v.in_valid_drag = true;
                                        cx.notify();
                                    });
                                    this.visible()
                                } else {
                                    this
                                }
                            })
                            .on_drop(cx.listener(|this, drag: &DragPanel, window, cx| {
                                this.on_drop(drag, None, true, window, cx)
                            })),
                    )
            })
            .into_any_element()
    }

    /// Check if the drag position is outside the window bounds
    fn check_drag_outside_window(
        &mut self,
        position: Point<Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let window_bounds = window.bounds();

        // Add a small margin (20px) to make it easier to trigger
        let margin = px(20.0);
        let is_outside = position.x < window_bounds.left() - margin
            || position.x > window_bounds.right() + margin
            || position.y < window_bounds.top() - margin
            || position.y > window_bounds.bottom() + margin;

        if is_outside != self.dragging_outside_window {
            self.dragging_outside_window = is_outside;
            cx.notify();
        }

        is_outside
    }

    /// Create a simple new window with just the dragged panel
    ///
    /// NOTE: This creates a minimal window container. The panel itself maintains its
    /// references to shared services (like rust analyzer) from the main window,
    /// so there's no duplication of services.
    ///
    /// The window is positioned so the tab bar appears directly under the cursor,
    /// giving the impression that the tab "follows" the mouse during the drag.
    fn create_window_with_panel(
        panel: Arc<dyn PanelView>,
        position: Point<Pixels>,
        _dock_area: WeakEntity<DockArea>,
        cx: &mut App,
    ) {
        let window_size = size(px(800.), px(600.));

        // Approximate height of title bar in the new window
        let title_bar_height = px(36.0);

        // Position window so the cursor is over the tab area (just below title bar)
        // This creates the illusion that the tab is "stuck" to the cursor
        let window_bounds = Bounds::new(
            Point {
                x: position.x - px(100.0), // Offset horizontally so cursor is near tab start
                y: position.y - title_bar_height - px(4.0), // Offset so cursor is on the tab itself
            },
            window_size,
        );

        let window_options = WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(window_bounds)),
            titlebar: None,
            window_min_size: Some(gpui::Size {
                width: px(400.),
                height: px(300.),
            }),
            kind: WindowKind::Normal,
            #[cfg(target_os = "linux")]
            window_background: gpui::WindowBackgroundAppearance::Opaque,
            #[cfg(target_os = "linux")]
            window_decorations: Some(gpui::WindowDecorations::Client),
            ..Default::default()
        };

        let _ = cx.open_window(window_options, move |window, cx| {
            use crate::Root;

            // Create a minimal new dock area for this detached window
            // This is NOT a copy of the main app - it's just a simple container
            let new_dock_area = cx.new(|cx| DockArea::new("detached-dock", Some(1), window, cx));
            let weak_new_dock = new_dock_area.downgrade();

            // Create a tab panel with just the one panel
            let new_tab_panel = cx.new(|cx| {
                let channel = weak_new_dock.upgrade().map(|d| d.read(cx).channel).unwrap_or_default();
                let mut tab_panel = Self::new(None, weak_new_dock.clone(), channel, window, cx);
                tab_panel.closable = true; // Allow closing this detached window
                tab_panel
            });

            new_tab_panel.update(cx, |view, cx| {
                view.add_panel(panel.clone(), window, cx);
            });

            // Set up the dock area with just this panel
            new_dock_area.update(cx, |dock, cx| {
                let dock_item = super::DockItem::Tabs {
                    view: new_tab_panel,
                    active_ix: 0,
                    items: vec![panel.clone()],
                };
                dock.set_center(dock_item, window, cx);
            });

            cx.new(|cx| Root::new(new_dock_area.into(), window, cx))
        });
    }

    /// Calculate the split direction based on the current mouse position
    fn on_panel_drag_move(
        &mut self,
        drag: &DragMoveEvent<DragPanel>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // Only process if we're in a valid same-channel drag
        if !self.in_valid_drag {
            return;
        }
        
        let bounds = drag.bounds;
        let position = drag.event.position;

        // Check if dragging outside window bounds for window extraction
        if self.check_drag_outside_window(position, window, cx) {
            self.will_split_placement = None;
            return;
        }

        // Check the mouse position to determine the split direction
        if position.x < bounds.left() + bounds.size.width * 0.35 {
            self.will_split_placement = Some(Placement::Left);
        } else if position.x > bounds.left() + bounds.size.width * 0.65 {
            self.will_split_placement = Some(Placement::Right);
        } else if position.y < bounds.top() + bounds.size.height * 0.35 {
            self.will_split_placement = Some(Placement::Top);
        } else if position.y > bounds.top() + bounds.size.height * 0.65 {
            self.will_split_placement = Some(Placement::Bottom);
        } else {
            // center to merge into the current tab
            self.will_split_placement = None;
        }
        cx.notify()
    }

    /// Handle the drop event when dragging a panel
    ///
    /// - `active` - When true, the panel will be active after the drop
    fn on_drop(
        &mut self,
        drag: &DragPanel,
        ix: Option<usize>,
        active: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        tracing::debug!("DROP: Panel being dropped on channel {:?}, drag from channel {:?}", self.channel, drag.channel);
        
        // Reset drag state
        self.in_valid_drag = false;
        
        // Verify that the drag is from the same channel
        // This prevents tabs from different dock systems from interfering with each other
        if drag.channel != self.channel {
            tracing::debug!("DROP: Rejected - drag from different channel (cross-channel drops not allowed)");
            return;
        }
        
        // Clone all needed data BEFORE any entity access to avoid borrow conflicts
        let panel = drag.panel.clone();
        let is_same_tab = drag.tab_panel == cx.entity();
        let will_split = self.will_split_placement;
        let dragging_outside = self.dragging_outside_window;
        let drag_start_position = drag.drag_start_position;
        let source_panel = drag.tab_panel.clone();
        let source_index = drag.source_index;
        let dock_area = self.dock_area.clone();
        let target_entity = cx.entity().clone();
        let panels_count = self.panels.len();
        let in_tiles = self.in_tiles;
        tracing::debug!("DROP: is_same_tab={}, ix={:?}, will_split={:?}", is_same_tab, ix, will_split);
        
        // Defer ALL drop handling to avoid entity borrow conflicts
        // Use window.defer instead of cx.defer_in to ensure we're outside the render phase
        window.defer(cx, move |window, cx| {
            // Check if we should create a new window (dragged outside bounds)
            if dragging_outside {
                if let Some(start_pos) = drag_start_position {
                    let panel_to_extract = panel.clone();

                    // Detach the panel from the source
                    if is_same_tab {
                        _ = target_entity.update(cx, |view, cx| {
                            view.detach_panel(panel_to_extract.clone(), window, cx);
                        });
                    } else {
                        _ = source_panel.update(cx, |view, cx| {
                            view.detach_panel(panel_to_extract.clone(), window, cx);
                            view.remove_self_if_empty(window, cx);
                        });
                    }

                    // Close the current window if it was the last tab and we're not in the main window
                    let should_close_window = panels_count == 1 && !in_tiles;

                    // Defer window creation and closing to avoid update conflicts
                    window.defer(cx, move |window, cx| {
                        if should_close_window {
                            window.remove_window();
                        }

                        // Create new window with the panel
                        TabPanel::create_window_with_panel(
                            panel_to_extract,
                            start_pos,
                            dock_area,
                            cx,
                        );
                    });

                    _ = target_entity.update(cx, |view, cx| {
                        view.dragging_outside_window = false;
                        cx.emit(PanelEvent::LayoutChanged);
                    });
                    return;
                }
            }

            // Handle reordering within the same tab panel
            if is_same_tab && ix.is_some() && will_split.is_none() {
                let target_ix = ix.unwrap();

                _ = target_entity.update(cx, |view, cx| {
                    // Only reorder if different positions
                    if source_index != target_ix {
                        // Remove panel from old position
                        let panel = view.panels.remove(source_index);

                        // Calculate new insert position
                        let insert_ix = if target_ix > source_index {
                            target_ix - 1
                        } else {
                            target_ix
                        };

                        // Insert at new position
                        view.panels.insert(insert_ix, panel);

                        // Update active index if needed
                        if view.active_ix == source_index {
                            view.active_ix = insert_ix;
                        } else if source_index < view.active_ix && insert_ix >= view.active_ix {
                            view.active_ix -= 1;
                        } else if source_index > view.active_ix && insert_ix <= view.active_ix {
                            view.active_ix += 1;
                        }

                        cx.emit(PanelEvent::LayoutChanged);
                        cx.notify();
                    }
                });
                return;
            }

            // If target is same tab, not splitting, and no specific index, do nothing.
            if is_same_tab && ix.is_none() && will_split.is_none() {
                return;
            }

            // Detach from source (if different tab panel)
            if !is_same_tab {
                _ = source_panel.update(cx, |view, cx| {
                    view.detach_panel(panel.clone(), window, cx);
                    view.remove_self_if_empty(window, cx);
                });
            }

            // Insert into target (and detach if same tab, all in one update)
            tracing::debug!("DROP: Inserting panel into target, will_split={:?}, is_same_tab={}", will_split, is_same_tab);
            _ = target_entity.update(cx, |view, cx| {
                if let Some(placement) = will_split {
                    tracing::debug!("DROP: Splitting with placement {:?}", placement);
                    // When splitting, handle detach differently
                    if is_same_tab {
                        tracing::debug!("DROP: Splitting from same tab - split_panel will handle panel management");
                        // Don't detach yet - split_panel needs the current panel structure intact
                        view.split_panel(panel.clone(), placement, None, window, cx);
                        // Now detach from the original tab after split is created
                        view.detach_panel(panel.clone(), window, cx);
                    } else {
                        tracing::debug!("DROP: Splitting from different tab");
                        view.split_panel(panel.clone(), placement, None, window, cx);
                    }
                } else {
                    // If same tab, detach first within this same update
                    if is_same_tab {
                        tracing::debug!("DROP: Not splitting, detaching from same tab first");
                        view.detach_panel(panel.clone(), window, cx);
                    }

                    if let Some(ix) = ix {
                        tracing::debug!("DROP: Inserting at index {}", ix);
                        view.insert_panel_at(panel.clone(), ix, window, cx)
                    } else {
                        tracing::debug!("DROP: Adding panel with active={}", active);
                        view.add_panel_with_active(panel.clone(), active, window, cx)
                    }
                }

                tracing::debug!("DROP: Drop complete, checking if empty");
                view.remove_self_if_empty(window, cx);
                cx.emit(PanelEvent::LayoutChanged);
            });
        });
    }

    /// Add panel with split placement
    fn split_panel(
        &self,
        panel: Arc<dyn PanelView>,
        placement: Placement,
        size: Option<Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        tracing::debug!("SPLIT: split_panel called with placement {:?}", placement);
        let dock_area = self.dock_area.clone();
        // wrap the panel in a TabPanel
        let channel = self.channel;
        let panel_for_new_tab = panel.clone();
        let new_tab_panel = cx.new(|cx| Self::new(None, dock_area.clone(), channel, window, cx));
        new_tab_panel.update(cx, |view, cx| {
            view.add_panel(panel_for_new_tab, window, cx);
        });
        tracing::debug!("SPLIT: Created new tab panel");

        let stack_panel = match self.stack_panel.as_ref().and_then(|panel| panel.upgrade()) {
            Some(panel) => {
                tracing::debug!("SPLIT: Found parent StackPanel");
                panel
            },
            None => {
                tracing::debug!("SPLIT: No parent StackPanel - handling root-level split");
                tracing::debug!("SPLIT: Current TabPanel entity: {:?}", cx.entity());
                tracing::debug!("SPLIT: Current TabPanel has {} panels", self.panels.len());

                // Handle root-level split: create a new StackPanel and update DockArea
                let axis = placement.axis();
                let current_tab_panel = cx.entity().clone();

                // Create new StackPanel with the appropriate axis
                let new_stack_panel = cx.new(|cx| {
                    let mut stack = StackPanel::new(axis, window, cx);
                    stack.parent = None; // This is the root level StackPanel
                    stack
                });

                // Defer all updates to avoid borrow conflicts
                let dock_area_clone = dock_area.clone();
                let new_stack_clone = new_stack_panel.clone();
                let new_tab_clone = new_tab_panel.clone();
                let current_tab_clone = current_tab_panel.clone();
                let panel_clone = panel.clone();

                window.defer(cx, move |window, cx| {
                    // Detach the panel from current tab if it exists there
                    // This handles the same-tab case where the panel hasn't been detached yet
                    _ = current_tab_clone.update(cx, |view, cx| {
                        // Check if the panel exists in current tab before detaching
                        if view.panels.iter().any(|p| Arc::ptr_eq(p, &panel_clone)) {
                            view.detach_panel(panel_clone.clone(), window, cx);
                            tracing::debug!("SPLIT: Detached panel from current tab");
                        }
                    });

                    // Update current TabPanel to reference the new StackPanel
                    _ = current_tab_clone.update(cx, |view, cx| {
                        view.stack_panel = Some(new_stack_clone.downgrade());
                        cx.notify();
                    });

                    // Update new TabPanel to reference the new StackPanel
                    _ = new_tab_clone.update(cx, |view, cx| {
                        view.stack_panel = Some(new_stack_clone.downgrade());
                        cx.notify();
                    });

                    // Add both TabPanels to the StackPanel in the correct order
                    _ = new_stack_clone.update(cx, |stack, cx| {
                        match placement {
                            Placement::Left | Placement::Top => {
                                // New panel goes first
                                stack.add_panel(Arc::new(new_tab_clone.clone()), size, dock_area_clone.clone(), window, cx);
                                stack.add_panel(Arc::new(current_tab_clone.clone()), None, dock_area_clone.clone(), window, cx);
                            }
                            Placement::Right | Placement::Bottom => {
                                // Current panel goes first
                                stack.add_panel(Arc::new(current_tab_clone.clone()), None, dock_area_clone.clone(), window, cx);
                                stack.add_panel(Arc::new(new_tab_clone.clone()), size, dock_area_clone.clone(), window, cx);
                            }
                        }
                    });

                    // Update DockArea to use the new StackPanel in the correct location
                    _ = dock_area_clone.upgrade().map(|dock| {
                        dock.update(cx, |dock_area, cx| {
                            let current_tab_id = current_tab_clone.entity_id();
                            tracing::debug!("SPLIT: Looking for TabPanel {:?} in DockArea", current_tab_id);

                            // Helper to check if a DockItem contains our TabPanel
                            let contains_tab_panel = |item: &DockItem| -> bool {
                                match item {
                                    DockItem::Tabs { view, .. } => view.entity_id() == current_tab_id,
                                    _ => false,
                                }
                            };

                            // Create the new DockItem::Split
                            let new_dock_item = DockItem::Split {
                                axis,
                                items: vec![],  // items will be managed by the StackPanel
                                sizes: vec![None, None],
                                view: new_stack_clone.clone(),
                            };

                            let mut found = false;

                            // Check center (items)
                            if contains_tab_panel(&dock_area.items) {
                                tracing::debug!("SPLIT: Found in center (items), replacing it");
                                dock_area.items = new_dock_item.clone();
                                found = true;
                            }
                            // Check left dock
                            else if let Some(left_dock) = &dock_area.left_dock {
                                if contains_tab_panel(&left_dock.read(cx).panel) {
                                    tracing::debug!("SPLIT: Found in left_dock, replacing its panel");
                                    let left_dock_entity = left_dock.clone();
                                    left_dock_entity.update(cx, |dock, cx| {
                                        dock.set_panel(new_dock_item.clone(), window, cx);
                                    });
                                    found = true;
                                }
                            }
                            // Check right dock
                            if !found {
                                if let Some(right_dock) = &dock_area.right_dock {
                                    if contains_tab_panel(&right_dock.read(cx).panel) {
                                        tracing::debug!("SPLIT: Found in right_dock, replacing its panel");
                                        let right_dock_entity = right_dock.clone();
                                        right_dock_entity.update(cx, |dock, cx| {
                                            dock.set_panel(new_dock_item.clone(), window, cx);
                                        });
                                        found = true;
                                    }
                                }
                            }
                            // Check bottom dock
                            if !found {
                                if let Some(bottom_dock) = &dock_area.bottom_dock {
                                    if contains_tab_panel(&bottom_dock.read(cx).panel) {
                                        tracing::debug!("SPLIT: Found in bottom_dock, replacing its panel");
                                        let bottom_dock_entity = bottom_dock.clone();
                                        bottom_dock_entity.update(cx, |dock, cx| {
                                            dock.set_panel(new_dock_item.clone(), window, cx);
                                        });
                                        found = true;
                                    }
                                }
                            }

                            if !found {
                                tracing::debug!("SPLIT: WARNING - Could not find TabPanel in any dock location!");
                            }

                            cx.notify();
                        })
                    });

                    tracing::debug!("SPLIT: Created root-level split with StackPanel");
                });

                return; // Early return since we've deferred everything
            }
        };

        let parent_axis = stack_panel.read(cx).axis;

        let ix = stack_panel
            .read(cx)
            .index_of_panel(Arc::new(cx.entity().clone()))
            .unwrap_or_default();

        if parent_axis.is_vertical() && placement.is_vertical() {
            stack_panel.update(cx, |view, cx| {
                view.insert_panel_at(
                    Arc::new(new_tab_panel),
                    ix,
                    placement,
                    size,
                    dock_area.clone(),
                    window,
                    cx,
                );
            });
        } else if parent_axis.is_horizontal() && placement.is_horizontal() {
            stack_panel.update(cx, |view, cx| {
                view.insert_panel_at(
                    Arc::new(new_tab_panel),
                    ix,
                    placement,
                    size,
                    dock_area.clone(),
                    window,
                    cx,
                );
            });
        } else {
            // 1. Create new StackPanel with new axis
            // 2. Move cx.entity() from parent StackPanel to the new StackPanel
            // 3. Add the new TabPanel to the new StackPanel at the correct index
            // 4. Add new StackPanel to the parent StackPanel at the correct index
            let tab_panel = cx.entity().clone();

            // Try to use the old stack panel, not just create a new one, to avoid too many nested stack panels
            let new_stack_panel = if stack_panel.read(cx).panels_len() <= 1 {
                stack_panel.update(cx, |view, cx| {
                    view.remove_all_panels(window, cx);
                    view.set_axis(placement.axis(), window, cx);
                });
                stack_panel.clone()
            } else {
                cx.new(|cx| {
                    let mut panel = StackPanel::new(placement.axis(), window, cx);
                    panel.parent = Some(stack_panel.downgrade());
                    panel
                })
            };

            new_stack_panel.update(cx, |view, cx| match placement {
                Placement::Left | Placement::Top => {
                    view.add_panel(Arc::new(new_tab_panel), size, dock_area.clone(), window, cx);
                    view.add_panel(
                        Arc::new(tab_panel.clone()),
                        None,
                        dock_area.clone(),
                        window,
                        cx,
                    );
                }
                Placement::Right | Placement::Bottom => {
                    view.add_panel(
                        Arc::new(tab_panel.clone()),
                        None,
                        dock_area.clone(),
                        window,
                        cx,
                    );
                    view.add_panel(Arc::new(new_tab_panel), size, dock_area.clone(), window, cx);
                }
            });

            if stack_panel != new_stack_panel {
                stack_panel.update(cx, |view, cx| {
                    view.replace_panel(
                        Arc::new(tab_panel.clone()),
                        new_stack_panel.clone(),
                        window,
                        cx,
                    );
                });
            }

            cx.spawn_in(window, async move |_, cx| {
                cx.update(|window, cx| {
                    tab_panel.update(cx, |view, cx| view.remove_self_if_empty(window, cx))
                })
            })
            .detach()
        }

        cx.emit(PanelEvent::LayoutChanged);
    }

    fn focus_active_panel(&self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(active_panel) = self.active_panel(cx) {
            active_panel.focus_handle(cx).focus(window);
        }
    }

    fn on_action_toggle_zoom(
        &mut self,
        _: &ToggleZoom,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.zoomable(cx).is_none() {
            return;
        }

        if !self.zoomed {
            cx.emit(PanelEvent::ZoomIn)
        } else {
            cx.emit(PanelEvent::ZoomOut)
        }
        self.zoomed = !self.zoomed;

        cx.spawn_in(window, {
            let zoomed = self.zoomed;
            async move |view, cx| {
                _ = cx.update(|window, cx| {
                    _ = view.update(cx, |view, cx| {
                        view.set_zoomed(zoomed, window, cx);
                    });
                });
            }
        })
        .detach();
    }

    fn on_action_close_panel(
        &mut self,
        _: &ClosePanel,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(panel) = self.active_panel(cx) {
            self.remove_panel(panel, window, cx);
        }

        // Remove self from the parent DockArea.
        // This is ensure to remove from Tiles
        if self.panels.is_empty() && self.in_tiles {
            let tab_panel = Arc::new(cx.entity());
            window.defer(cx, {
                let dock_area = self.dock_area.clone();
                move |window, cx| {
                    _ = dock_area.update(cx, |this, cx| {
                        this.remove_panel_from_all_docks(tab_panel, window, cx);
                    });
                }
            });
        }
    }

    // Bind actions to the tab panel, only when the tab panel is not collapsed.
    fn bind_actions(&self, cx: &mut Context<Self>) -> Div {
        v_flex().when(!self.collapsed, |this| {
            this.on_action(cx.listener(Self::on_action_toggle_zoom))
                .on_action(cx.listener(Self::on_action_close_panel))
        })
    }
}

impl Focusable for TabPanel {
    fn focus_handle(&self, cx: &App) -> gpui::FocusHandle {
        if let Some(active_panel) = self.active_panel(cx) {
            active_panel.focus_handle(cx)
        } else {
            self.focus_handle.clone()
        }
    }
}
impl EventEmitter<DismissEvent> for TabPanel {}
impl EventEmitter<PanelEvent> for TabPanel {}
impl Render for TabPanel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl gpui::IntoElement {
        let focus_handle = self.focus_handle(cx);
        let active_panel = self.active_panel(cx);
        let mut state = TabState {
            closable: self.closable(cx),
            draggable: self.draggable(cx),
            droppable: self.droppable(cx),
            zoomable: self.zoomable(cx),
            active_panel,
            channel: self.channel,
        };

        // 1. When is the final panel in the dock, it will not able to close.
        // 2. When is in the Tiles, it will always able to close (by active panel state).
        if !state.draggable && !self.in_tiles {
            state.closable = false;
        }

        self.bind_actions(cx)
            .id("tab-panel")
            .track_focus(&focus_handle)
            .tab_group()
            .size_full()
            .overflow_hidden()
            // NO BACKGROUND - allow transparency for viewports
            .child(self.render_title_bar(&state, window, cx))
            .child(self.render_active_panel(&state, window, cx))
    }
}
