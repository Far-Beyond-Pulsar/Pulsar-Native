use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;

use gpui::{
    div, prelude::FluentBuilder, px, relative, rems, size, App, AppContext, Bounds, Context,
    Corner, DismissEvent, Div, DragMoveEvent, Empty, Entity, EventEmitter, FocusHandle, Focusable,
    InteractiveElement as _, IntoElement, ParentElement, Pixels, Point, ReadGlobal, Render,
    ScrollHandle, SharedString, StatefulInteractiveElement, StyleRefinement, Styled, UpdateGlobal,
    WeakEntity, Window, WindowBounds, WindowKind, WindowOptions,
};
use rust_i18n::t;

use crate::{
    button::{Button, ButtonVariants as _},
    context_menu::{ContextMenu, ContextMenuExt},
    dock::PanelInfo,
    h_flex,
    menu::popup_menu::PopupMenuItem,
    popup_menu::{PopupMenu, PopupMenuExt},
    tab::{Tab, TabBar},
    v_flex, ActiveTheme, AxisExt, IconName, PixelsExt, Placement, Selectable, Sizable,
};

use super::{
    ClosePanel, Dock, DockArea, DockItem, DockPlacement, Panel, PanelControl, PanelEvent,
    PanelState, PanelStyle, PanelView, StackPanel, ToggleZoom,
};

use ui_types_common::window_types::{WindowId, WindowRequest};
use window_manager;

pub mod drag_drop;
pub mod render;
pub mod serialization;
pub mod split;

#[derive(Clone)]
pub(crate) struct TabState {
    pub(crate) closable: bool,
    pub(crate) zoomable: Option<PanelControl>,
    pub(crate) draggable: bool,
    pub(crate) droppable: bool,
    pub(crate) active_panel: Option<Arc<dyn PanelView>>,
    pub(crate) channel: DockChannel,
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
    pub(crate) fn new(
        panel: Arc<dyn PanelView>,
        tab_panel: Entity<TabPanel>,
        channel: DockChannel,
    ) -> Self {
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
                this.border_2()
                    .border_color(cx.theme().accent)
                    .text_color(cx.theme().primary_foreground)
                    .bg(cx.theme().accent)
                    .opacity(0.95)
                    .shadow_2xl()
            })
            .when(!is_outside_bounds, |this| {
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
    pub(crate) focus_handle: FocusHandle,
    pub(crate) dock_area: WeakEntity<DockArea>,
    /// The stock_panel can be None, if is None, that means the panels can't be split or move
    pub(crate) stack_panel: Option<WeakEntity<StackPanel>>,
    pub(crate) panels: Vec<Arc<dyn PanelView>>,
    pub(crate) active_ix: usize,
    /// If this is true, the Panel closable will follow the active panel's closable,
    /// otherwise this TabPanel will not able to close
    ///
    /// This is used for Dock to limit the last TabPanel not able to close, see [`super::Dock::new`].
    pub(crate) closable: bool,

    pub(crate) tab_bar_scroll_handle: ScrollHandle,
    pub(crate) zoomed: bool,
    pub(crate) collapsed: bool,
    /// When drag move, will get the placement of the panel to be split
    pub(crate) will_split_placement: Option<Placement>,
    /// Is TabPanel used in Tiles.
    pub(crate) in_tiles: bool,
    /// Track the index where a dragged tab should be inserted for reordering
    pub(crate) pending_reorder_index: Option<usize>,
    /// Track if we're currently dragging outside window bounds
    pub(crate) dragging_outside_window: bool,
    /// Dock channel - isolates this TabPanel to only interact with same-channel drags
    pub(crate) channel: DockChannel,
    /// Track if we're in a valid same-channel drag (set by drag_over predicate)
    pub(crate) in_valid_drag: bool,
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

    pub(crate) fn add_panel_with_active(
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
        if active {
            self.set_active_tab(self.panels.len() - 1, window, cx);
        }
        cx.emit(PanelEvent::LayoutChanged);
        cx.notify();
    }

    pub fn insert_panel_at(
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

    /// Close all tabs except the specified one
    pub fn close_other_tabs(
        &mut self,
        keep_index: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if keep_index >= self.panels.len() {
            return;
        }

        let panels_to_remove: Vec<_> = self
            .panels
            .iter()
            .enumerate()
            .filter(|(i, p)| *i != keep_index && p.closable(cx))
            .map(|(_, p)| p.clone())
            .collect();

        for panel in panels_to_remove {
            self.remove_panel(panel, window, cx);
        }
    }

    /// Close all tabs to the left of the specified index
    pub fn close_tabs_to_left(
        &mut self,
        from_index: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if from_index == 0 || from_index >= self.panels.len() {
            return;
        }

        let panels_to_remove: Vec<_> = self
            .panels
            .iter()
            .take(from_index)
            .filter(|p| p.closable(cx))
            .cloned()
            .collect();

        for panel in panels_to_remove {
            self.remove_panel(panel, window, cx);
        }
    }

    /// Close all tabs to the right of the specified index
    pub fn close_tabs_to_right(
        &mut self,
        from_index: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if from_index >= self.panels.len() {
            return;
        }

        let panels_to_remove: Vec<_> = self
            .panels
            .iter()
            .skip(from_index + 1)
            .filter(|p| p.closable(cx))
            .cloned()
            .collect();

        for panel in panels_to_remove {
            self.remove_panel(panel, window, cx);
        }
    }

    /// Close all tabs that are saved (don't have unsaved changes)
    pub fn close_all_saved_tabs(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let panels_to_remove: Vec<_> = self
            .panels
            .iter()
            .filter(|p| p.closable(cx))
            .cloned()
            .collect();

        for panel in panels_to_remove {
            self.remove_panel(panel, window, cx);
        }
    }

    pub(crate) fn detach_panel(
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
    pub(crate) fn remove_self_if_empty(&self, window: &mut Window, cx: &mut Context<Self>) {
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

    pub(crate) fn is_locked(&self, cx: &App) -> bool {
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
    pub(crate) fn is_last_panel(&self, cx: &App) -> bool {
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
    pub(crate) fn visible_panels<'a>(
        &'a self,
        cx: &'a App,
    ) -> impl Iterator<Item = Arc<dyn PanelView>> + 'a {
        self.panels.iter().filter_map(|panel| {
            if panel.visible(cx) {
                Some(panel.clone())
            } else {
                None
            }
        })
    }

    /// Return true if the tab panel is draggable.
    pub(crate) fn draggable(&self, cx: &App) -> bool {
        let Some(dock_area) = self.dock_area.upgrade() else {
            return false;
        };

        if dock_area.read(cx).is_locked() {
            return false;
        }

        if self.zoomed {
            return false;
        }

        if let Some(parent) = &self.stack_panel {
            if let Some(stack_panel) = parent.upgrade() {
                if stack_panel.read(cx).is_last_panel(cx) && self.panels.len() <= 1 {
                    return false;
                }
            }
        }

        true
    }

    /// Return true if the tab panel is droppable.
    pub(crate) fn droppable(&self, cx: &App) -> bool {
        let Some(dock_area) = self.dock_area.upgrade() else {
            return false;
        };

        if dock_area.read(cx).is_locked() {
            return false;
        }

        if self.zoomed {
            return false;
        }

        true
    }

    pub(crate) fn focus_active_panel(&self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(active_panel) = self.active_panel(cx) {
            active_panel.focus_handle(cx).focus(window);
        }
    }

    pub(crate) fn on_action_toggle_zoom(
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

    pub(crate) fn on_action_close_panel(
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
    pub(crate) fn bind_actions(&self, cx: &mut Context<Self>) -> Div {
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
